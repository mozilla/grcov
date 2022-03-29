use cargo_binutils::Tool;
use is_executable::IsExecutable;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use log::warn;
use walkdir::WalkDir;

pub fn run(cmd: impl AsRef<OsStr>, args: &[&OsStr]) -> Result<Vec<u8>, String> {
    let mut command = Command::new(cmd);
    command.args(args);
    let output = match command.output() {
        Ok(output) => output,
        Err(e) => return Err(format!("Failed to execute {:?}\n{}", command, e)),
    };
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

    let mut args = vec![
        "merge".as_ref(),
        "-sparse".as_ref(),
        "-o".as_ref(),
        profdata_path.as_ref(),
    ];
    args.splice(2..2, profraw_paths.iter().map(PathBuf::as_ref));

    get_profdata_path().and_then(|p| run(&p, &args))?;

    let metadata = fs::metadata(binary_path)
        .unwrap_or_else(|e| panic!("Failed to open directory '{:?}': {:?}.", binary_path, e));

    let binaries = if metadata.is_file() {
        vec![binary_path.to_owned()]
    } else {
        let mut paths = vec![];

        for entry in WalkDir::new(&binary_path) {
            let entry =
                entry.unwrap_or_else(|_| panic!("Failed to open directory '{:?}'.", binary_path));

            if entry.path().is_executable() && entry.metadata().unwrap().len() > 0 {
                paths.push(entry.into_path());
            }
        }

        paths
    };

    let mut results = vec![];
    let cov_tool_path = Tool::Cov.path().unwrap();

    for binary in binaries {
        let args = [
            "export".as_ref(),
            binary.as_ref(),
            "--instr-profile".as_ref(),
            profdata_path.as_ref(),
            "--format".as_ref(),
            "lcov".as_ref(),
        ];

        match run(&cov_tool_path, &args) {
            Ok(result) => results.push(result),
            Err(err_str) => warn!(
                "Suppressing error returned by llvm-cov tool for binary {:?}\n{}",
                binary, err_str
            ),
        }
    }

    Ok(results)
}

fn get_profdata_path() -> Result<PathBuf, String> {
    let path = Tool::Profdata.path().map_err(|x| x.to_string())?;
    if !path.exists() {
        Err(String::from("We couldn't find llvm-profdata. Try installing the llvm-tools component with `rustup component add llvm-tools-preview`."))
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
            &tmp_path.join("Cargo.toml"),
        )
        .expect("Failed to copy file");
        fs::create_dir(&tmp_path.join("src")).expect("Failed to create dir");
        fs::copy(
            PathBuf::from("tests/rust/src/main.rs"),
            &tmp_path.join("src/main.rs"),
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
        let binary_path = "target/debug/rust-code-coverage-sample";
        #[cfg(windows)]
        let binary_path = "target/debug/rust-code-coverage-sample.exe";

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
        assert!(output_lcov.lines().any(|line| line.contains("FN:3")
            && line.contains("rust_code_coverage_sample")
            && line.contains("Ciao")));
        assert!(output_lcov.lines().any(|line| line.contains("FN:8")
            && line.contains("rust_code_coverage_sample")
            && line.contains("main")));
        assert!(output_lcov.lines().any(|line| line.contains("FNDA:0")
            && line.contains("rust_code_coverage_sample")
            && line.contains("Ciao")));
        assert!(output_lcov.lines().any(|line| line.contains("FNDA:1")
            && line.contains("rust_code_coverage_sample")
            && line.contains("main")));
        assert!(output_lcov.lines().any(|line| line == "FNF:2"));
        assert!(output_lcov.lines().any(|line| line == "FNH:1"));
        assert!(output_lcov.lines().any(|line| line == "DA:3,0"));
        assert!(output_lcov.lines().any(|line| line == "DA:8,1"));
        assert!(output_lcov.lines().any(|line| line == "DA:9,1"));
        assert!(output_lcov.lines().any(|line| line == "DA:10,1"));
        assert!(output_lcov.lines().any(|line| line == "DA:11,1"));
        assert!(output_lcov.lines().any(|line| line == "DA:12,1"));
        assert!(output_lcov.lines().any(|line| line == "BRF:0"));
        assert!(output_lcov.lines().any(|line| line == "BRH:0"));
        assert!(output_lcov.lines().any(|line| line == "LF:6"));
        assert!(output_lcov.lines().any(|line| line == "LH:5"));
        assert!(output_lcov.lines().any(|line| line == "end_of_record"));
    }
}
