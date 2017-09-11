extern crate walkdir;
extern crate serde_json;

use std::env;
use std::process::Command;
use walkdir::WalkDir;
use std::path::Path;
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
    let status = Command::new("./a.out")
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
    let mut args: Vec<String> = Vec::new();
    args.push("--".to_string());
    if llvm {
        args.push("--llvm".to_string());
    }
    args.push("-t".to_string());
    args.push(output_format.to_string());
    if output_format == "coveralls" {
        args.push("--token".to_string());
        args.push("TOKEN".to_string());
        args.push("--commit-sha".to_string());
        args.push("COMMIT".to_string());
        args.push("-s".to_string());
        args.push(path.to_str().unwrap().to_string());
    }

    let output = Command::new("cargo")
                         .arg("run")
                         .arg(path)
                         .args(args)
                         .output()
                         .expect("Failed to run grcov");
    let s = String::from_utf8(output.stdout).unwrap();
    s
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

fn check_equal_ade(expected_output: &String, output: &String) {
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

        let exp = expected.iter().find(|&&ref x| check_equal_inner(x, out, skip_methods));
        assert!(exp.is_some(), "Got unexpected {} - Expected output: {:?}", out, expected_output);
    }

    for exp in &expected {
        let out = actual.iter().find(|&&ref x| check_equal_inner(x, exp, skip_methods));
        assert!(out.is_some(), "Missing {} - Full output: {:?}", exp, output);
    }

    assert_eq!(expected.len(), actual_len, "Got same number of expected records.");
}

fn check_equal_coveralls(expected_output: &String, output: &String) {
    let expected: Value = serde_json::from_str(expected_output).unwrap();
    let actual: Value = serde_json::from_str(output).unwrap();

    assert_eq!(expected["git"]["branch"], actual["git"]["branch"]);
    assert_eq!(expected["git"]["head"]["id"], actual["git"]["head"]["id"]);
    assert_eq!(expected["repo_token"], actual["repo_token"]);
    assert_eq!(expected["service_job_number"], actual["service_job_number"]);
    assert_eq!(expected["service_name"], actual["service_name"]);
    assert_eq!(expected["service_number"], actual["service_number"]);

    let mut actual_len = 0;

    let actual_source_files = actual["source_files"].as_array().unwrap();
    let expected_source_files = expected["source_files"].as_array().unwrap();

    for out in actual_source_files {
        let exp = expected_source_files.iter().find(|&&ref x| x["name"] == out["name"]);
        assert!(exp.is_some(), "Got unexpected {} - Expected output: {:?}", out, expected_output);

        let exp = exp.unwrap();

        assert_eq!(exp["name"], out["name"]);
        assert_eq!(exp["source_digest"], out["source_digest"], "Got wrong digest for {}", exp["name"]);
        assert_eq!(exp["coverage"], out["coverage"], "Got wrong coverage for {}", exp["name"]);

        actual_len += 1;
    }

    for exp in expected_source_files {
        let out = actual_source_files.iter().find(|&&ref x| x["name"] == exp["name"]);
        assert!(out.is_some(), "Missing {} - Full output: {:?}", exp, output);

        let out = out.unwrap();

        assert_eq!(out["name"], exp["name"]);
        assert_eq!(out["source_digest"], exp["source_digest"], "Got wrong digest for {}", out["name"]);
        assert_eq!(out["coverage"], exp["coverage"], "Got wrong coverage for {}", out["name"]);
    }

    assert_eq!(expected_source_files.len(), actual_len, "Got same number of source files.");
}

#[test]
fn test_integration() {
    for entry in WalkDir::new("tests").min_depth(1) {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            println!("\n\n{}", path.display());

            make_clean(path);

            println!("GCC");
            make(path, "g++");
            run(path);
            check_equal_ade(&read_expected(path, "gcc", "ade"), &run_grcov(path, false, "ade"));
            check_equal_coveralls(&read_expected(path, "gcc", "coveralls"), &run_grcov(path, false, "coveralls"));
            make_clean(path);

            // On CI, don't test llvm, as there are problems for now.
            let skip_llvm = env::var("CONTINUOUS_INTEGRATION").is_ok();

            println!("\nLLVM");
            make(path, "clang++");
            run(path);
            if !skip_llvm {
                check_equal_ade(&read_expected(path, "llvm", "ade"), &run_grcov(path, true, "ade"));
                check_equal_coveralls(&read_expected(path, "llvm", "coveralls"), &run_grcov(path, true, "coveralls"));
            }
            make_clean(path);
        }
    }
}
