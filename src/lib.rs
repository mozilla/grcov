#![recursion_limit = "1024"]
#![allow(clippy::too_many_arguments)]

mod defs;
pub use crate::defs::*;

mod producer;
pub use crate::producer::*;

mod gcov;
pub use crate::gcov::*;

mod llvm_tools;
pub use crate::llvm_tools::*;

mod parser;
pub use crate::parser::*;

mod filter;
pub use crate::filter::*;

mod symlink;

mod path_rewriting;
pub use crate::path_rewriting::*;

mod output;
pub use crate::output::*;

mod cobertura;
pub use crate::cobertura::*;

mod reader;
pub use crate::reader::*;

mod covdir;
pub use crate::covdir::*;

pub mod html;

mod file_filter;
pub use crate::file_filter::*;

use log::{error, warn};
use std::fs;
use std::io::{BufReader, Cursor};
use std::{
    collections::{btree_map, hash_map},
    path::Path,
};
use walkdir::WalkDir;

// Merge results, without caring about duplicate lines (they will be removed at the end).
pub fn merge_results(result: &mut CovResult, result2: CovResult) -> bool {
    let mut warn_overflow = false;
    for (&line_no, &execution_count) in &result2.lines {
        match result.lines.entry(line_no) {
            btree_map::Entry::Occupied(c) => {
                let v = c.get().checked_add(execution_count).unwrap_or_else(|| {
                    warn_overflow = true;
                    std::u64::MAX
                });

                *c.into_mut() = v;
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

    warn_overflow
}

fn add_results(
    mut results: Vec<(String, CovResult)>,
    result_map: &SyncCovResultMap,
    source_dir: Option<&Path>,
) {
    let mut map = result_map.lock().unwrap();
    let mut warn_overflow = false;
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
                warn_overflow |= merge_results(obj.into_mut(), result.1);
            }
            hash_map::Entry::Vacant(v) => {
                v.insert(result.1);
            }
        };
    }

    if warn_overflow {
        warn!("Execution count overflow detected.");
    }
}

fn rename_single_files(results: &mut Vec<(String, CovResult)>, stem: &str) {
    // sometimes the gcno just contains foo.c
    // so in such case (with option --guess-directory-when-missing)
    // we guess the filename in using the buffer stem
    if let Some(parent) = Path::new(stem).parent() {
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
                error!("Error parsing file {}: {}", $f, err);
                continue;
            }
        }
    };
}

pub fn consumer(
    working_dir: &Path,
    source_dir: Option<&Path>,
    result_map: &SyncCovResultMap,
    receiver: JobReceiver,
    branch_enabled: bool,
    guess_directory: bool,
    binary_path: Option<&Path>,
) {
    let mut gcov_type = GcovType::Unknown;

    while let Ok(work_item) = receiver.recv() {
        if work_item.is_none() {
            break;
        }
        let work_item = work_item.unwrap();
        let new_results = match work_item.format {
            ItemFormat::Gcno => {
                match work_item.item {
                    ItemType::Path((stem, gcno_path)) => {
                        // GCC
                        if let Err(e) = run_gcov(&gcno_path, branch_enabled, working_dir) {
                            error!("Error when running gcov: {}", e);
                            continue;
                        };
                        let gcov_ext = get_gcov_output_ext();
                        let gcov_path =
                            gcno_path.file_name().unwrap().to_str().unwrap().to_string() + gcov_ext;
                        let gcov_path = working_dir.join(gcov_path);
                        if gcov_type == GcovType::Unknown {
                            gcov_type = if gcov_path.exists() {
                                GcovType::SingleFile
                            } else {
                                GcovType::MultipleFiles
                            };
                        }

                        let mut new_results = if gcov_type == GcovType::SingleFile {
                            let new_results = try_parse!(
                                if gcov_ext.ends_with("gz") {
                                    parse_gcov_gz(&gcov_path)
                                } else if gcov_ext.ends_with("gcov") {
                                    parse_gcov(&gcov_path)
                                } else {
                                    panic!("Invalid gcov extension: {}", gcov_ext);
                                },
                                work_item.name
                            );
                            fs::remove_file(gcov_path).unwrap();
                            new_results
                        } else {
                            let mut new_results: Vec<(String, CovResult)> = Vec::new();

                            for entry in WalkDir::new(&working_dir).min_depth(1) {
                                let gcov_path = entry.unwrap();
                                let gcov_path = gcov_path.path();

                                new_results.append(&mut try_parse!(
                                    if gcov_path.extension().unwrap() == "gz" {
                                        parse_gcov_gz(gcov_path)
                                    } else {
                                        parse_gcov(gcov_path)
                                    },
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
                        match Gcno::compute(
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
                            }
                            Err(e) => {
                                // Just print the error, don't panic and continue
                                error!("Error in computing counters: {}", e);
                                Vec::new()
                            }
                        }
                    }
                    ItemType::Content(_) => {
                        error!("Invalid content type");
                        continue;
                    }
                    ItemType::Paths(_) => {
                        error!("Invalid content type");
                        continue;
                    }
                }
            }
            ItemFormat::Profraw => {
                if binary_path.is_none() {
                    error!("The path to the compiled binary must be given as an argument when source-based coverage is used");
                    continue;
                }

                if let ItemType::Paths(profraw_paths) = work_item.item {
                    match llvm_tools::profraws_to_lcov(
                        profraw_paths.as_slice(),
                        binary_path.as_ref().unwrap(),
                        working_dir,
                    ) {
                        Ok(lcovs) => {
                            let mut new_results: Vec<(String, CovResult)> = Vec::new();

                            for lcov in lcovs {
                                new_results.append(&mut try_parse!(
                                    parse_lcov(lcov, branch_enabled),
                                    work_item.name
                                ));
                            }

                            new_results
                        }
                        Err(e) => {
                            error!("Error while executing llvm tools: {}", e);
                            continue;
                        }
                    }
                } else {
                    error!("Invalid content type");
                    continue;
                }
            }
            ItemFormat::Info | ItemFormat::JacocoXml => {
                if let ItemType::Content(content) = work_item.item {
                    if work_item.format == ItemFormat::Info {
                        try_parse!(parse_lcov(content, branch_enabled), work_item.name)
                    } else {
                        let buffer = BufReader::new(Cursor::new(content));
                        try_parse!(parse_jacoco_xml_report(buffer), work_item.name)
                    }
                } else {
                    error!("Invalid content type");
                    continue;
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
    use std::io::Read;
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
        assert!(!func.executed);
        func = result.functions.get("f2").unwrap();
        assert_eq!(func.start, 2);
        assert!(func.executed);
    }

    #[test]
    fn test_merge_relative_path() {
        let mut f = File::open("./test/relative_path/relative_path.info")
            .expect("Failed to open lcov file");
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        let results = parse_lcov(buf, false).unwrap();
        let result_map: Arc<SyncCovResultMap> = Arc::new(Mutex::new(
            FxHashMap::with_capacity_and_hasher(1, Default::default()),
        ));
        add_results(
            results,
            &result_map,
            Some(Path::new("./test/relative_path")),
        );
        let result_map = Arc::try_unwrap(result_map).unwrap().into_inner().unwrap();

        assert!(result_map.len() == 1);

        let cpp_file =
            canonicalize_path(Path::new("./test/relative_path/foo/bar/oof.cpp")).unwrap();
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
        let mut f = File::open("./test/relative_path/relative_path.info")
            .expect("Failed to open lcov file");
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        let results = parse_lcov(buf, false).unwrap();
        let result_map: Arc<SyncCovResultMap> = Arc::new(Mutex::new(
            FxHashMap::with_capacity_and_hasher(3, Default::default()),
        ));
        add_results(results, &result_map, None);
        let result_map = Arc::try_unwrap(result_map).unwrap().into_inner().unwrap();

        assert!(result_map.len() == 3);
    }
}
