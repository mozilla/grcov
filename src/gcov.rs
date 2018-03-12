use std::path::PathBuf;
use std::process::{Command, Stdio};
use semver::Version;

/*
#[link(name = "gcov")]
extern {
    fn __gcov_read_unsigned() -> u32;
    fn __gcov_open(name: *const c_char) -> i32;
    fn __gcov_close();
}

fn gcov_open(file: String) -> i32 {
    let c_to_print = CString::new(file).unwrap();
    return unsafe { __gcov_open(c_to_print.as_ptr()) };
}

fn gcov_read_unsigned() -> u32 {
    return unsafe { __gcov_read_unsigned() };
}

fn prova() {
  if gcov_open("/home/marco/Documenti/workspace/grcov/tests/llvm/main.gcda".to_string()) == 1 {
    println!("2");
  }

  println!("{:x}", gcov_read_unsigned());

  if gcov_open("/home/marco/Documenti/workspace/grcov/tests/basic/main.gcda".to_string()) == 1 {
    println!("1");
  }

  println!("{:x}", gcov_read_unsigned());
}*/

pub fn run_gcov(gcno_path: &PathBuf, branch_enabled: bool, working_dir: &PathBuf) {
    let mut command = Command::new("gcov");
    let command = if branch_enabled {
        command.arg("-b").arg("-c")
    } else {
        &mut command
    };
    let status = command.arg(gcno_path)
                        .arg("-i") // Generate intermediate gcov format, faster to parse.
                        .current_dir(working_dir)
                        .stdout(Stdio::null())
                        .stderr(Stdio::null());

    /*if cfg!(unix) {
        status.spawn()
              .expect("Failed to execute gcov process");
    } else {*/
        let status = status.status()
                           .expect("Failed to execute gcov process");
        assert!(status.success(), "gcov wasn't successfully executed on {}", gcno_path.display());
    //}
}

fn is_recent_version(gcov_output: &str) -> bool {
    let min_ver = Version {
        major: 4,
        minor: 9,
        patch: 0,
        pre: vec!(),
        build: vec!(),
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
        assert!(!is_recent_version("gcov (Ubuntu 4.3.0-12ubuntu2) 4.3.0 20170406"));
        assert!(is_recent_version("gcov (Ubuntu 4.9.0-12ubuntu2) 4.9.0 20170406"));
        assert!(is_recent_version("gcov (Ubuntu 6.3.0-12ubuntu2) 6.3.0 20170406"));
    }
}
