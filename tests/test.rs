extern crate walkdir;

use std::process::Command;
use walkdir::WalkDir;
use std::path::Path;
use std::fs::File;
use std::io::BufReader;
use std::io::BufRead;
use std::io::Read;

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

fn read_expected(path: &Path) -> String {
    let mut f = File::open(path.join("expected.txt")).unwrap();
    let mut s = String::new();
    f.read_to_string(&mut s).unwrap();
    s
}

fn run_grcov(path: &Path) -> String {
    let output = Command::new("cargo")
                         .arg("run")
                         .arg(path)
                         .output()
                         .expect("Failed to run grcov");
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

            assert_eq!(output, expected_output);

            make_clean(path);
        }
    }
}
