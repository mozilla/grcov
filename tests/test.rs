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

fn read_expected(path: &Path, compiler: &str) -> Vec<String> {
    let name = format!("expected_{}.txt", compiler);
    let mut f = File::open(path.join(&name)).expect(format!("{} file not found", name).as_str());
    let mut s = String::new();
    f.read_to_string(&mut s).unwrap();
    let mut v = Vec::new();
    for line in s.lines() {
        v.push(line.to_string());
    }
    v
}

fn run_grcov(path: &Path, llvm: bool) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if llvm {
        args.push("--".to_string());
        args.push("--llvm".to_string());
    }

    let output = Command::new("cargo")
                         .arg("run")
                         .arg(path)
                         .args(args)
                         .output()
                         .expect("Failed to run grcov");
    let s = String::from_utf8(output.stdout).unwrap();
    let mut v = Vec::new();
    for line in s.lines() {
        v.push(line.to_string());
    }
    v
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

fn check_equal(expected_output: &[String], output: &[String]) {
    let mut expected: Vec<Value> = Vec::new();
    for line in expected_output {
        expected.push(serde_json::from_str(line).unwrap());
    }

    let mut actual: Vec<Value> = Vec::new();
    for line in output {
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

    assert_eq!(expected.len(), actual_len, "Got same number of expected records.")
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
            check_equal(&read_expected(path, "gcc"), &run_grcov(path, false));
            make_clean(path);

            // On CI, don't test llvm, as there are problems for now.
            let skip_llvm = env::var("CONTINUOUS_INTEGRATION").is_ok();

            println!("\nLLVM");
            make(path, "clang++");
            run(path);
            if !skip_llvm {
                check_equal(&read_expected(path, "llvm"), &run_grcov(path, true));
            }
            make_clean(path);
        }
    }
}
