//! Design decisions:
//!
//!   * Use context.pwd - Don't change the actual current working dir as tests run in parallel.
//!   * To prevent thrashing with standard builds, target_dir is overridden.
//!   * Keep coverage-report dir clean so that it can be zipped up by CI.
extern crate structopt;

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use structopt::{StructOpt};

type Env = HashMap<OsString, OsString>;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Context {
    pwd: PathBuf,
    args: Vec<OsString>,
    env: Env,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            pwd: std::env::current_dir().expect("no pwd"),
            args: std::env::args_os().collect(),
            env: std::env::vars_os().collect(),
        }
    }
}

fn main() {
    let context = Context::default();

    match parse_args(context) {
        Ok(actions) => {
            if let Err(err) = acts(&actions) {
                eprintln!("Error executing: {}", err);
                std::process::exit(-2);
            }
        }
        Err(err) => {
            eprintln!("Error parsing: {}", err);
            std::process::exit(-1);
        }
    }
}

fn acts(actions: &Vec<Action>) -> Result<(), Box<dyn std::error::Error>> {
    for action in actions {
        let mut cmd = act(&action);
        //println!("running: {:?}", cmd);
        let output = cmd.status()?;
        if !output.success() {
            panic!("unexpected exit code: {:?} running {:?}", output, cmd);
        }
    }
    Ok(())
}

fn act(action: &Action) -> Command {
    match action {
        Action::SetupEnv(setup_env_data) => setup_env(setup_env_data),
        Action::Report(report_data) => report(&report_data),
    }
}

fn report(report_data: &Report) -> Command {
    // Assume we're in the same dir as grcov (which we are for CI).
    let exe = std::env::current_exe().expect("no current exe");
    let exe_dir = exe.parent().expect("executable wasn't in a dir");
    let grcov_location = if exe_dir.join("grcov").exists() {
        exe_dir.join("grcov")
    } else if exe_dir.join("grcov.exe").exists() {
        exe_dir.join("grcov.exe")
    } else {
        PathBuf::from("grcov") // If we aren't next to it, pick it up from the path.
    };

    let mut grcov = Command::new(grcov_location);
    let output_path = &report_data
        .path
        .join("..")
        .join("..")
        .join("coverage-report");
    fs::create_dir(output_path).expect("dir created");
    grcov
        .current_dir(&report_data.context.pwd)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .arg(&report_data.path)
        .arg("--llvm")
        .arg("--output-type")
        .arg(&report_data.output_type)
        .arg("--output-path")
        .arg(output_path);
    grcov
}

fn setup_env(setup_env: &SetupEnv) -> Command {
    //println!("running setup env {:?}", setup_env.command);
    let empty: Vec<_>;
    let build_args = if setup_env.command.is_empty() {
        empty = vec![];
        &empty
    } else {
        &setup_env.command[1..]
    };

    let mut build_cmd = Command::new(&setup_env.command[0]);
    build_cmd.current_dir(&setup_env.context.pwd);
    build_cmd
        .arg("+nightly") // Coverage is not stable.
        .args(build_args)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let profile = if setup_env
        .context
        .args
        .contains(&OsString::from("--release"))
    {
        "release"
    } else {
        "debug"
    };

    for (key, val) in get_coverage_env_vars(&setup_env.context.env, profile) {
        build_cmd.env(&key, &val);
    }
    build_cmd
}

/// These are the concrete tasks to run (the outputs)
#[derive(Eq, PartialEq, Debug)]
enum Action {
    SetupEnv(SetupEnv),
    Report(Report),
}

#[derive(Eq, PartialEq, Debug)]
struct SetupEnv {
    command: Vec<OsString>,
    context: Context,
}
#[derive(Eq, PartialEq, Debug)]
struct Report {
    path: PathBuf,
    context: Context,
    output_type: String,
}

#[derive(Debug, StructOpt)]
#[structopt(name = "grcov", about = "Parse, collect and aggregate code coverage data for multiple source files")]
enum App {
    Env {
        #[structopt(short)]
        build_cmd: Option<Vec<OsString>>,
    },
    Build {
        #[structopt(short)]
        build_cmd: Option<Vec<OsString>>,
    },
    Test {
        #[structopt(short)]
        build_cmd: Option<Vec<OsString>>,
    },
    Report {
        #[structopt(short)]
        test_cmd: Option<Vec<OsString>>,

        #[structopt(short)]
        output_type: Option<OsString>,

        #[structopt(long, parse(try_from_str))]
        release: Option<bool>
    }
}

fn parse_args(mut context: Context) -> Result<Vec<Action>, Box<dyn std::error::Error>> {
    context.args.remove(1); // remove the first arg that cargo adds so this is like normal args.
    let app : App = StructOpt::from_iter(std::env::args_os().skip(1));
    match app {
        App::Env{ build_cmd} => {
            let command = build_cmd.unwrap_or(vec![OsString::from("cargo"), OsString::from("test")]);
            return Ok(vec![Action::SetupEnv(SetupEnv {
                command,
                context: context.clone(),
            })]);
        },
        App::Build{build_cmd} => {
            let mut command = build_cmd.unwrap_or(vec![]);
            command.insert(0, OsString::from("cargo"));
            command.insert(1, OsString::from("build"));
            return Ok(vec![Action::SetupEnv(SetupEnv {
                command,
                context: context.clone(),
            })]);
        },
        App::Test{build_cmd} => {
            let mut command = build_cmd
                .unwrap_or(vec![]);
            command.insert(0, OsString::from("cargo"));
            command.insert(1, OsString::from("test"));
            return Ok(vec![Action::SetupEnv(SetupEnv {
                command,
                context: context.clone(),
            })]);
        },
        App::Report{test_cmd, release, output_type} => {
            let command = test_cmd;
                let is_release = release.unwrap_or(false);
        
                let target_dir = context
                    .pwd
                    .join(PathBuf::from(get_target_dir(&context.env)));
        
                let profile = if is_release { "release" } else { "debug" };
                let profile_dir = target_dir.join(profile);
        
                let maybe_action = if let Some(command) = command {
                    Some(Action::SetupEnv(SetupEnv {
                        command,
                        context: context.clone(),
                    }))
                } else {
                    ensure_tests_have_run(&context, is_release, &profile_dir)
                };
        
                let mut actions = vec![Action::Report(Report {
                    path: profile_dir,
                    context,
                    output_type: output_type.unwrap_or(OsString::from("html")).to_owned().to_string_lossy().to_string(),
                })];
                if let Some(action) = maybe_action {
                    actions.insert(0, action);
                }
        
                //println!("Actions: {:#?}", actions);
                return Ok(actions);
        }
    }
}

fn ensure_tests_have_run(
    context: &Context,
    is_release: bool,
    profile_dir: &Path,
) -> Option<Action> {
    // If a default.profraw file exists then the build/tests have been run.
    if let Ok(_) = fs::metadata(profile_dir.join("default.profraw")) {
        println!("Found coverage data - skipping triggering tests");
        return None;
    }

    let mut command = vec![OsString::from("cargo"), OsString::from("test")];
    if is_release {
        command.push(OsString::from("--release"));
    }

    Some(Action::SetupEnv(SetupEnv {
        command,
        context: context.clone(),
    }))
}

fn parse_flags(flags: OsString) -> Vec<String> {
    let mut result = Vec::new();
    for flag in flags.to_string_lossy().split_whitespace() {
        result.push(flag.to_string());
    }
    result
}

/// Add a flag in and override if there is an existing flag
fn add(flags: &mut Vec<String>, key: &str, value: Option<&str>) {
    let mut found = false;
    for flag in flags.iter_mut() {
        if flag.starts_with(key) {
            found = true;
            if let Some(value) = value {
                (*flag).clear();
                (*flag).push_str(key);
                (*flag).push('=');
                (*flag).push_str(value);
            }
            break;
        }
    }
    if !found {
        let mut entry = key.to_string();
        if let Some(value) = value {
            entry.push('=');
            entry.push_str(value);
        }
        flags.push(entry);
    }
}

fn get_target_dir(env: &Env) -> OsString {
    let default_target_dir = "target".into();
    let cargo_target_dir = OsString::from("CARGO_TARGET_DIR");
    PathBuf::from(
        env.get(&cargo_target_dir)
            .unwrap_or_else(|| &default_target_dir)
            .to_owned(),
    )
    .join("coverage")
    .into()
}

fn get_coverage_env_vars(env: &Env, profile: &str) -> Vec<(OsString, OsString)> {
    let rust_flags = OsString::from("RUSTFLAGS");
    let llvm_profdata_dir = OsString::from("LLVM_PROFDATA_DIR");
    let empty = OsString::new();

    let target_dir = get_target_dir(env);
    let default_prof_data_dir = PathBuf::from(&target_dir).join(profile);
    let prof_data_dir = env
        .get(&llvm_profdata_dir)
        .map(|v| PathBuf::from(v))
        .unwrap_or_else(|| default_prof_data_dir);

    // Need to ensure dir exists before can be canonicalized:
    std::fs::create_dir_all(&prof_data_dir).expect("dir not created");
    // An absolute dir is needed as current dirs may change.
    let prof_data_dir = prof_data_dir.canonicalize().expect("canonicalize dir");

    let mut flags = parse_flags(env.get(&rust_flags).unwrap_or_else(|| &empty).clone());

    //TODO: The path to the compiled binary must be given as an argument when source-based coverage is used
    //add(&mut flags, "-Zinstrument-coverage", None);
    add(&mut flags, "-Zprofile", None);
    add(&mut flags, "-Ccodegen-units", Some("1"));
    add(&mut flags, "-Copt-level", Some("0"));
    add(&mut flags, "-Clink-dead-code", None);
    add(&mut flags, "-Coverflow-checks", Some("off"));

    let mut new_flags = String::new();
    for flag in flags {
        new_flags.push_str(&flag);
        new_flags.push(' ');
    }

    let flags = OsString::from(new_flags);

    println!("PROFRAW = {:?}", &prof_data_dir);
    vec![
        (OsString::from("RUSTFLAGS"), flags),
        (OsString::from("CARGO_INCREMENTAL"), OsString::from("0")),
        (
            OsString::from("CARGO_TARGET_DIR"),
            OsString::from(target_dir),
        ),
        (
            OsString::from("RUSTDOCFLAGS"),
            OsString::from("-Cpanic=abort"),
        ),
        // dictates where default.profraw gets saved to.
        // We override the default to ensure its put within the target dir
        // so that it will be cleaned up by cargo clean.
        (
            OsString::from("LLVM_PROFILE_FILE"),
            OsString::from(prof_data_dir.join("default.profraw")),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::{tempdir, TempDir};

    fn cast(vec: &Vec<&str>) -> Vec<OsString> {
        vec.iter().map(|element| OsString::from(element)).collect()
    }

    #[test]
    fn grcov_report() {
        let temp_dir = crate_project();

        grcov(&vec!["report"], &temp_dir);

        assert_html_coverage(temp_dir.path());
    }

    // #[test]
    // fn grcov_report_release() {
    //     let temp_dir = crate_project();

    //     grcov(&vec!["report", "--release"], &temp_dir);

    //     assert_html_coverage(temp_dir.path(), "release");
    // }

    // #[test]
    // fn grcov_env_cargo_release_then_report_release() {
    //     let temp_dir = crate_project();
    //     grcov(&vec!["env", "--", "cargo", "build", "--release"], &temp_dir);
    //     grcov(&vec!["report", "--release"], &temp_dir);

    //     assert_html_coverage(temp_dir.path(), "release");
    // }

    #[test]
    fn grcov_report_all_targets() {
        let temp_dir = crate_project();
        grcov(
            &vec!["report", "--", "cargo", "test", "--all-targets"],
            &temp_dir,
        );

        assert_html_coverage(temp_dir.path());
    }

    #[test]
    fn grcov_env_cargo_test_then_report() {
        let temp_dir = crate_project();

        grcov(&vec!["env", "--", "cargo", "test"], &temp_dir);
        grcov(&vec!["report"], &temp_dir);

        assert_html_coverage(temp_dir.path());
    }

    #[test]
    fn grcov_env_cargo_build_then_grcov_report() {
        let temp_dir = crate_project();

        grcov(&vec!["env", "--", "cargo", "build"], &temp_dir);
        grcov(&vec!["report"], &temp_dir);

        assert_html_coverage(temp_dir.path());
    }

    #[test]
    fn grcov_build_then_grcov_report() {
        let temp_dir = crate_project();

        grcov(&vec!["build"], &temp_dir);
        grcov(&vec!["report"], &temp_dir);

        assert_html_coverage(temp_dir.path());
    }

    #[test]
    fn grcov_test_then_grcov_report() {
        let temp_dir = crate_project();

        grcov(&vec!["test"], &temp_dir);

        //TODO: fail if tests run multiple times!
        grcov(&vec!["report"], &temp_dir);

        assert_html_coverage(temp_dir.path());
    }

    fn grcov(args: &Vec<&str>, temp_dir: &TempDir) {
        let mut args = cast(args);
        args.insert(0, OsString::from("cargo-grcov"));
        args.insert(1, OsString::from("cargo"));
        let context = Context {
            pwd: temp_dir.path().to_path_buf(),
            args,
            env: Env::new(),
        };
        let args = &parse_args(context).unwrap();
        acts(args).unwrap();
    }

    fn assert_html_coverage(path: &Path) {
        assert!(std::fs::metadata(
            path.join("target")
                .join("coverage-report")
                .join("index.html")
        )
        .unwrap()
        .is_file());
    }

    fn crate_project() -> TempDir {
        let temp_dir = tempdir().expect("couldn't create temp dir");
        let dir = temp_dir.path();
        fs::write(
            dir.join("Cargo.toml"),
            r#"[package]
        name="testy"
        version="0.0.1"
        "#,
        )
        .expect("write Cargo.toml");
        let src_dir = dir.join("src");
        fs::create_dir(&src_dir).expect("mkdir src");
        fs::write(
            src_dir.join("main.rs"),
            r#"
        fn main() {
            println!("cover me");
        }

        #[test]
        fn test() {
            main();
        }
        "#,
        )
        .expect("write src/main.rs");
        temp_dir
    }
}
