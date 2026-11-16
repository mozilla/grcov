use std::env;
use std::fmt;
use std::path::Path;
use std::process::Command;

#[derive(Debug)]
pub enum GcovToolError {
    ProcessFailure,
    Failure((String, String)),
}

impl fmt::Display for GcovToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            GcovToolError::ProcessFailure => write!(f, "Failed to execute gcov process"),
            GcovToolError::Failure((ref path, ref stderr)) => {
                writeln!(f, "gcov execution failed on {path}")?;
                writeln!(f, "gcov stderr: {stderr}")
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

/// Runs gcov on a gcno and returns its JSON output, which needs gcov 9.1 or newer.
pub fn run_gcov(gcno_path: &Path, branch_enabled: bool) -> Result<Vec<u8>, GcovToolError> {
    let mut command = Command::new(get_gcov());
    if branch_enabled {
        command.arg("-b").arg("-c");
    }
    let output = command
        .arg(gcno_path)
        .arg("-i") // JSON intermediate format, faster to parse.
        .arg("-t") // Write it to stdout instead of to a file.
        .output()
        .map_err(|_| GcovToolError::ProcessFailure)?;

    if !output.status.success() {
        return Err(GcovToolError::Failure((
            gcno_path.to_str().unwrap().to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
        )));
    }

    Ok(output.stdout)
}
