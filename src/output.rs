use rustc_hash::FxHashMap;
use serde_json::{self, Value};
use std::cell::RefCell;
use std::collections::{hash_map, BTreeSet};
use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::PathBuf;
use std::rc::Rc;
use uuid::Uuid;
use md5::{Md5, Digest};

use crate::defs::*;

fn get_target_output_writable(output_file: Option<&str>) -> Box<Write> {
    let write_target: Box<Write> = match output_file {
        Some(filename) => Box::new(File::create(filename).unwrap()),
        None => {
            let stdout = io::stdout();
            Box::new(stdout)
        }
    };
    return write_target;
}

pub fn output_activedata_etl(results: CovResultIter, output_file: Option<&str>) {
    let mut writer = BufWriter::new(get_target_output_writable(output_file));

    for (_, rel_path, result) in results {
        let covered: Vec<u32> = result
            .lines
            .iter()
            .filter(|&(_, v)| *v > 0)
            .map(|(k, _)| k)
            .cloned()
            .collect();
        let uncovered: Vec<u32> = result
            .lines
            .iter()
            .filter(|&(_, v)| *v == 0)
            .map(|(k, _)| k)
            .cloned()
            .collect();

        let mut orphan_covered: BTreeSet<u32> = covered.iter().cloned().collect();
        let mut orphan_uncovered: BTreeSet<u32> = uncovered.iter().cloned().collect();

        let end: u32 = result.lines.keys().last().unwrap_or(&0) + 1;

        let mut start_indexes: Vec<u32> = Vec::new();
        for function in result.functions.values() {
            start_indexes.push(function.start);
        }
        start_indexes.sort();

        for (name, function) in &result.functions {
            // println!("{} {} {}", name, function.executed, function.start);

            let mut func_end = end;

            for start in &start_indexes {
                if *start > function.start {
                    func_end = *start;
                    break;
                }
            }

            let mut lines_covered: Vec<u32> = Vec::new();
            for line in covered
                .iter()
                .filter(|&&x| x >= function.start && x < func_end)
            {
                lines_covered.push(*line);
                orphan_covered.remove(line);
            }

            let mut lines_uncovered: Vec<u32> = Vec::new();
            for line in uncovered
                .iter()
                .filter(|&&x| x >= function.start && x < func_end)
            {
                lines_uncovered.push(*line);
                orphan_uncovered.remove(line);
            }

            writeln!(
                writer,
                "{}",
                json!({
                    "language": "c/c++",
                    "file": {
                        "name": rel_path,
                    },
                    "method": {
                        "name": name,
                        "covered": lines_covered,
                        "uncovered": lines_uncovered,
                        "total_covered": lines_covered.len(),
                        "total_uncovered": lines_uncovered.len(),
                        "percentage_covered": lines_covered.len() as f32 / (lines_covered.len() + lines_uncovered.len()) as f32,
                    }
                })
            ).unwrap();
        }

        let orphan_covered: Vec<u32> = orphan_covered.into_iter().collect();
        let orphan_uncovered: Vec<u32> = orphan_uncovered.into_iter().collect();

        // The orphan lines will represent the file as a whole.
        writeln!(
            writer,
            "{}",
            json!({
                "language": "c/c++",
                "is_file": true,
                "file": {
                    "name": rel_path,
                    "covered": covered,
                    "uncovered": uncovered,
                    "total_covered": covered.len(),
                    "total_uncovered": uncovered.len(),
                    "percentage_covered": covered.len() as f32 / (covered.len() + uncovered.len()) as f32,
                },
                "method": {
                    "covered": orphan_covered,
                    "uncovered": orphan_uncovered,
                    "total_covered": orphan_covered.len(),
                    "total_uncovered": orphan_uncovered.len(),
                    "percentage_covered": orphan_covered.len() as f32 / (orphan_covered.len() + orphan_uncovered.len()) as f32,
                }
            })
        ).unwrap();
    }
}

pub fn output_fuzzmanager(results: CovResultIter, output_file: Option<&str>) {
    let mut writer = BufWriter::new(get_target_output_writable(output_file));
    let mut relative: FxHashMap<PathBuf, Rc<RefCell<FMDirStats>>> = FxHashMap::default();
    let global = Rc::new(RefCell::new(FMDirStats::new("".to_string())));
    relative.insert(PathBuf::from(""), global.clone());
    
    for (abs_path, rel_path, result) in results {
        let path = if rel_path.is_relative() {
            rel_path
        } else {
            abs_path
        };

        let parent = path.parent().unwrap();
        let mut ancestors = Vec::new();
        for ancestor in parent.ancestors() {
            ancestors.push(ancestor);
            if relative.contains_key(ancestor) {
                break;
            }
        }

        let mut prev_stats = global.clone();

        while let Some(ancestor) = ancestors.pop() {
            prev_stats = match relative.entry(ancestor.to_path_buf()) {
                hash_map::Entry::Occupied(s) => s.get().clone(),
                hash_map::Entry::Vacant(p) => {
                    let mut prev_stats = prev_stats.borrow_mut();
                    let path_tail = if ancestor == PathBuf::from("/") {
                        "/".to_string()
                    } else {
                        ancestor.file_name().unwrap().to_str().unwrap().to_string()
                    };
                    prev_stats.dirs.push(Rc::new(RefCell::new(FMDirStats::new(path_tail))));
                    let last = prev_stats.dirs.last_mut().unwrap();
                    p.insert(last.clone());
                    last.clone()
                },
            };
        }

        let last_line = *result.lines.keys().last().unwrap_or(&0) as usize;
        let mut lines: Vec<i64> = vec![-1; last_line];
        for (line_num, line_count) in result.lines.iter() {
            unsafe {
                *lines.get_unchecked_mut((*line_num - 1) as usize) = *line_count as i64;
            }
        }
        
        prev_stats.borrow_mut().files.push(FMFileStats::new(path.file_name().unwrap().to_str().unwrap().to_string(), lines));
    }

    let mut global = global.borrow_mut();
    global.set_stats();

    serde_json::to_writer(
        &mut writer,
        &global.to_json(),
    ).unwrap();
}

pub fn output_lcov(results: CovResultIter, output_file: Option<&str>) {
    let mut writer = BufWriter::new(get_target_output_writable(output_file));
    writer.write_all(b"TN:\n").unwrap();

    for (_, rel_path, result) in results {
        // println!("{} {:?}", rel_path, result.lines);

        writeln!(writer, "SF:{}", rel_path.display()).unwrap();

        for (name, function) in &result.functions {
            writeln!(writer, "FN:{},{}", function.start, name).unwrap();
        }
        for (name, function) in &result.functions {
            writeln!(
                writer,
                "FNDA:{},{}",
                if function.executed { 1 } else { 0 },
                name
            )
            .unwrap();
        }
        if !result.functions.is_empty() {
            writeln!(writer, "FNF:{}", result.functions.len()).unwrap();
            writeln!(
                writer,
                "FNH:{}",
                result.functions.values().filter(|x| x.executed).count()
            )
            .unwrap();
        }

        // branch coverage information
        let mut branch_hit = 0;
        for (line, ref taken) in &result.branches {
            for (n, b_t) in taken.iter().enumerate() {
                writeln!(
                    writer,
                    "BRDA:{},0,{},{}",
                    line,
                    n,
                    if *b_t { "1" } else { "-" }
                )
                .unwrap();
                if *b_t {
                    branch_hit += 1;
                }
            }
        }

        writeln!(writer, "BRF:{}", result.branches.len()).unwrap();
        writeln!(writer, "BRH:{}", branch_hit).unwrap();

        for (line, execution_count) in &result.lines {
            writeln!(writer, "DA:{},{}", line, execution_count).unwrap();
        }
        writeln!(writer, "LF:{}", result.lines.len()).unwrap();
        writeln!(
            writer,
            "LH:{}",
            result.lines.values().filter(|&v| *v > 0).count()
        )
        .unwrap();
        writer.write_all(b"end_of_record\n").unwrap();
    }
}

fn get_digest(path: PathBuf) -> String {
    match File::open(path) {
        Ok(mut f) => {
            let mut buffer = Vec::new();
            f.read_to_end(&mut buffer).unwrap();
            let mut hasher = Md5::new();
            hasher.input(buffer.as_slice());
            format!("{:x}", hasher.result())
        }
        Err(_) => Uuid::new_v4().to_string(),
    }
}

pub fn output_coveralls(
    results: CovResultIter,
    repo_token: &str,
    service_name: &str,
    service_number: &str,
    service_job_number: &str,
    commit_sha: &str,
    with_function_info: bool,
    output_file: Option<&str>,
) {
    let mut source_files = Vec::new();

    for (abs_path, rel_path, result) in results {
        let end: u32 = result.lines.keys().last().unwrap_or(&0) + 1;

        let mut coverage = Vec::new();
        for line in 1..end {
            let entry = result.lines.get(&line);
            if let Some(c) = entry {
                coverage.push(Value::from(*c));
            } else {
                coverage.push(Value::Null);
            }
        }

        let mut branches = Vec::new();
        for (line, ref taken) in &result.branches {
            for (n, b_t) in taken.iter().enumerate() {
                branches.push(*line);
                branches.push(0);
                branches.push(n as u32);
                branches.push(if *b_t { 1 } else { 0 });
            }
        }

        if !with_function_info {
            source_files.push(json!({
                "name": rel_path,
                "source_digest": get_digest(abs_path),
                "coverage": coverage,
                "branches": branches,
            }));
        } else {
            let mut functions = Vec::new();
            for (name, function) in &result.functions {
                functions.push(json!({
                    "name": name,
                    "start": function.start,
                    "exec": function.executed,
                }));
            }

            source_files.push(json!({
                "name": rel_path,
                "source_digest": get_digest(abs_path),
                "coverage": coverage,
                "branches": branches,
                "functions": functions,
            }));
        }
    }

    let mut writer = BufWriter::new(get_target_output_writable(output_file));
    serde_json::to_writer(
        &mut writer,
        &json!({
            "repo_token": repo_token,
            "git": {
              "head": {
                "id": commit_sha,
              },
              "branch": "master",
            },
            "source_files": source_files,
            "service_name": service_name,
            "service_number": service_number,
            "service_job_number": service_job_number,
        }),
    )
    .unwrap();
}

pub fn output_files(results: CovResultIter, output_file: Option<&str>) {
    let mut writer = BufWriter::new(get_target_output_writable(output_file));
    for (_, rel_path, _) in results {
        writeln!(writer, "{}", rel_path.display()).unwrap();
    }
}

#[cfg(test)]
mod tests {

    extern crate tempfile;
    use super::*;
    use std::collections::BTreeMap;

    fn read_file(path: &PathBuf) -> String {
        let mut f = File::open(path).expect(format!("{:?} file not found", path.file_name()).as_str());
        let mut s = String::new();
        f.read_to_string(&mut s).unwrap();
        s
    }
    
    #[test]
    fn test_fuzzmanager() {
        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let file_name = "test_fuzzmanager.json";
        let file_path = tmp_dir.path().join(&file_name);

        let results = vec![
            (PathBuf::from("foo/bar/a.cpp"),
             PathBuf::from("foo/bar/a.cpp"),
             CovResult {
                 lines: [(1, 10), (2, 11)].iter().cloned().collect(),
                 branches: BTreeMap::new(),
                 functions: FxHashMap::default(),
             }),
            (PathBuf::from("foo/bar/b.cpp"),
             PathBuf::from("foo/bar/b.cpp"),
             CovResult {
                 lines: [(1, 0), (2, 10), (4, 0)].iter().cloned().collect(),
                 branches: BTreeMap::new(),
                 functions: FxHashMap::default(),
             }),
            (PathBuf::from("foo/c.cpp"),
             PathBuf::from("foo/c.cpp"),
             CovResult {
                 lines: [(1, 10), (4, 1)].iter().cloned().collect(),
                 branches: BTreeMap::new(),
                 functions: FxHashMap::default(),
             }),
            (PathBuf::from("/foo/d.cpp"),
             PathBuf::from("/foo/d.cpp"),
             CovResult {
                 lines: [(1, 10), (2, 0)].iter().cloned().collect(),
                 branches: BTreeMap::new(),
                 functions: FxHashMap::default(),
             }),
        ];

        let results = Box::new(results.into_iter());
        output_fuzzmanager(results, Some(file_path.to_str().unwrap()));        

        let results: Value = serde_json::from_str(&read_file(&file_path)).unwrap();
        let expected_path = PathBuf::from("./test/").join(&file_name);
        let expected: Value = serde_json::from_str(&read_file(&expected_path)).unwrap();

        assert_eq!(results, expected);
    }
}
