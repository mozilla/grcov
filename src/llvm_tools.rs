use rayon::prelude::{IntoParallelIterator, ParallelIterator};
use std::env;
use std::env::consts::EXE_SUFFIX;
use std::error::Error;
use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use log::warn;

pub static LLVM_PATH: OnceLock<PathBuf> = OnceLock::new();

pub fn run_with_stdin(
    cmd: impl AsRef<OsStr>,
    stdin: impl AsRef<str>,
    args: &[&OsStr],
) -> Result<Vec<u8>, String> {
    let mut command = Command::new(cmd.as_ref());
    let err_fn = |e| format!("Failed to execute {:?}\n{}", cmd.as_ref(), e);

    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let mut child = command.spawn().map_err(err_fn)?;
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin.as_ref().as_bytes())
        .map_err(err_fn)?;

    let output = child.wait_with_output().map_err(err_fn)?;
    if !output.status.success() {
        return Err(format!(
            "Failure while running {:?}\n{}\n\nSTDIN:`{}`",
            command,
            String::from_utf8_lossy(&output.stderr),
            stdin.as_ref()
        ));
    }

    Ok(output.stdout)
}

pub fn run(cmd: impl AsRef<OsStr>, args: &[&OsStr]) -> Result<Vec<u8>, String> {
    let mut command = Command::new(cmd);
    command.args(args);

    let output = command
        .output()
        .map_err(|e| format!("Failed to execute {command:?}\n{e}"))?;

    if !output.status.success() {
        return Err(format!(
            "Failure while running {:?}\n{}",
            command,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(output.stdout)
}

pub fn find_binaries(binary_path: &Path) -> Vec<PathBuf> {
    let metadata = fs::metadata(binary_path)
        .unwrap_or_else(|e| panic!("Failed to open directory '{:?}': {:?}.", binary_path, e));

    if metadata.is_file() {
        vec![binary_path.to_owned()]
    } else {
        crate::file_walker::find_binaries(binary_path)
    }
}

/// Turns multiple .profraw and/or .profdata files into an lcov file.
pub fn llvm_profiles_to_lcov(
    profile_paths: &[PathBuf],
    binary_path: &Path,
    working_dir: &Path,
) -> Result<Vec<Vec<u8>>, String> {
    let profdata_path = working_dir.join("grcov.profdata");

    let args = vec![
        "merge".as_ref(),
        "-f".as_ref(),
        "-".as_ref(),
        "-sparse".as_ref(),
        "-o".as_ref(),
        profdata_path.as_ref(),
    ];

    let stdin_paths: String = profile_paths.iter().fold("".into(), |mut a, x| {
        a.push_str(x.to_string_lossy().as_ref());
        a.push('\n');
        a
    });

    get_profdata_path().and_then(|p| run_with_stdin(p, &stdin_paths, &args))?;

    let binaries = find_binaries(binary_path);

    let cov_tool_path = get_cov_path()?;
    let results = binaries
        .into_par_iter()
        .filter_map(|binary| {
            let args = [
                "export".as_ref(),
                binary.as_ref(),
                "--instr-profile".as_ref(),
                profdata_path.as_ref(),
                "--format".as_ref(),
                "lcov".as_ref(),
            ];

            match run(&cov_tool_path, &args) {
                Ok(result) => Some(result),
                Err(err_str) => {
                    warn!(
                        "Suppressing error returned by llvm-cov tool for binary {binary:?}\n{err_str}"
                    );
                    None
                }
            }
        })
        .collect::<Vec<_>>();

    Ok(results)
}

// The sysroot and rustlib functions are coming from https://github.com/rust-embedded/cargo-binutils/blob/a417523fa990c258509696507d1ce05f85dedbc4/src/rustc.rs.
fn sysroot() -> Result<String, Box<dyn Error>> {
    let rustc = env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = Command::new(rustc).arg("--print").arg("sysroot").output()?;
    // Note: We must trim() to remove the `\n` from the end of stdout
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

// See: https://github.com/rust-lang/rust/blob/564758c4c329e89722454dd2fbb35f1ac0b8b47c/src/bootstrap/dist.rs#L2334-L2341
fn rustlib() -> Result<PathBuf, Box<dyn Error>> {
    let sysroot = sysroot()?;
    let mut pathbuf = PathBuf::from(sysroot);
    pathbuf.push("lib");
    pathbuf.push("rustlib");
    pathbuf.push(rustc_version::version_meta()?.host); // TODO: Prevent calling rustc_version::version_meta() multiple times
    pathbuf.push("bin");
    Ok(pathbuf)
}

fn llvm_tool_path(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let mut path = rustlib()?;
    path.push(format!("llvm-{name}{EXE_SUFFIX}"));
    Ok(path)
}

fn get_profdata_path() -> Result<PathBuf, String> {
    let path = if let Some(mut path) = LLVM_PATH.get().cloned() {
        path.push(format!("llvm-profdata{EXE_SUFFIX}"));
        path
    } else {
        llvm_tool_path("profdata").map_err(|x| x.to_string())?
    };

    if !path.exists() {
        Err(String::from("We couldn't find llvm-profdata. Try installing the llvm-tools component with `rustup component add llvm-tools` or specifying the --llvm-path option."))
    } else {
        Ok(path)
    }
}

fn get_cov_path() -> Result<PathBuf, String> {
    let path = if let Some(mut path) = LLVM_PATH.get().cloned() {
        path.push(format!("llvm-cov{EXE_SUFFIX}"));
        path
    } else {
        llvm_tool_path("cov").map_err(|x| x.to_string())?
    };

    if !path.exists() {
        Err(String::from("We couldn't find llvm-cov. Try installing the llvm-tools component with `rustup component add llvm-tools` or specifying the --llvm-path option."))
    } else {
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use tempfile::TempDir;
    use walkdir::WalkDir;

    const FIXTURES_BASE: &str = "tests/rust/";

    fn get_binary_path(name: &str) -> String {
        #[cfg(unix)]
        let binary_path = format!(
            "{}/debug/{}",
            std::env::var("CARGO_TARGET_DIR").unwrap_or("target".to_string()),
            name
        );
        #[cfg(windows)]
        let binary_path = format!(
            "{}/debug/{}.exe",
            std::env::var("CARGO_TARGET_DIR").unwrap_or("target".to_string()),
            name
        );

        binary_path
    }

    /// Create a temp dir and copy the fixture project in it.
    fn copy_fixture(fixture: &str) -> TempDir {
        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path();
        let fixture_path = Path::new(FIXTURES_BASE).join(fixture);

        let mut entries = VecDeque::new();
        entries.push_front(fixture_path.clone());

        while let Some(entry) = entries.pop_back() {
            for item in WalkDir::new(&entry) {
                let Ok(item) = item else {
                    continue;
                };
                if item.path() == entry {
                    continue;
                }

                let new_tmp = tmp_path.join(
                    item.path()
                        .strip_prefix(fixture_path.clone())
                        .expect("prefix should be fixture path"),
                );

                if item.path().is_file() {
                    // regular file
                    fs::copy(item.path(), new_tmp).expect("Failed to copy file to tmp dir");
                } else {
                    // directory
                    fs::create_dir_all(new_tmp).expect("Failed to create dir");
                    entries.push_front(item.path().to_path_buf());
                }
            }
        }

        tmp_dir
    }

    fn setup_env_and_run_program(fixture: &str) -> TempDir {
        let tmp_dir = copy_fixture(fixture);
        let tmp_path = tmp_dir.path();

        let status = Command::new("cargo")
            .arg("run")
            .env("RUSTFLAGS", "-Cinstrument-coverage")
            .env("LLVM_PROFILE_FILE", tmp_path.join("default.profraw"))
            .current_dir(tmp_path)
            .status()
            .expect("Failed to build");
        assert!(status.success());

        tmp_dir
    }

    fn check_basic_lcov_output(lcov: &str) {
        assert!(lcov
            .lines()
            .any(|line| line.contains("SF") && line.contains("src") && line.contains("main.rs")));
        assert!(lcov
            .lines()
            .any(|line| line.contains("FN:8") && line.contains("basic") && line.contains("main")));
        assert!(lcov.lines().any(|line| line.contains("FNDA:1")
            && line.contains("basic")
            && line.contains("main")));
        assert!(lcov.lines().any(|line| line.contains("FNDA:1")
            && line.contains("basic")
            && line.contains("main")));
        assert!(lcov.lines().any(|line| line == "FNH:1"));
        assert!(lcov.lines().any(|line| line == "DA:8,1"));
        assert!(lcov.lines().any(|line| line == "DA:9,1"));
        assert!(lcov.lines().any(|line| line == "DA:11,1"));
        assert!(lcov.lines().any(|line| line == "DA:12,1"));
        assert!(lcov.lines().any(|line| line == "BRF:0"));
        assert!(lcov.lines().any(|line| line == "BRH:0"));
        assert!(lcov.lines().any(|line| line == "LF:4"));
        assert!(lcov.lines().any(|line| line == "LH:4"));
        assert!(lcov.lines().any(|line| line == "end_of_record"));
    }

    #[test]
    fn test_wrong_binary_file() {
        let tmp_dir = setup_env_and_run_program("basic");
        let tmp_path = tmp_dir.path();

        let lcovs = llvm_profiles_to_lcov(
            &[tmp_path.join("default.profraw")],
            &PathBuf::from("src"), // There is no binary file in src
            tmp_path,
        );
        assert!(lcovs.is_ok());
        let lcovs = lcovs.unwrap();
        assert_eq!(lcovs.len(), 0);
    }

    #[test]
    fn test_profraws_to_lcov() {
        let tmp_dir = setup_env_and_run_program("basic");
        let tmp_path = tmp_dir.path();
        let binary_path = get_binary_path("basic");

        let lcovs = llvm_profiles_to_lcov(
            &[tmp_path.join("default.profraw")],
            &tmp_path.join(binary_path),
            tmp_path,
        );
        assert!(lcovs.is_ok(), "Error: {}", lcovs.unwrap_err());
        let lcovs = lcovs.unwrap();
        assert_eq!(lcovs.len(), 1);
        let output_lcov = String::from_utf8_lossy(&lcovs[0]);
        println!("{output_lcov}");

        check_basic_lcov_output(&output_lcov);
    }

    #[test]
    fn test_profdatas_to_lcov() {
        let tmp_dir = setup_env_and_run_program("basic");
        let tmp_path = tmp_dir.path();
        let binary_path = get_binary_path("basic");

        // Manually transform the profraw into a profdata
        let profdata_dir = tempfile::tempdir().expect("tempdir error");
        let profdata_dir_path = profdata_dir.path();
        let profdata_path = profdata_dir_path.join("default.profdata");
        let status = Command::new(get_profdata_path().unwrap())
            .args([
                "merge",
                "-sparse",
                tmp_path.join("default.profraw").to_str().unwrap(),
                "-o",
                profdata_path.to_str().unwrap(),
            ])
            .status();

        assert_eq!(status.unwrap().code().unwrap(), 0);

        let lcovs = llvm_profiles_to_lcov(&[profdata_path], &tmp_path.join(binary_path), tmp_path);

        assert!(lcovs.is_ok(), "Error: {}", lcovs.unwrap_err());
        let lcovs = lcovs.unwrap();
        assert_eq!(lcovs.len(), 1);
        let output_lcov = String::from_utf8_lossy(&lcovs[0]);
        println!("{output_lcov}");

        check_basic_lcov_output(&output_lcov);
    }

    #[test]
    fn test_llvm_aggregate_profraws() {
        let tmp_dir = copy_fixture("hello_name");
        let tmp_path = tmp_dir.path();
        let bin_path = get_binary_path("hello_name");

        let path_without = tmp_path.join("without-arg.profraw");
        let path_with = tmp_path.join("with-arg.profraw");

        // Run the program twice:
        // - Once without arguments,
        // - Once with a simple argument to enter the other side of the if.
        let status = Command::new("cargo")
            .arg("run")
            .env("RUSTFLAGS", "-Cinstrument-coverage")
            .env("LLVM_PROFILE_FILE", &path_without)
            .current_dir(tmp_path)
            .status()
            .expect("Failed to build");
        assert!(status.success(), "Error when running `cargo run`");

        let status = Command::new("cargo")
            .arg("run")
            .arg("--")
            .arg("John")
            .env("RUSTFLAGS", "-Cinstrument-coverage")
            .env("LLVM_PROFILE_FILE", &path_with)
            .current_dir(tmp_path)
            .status()
            .expect("Failed to build");
        assert!(status.success(), "Error when running `cargo run`");

        let lcovs = llvm_profiles_to_lcov(
            &[path_with, path_without],
            &tmp_path.join(bin_path),
            tmp_path,
        );

        assert!(lcovs.is_ok(), "Error: {}", lcovs.unwrap_err());
        let lcovs = lcovs.unwrap();
        assert_eq!(lcovs.len(), 1);
        let output_lcov = String::from_utf8_lossy(&lcovs[0]);
        println!("{output_lcov}");

        let lcov = String::from_utf8_lossy(&lcovs[0]);

        let lcov_entries = [
            "FNF:1",  // # of function found
            "FNH:1",  // # of function hit
            "DA:1,2", // Line 1 hit 2 times
            "DA:2,2", // Line 2 hit 2 times
            "DA:3,1", // Line 3 hit 1 time
            "DA:4,1", // Line 4 hit 1 time
            "DA:5,1", // Line 5 hit 1 time
            "DA:6,1", // Line 6 hit 1 time
            "DA:7,2", // Line 7 hit 2 time
            "BRF:0",  // # of branch found
            "BRH:0",  // # of branch hit
            "LF:7",   // # of line found
            "LH:7",   // # of line hit
        ];

        for entry in lcov_entries {
            assert!(lcov.contains(&format!("{entry}\n")));
        }

        let main_path = tmp_path
            .join("src")
            .join("main.rs")
            .to_string_lossy()
            .into_owned();
        assert!(
            lcov.lines()
                .any(|line| line.contains("SF:") && line.contains(&main_path)),
            "Missing source file declaration (SF) in lcov report",
        );
    }
}
