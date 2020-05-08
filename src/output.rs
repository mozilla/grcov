use crossbeam::crossbeam_channel::unbounded;
use md5::{Digest, Md5};
use rustc_hash::FxHashMap;
use serde_json::{self, json, Value};
use std::cell::RefCell;
use std::collections::{hash_map, BTreeSet};
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::{
    process::{self, Command, Stdio},
    thread,
};
use uuid::Uuid;

use crate::defs::*;
use crate::html;

fn get_target_output_writable(output_file: Option<&str>) -> Box<dyn Write> {
    let write_target: Box<dyn Write> = match output_file {
        Some(filename) => Box::new(File::create(filename).unwrap()),
        None => {
            let stdout = io::stdout();
            Box::new(stdout)
        }
    };
    write_target
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

pub fn output_covdir(results: CovResultIter, output_file: Option<&str>) {
    let mut writer = BufWriter::new(get_target_output_writable(output_file));
    let mut relative: FxHashMap<PathBuf, Rc<RefCell<CDDirStats>>> = FxHashMap::default();
    let global = Rc::new(RefCell::new(CDDirStats::new("".to_string())));
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
                    prev_stats
                        .dirs
                        .push(Rc::new(RefCell::new(CDDirStats::new(path_tail))));
                    let last = prev_stats.dirs.last_mut().unwrap();
                    p.insert(last.clone());
                    last.clone()
                }
            };
        }

        prev_stats.borrow_mut().files.push(CDFileStats::new(
            path.file_name().unwrap().to_str().unwrap().to_string(),
            result.lines,
        ));
    }

    let mut global = global.borrow_mut();
    global.set_stats();

    serde_json::to_writer(&mut writer, &global.to_json()).unwrap();
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
        let mut branch_count = 0;
        let mut branch_hit = 0;
        for (line, ref taken) in &result.branches {
            branch_count += taken.len();
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

        writeln!(writer, "BRF:{}", branch_count).unwrap();
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
    if let Ok(mut f) = File::open(path) {
        let mut buffer = Vec::new();
        f.read_to_end(&mut buffer).unwrap();
        let mut hasher = Md5::new();
        hasher.input(buffer.as_slice());
        format!("{:x}", hasher.result())
    } else {
        Uuid::new_v4().to_string()
    }
}

/// Runs git with given array of arguments (as strings), and returns whatever git printed to
/// stdout. On error, returns empty string. Standard input and error are redirected from/to null.
fn get_git_output<I, S>(args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new("git")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .and_then(|child| child.wait_with_output())
        .ok() // Discard the error type -- we won't handle it anyway
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .unwrap_or_else(|| String::new())
}

/// Returns a JSON object describing the given commit. Coveralls uses that to display commit info.
///
/// \a vcs_branch is what user passed on the command line via `--vcs-branch`. This is included in
/// the output, but doesn't affect the rest of the info (e.g. this function doesn't check if that
/// branch actually points to the given commit).
fn get_coveralls_git_info(commit_sha: &str, vcs_branch: &str) -> Value {
    let status = Command::new("git")
        .arg("status")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|exit_status| exit_status.success());
    if let Ok(true) = status {
        // We have a valid Git repo -- the rest of the function will handle this case
    } else {
        return json!({
            "head": {
                "id": commit_sha,
            },
            "branch": vcs_branch,
        });
    }

    // Runs `git log` with a given format, to extract some piece of commit info. On failure,
    // returns empty string.
    let gitlog = |format| -> String {
        get_git_output(&[
            "log",
            "--max-count=1",
            &format!("--pretty=format:{}", format),
            commit_sha,
        ])
    };

    let author_name = gitlog("%aN");
    let author_email = gitlog("%ae");
    let committer_name = gitlog("%cN");
    let committer_email = gitlog("%ce");
    let message = gitlog("%s");

    let remotes: Value = {
        let output = get_git_output(&["remote", "--verbose"]);

        let mut remotes = Vec::<Value>::new();
        for line in output.lines() {
            if line.ends_with(" (fetch)") {
                let mut fields = line.split_whitespace();
                match (fields.next(), fields.next()) {
                    (Some(name), Some(url)) => remotes.push(json!({"name": name, "url": url})),
                    _ => (),
                };
            }
        }
        json!(remotes)
    };

    json!({
        "head": {
            "id": commit_sha,
            "author_name": author_name,
            "author_email": author_email,
            "committer_name": committer_name,
            "committer_email": committer_email,
            "message": message,
        },
        "branch": vcs_branch,
        "remotes": remotes,
    })
}

pub fn output_coveralls(
    results: CovResultIter,
    repo_token: Option<&str>,
    service_name: Option<&str>,
    service_number: &str,
    service_job_id: Option<&str>,
    service_pull_request: &str,
    commit_sha: &str,
    with_function_info: bool,
    output_file: Option<&str>,
    vcs_branch: &str,
    parallel: bool,
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

    let git = get_coveralls_git_info(commit_sha, vcs_branch);

    let mut result = json!({
        "git": git,
        "source_files": source_files,
        "service_number": service_number,
        "service_pull_request": service_pull_request,
        "parallel": parallel,
    });

    if let (Some(repo_token), Some(obj)) = (repo_token, result.as_object_mut()) {
        obj.insert("repo_token".to_string(), json!(repo_token));
    }

    if let (Some(service_name), Some(obj)) = (service_name, result.as_object_mut()) {
        obj.insert("service_name".to_string(), json!(service_name));
    }

    if let (Some(service_job_id), Some(obj)) = (service_job_id, result.as_object_mut()) {
        obj.insert("service_job_id".to_string(), json!(service_job_id));
    }

    let mut writer = BufWriter::new(get_target_output_writable(output_file));
    serde_json::to_writer(&mut writer, &result).unwrap();
}

pub fn output_files(results: CovResultIter, output_file: Option<&str>) {
    let mut writer = BufWriter::new(get_target_output_writable(output_file));
    for (_, rel_path, _) in results {
        writeln!(writer, "{}", rel_path.display()).unwrap();
    }
}

pub fn output_html(results: CovResultIter, output_dir: Option<&str>, num_threads: usize) {
    let output = if let Some(output_dir) = output_dir {
        PathBuf::from(output_dir)
    } else {
        PathBuf::from("./html")
    };

    if output.exists() {
        if !output.is_dir() {
            eprintln!("{} is not a directory", output.to_str().unwrap());
            return;
        }
    } else if std::fs::create_dir(&output).is_err() {
        eprintln!("Cannot create directory {}", output.to_str().unwrap());
        return;
    }

    let (sender, receiver) = unbounded();

    let stats = Arc::new(Mutex::new(HtmlGlobalStats::default()));
    let mut threads = Vec::with_capacity(num_threads);
    let config = html::get_config();
    for i in 0..num_threads {
        let receiver = receiver.clone();
        let output = output.clone();
        let config = config.clone();
        let stats = stats.clone();
        let t = thread::Builder::new()
            .name(format!("Consumer HTML {}", i))
            .spawn(move || {
                html::consumer_html(receiver, stats, output, config);
            })
            .unwrap();

        threads.push(t);
    }

    for (abs_path, rel_path, result) in results {
        sender
            .send(Some(HtmlItem {
                abs_path,
                rel_path,
                result,
            }))
            .unwrap();
    }

    for _ in 0..num_threads {
        sender.send(None).unwrap();
    }

    for t in threads {
        if t.join().is_err() {
            process::exit(1);
        }
    }

    let global = Arc::try_unwrap(stats).unwrap().into_inner().unwrap();

    html::gen_index(global, config, &output);
    html::write_static_files(output);
}

#[cfg(test)]
mod tests {

    extern crate tempfile;
    use super::*;
    use std::collections::BTreeMap;

    fn read_file(path: &PathBuf) -> String {
        let mut f =
            File::open(path).expect(format!("{:?} file not found", path.file_name()).as_str());
        let mut s = String::new();
        f.read_to_string(&mut s).unwrap();
        s
    }

    #[test]
    fn test_lcov_brf_brh() {
        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let file_name = "test_lcov_brf_brh.info";
        let file_path = tmp_dir.path().join(&file_name);

        let results = vec![(
            PathBuf::from("foo/bar/a.cpp"),
            PathBuf::from("foo/bar/a.cpp"),
            CovResult {
                lines: [(1, 10), (2, 11)].iter().cloned().collect(),
                branches: {
                    let mut map = BTreeMap::new();
                    // 3 hit branches over 10
                    map.insert(1, vec![true, false, false, true, false, false]);
                    map.insert(2, vec![false, false, false, true]);
                    map
                },
                functions: FxHashMap::default(),
            },
        )];

        let results = Box::new(results.into_iter());
        output_lcov(results, Some(file_path.to_str().unwrap()));

        let results = read_file(&file_path);

        assert!(results.contains("BRF:10\n"));
        assert!(results.contains("BRH:3\n"));
    }

    #[test]
    fn test_covdir() {
        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let file_name = "test_covdir.json";
        let file_path = tmp_dir.path().join(&file_name);

        let results = vec![
            (
                PathBuf::from("foo/bar/a.cpp"),
                PathBuf::from("foo/bar/a.cpp"),
                CovResult {
                    lines: [(1, 10), (2, 11)].iter().cloned().collect(),
                    branches: BTreeMap::new(),
                    functions: FxHashMap::default(),
                },
            ),
            (
                PathBuf::from("foo/bar/b.cpp"),
                PathBuf::from("foo/bar/b.cpp"),
                CovResult {
                    lines: [(1, 0), (2, 10), (4, 0)].iter().cloned().collect(),
                    branches: BTreeMap::new(),
                    functions: FxHashMap::default(),
                },
            ),
            (
                PathBuf::from("foo/c.cpp"),
                PathBuf::from("foo/c.cpp"),
                CovResult {
                    lines: [(1, 10), (4, 1)].iter().cloned().collect(),
                    branches: BTreeMap::new(),
                    functions: FxHashMap::default(),
                },
            ),
            (
                PathBuf::from("/foo/d.cpp"),
                PathBuf::from("/foo/d.cpp"),
                CovResult {
                    lines: [(1, 10), (2, 0)].iter().cloned().collect(),
                    branches: BTreeMap::new(),
                    functions: FxHashMap::default(),
                },
            ),
        ];

        let results = Box::new(results.into_iter());
        output_covdir(results, Some(file_path.to_str().unwrap()));

        let results: Value = serde_json::from_str(&read_file(&file_path)).unwrap();
        let expected_path = PathBuf::from("./test/").join(&file_name);
        let expected: Value = serde_json::from_str(&read_file(&expected_path)).unwrap();

        assert_eq!(results, expected);
    }

    #[test]
    fn test_coveralls_service_job_id() {
        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let file_name = "test_coveralls_service_job_id.json";
        let file_path = tmp_dir.path().join(&file_name);

        let results = vec![(
            PathBuf::from("foo/bar/a.cpp"),
            PathBuf::from("foo/bar/a.cpp"),
            CovResult {
                lines: [(1, 10), (2, 11)].iter().cloned().collect(),
                branches: BTreeMap::new(),
                functions: FxHashMap::default(),
            },
        )];

        let results = Box::new(results.into_iter());
        let expected_service_job_id: &str = "100500";
        let with_function_info: bool = true;
        let parallel: bool = true;
        output_coveralls(
            results,
            None,
            None,
            "unused",
            Some(expected_service_job_id),
            "unused",
            "unused",
            with_function_info,
            Some(file_path.to_str().unwrap()),
            "unused",
            parallel,
        );

        let results: Value = serde_json::from_str(&read_file(&file_path)).unwrap();

        assert_eq!(results["service_job_id"], expected_service_job_id);
    }

    #[test]
    fn test_coveralls_token_field_is_absent_if_arg_is_none() {
        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let file_name = "test_coveralls_token.json";
        let file_path = tmp_dir.path().join(&file_name);

        let results = vec![(
            PathBuf::from("foo/bar/a.cpp"),
            PathBuf::from("foo/bar/a.cpp"),
            CovResult {
                lines: [(1, 10), (2, 11)].iter().cloned().collect(),
                branches: BTreeMap::new(),
                functions: FxHashMap::default(),
            },
        )];

        let results = Box::new(results.into_iter());
        let token = None;
        let with_function_info: bool = true;
        let parallel: bool = true;
        output_coveralls(
            results,
            token,
            None,
            "unused",
            None,
            "unused",
            "unused",
            with_function_info,
            Some(file_path.to_str().unwrap()),
            "unused",
            parallel,
        );

        let results: Value = serde_json::from_str(&read_file(&file_path)).unwrap();

        assert_eq!(results.get("repo_token"), None);
    }

    #[test]
    fn test_coveralls_service_fields_are_absent_if_args_are_none() {
        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let file_name = "test_coveralls_service_fields.json";
        let file_path = tmp_dir.path().join(&file_name);

        let results = vec![(
            PathBuf::from("foo/bar/a.cpp"),
            PathBuf::from("foo/bar/a.cpp"),
            CovResult {
                lines: [(1, 10), (2, 11)].iter().cloned().collect(),
                branches: BTreeMap::new(),
                functions: FxHashMap::default(),
            },
        )];

        let results = Box::new(results.into_iter());
        let service_name = None;
        let service_job_id = None;
        let with_function_info: bool = true;
        let parallel: bool = true;
        output_coveralls(
            results,
            None,
            service_name,
            "unused",
            service_job_id,
            "unused",
            "unused",
            with_function_info,
            Some(file_path.to_str().unwrap()),
            "unused",
            parallel,
        );

        let results: Value = serde_json::from_str(&read_file(&file_path)).unwrap();

        assert_eq!(results.get("service_name"), None);
        assert_eq!(results.get("service_job_id"), None);
    }
}
