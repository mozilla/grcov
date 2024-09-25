use lazy_static::lazy_static;
use semver::Version;
use std::env;
use std::fmt;
use std::path::Path;
use std::process::Command;

#[derive(Debug)]
pub enum GcovToolError {
    ProcessFailure,
    Failure((String, String, String)),
}

impl fmt::Display for GcovToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            GcovToolError::ProcessFailure => write!(f, "Failed to execute gcov process"),
            GcovToolError::Failure((ref path, ref stdout, ref stderr)) => {
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
    gcno_path: &Path,
    branch_enabled: bool,
    working_dir: &Path,
) -> Result<(), GcovToolError> {
    let mut command = Command::new(get_gcov());
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
        return Err(GcovToolError::ProcessFailure);
    };

    if !output.status.success() {
        return Err(GcovToolError::Failure((
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
            let output = Command::new(get_gcov())
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
            let min_ver = Version::new(9, 1, 0);
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
    let version = gcov_output
        .split([' ', '\n'])
        .filter_map(|value| Version::parse(value.trim()).ok())
        .last();
    assert!(version.is_some(), "no version found for `gcov`.");

    version.unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version() {
        assert_eq!(
            parse_version("gcov (Ubuntu 4.3.0-12ubuntu2) 4.3.0 20170406"),
            Version::new(4, 3, 0)
        );
        assert_eq!(
            parse_version("gcov (Ubuntu 4.9.0-12ubuntu2) 4.9.0 20170406"),
            Version::new(4, 9, 0)
        );
        assert_eq!(
            parse_version("gcov (Ubuntu 6.3.0-12ubuntu2) 6.3.0 20170406"),
            Version::new(6, 3, 0)
        );
        assert_eq!(parse_version("gcov (GCC) 12.2.0"), Version::new(12, 2, 0));
        assert_eq!(parse_version("gcov (GCC) 12.2.0\r"), Version::new(12, 2, 0));
    }
}
