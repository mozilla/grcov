use cargo_binutils::Tool;
use std::path::PathBuf;
use std::process::Command;

pub fn profraw_to_lcov(
    profraw_path: &PathBuf,
    binary_path: &String,
    working_dir: &PathBuf,
) -> Result<Vec<u8>, String> {
    let profdata_path = working_dir.join(
        profraw_path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string()
            + ".profdata",
    );

    let args = [
        "merge",
        "-sparse",
        profraw_path.to_str().unwrap(),
        "-o",
        profdata_path.to_str().unwrap(),
    ];
    let output = match Command::new(&Tool::Profdata.path().unwrap())
        .args(&args)
        .output()
    {
        Ok(output) => output,
        Err(e) => return Err(format!("Failed to execute llvm-profdata\n{}", e)),
    };
    if !output.status.success() {
        return Err(format!(
            "Failure while running llvm-profdata\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    // TODO: Use demangler.
    let args = [
        "export",
        binary_path,
        "--instr-profile",
        profdata_path.to_str().unwrap(),
        "--format",
        "lcov",
    ];
    let output = match Command::new(&Tool::Cov.path().unwrap())
        .args(&args)
        .output()
    {
        Ok(output) => output,
        Err(e) => return Err(format!("Failed to execute llvm-cov\n{}", e)),
    };
    if !output.status.success() {
        return Err(format!(
            "Failure while running llvm-cov\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    return Ok(output.stdout);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profraw_to_lcov() {
        let output = Command::new("rustc").arg("--version").output().unwrap();
        if !String::from_utf8_lossy(&output.stdout).contains("nightly") {
            return;
        }

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();

        assert_eq!(
            profraw_to_lcov(
                &PathBuf::from("test/default.profraw"),
                &"".to_string(),
                &tmp_path
            ),
            Err("Failure while running llvm-cov
No filenames specified!
"
            .to_string())
        );

        let lcov = profraw_to_lcov(
            &PathBuf::from("test/default.profraw"),
            &"test/rust-code-coverage-sample".to_string(),
            &tmp_path,
        );
        assert!(lcov.is_ok());
        assert_eq!(
            String::from_utf8_lossy(&lcov.unwrap()),
            "SF:src/main.rs
FN:9,_RNvCs8GdnNLnutVs_25rust_code_coverage_sample4main
FNDA:1,_RNvCs8GdnNLnutVs_25rust_code_coverage_sample4main
FNF:1
FNH:1
DA:9,1
DA:11,1
DA:12,1
LF:3
LH:3
end_of_record
"
        );
    }
}
