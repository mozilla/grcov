extern crate walkdir;
extern crate serde_json;

use std::env;
use std::process::Command;
use walkdir::WalkDir;
use std::path::Path;
use std::fs::File;
use std::io::Read;
use serde_json::Value;

fn make(path: &Path) {
    let status = Command::new("make")
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

fn read_expected(path: &Path) -> Vec<String> {
    let mut f = File::open(path.join("expected.txt")).unwrap();
    let mut s = String::new();
    f.read_to_string(&mut s).unwrap();
    let mut v = Vec::new();
    for line in s.lines() {
        v.push(line.to_string());
    }
    v
}

fn run_grcov(path: &Path) -> Vec<String> {
    let output = Command::new("cargo")
                         .arg("run")
                         .arg(path)
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

fn check_equal(expected_output: Vec<String>, output: Vec<String>) {
    let mut expected: Vec<Value> = Vec::new();
    for line in expected_output.iter() {
        expected.push(serde_json::from_str(line).unwrap());
    }

    let mut actual: Vec<Value> = Vec::new();
    for line in output.iter() {
        actual.push(serde_json::from_str(line).unwrap());
    }

    // On CI and without gcc-6, don't check /usr/include files, as they are different between GCC versions and the expected files are built using gcc-6.
    let skip_builtin = env::var("COMPILER_VER").is_ok() && env::var("COMPILER_VER").unwrap() != "6";
    // On CI, don't check methods, as on different machines names are slightly differently mangled.
    let skip_methods = skip_builtin || env::var("CONTINUOUS_INTEGRATION").is_ok();

    let mut actual_len = 0;
    for out in actual.iter() {
        if out["file"]["name"].as_str().unwrap().contains("/usr/include") && skip_builtin {
            continue;
        }
        actual_len += 1;

        let exp = expected.iter().find(|&&ref x| check_equal_inner(x, out, skip_methods));
        assert!(exp.is_some(), "Got unexpected {}", out);
    }

    let mut expected_len = 0;
    for exp in expected.iter() {
        if exp["file"]["name"].as_str().unwrap().contains("/usr/include") && skip_builtin {
            continue;
        }
        expected_len += 1;

        let out = actual.iter().find(|&&ref x| check_equal_inner(x, exp, skip_methods));
        assert!(out.is_some(), "Missing {}", exp);
    }

    assert_eq!(expected_len, actual_len, "Got same number of expected records.")
}

#[test]
fn test_integration() {
    for entry in WalkDir::new("tests").min_depth(1) {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            println!("{}", path.display());

            make(path);
            run(path);

            check_equal(read_expected(path), run_grcov(path));

            make_clean(path);
        }
    }
}
