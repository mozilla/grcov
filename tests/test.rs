use globset::{Glob, GlobSetBuilder};
use regex::Regex;
use serde_json::Value;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::{env, fs};
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

fn get_tool(name: &str, default: &str) -> String {
    match env::var(name) {
        Ok(s) => s,
        Err(_) => default.to_string(),
    }
}

fn make(path: &Path, compiler: &str) {
    let mut args = Vec::new();

    let c_cpp_globs = vec!["*.cpp", "*.c"];

    let mut glob_builder = GlobSetBuilder::new();
    for c_cpp_glob in &c_cpp_globs {
        glob_builder.add(Glob::new(c_cpp_glob).unwrap());
    }
    let c_cpp_globset = glob_builder.build().unwrap();
    for entry in WalkDir::new(path) {
        let entry = entry.expect("Failed to open directory.");

        if c_cpp_globset.is_match(entry.file_name()) {
            args.push(entry.file_name().to_os_string());
        }
    }

    let status = Command::new(compiler)
        .arg("-fprofile-arcs")
        .arg("-ftest-coverage")
        .arg("-O0")
        .arg("-fno-inline")
        .args(args)
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

    // Clean profraw files after running the binary, or grcov will think source-based coverage was used.
    rm_files(path, vec!["*.profraw"]);
}

fn read_file(path: &Path) -> String {
    println!("Read file: {:?}", path);
    let mut f =
        File::open(path).unwrap_or_else(|_| panic!("{:?} file not found", path.file_name()));
    let mut s = String::new();
    f.read_to_string(&mut s).unwrap();
    s
}

fn read_expected(
    path: &Path,
    compiler: &str,
    compiler_ver: &str,
    format: &str,
    additional: Option<&str>,
) -> String {
    let os_name = if cfg!(windows) {
        "win"
    } else if cfg!(target_os = "macos") {
        "mac"
    } else {
        "linux"
    };

    let base_name = format!("expected{}", additional.unwrap_or_default());

    let name_with_ver_and_os = format!(
        "{}_{}_{}_{}.{}",
        base_name, compiler, compiler_ver, os_name, format
    );

    let name = if path.join(&name_with_ver_and_os).exists() {
        name_with_ver_and_os
    } else {
        let name_with_ver = format!("{}_{}_{}.{}", base_name, compiler, compiler_ver, format);
        if path.join(&name_with_ver).exists() {
            name_with_ver
        } else {
            let name_with_os = format!("{}_{}_{}.{}", base_name, compiler, os_name, format);
            if path.join(&name_with_os).exists() {
                name_with_os
            } else {
                format!("{}_{}.{}", base_name, compiler, format)
            }
        }
    };
    read_file(&path.join(name))
}

/// Returns the path to grcov executable.
fn get_cmd_path() -> &'static str {
    let mut cmd_path = if cfg!(windows) {
        ".\\target\\debug\\grcov.exe"
    } else {
        "./target/debug/grcov"
    };

    if !PathBuf::from(cmd_path).exists() {
        cmd_path = if cfg!(windows) {
            ".\\target\\release\\grcov.exe"
        } else {
            "./target/release/grcov"
        };
    }

    cmd_path
}

fn run_grcov(paths: Vec<&Path>, source_root: &Path, output_format: &str) -> String {
    let mut args: Vec<&str> = Vec::new();

    for path in &paths {
        args.push(path.to_str().unwrap());
    }
    args.push("-t");
    args.push(output_format);
    if output_format == "coveralls" {
        args.push("--token");
        args.push("TOKEN");
        args.push("--service-name");
        args.push("");
        args.push("--service-job-id");
        args.push("");
        args.push("--commit-sha");
        args.push("COMMIT");
        args.push("-s");
        args.push(source_root.to_str().unwrap());
        args.push("--branch");
    }
    args.push("--ignore");
    args.push("C:/*");
    args.push("--ignore");
    args.push("/usr/*");
    args.push("--ignore");
    args.push("/Applications/*");
    args.push("--guess-directory-when-missing");

    let output = Command::new(get_cmd_path())
        .args(args)
        .output()
        .expect("Failed to run grcov");
    let err = String::from_utf8(output.stderr).unwrap();
    eprintln!("{}", err);
    String::from_utf8(output.stdout).unwrap()
}

fn rm_files(directory: &Path, file_globs: Vec<&str>) {
    let mut glob_builder = GlobSetBuilder::new();
    for file_glob in &file_globs {
        glob_builder.add(Glob::new(file_glob).unwrap());
    }
    let to_remove_globset = glob_builder.build().unwrap();

    for entry in WalkDir::new(directory) {
        let entry = entry.expect("Failed to open directory.");

        if to_remove_globset.is_match(entry.file_name()) {
            fs::remove_file(entry.path()).unwrap();
        }
    }
}

fn do_clean(directory: &Path) {
    rm_files(
        directory,
        vec!["a.out", "a.exe", "*.gcno", "*.gcda", "*.zip", "*.profraw"],
    );
}

fn check_equal_inner(a: &Value, b: &Value, skip_methods: bool) -> bool {
    a["is_file"] == b["is_file"]
        && a["language"] == b["language"]
        && (skip_methods || a["method"]["name"] == b["method"]["name"])
        && a["method"]["covered"] == b["method"]["covered"]
        && a["method"]["uncovered"] == b["method"]["uncovered"]
        && a["method"]["percentage_covered"] == b["method"]["percentage_covered"]
        && a["method"]["total_covered"] == b["method"]["total_covered"]
        && a["method"]["total_uncovered"] == b["method"]["total_uncovered"]
        && a["file"]["name"] == b["file"]["name"]
        && a["file"]["covered"] == b["file"]["covered"]
        && a["file"]["uncovered"] == b["file"]["uncovered"]
        && a["file"]["percentage_covered"] == b["file"]["percentage_covered"]
        && a["file"]["total_covered"] == b["file"]["total_covered"]
        && a["file"]["total_uncovered"] == b["file"]["total_uncovered"]
}

fn check_equal_ade(expected_output: &str, output: &str) {
    let mut expected: Vec<Value> = Vec::new();
    for line in expected_output.lines() {
        expected.push(serde_json::from_str(line).unwrap());
    }

    let mut actual: Vec<Value> = Vec::new();
    for line in output.lines() {
        let parsed = serde_json::from_str(line).unwrap();
        println!("{}", parsed);
        actual.push(parsed);
    }

    // On CI, don't check methods, as on different machines names are slightly differently mangled.
    let skip_methods = env::var("CONTINUOUS_INTEGRATION").is_ok();

    let mut actual_len = 0;
    for out in &actual {
        if out["file"]["name"].as_str().unwrap().starts_with("/usr/") {
            continue;
        }
        actual_len += 1;

        let exp = expected
            .iter()
            .find(|x| check_equal_inner(x, out, skip_methods));
        assert!(
            exp.is_some(),
            "Got unexpected {} - Expected one of: {}",
            out,
            expected_output
        );
    }

    for exp in &expected {
        let out = actual
            .iter()
            .find(|x| check_equal_inner(x, exp, skip_methods));
        assert!(out.is_some(), "Missing {} - Full output: {}", exp, output);
    }

    assert_eq!(
        expected.len(),
        actual_len,
        "Got same number of expected records."
    );
}

fn check_equal_coveralls(expected_output: &str, output: &str, skip_branches: bool) {
    let expected: Value = serde_json::from_str(expected_output).unwrap();
    let actual: Value = serde_json::from_str(output).unwrap();

    println!("{}", serde_json::to_string_pretty(&actual).unwrap());

    assert_eq!(expected["git"]["branch"], actual["git"]["branch"]);
    assert_eq!(expected["git"]["head"]["id"], actual["git"]["head"]["id"]);
    assert_eq!(expected["repo_token"], actual["repo_token"]);
    assert_eq!(expected["service_job_id"], actual["service_job_id"]);
    assert_eq!(expected["service_name"], actual["service_name"]);
    assert_eq!(expected["service_number"], actual["service_number"]);

    // On CI, don't check line counts, as on different compiler versions they are slightly different.
    let skip_line_counts = env::var("CONTINUOUS_INTEGRATION").is_ok();

    let actual_source_files = actual["source_files"].as_array().unwrap();
    let expected_source_files = expected["source_files"].as_array().unwrap();

    for exp in expected_source_files {
        let out = actual_source_files
            .iter()
            .find(|x| x["name"] == exp["name"]);
        assert!(out.is_some(), "Missing {} - Full output: {:?}", exp, output);

        let out = out.unwrap();

        assert_eq!(exp["name"], out["name"]);
        assert_eq!(
            exp["source_digest"], out["source_digest"],
            "Got correct digest for {}",
            exp["name"]
        );
        if !skip_line_counts {
            assert_eq!(
                exp["coverage"], out["coverage"],
                "Got correct coverage for {}",
                exp["name"]
            );
        } else {
            let expected_coverage = exp["coverage"].as_array().unwrap();
            let actual_coverage = out["coverage"].as_array().unwrap();
            assert_eq!(
                expected_coverage.len(),
                actual_coverage.len(),
                "Got same number of lines."
            );
            for i in 0..expected_coverage.len() {
                if expected_coverage[i].is_null() {
                    assert!(
                        actual_coverage[i].is_null(),
                        "Got correct coverage at line {} for {}",
                        i,
                        exp["name"]
                    );
                } else {
                    assert_eq!(
                        expected_coverage[i].as_i64().unwrap() > 0,
                        actual_coverage[i].as_i64().unwrap() > 0,
                        "Got correct coverage at line {} for {}",
                        i,
                        exp["name"]
                    );
                }
            }
        }
        if !skip_line_counts || !skip_branches {
            assert_eq!(
                exp["branches"], out["branches"],
                "Got correct branch coverage for {}",
                exp["name"]
            );
        }
    }

    for out in actual_source_files {
        let exp = expected_source_files
            .iter()
            .find(|x| x["name"] == out["name"]);
        assert!(
            exp.is_some(),
            "Got unexpected {} - Expected output: {:?}",
            out,
            expected_output
        );
    }

    assert_eq!(
        expected_source_files.len(),
        actual_source_files.len(),
        "Got same number of source files."
    );
}

fn check_equal_covdir(expected_output: &str, output: &str) {
    let expected: Value = serde_json::from_str(expected_output).unwrap();
    let actual: Value = serde_json::from_str(output).unwrap();

    println!("{}", serde_json::to_string_pretty(&actual).unwrap());

    for field in &[
        "coveragePercent",
        "linesCovered",
        "linesMissed",
        "linesTotal",
        "name",
    ] {
        assert_eq!(expected[field], actual[field])
    }
}

fn get_version(compiler: &str) -> String {
    let output = Command::new(compiler)
        .arg("--version")
        .output()
        .expect("Failed to retrieve version.");

    assert!(
        output.status.success(),
        "Failed to run program to retrieve version."
    );

    let version = String::from_utf8(output.stdout).unwrap();
    get_compiler_major(&version)
}

fn get_compiler_major(version: &str) -> String {
    let re = Regex::new(r"(?:version |(?:gcc \([^\)]+\) )*)([0-9]+)\.[0-9]+\.[0-9]+").unwrap();
    match re.captures(version) {
        Some(caps) => caps.get(1).unwrap().as_str().to_string(),
        None => panic!("Compiler version not found"),
    }
}

fn create_zip(zip_path: &Path, base_dir: &Path, base_dir_in_zip: Option<&str>, files_glob: &str) {
    let mut glob_builder = GlobSetBuilder::new();
    glob_builder.add(Glob::new(files_glob).unwrap());
    let globset = glob_builder.build().unwrap();
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in WalkDir::new(base_dir) {
        let entry = entry.expect("Failed to open directory.");
        if globset.is_match(entry.file_name()) {
            files.push(entry.path().to_path_buf());
        }
    }

    let zipfile = File::create(base_dir.join(zip_path))
        .unwrap_or_else(|_| panic!("Cannot create file {:?}", zip_path));
    let mut zip = ZipWriter::new(zipfile);
    for ref file_path in files {
        let mut file =
            File::open(file_path).unwrap_or_else(|_| panic!("Cannot open file {:?}", file_path));
        let file_size = file
            .metadata()
            .unwrap_or_else(|_| panic!("Cannot get metadata for {:?}", file_path))
            .len() as usize;
        let mut content: Vec<u8> = Vec::with_capacity(file_size + 1);
        file.read_to_end(&mut content)
            .unwrap_or_else(|_| panic!("Cannot read {:?}", file_path));

        let filename_in_zip = file_path.file_name().unwrap().to_str().unwrap();
        let filename_in_zip = match base_dir_in_zip {
            Some(p) => p.to_owned() + "/" + filename_in_zip,
            None => filename_in_zip.to_string(),
        };

        zip.start_file(filename_in_zip, SimpleFileOptions::default())
            .unwrap_or_else(|_| panic!("Cannot create zip for {:?}", zip_path));
        zip.write_all(content.as_slice())
            .unwrap_or_else(|_| panic!("Cannot write {:?}", zip_path));
    }

    if let Some(path) = base_dir_in_zip {
        let path = PathBuf::from(path);
        let mut path = Some(path.as_path());
        while let Some(parent) = path {
            let ancestor = parent.to_str().unwrap();
            if !ancestor.is_empty() {
                zip.add_directory(ancestor, SimpleFileOptions::default())
                    .unwrap_or_else(|_| panic!("Cannot add a directory"));
            }
            path = parent.parent();
        }
    }

    zip.finish()
        .unwrap_or_else(|_| panic!("Unable to write zip structure for {:?}", zip_path));
}

#[test]
fn test_integration() {
    for entry in WalkDir::new("tests").min_depth(1) {
        let entry = entry.unwrap();
        let path = entry.path();

        if path.starts_with("tests/basic_zip_zip")
            || path.starts_with("tests/basic_zip_dir")
            || path.starts_with("tests/rust")
        {
            continue;
        }

        // Only tests/basic is supported on Windows for now.
        if cfg!(windows) && path != Path::new("tests/basic") {
            continue;
        }

        if path.is_dir() {
            println!("\n\n{}", path.display());

            let skip_branches = path == Path::new("tests/template")
                || path == Path::new("tests/include")
                || path == Path::new("tests/include2")
                || path == Path::new("tests/class");

            do_clean(path);

            if cfg!(target_os = "linux") {
                println!("\nGCC: {:?}", path);
                let gpp = &get_tool("GCC_CXX", "g++");
                let gcc_version = get_version(gpp);
                make(path, gpp);
                run(path);
                check_equal_coveralls(
                    &read_expected(path, "gcc", &gcc_version, "coveralls", None),
                    &run_grcov(vec![path], path, "coveralls"),
                    skip_branches,
                );
                check_equal_ade(
                    &read_expected(path, "gcc", &gcc_version, "ade", None),
                    &run_grcov(vec![path], &PathBuf::from(""), "ade"),
                );
                check_equal_covdir(
                    &read_expected(path, "gcc", &gcc_version, "covdir", None),
                    &run_grcov(vec![path], path, "covdir"),
                );
                do_clean(path);
            }

            println!("\nLLVM: {:?}", path);
            let clangpp = &get_tool("CLANG_CXX", "clang++");
            let clang_version = get_version(clangpp);
            make(path, clangpp);
            run(path);
            check_equal_coveralls(
                &read_expected(path, "llvm", &clang_version, "coveralls", None),
                &run_grcov(vec![path], path, "coveralls"),
                skip_branches,
            );
            check_equal_ade(
                &read_expected(path, "llvm", &clang_version, "ade", None),
                &run_grcov(vec![path], &PathBuf::from(""), "ade"),
            );
            check_equal_covdir(
                &read_expected(path, "llvm", &clang_version, "covdir", None),
                &run_grcov(vec![path], path, "covdir"),
            );

            do_clean(path);
        }
    }
}

#[test]
fn test_integration_zip_zip() {
    let compilers = vec![get_tool("GCC_CXX", "g++"), get_tool("CLANG_CXX", "clang++")];

    for compiler in compilers {
        let is_llvm = compiler.contains("clang");

        if !cfg!(target_os = "linux") && !is_llvm {
            continue;
        }

        let name = if is_llvm { "llvm" } else { "gcc" };
        let path = &PathBuf::from("tests/basic_zip_zip");

        println!("\n{}", name.to_uppercase());
        let compiler_version = get_version(&compiler);

        do_clean(path);
        make(path, &compiler);
        run(path);

        let gcno_zip_path = PathBuf::from("gcno.zip");
        let gcda_zip_path = PathBuf::from("gcda.zip");
        let gcda0_zip_path = PathBuf::from("gcda0.zip");
        let gcda1_zip_path = PathBuf::from("gcda1.zip");

        create_zip(&gcno_zip_path, path, None, "*.gcno");
        create_zip(&gcda_zip_path, path, None, "*.gcda");
        create_zip(&gcda0_zip_path, path, None, "");

        let gcno_zip_path = path.join(gcno_zip_path);
        let gcda_zip_path = path.join(gcda_zip_path);
        let gcda0_zip_path = path.join(gcda0_zip_path);
        let gcda1_zip_path = path.join(gcda1_zip_path);

        // no gcda
        println!("No gcda");
        check_equal_coveralls(
            &read_expected(path, name, &compiler_version, "coveralls", Some("_no_gcda")),
            &run_grcov(vec![&gcno_zip_path, &gcda0_zip_path], path, "coveralls"),
            false,
        );

        check_equal_covdir(
            &read_expected(path, name, &compiler_version, "covdir", Some("_no_gcda")),
            &run_grcov(vec![&gcno_zip_path, &gcda0_zip_path], path, "covdir"),
        );

        // one gcda
        println!("One gcda");
        check_equal_coveralls(
            &read_expected(path, name, &compiler_version, "coveralls", None),
            &run_grcov(vec![&gcno_zip_path, &gcda_zip_path], path, "coveralls"),
            false,
        );

        check_equal_covdir(
            &read_expected(path, name, &compiler_version, "covdir", None),
            &run_grcov(vec![&gcno_zip_path, &gcda_zip_path], path, "covdir"),
        );

        // two gcdas
        std::fs::copy(&gcda_zip_path, &gcda1_zip_path)
            .unwrap_or_else(|_| panic!("Failed to copy {:?}", &gcda_zip_path));

        println!("Two gcdas");
        check_equal_coveralls(
            &read_expected(
                path,
                name,
                &compiler_version,
                "coveralls",
                Some("_two_gcda"),
            ),
            &run_grcov(
                vec![&gcno_zip_path, &gcda_zip_path, &gcda1_zip_path],
                path,
                "coveralls",
            ),
            false,
        );

        check_equal_covdir(
            &read_expected(path, name, &compiler_version, "covdir", Some("_two_gcda")),
            &run_grcov(
                vec![&gcno_zip_path, &gcda_zip_path, &gcda1_zip_path],
                path,
                "covdir",
            ),
        );

        do_clean(path);
    }
}

#[test]
fn test_integration_zip_dir() {
    let compilers = vec![get_tool("GCC_CXX", "g++"), get_tool("CLANG_CXX", "clang++")];

    for compiler in compilers {
        let is_llvm = compiler.contains("clang");

        if !cfg!(target_os = "linux") && !is_llvm {
            continue;
        }

        let name = if is_llvm { "llvm" } else { "gcc" };
        let base_path = &PathBuf::from("tests/basic_zip_dir");
        let path = &base_path.join("foo_dir").join("bar_dir");

        println!("\n{}", name.to_uppercase());
        let compiler_version = get_version(&compiler);

        do_clean(path);
        make(path, &compiler);
        run(path);

        let gcno_zip_path = PathBuf::from("gcno.zip");

        create_zip(&gcno_zip_path, path, Some("foo_dir/bar_dir"), "*.gcno");

        // remove the gcno to avoid to have it when exploring the dir
        rm_files(path, vec!["*.gcno"]);

        let gcno_zip_path = path.join(gcno_zip_path);

        check_equal_coveralls(
            &read_expected(base_path, name, &compiler_version, "coveralls", None),
            &run_grcov(vec![&gcno_zip_path, base_path], path, "coveralls"),
            false,
        );

        check_equal_covdir(
            &read_expected(base_path, name, &compiler_version, "covdir", None),
            &run_grcov(vec![&gcno_zip_path, base_path], path, "covdir"),
        );

        do_clean(path);
    }
}

#[test]
fn test_integration_guess_single_file() {
    let zip_path = PathBuf::from("tests/guess_single_file.zip");
    let json_path = PathBuf::from("tests/guess_single_file.json");

    check_equal_covdir(
        &read_file(&json_path),
        &run_grcov(vec![&zip_path], &PathBuf::from(""), "covdir"),
    );
}

#[test]
fn test_coveralls_works_with_just_token_arg() {
    for output in &["coveralls", "coveralls+"] {
        let status = Command::new(get_cmd_path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .args(vec![".", "-t", output, "--token", "123"])
            .status()
            .expect("Failed to run grcov");
        assert!(status.success());
    }
}

#[test]
fn test_coveralls_works_with_just_service_name_and_job_id_args() {
    for output in &["coveralls", "coveralls+"] {
        let status = Command::new(get_cmd_path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .args(vec![
                ".",
                "-t",
                output,
                "--service-name",
                "travis-ci",
                "--service-job-id",
                "456",
            ])
            .status()
            .expect("Failed to run grcov");
        assert!(status.success());
    }
}

#[test]
fn test_coveralls_service_name_is_not_sufficient() {
    for output in &["coveralls", "coveralls+"] {
        let status = Command::new(get_cmd_path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .args(vec![".", "-t", output, "--service-name", "travis-ci"])
            .status()
            .expect("Failed to run grcov");
        assert!(!status.success());
    }
}

#[test]
fn test_coveralls_service_job_id_is_not_sufficient() {
    for output in &["coveralls", "coveralls+"] {
        let status = Command::new(get_cmd_path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .args(vec![".", "-t", output, "--service-job-id", "456"])
            .status()
            .expect("Failed to run grcov");
        assert!(!status.success());
    }
}
