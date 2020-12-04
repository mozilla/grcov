use semver::Version;
use std::env;
use std::fmt;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug)]
pub enum GcovError {
    ProcessFailure,
    Failure((String, String, String)),
}

impl fmt::Display for GcovError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            GcovError::ProcessFailure => write!(f, "Failed to execute gcov process"),
            GcovError::Failure((ref path, ref stdout, ref stderr)) => {
                writeln!(f, "gcov execution failed on {}", path)?;
                writeln!(f, "gcov stdout: {}", stdout)?;
                writeln!(f, "gcov stderr: {}", stderr)
            }
        }
    }
}

fn get_gcov() -> String {
    if let Ok(s) = env::var("GCOV") {
        s
    } else {
        "gcov".to_string()
    }
}

pub fn run_gcov(
    gcno_path: &PathBuf,
    branch_enabled: bool,
    working_dir: &PathBuf,
) -> Result<(), GcovError> {
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

    let output = if let Ok(output) = status.output() {
        output
    } else {
        return Err(GcovError::ProcessFailure);
    };

    if !output.status.success() {
        return Err(GcovError::Failure((
            gcno_path.to_str().unwrap().to_string(),
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
        )));
    }

    Ok(())
}

pub fn get_gcov_version() -> &'static Version {
    lazy_static! {
        static ref V: Version = {
            let output = Command::new(&get_gcov())
                .arg("--version")
                .output()
                .expect("Failed to execute `gcov`. `gcov` is required (it is part of GCC).");
            assert!(output.status.success(), "`gcov` failed to execute.");
            let output = String::from_utf8(output.stdout).unwrap();
            parse_version(&output)
        };
    }
    &V
}

pub fn get_gcov_output_ext() -> &'static str {
    lazy_static! {
        static ref E: &'static str = {
            let min_ver = Version {
                major: 9,
                minor: 1,
                patch: 0,
                pre: vec![],
                build: vec![],
            };
            if get_gcov_version() >= &min_ver {
                ".gcov.json.gz"
            } else {
                ".gcov"
            }
        };
    }
    &E
}

fn parse_version(gcov_output: &str) -> Version {
    let mut versions: Vec<_> = gcov_output
        .split(|c| c == ' ' || c == '\n')
        .filter_map(|value| Version::parse(value).ok())
        .collect();
    assert!(!versions.is_empty(), "no version found for `gcov`.");

    versions.pop().unwrap()
}

pub fn check_gcov_version() -> bool {
    let min_ver = Version {
        major: 4,
        minor: 9,
        patch: 0,
        pre: vec![],
        build: vec![],
    };
    let version = get_gcov_version();
    version >= &min_ver
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version() {
        assert_eq!(
            parse_version("gcov (Ubuntu 4.3.0-12ubuntu2) 4.3.0 20170406"),
            Version {
                major: 4,
                minor: 3,
                patch: 0,
                pre: vec![],
                build: vec![],
            }
        );
        assert_eq!(
            parse_version("gcov (Ubuntu 4.9.0-12ubuntu2) 4.9.0 20170406"),
            Version {
                major: 4,
                minor: 9,
                patch: 0,
                pre: vec![],
                build: vec![],
            }
        );
        assert_eq!(
            parse_version("gcov (Ubuntu 6.3.0-12ubuntu2) 6.3.0 20170406"),
            Version {
                major: 6,
                minor: 3,
                patch: 0,
                pre: vec![],
                build: vec![],
            }
        );
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
