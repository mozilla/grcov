use semver::Version;
use std::env;
use std::path::PathBuf;
use std::process::Command;

fn get_gcov() -> String {
    match env::var("GCOV") {
        Ok(s) => s,
        Err(_) => "gcov".to_string(),
    }
}

pub fn run_gcov(gcno_path: &PathBuf, branch_enabled: bool, working_dir: &PathBuf) {
    let mut command = Command::new(&get_gcov());
    let command = if branch_enabled {
        command.arg("-b").arg("-c")
    } else {
        &mut command
    };
    let status = command
        .arg(gcno_path)
        .arg("-i") // Generate intermediate gcov format, faster to parse.
        .current_dir(working_dir);

    let output = status.output().expect("Failed to execute gcov process");

    if !output.status.success() {
        eprintln!("gcov stdout: {}", String::from_utf8_lossy(&output.stdout));
        eprintln!("gcov stderr: {}", String::from_utf8_lossy(&output.stderr));

        panic!("gcov execution failed on {}", gcno_path.display());
    }
}

fn is_recent_version(gcov_output: &str) -> bool {
    let min_ver = Version {
        major: 4,
        minor: 9,
        patch: 0,
        pre: vec![],
        build: vec![],
    };

    gcov_output.split(' ').all(|value| {
        if let Ok(ver) = Version::parse(value) {
            ver >= min_ver
        } else {
            true
        }
    })
}

pub fn check_gcov_version() -> bool {
    let output = Command::new("gcov")
        .arg("--version")
        .output()
        .expect("Failed to execute `gcov`. `gcov` is required (it is part of GCC).");

    assert!(output.status.success(), "`gcov` failed to execute.");

    is_recent_version(&String::from_utf8(output.stdout).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_recent_version() {
        assert!(!is_recent_version(
            "gcov (Ubuntu 4.3.0-12ubuntu2) 4.3.0 20170406"
        ));
        assert!(is_recent_version(
            "gcov (Ubuntu 4.9.0-12ubuntu2) 4.9.0 20170406"
        ));
        assert!(is_recent_version(
            "gcov (Ubuntu 6.3.0-12ubuntu2) 6.3.0 20170406"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn test_check_gcov_version() {
        check_gcov_version();
    }

    #[cfg(windows)]
    #[test]
    #[should_panic]
    fn test_check_gcov_version() {
        check_gcov_version();
    }
}
