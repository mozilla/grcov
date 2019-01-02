use serde_json::{self, Value};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::PathBuf;
use uuid::Uuid;
extern crate md5;

use defs::*;

pub fn output_activedata_etl(results: CovResultIter) {
    let stdout = io::stdout();
    let mut writer = BufWriter::new(stdout.lock());

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

pub fn output_lcov(results: CovResultIter) {
    let stdout = io::stdout();
    let mut writer = BufWriter::new(stdout.lock());

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
            ).unwrap();
        }
        if !result.functions.is_empty() {
            writeln!(writer, "FNF:{}", result.functions.len()).unwrap();
            writeln!(
                writer,
                "FNH:{}",
                result.functions.values().filter(|x| x.executed).count()
            ).unwrap();
        }

        // branch coverage information
        let mut branch_hit = 0;
        for (&(line, number), &taken) in &result.branches {
            writeln!(
                writer,
                "BRDA:{},0,{},{}",
                line,
                number,
                if taken { "1" } else { "-" }
            ).unwrap();
            if taken {
                branch_hit += 1;
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
        ).unwrap();
        writer.write_all(b"end_of_record\n").unwrap();
    }
}

fn get_digest(path: PathBuf) -> String {
    match File::open(path) {
        Ok(mut f) => {
            let mut buffer = Vec::new();
            f.read_to_end(&mut buffer).unwrap();
            format!("{:x}", md5::compute(buffer.as_slice()))
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
        for (&(line, number), &taken) in &result.branches {
            branches.push(line);
            branches.push(0);
            branches.push(number);
            branches.push(if taken { 1 } else { 0 });
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

    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    serde_json::to_writer(
        &mut stdout,
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
    ).unwrap();
}

pub fn output_files(results: CovResultIter) {
    let stdout = io::stdout();
    let mut writer = BufWriter::new(stdout.lock());

    for (_, rel_path, _) in results {
        writeln!(writer, "{}", rel_path.display()).unwrap();
    }
}
