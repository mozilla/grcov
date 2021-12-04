use cargo_binutils::Tool;
use is_executable::IsExecutable;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

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

    let binaries = if binary_path.is_file() {
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

        if let Ok(result) = run(&cov_tool_path, &args) {
            results.push(result);
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

    #[test]
    fn test_profraws_to_lcov() {
        let output = Command::new("rustc").arg("--version").output().unwrap();
        if !String::from_utf8_lossy(&output.stdout).contains("nightly") {
            return;
        }

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();

        let lcovs = profraws_to_lcov(
            &[PathBuf::from("test/default.profraw")],
            &PathBuf::from("src"),
            &tmp_path,
        );
        assert!(lcovs.is_ok());
        let lcovs = lcovs.unwrap();
        assert_eq!(lcovs.len(), 0);

        let lcovs = profraws_to_lcov(
            &[PathBuf::from("test/default.profraw")],
            &PathBuf::from("test/rust-code-coverage-sample"),
            &tmp_path,
        );
        assert!(lcovs.is_ok());
        let lcovs = lcovs.unwrap();
        assert_eq!(lcovs.len(), 1);
        assert_eq!(
            String::from_utf8_lossy(&lcovs[0]),
            "SF:src/main.rs
FN:3,_RNvXCsbfwyntYdFII_25rust_code_coverage_sampleNtB2_4CiaoNtNtCshypYLURccL2_4core3fmt5Debug3fmt
FN:8,_RNvCsbfwyntYdFII_25rust_code_coverage_sample4main
FNDA:0,_RNvXCsbfwyntYdFII_25rust_code_coverage_sampleNtB2_4CiaoNtNtCshypYLURccL2_4core3fmt5Debug3fmt
FNDA:1,_RNvCsbfwyntYdFII_25rust_code_coverage_sample4main
FNF:2
FNH:1
DA:3,0
DA:8,1
DA:9,1
DA:10,1
DA:11,1
DA:12,1
BRF:0
BRH:0
LF:6
LH:5
end_of_record
"
        );
    }
}
