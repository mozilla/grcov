extern crate walkdir;
extern crate serde_json;

use std::env;
use std::process::Command;
use walkdir::WalkDir;
use std::path::{PathBuf, Path};
use std::fs::File;
use std::io::Read;
use serde_json::Value;

fn make(path: &Path, compiler: &str) {
    let status = Command::new("make")
                         .arg(format!("COMPILER={}", compiler))
                         .current_dir(path)
                         .status()
                         .expect("Failed to build");
    assert!(status.success());
}

fn run(path: &Path) {
    let program = if !cfg!(windows) {
      PathBuf::from("./a.out")
    } else {
      path.join("a.exe")
    };

    let status = Command::new(program)
                         .current_dir(path)
                         .status()
                         .expect("Failed to run");
    assert!(status.success());
}

fn read_expected(path: &Path, compiler: &str, format: &str) -> String {
    let name = format!("expected_{}.{}", compiler, format);
    let mut f = File::open(path.join(&name)).expect(format!("{} file not found", name).as_str());
    let mut s = String::new();
    f.read_to_string(&mut s).unwrap();
    s
}

fn run_grcov(path: &Path, llvm: bool, output_format: &str) -> String {
    let mut args: Vec<&str> = Vec::new();
    args.push("--");
    if llvm {
        args.push("--llvm");
    }
    args.push("-t");
    args.push(output_format);
    if output_format == "coveralls" {
        args.push("--token");
        args.push("TOKEN");
        args.push("--commit-sha");
        args.push("COMMIT");
        args.push("-s");
        args.push(path.to_str().unwrap());
        args.push("--branch");
    }

    let output = Command::new("cargo")
                         .arg("run")
                         .arg(path)
                         .args(args)
                         .output()
                         .expect("Failed to run grcov");
    let err = String::from_utf8(output.stderr).unwrap();
    println!("{}", err);
    String::from_utf8(output.stdout).unwrap()
}

fn make_clean(path: &Path) {
    let status = Command::new("make")
                         .arg("clean")
                         .current_dir(path)
                         .status()
                         .expect("Failed to clean");
    assert!(status.success());
}

fn check_equal_inner(a: &Value, b: &Value, skip_methods: bool) -> bool {
    a["is_file"] == b["is_file"] &&
    a["language"] == b["language"] &&
    (skip_methods || a["method"]["name"] == b["method"]["name"]) &&
    a["method"]["covered"] == b["method"]["covered"] &&
    a["method"]["uncovered"] == b["method"]["uncovered"] &&
    a["method"]["percentage_covered"] == b["method"]["percentage_covered"] &&
    a["method"]["total_covered"] == b["method"]["total_covered"] &&
    a["method"]["total_uncovered"] == b["method"]["total_uncovered"] &&
    a["file"]["name"] == b["file"]["name"] &&
    a["file"]["covered"] == b["file"]["covered"] &&
    a["file"]["uncovered"] == b["file"]["uncovered"] &&
    a["file"]["percentage_covered"] == b["file"]["percentage_covered"] &&
    a["file"]["total_covered"] == b["file"]["total_covered"] &&
    a["file"]["total_uncovered"] == b["file"]["total_uncovered"]
}

fn check_equal_ade(expected_output: &str, output: &str) {
    let mut expected: Vec<Value> = Vec::new();
    for line in expected_output.lines() {
        expected.push(serde_json::from_str(line).unwrap());
    }

    let mut actual: Vec<Value> = Vec::new();
    for line in output.lines() {
        actual.push(serde_json::from_str(line).unwrap());
    }

    // On CI, don't check methods, as on different machines names are slightly differently mangled.
    let skip_methods = env::var("CONTINUOUS_INTEGRATION").is_ok();

    let mut actual_len = 0;
    for out in &actual {
        if out["file"]["name"].as_str().unwrap().starts_with("/usr/") {
            continue;
        }
        actual_len += 1;

        let exp = expected.iter().find(|x| check_equal_inner(x, out, skip_methods));
        assert!(exp.is_some(), "Got unexpected {} - Expected output: {:?}", out, expected_output);
    }

    for exp in &expected {
        let out = actual.iter().find(|x| check_equal_inner(x, exp, skip_methods));
        assert!(out.is_some(), "Missing {} - Full output: {:?}", exp, output);
    }

    assert_eq!(expected.len(), actual_len, "Got same number of expected records.");
}

fn check_equal_coveralls(expected_output: &str, output: &str, skip_branches: bool) {
    let expected: Value = serde_json::from_str(expected_output).unwrap();
    let actual: Value = serde_json::from_str(output).unwrap();

    println!("{}", serde_json::to_string_pretty(&actual).unwrap());

    assert_eq!(expected["git"]["branch"], actual["git"]["branch"]);
    assert_eq!(expected["git"]["head"]["id"], actual["git"]["head"]["id"]);
    assert_eq!(expected["repo_token"], actual["repo_token"]);
    assert_eq!(expected["service_job_number"], actual["service_job_number"]);
    assert_eq!(expected["service_name"], actual["service_name"]);
    assert_eq!(expected["service_number"], actual["service_number"]);

    // On CI, don't check line counts, as on different compiler versions they are slightly different.
    let skip_line_counts = env::var("CONTINUOUS_INTEGRATION").is_ok();

    let actual_source_files = actual["source_files"].as_array().unwrap();
    let expected_source_files = expected["source_files"].as_array().unwrap();

    for exp in expected_source_files {
        let out = actual_source_files.iter().find(|x| x["name"] == exp["name"]);
        assert!(out.is_some(), "Missing {} - Full output: {:?}", exp, output);

        let out = out.unwrap();

        assert_eq!(exp["name"], out["name"]);
        assert_eq!(exp["source_digest"], out["source_digest"], "Got correct digest for {}", exp["name"]);
        if !skip_line_counts {
            assert_eq!(exp["coverage"], out["coverage"], "Got correct coverage for {}", exp["name"]);
        } else {
            let expected_coverage = exp["coverage"].as_array().unwrap();
            let actual_coverage = out["coverage"].as_array().unwrap();
            assert_eq!(expected_coverage.len(), actual_coverage.len(), "Got same number of lines.");
            for i in 0..expected_coverage.len() {
                if expected_coverage[i].is_null() {
                    assert!(actual_coverage[i].is_null(), "Got correct coverage at line {} for {}", i, exp["name"]);
                } else {
                    assert_eq!(expected_coverage[i].as_i64().unwrap() > 0, actual_coverage[i].as_i64().unwrap() > 0, "Got correct coverage at line {} for {}", i, exp["name"]);
                }
            }
        }
        if !skip_line_counts || !skip_branches {
            assert_eq!(exp["branches"], out["branches"], "Got correct branch coverage for {}", exp["name"]);
        }
    }

    for out in actual_source_files {
        let exp = expected_source_files.iter().find(|x| x["name"] == out["name"]);
        assert!(exp.is_some(), "Got unexpected {} - Expected output: {:?}", out, expected_output);
    }

    assert_eq!(expected_source_files.len(), actual_source_files.len(), "Got same number of source files.");
}

#[test]
fn test_integration() {
    for entry in WalkDir::new("tests").min_depth(1) {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            println!("\n\n{}", path.display());

            let skip_branches = path == Path::new("tests/template") || path == Path::new("tests/include") ||
                                path == Path::new("tests/include2") || path == Path::new("tests/class");

            make_clean(path);

            println!("GCC");
            make(path, "g++");
            run(path);
            check_equal_ade(&read_expected(path, "gcc", "ade"), &run_grcov(path, false, "ade"));
            check_equal_coveralls(&read_expected(path, "gcc", "coveralls"), &run_grcov(path, false, "coveralls"), skip_branches);
            make_clean(path);

            // On CI, don't test llvm, as there are problems for now.
            let skip_llvm = env::var("CONTINUOUS_INTEGRATION").is_ok();

            println!("\nLLVM");
            make(path, "clang++");
            run(path);
            if !skip_llvm {
                check_equal_ade(&read_expected(path, "llvm", "ade"), &run_grcov(path, true, "ade"));
                check_equal_coveralls(&read_expected(path, "llvm", "coveralls"), &run_grcov(path, true, "coveralls"), skip_branches);
            }
            make_clean(path);
        }
    }
}
