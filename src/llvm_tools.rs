use once_cell::sync::OnceCell;
use rayon::prelude::{IntoParallelIterator, ParallelIterator};
use std::env;
use std::env::consts::EXE_SUFFIX;
use std::error::Error;
use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use log::warn;
use walkdir::WalkDir;

pub static LLVM_PATH: OnceCell<PathBuf> = OnceCell::new();

pub fn is_binary(path: impl AsRef<Path>) -> bool {
    if let Ok(oty) = infer::get_from_path(path) {
        if let Some("dll" | "exe" | "elf" | "mach") = oty.map(|x| x.extension()) {
            return true;
        }
    }
    false
}

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
            "Failure while running {:?}\n{}",
            command,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(output.stdout)
}

pub fn run(cmd: impl AsRef<OsStr>, args: &[&OsStr]) -> Result<Vec<u8>, String> {
    let mut command = Command::new(cmd);
    command.args(args);

    let output = command
        .output()
        .map_err(|e| format!("Failed to execute {:?}\n{}", command, e))?;

    if !output.status.success() {
        return Err(format!(
            "Failure while running {:?}\n{}",
            command,
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(output.stdout)
}

pub fn profraws_to_lcov(
    profraw_paths: &[PathBuf],
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

    let stdin_paths: String = profraw_paths.iter().fold("".into(), |mut a, x| {
        a.push_str(x.to_string_lossy().as_ref());
        a.push('\n');
        a
    });

    get_profdata_path().and_then(|p| run_with_stdin(p, &stdin_paths, &args))?;

    let metadata = fs::metadata(binary_path)
        .unwrap_or_else(|e| panic!("Failed to open directory '{:?}': {:?}.", binary_path, e));

    let binaries = if metadata.is_file() {
        vec![binary_path.to_owned()]
    } else {
        let mut paths = vec![];

        for entry in WalkDir::new(binary_path).follow_links(true) {
            let entry = entry
                .unwrap_or_else(|e| panic!("Failed to open directory '{:?}': {}", binary_path, e));

            if is_binary(entry.path()) && entry.metadata().unwrap().len() > 0 {
                paths.push(entry.into_path());
            }
        }

        paths
    };

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
                        "Suppressing error returned by llvm-cov tool for binary {:?}\n{}",
                        binary, err_str
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
    path.push(format!("llvm-{}{}", name, EXE_SUFFIX));
    Ok(path)
}

fn get_profdata_path() -> Result<PathBuf, String> {
    let path = if let Some(mut path) = LLVM_PATH.get().cloned() {
        path.push(format!("llvm-profdata{}", EXE_SUFFIX));
        path
    } else {
        llvm_tool_path("profdata").map_err(|x| x.to_string())?
    };

    if !path.exists() {
        Err(String::from("We couldn't find llvm-profdata. Try installing the llvm-tools component with `rustup component add llvm-tools-preview` or specifying the --llvm-path option."))
    } else {
        Ok(path)
    }
}

fn get_cov_path() -> Result<PathBuf, String> {
    let path = if let Some(mut path) = LLVM_PATH.get().cloned() {
        path.push(format!("llvm-cov{}", EXE_SUFFIX));
        path
    } else {
        llvm_tool_path("cov").map_err(|x| x.to_string())?
    };

    if !path.exists() {
        Err(String::from("We couldn't find llvm-cov. Try installing the llvm-tools component with `rustup component add llvm-tools-preview` or specifying the --llvm-path option."))
    } else {
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_profraws_to_lcov() {
        let output = Command::new("rustc").arg("--version").output().unwrap();
        if !String::from_utf8_lossy(&output.stdout).contains("nightly") {
            return;
        }

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();

        fs::copy(
            PathBuf::from("tests/rust/Cargo.toml"),
            tmp_path.join("Cargo.toml"),
        )
        .expect("Failed to copy file");
        fs::create_dir(tmp_path.join("src")).expect("Failed to create dir");
        fs::copy(
            PathBuf::from("tests/rust/src/main.rs"),
            tmp_path.join("src/main.rs"),
        )
        .expect("Failed to copy file");

        let status = Command::new("cargo")
            .arg("run")
            .env("RUSTFLAGS", "-Cinstrument-coverage")
            .env("LLVM_PROFILE_FILE", tmp_path.join("default.profraw"))
            .current_dir(&tmp_path)
            .status()
            .expect("Failed to build");
        assert!(status.success());

        let lcovs = profraws_to_lcov(
            &[tmp_path.join("default.profraw")],
            &PathBuf::from("src"),
            &tmp_path,
        );
        assert!(lcovs.is_ok());
        let lcovs = lcovs.unwrap();
        assert_eq!(lcovs.len(), 0);

        #[cfg(unix)]
        let binary_path = format!(
            "{}/debug/rust-code-coverage-sample",
            std::env::var("CARGO_TARGET_DIR").unwrap_or("target".to_string())
        );
        #[cfg(windows)]
        let binary_path = format!(
            "{}/debug/rust-code-coverage-sample.exe",
            std::env::var("CARGO_TARGET_DIR").unwrap_or("target".to_string())
        );

        let lcovs = profraws_to_lcov(
            &[tmp_path.join("default.profraw")],
            &tmp_path.join(binary_path),
            &tmp_path,
        );
        assert!(lcovs.is_ok());
        let lcovs = lcovs.unwrap();
        assert_eq!(lcovs.len(), 1);
        let output_lcov = String::from_utf8_lossy(&lcovs[0]);
        println!("{}", output_lcov);
        assert!(output_lcov
            .lines()
            .any(|line| line.contains("SF") && line.contains("src") && line.contains("main.rs")));
        if rustc_version::version_meta().unwrap().channel != rustc_version::Channel::Nightly {
            assert!(output_lcov.lines().any(|line| line.contains("FN:3")
                && line.contains("rust_code_coverage_sample")
                && line.contains("Ciao")));
        }
        assert!(output_lcov.lines().any(|line| line.contains("FN:8")
            && line.contains("rust_code_coverage_sample")
            && line.contains("main")));
        if rustc_version::version_meta().unwrap().channel != rustc_version::Channel::Nightly {
            assert!(output_lcov.lines().any(|line| line.contains("FNDA:0")
                && line.contains("rust_code_coverage_sample")
                && line.contains("Ciao")));
        } else {
            assert!(output_lcov.lines().any(|line| line.contains("FNDA:1")
                && line.contains("rust_code_coverage_sample")
                && line.contains("main")));
        }
        assert!(output_lcov.lines().any(|line| line.contains("FNDA:1")
            && line.contains("rust_code_coverage_sample")
            && line.contains("main")));
        if rustc_version::version_meta().unwrap().channel != rustc_version::Channel::Nightly {
            assert!(output_lcov.lines().any(|line| line == "FNF:2"));
        }
        assert!(output_lcov.lines().any(|line| line == "FNH:1"));
        if rustc_version::version_meta().unwrap().channel != rustc_version::Channel::Nightly {
            assert!(output_lcov.lines().any(|line| line == "DA:3,0"));
        }
        assert!(output_lcov.lines().any(|line| line == "DA:8,1"));
        assert!(output_lcov.lines().any(|line| line == "DA:9,1"));
        assert!(output_lcov.lines().any(|line| line == "DA:10,1"));
        assert!(output_lcov.lines().any(|line| line == "DA:11,1"));
        assert!(output_lcov.lines().any(|line| line == "DA:12,1"));
        assert!(output_lcov.lines().any(|line| line == "BRF:0"));
        assert!(output_lcov.lines().any(|line| line == "BRH:0"));
        if rustc_version::version_meta().unwrap().channel == rustc_version::Channel::Nightly {
            assert!(output_lcov.lines().any(|line| line == "LF:5"));
            assert!(output_lcov.lines().any(|line| line == "LH:5"));
        } else {
            assert!(output_lcov.lines().any(|line| line == "LF:6"));
            assert!(output_lcov.lines().any(|line| line == "LH:5"));
        }
        assert!(output_lcov.lines().any(|line| line == "end_of_record"));
    }
}
