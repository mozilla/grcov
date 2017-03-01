extern crate walkdir;

use std::process::Command;
use walkdir::WalkDir;
use std::path::Path;
use std::fs::File;
use std::io::BufReader;
use std::io::BufRead;
use std::io::Read;
use std::str::Lines;

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

#[test]
fn test_integration() {
    for entry in WalkDir::new("tests").min_depth(1) {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            make(path);
            run(path);

            let expected_output = read_expected(path);

            let output = run_grcov(path);

            for line in expected_output.iter() {
                assert_eq!(output.iter().find(|&&ref x| x == line), Some(line), "Expected result not present for {}", path.display());
            }

            for line in output.iter() {
                assert_eq!(expected_output.iter().find(|&&ref x| x == line), Some(line), "Unexpected result present for {}", path.display());
            }

            make_clean(path);
        }
    }
}
