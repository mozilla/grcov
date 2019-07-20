#![recursion_limit = "1024"]
extern crate chrono;
#[macro_use]
extern crate serde_json;
extern crate crossbeam;
#[macro_use]
extern crate fomat_macros;
extern crate globset;
extern crate semver;
extern crate smallvec;
extern crate tempfile;
extern crate uuid;
extern crate walkdir;
extern crate xml;
extern crate zip;
extern crate rustc_hash;

mod defs;
pub use crate::defs::*;

mod producer;
pub use crate::producer::*;

mod gcov;
pub use crate::gcov::*;

mod parser;
pub use crate::parser::*;

mod filter;
pub use crate::filter::*;

mod path_rewriting;
pub use crate::path_rewriting::*;

mod output;
pub use crate::output::*;

mod reader;
pub use crate::reader::*;

mod covdir;
pub use crate::covdir::*;

pub mod html;

use std::collections::{btree_map, hash_map};
use std::fs;
use std::io::{BufReader, Cursor};
use std::path::PathBuf;
use walkdir::WalkDir;


// Merge results, without caring about duplicate lines (they will be removed at the end).
pub fn merge_results(result: &mut CovResult, result2: CovResult) {
    for (&line_no, &execution_count) in &result2.lines {
        match result.lines.entry(line_no) {
            btree_map::Entry::Occupied(c) => {
                *c.into_mut() += execution_count;
            }
            btree_map::Entry::Vacant(v) => {
                v.insert(execution_count);
            }
        };
    }

    for (line_no, taken) in result2.branches {
        match result.branches.entry(line_no) {
            btree_map::Entry::Occupied(c) => {
                let v = c.into_mut();
                for (x, y) in taken.iter().zip(v.iter_mut()) {
                    *y |= x;
                }
                let l = v.len();
                if taken.len() > l {
                    v.extend(&taken[l..]);
                }
            }
            btree_map::Entry::Vacant(v) => {
                v.insert(taken);
            }
        };
    }

    for (name, function) in result2.functions {
        match result.functions.entry(name) {
            hash_map::Entry::Occupied(f) => f.into_mut().executed |= function.executed,
            hash_map::Entry::Vacant(v) => {
                v.insert(function);
            }
        };
    }
}

fn add_results(
    mut results: Vec<(String, CovResult)>,
    result_map: &SyncCovResultMap,
    source_dir: &Option<PathBuf>,
) {
    let mut map = result_map.lock().unwrap();
    for result in results.drain(..) {
        let path = match source_dir {
            Some(source_dir) => {
                // the goal here is to be able to merge results for paths like foo/./bar and foo/bar
                if let Ok(p) = canonicalize_path(source_dir.join(&result.0)) {
                    String::from(p.to_str().unwrap())
                } else {
                    result.0
                }
            }
            None => result.0,
        };
        match map.entry(path) {
            hash_map::Entry::Occupied(obj) => {
                merge_results(obj.into_mut(), result.1);
            }
            hash_map::Entry::Vacant(v) => {
                v.insert(result.1);
            }
        };
    }
}

fn rename_single_files(results: &mut Vec<(String, CovResult)>, stem: &str) {
    // sometimes the gcno just contains foo.c
    // so in such case (with option --guess-directory-when-missing)
    // we guess the filename in using the buffer stem
    if let Some(parent) = PathBuf::from(stem).parent() {
        for (file, _) in results.iter_mut() {
            if has_no_parent(file) {
                *file = parent.join(&file).to_str().unwrap().to_string();
            }
        }
    }
}

// Some versions of GCC, because of a bug, generate multiple gcov files for each
// gcno, so we have to support this case too for the time being.
#[derive(PartialEq, Eq)]
enum GcovType {
    Unknown,
    SingleFile,
    MultipleFiles,
}

macro_rules! try_parse {
    ($v:expr, $f:expr) => {
        match $v {
            Ok(val) => val,
            Err(err) => {
                eprintln!("Error parsing file {}:", $f);
                eprintln!("{}", err);
                std::process::exit(1);
            }
        }
    };
}

pub fn consumer(
    working_dir: &PathBuf,
    source_dir: &Option<PathBuf>,
    result_map: &SyncCovResultMap,
    receiver: JobReceiver,
    branch_enabled: bool,
    guess_directory: bool,
) {
    let mut gcov_type = GcovType::Unknown;

    while let Ok(work_item) = receiver.recv() {
        if work_item.is_none() {
            break;
        }
        let work_item = work_item.unwrap();
        let new_results = match work_item.format {
            ItemFormat::GCNO => {
                match work_item.item {
                    ItemType::Path((stem, gcno_path)) => {
                        // GCC
                        run_gcov(&gcno_path, branch_enabled, working_dir);
                        let gcov_path =
                            gcno_path.file_name().unwrap().to_str().unwrap().to_string() + ".gcov";
                        let gcov_path = working_dir.join(gcov_path);
                        if gcov_type == GcovType::Unknown {
                            gcov_type = if gcov_path.exists() {
                                GcovType::SingleFile
                            } else {
                                GcovType::MultipleFiles
                            };
                        }

                        let mut new_results = if gcov_type == GcovType::SingleFile {
                            let new_results = try_parse!(parse_gcov(&gcov_path), work_item.name);
                            fs::remove_file(gcov_path).unwrap();
                            new_results
                        } else {
                            let mut new_results: Vec<(String, CovResult)> = Vec::new();

                            for entry in WalkDir::new(&working_dir).min_depth(1) {
                                let gcov_path = entry.unwrap();
                                let gcov_path = gcov_path.path();

                                new_results.append(&mut try_parse!(
                                    parse_gcov(&gcov_path),
                                    work_item.name
                                ));

                                fs::remove_file(gcov_path).unwrap();
                            }

                            new_results
                        };

                        if guess_directory {
                            rename_single_files(&mut new_results, &stem);
                        }
                        new_results
                    }
                    ItemType::Buffers(buffers) => {
                        // LLVM
                        match GCNO::compute(
                            &buffers.stem,
                            buffers.gcno_buf,
                            buffers.gcda_buf,
                            branch_enabled,
                        ) {
                            Ok(mut r) => {
                                if guess_directory {
                                    rename_single_files(&mut r, &buffers.stem);
                                }
                                r
                            },
                            Err(e) => {
                                // Just print the error, don't panic and continue
                                eprintln!("Error in computing counters:\n{}", e);
                                Vec::new()
                            }
                        }
                    }
                    ItemType::Content(_) => {
                        panic!("Invalid content type");
                    }
                }
            }
            ItemFormat::INFO | ItemFormat::JACOCO_XML => {
                if let ItemType::Content(content) = work_item.item {
                    let buffer = BufReader::new(Cursor::new(content));
                    if work_item.format == ItemFormat::INFO {
                        try_parse!(parse_lcov(buffer, branch_enabled), work_item.name)
                    } else {
                        try_parse!(parse_jacoco_xml_report(buffer), work_item.name)
                    }
                } else {
                    panic!("Invalid content type")
                }
            }
        };

        add_results(new_results, result_map, source_dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustc_hash::FxHashMap;
    use std::fs::File;
    use std::sync::{Arc, Mutex};

    #[test]
    fn test_merge_results() {
        let mut functions1: FunctionMap = FxHashMap::default();
        functions1.insert(
            "f1".to_string(),
            Function {
                start: 1,
                executed: false,
            },
        );
        functions1.insert(
            "f2".to_string(),
            Function {
                start: 2,
                executed: false,
            },
        );
        let mut result = CovResult {
            lines: [(1, 21), (2, 7), (7, 0)].iter().cloned().collect(),
            branches: [
                (1, vec![false, false]),
                (2, vec![false, true]),
                (4, vec![true]),
            ]
            .iter()
            .cloned()
            .collect(),
            functions: functions1,
        };
        let mut functions2: FunctionMap = FxHashMap::default();
        functions2.insert(
            "f1".to_string(),
            Function {
                start: 1,
                executed: false,
            },
        );
        functions2.insert(
            "f2".to_string(),
            Function {
                start: 2,
                executed: true,
            },
        );
        let result2 = CovResult {
            lines: [(1, 21), (3, 42), (4, 7), (2, 0), (8, 0)]
                .iter()
                .cloned()
                .collect(),
            branches: [
                (1, vec![false, false]),
                (2, vec![false, true]),
                (3, vec![true]),
            ]
            .iter()
            .cloned()
            .collect(),
            functions: functions2,
        };

        merge_results(&mut result, result2);
        assert_eq!(
            result.lines,
            [(1, 42), (2, 7), (3, 42), (4, 7), (7, 0), (8, 0)]
                .iter()
                .cloned()
                .collect()
        );
        assert_eq!(
            result.branches,
            [
                (1, vec![false, false]),
                (2, vec![false, true]),
                (3, vec![true]),
                (4, vec![true]),
            ]
            .iter()
            .cloned()
            .collect()
        );
        assert!(result.functions.contains_key("f1"));
        assert!(result.functions.contains_key("f2"));
        let mut func = result.functions.get("f1").unwrap();
        assert_eq!(func.start, 1);
        assert_eq!(func.executed, false);
        func = result.functions.get("f2").unwrap();
        assert_eq!(func.start, 2);
        assert_eq!(func.executed, true);
    }

    #[test]
    fn test_merge_relative_path() {
        let f = File::open("./test/relative_path/relative_path.info")
            .expect("Failed to open lcov file");
        let file = BufReader::new(&f);
        let results = parse_lcov(file, false).unwrap();
        let result_map: Arc<SyncCovResultMap> = Arc::new(Mutex::new(FxHashMap::with_capacity_and_hasher(1, Default::default())));
        add_results(
            results,
            &result_map,
            &Some(PathBuf::from("./test/relative_path")),
        );
        let result_map = Arc::try_unwrap(result_map).unwrap().into_inner().unwrap();

        assert!(result_map.len() == 1);

        let cpp_file =
            canonicalize_path(PathBuf::from("./test/relative_path/foo/bar/oof.cpp")).unwrap();
        let cpp_file = cpp_file.to_str().unwrap();
        let cov_result = result_map.get(cpp_file).unwrap();

        assert_eq!(
            cov_result.lines,
            [(1, 63), (2, 63), (3, 84), (4, 42)]
                .iter()
                .cloned()
                .collect()
        );
        assert!(cov_result.functions.contains_key("myfun"));
    }

    #[test]
    fn test_ignore_relative_path() {
        let f = File::open("./test/relative_path/relative_path.info")
            .expect("Failed to open lcov file");
        let file = BufReader::new(&f);
        let results = parse_lcov(file, false).unwrap();
        let result_map: Arc<SyncCovResultMap> = Arc::new(Mutex::new(FxHashMap::with_capacity_and_hasher(3, Default::default())));
        add_results(results, &result_map, &None);
        let result_map = Arc::try_unwrap(result_map).unwrap().into_inner().unwrap();

        assert!(result_map.len() == 3);
    }
}
