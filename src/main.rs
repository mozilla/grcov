#[cfg(feature = "tc")]
use tcmalloc::TCMalloc;

#[cfg(feature = "tc")]
#[global_allocator]
static GLOBAL: TCMalloc = TCMalloc;

extern crate clap;
extern crate crossbeam;
extern crate grcov;
extern crate num_cpus;
extern crate rustc_hash;
extern crate serde_json;
extern crate simplelog;
extern crate tempfile;

use clap::{crate_authors, crate_version, App, Arg, ArgGroup};
use crossbeam::crossbeam_channel::bounded;
use log::error;
use rustc_hash::FxHashMap;
use serde_json::Value;
use simplelog::{Config, LevelFilter, TermLogger, TerminalMode, WriteLogger};
use std::fs::{self, File};
use std::ops::Deref;
use std::panic;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::{process, thread};

use grcov::*;

fn main() {
    let default_num_threads = 1.max(num_cpus::get() - 1).to_string();

    let matches = App::new("grcov")
                          .version(crate_version!())
                          .author(crate_authors!("\n"))
                          .about("Parse, collect and aggregate code coverage data for multiple source files")

                          .arg(Arg::with_name("paths")
                               .help("Sets the input paths to use")
                               .required(true)
                               .multiple(true)
                               .takes_value(true))

                          .arg(Arg::with_name("output_type")
                               .help("Sets a custom output type")
                               .long_help(
"Sets a custom output type:
- *lcov* for the lcov INFO format;
- *coveralls* for the Coveralls specific format;
- *coveralls+* for the Coveralls specific format with function information;
- *ade* for the ActiveData-ETL specific format;
- *files* to only return a list of files.
")
                               .short("t")
                               .long("output-type")
                               .value_name("OUTPUT TYPE")
                               .default_value("lcov")
                               .possible_values(&["ade", "lcov", "coveralls", "coveralls+", "files", "covdir", "html"])
                               .takes_value(true)
                               .requires_ifs(&[
                                   ("coveralls", "coveralls_auth"),
                                   ("coveralls+", "coveralls_auth")
                               ]))

                          .arg(Arg::with_name("output_file")
                               .help("Specifies the output file")
                               .short("o")
                               .long("output-file")
                               .value_name("FILE")
                               .takes_value(true))

                          .arg(Arg::with_name("source_dir")
                               .help("Specifies the root directory of the source files")
                               .short("s")
                               .long("source-dir")
                               .value_name("DIRECTORY")
                               .takes_value(true))

                          .arg(Arg::with_name("prefix_dir")
                               .help("Specifies a prefix to remove from the paths (e.g. if grcov is run on a different machine than the one that generated the code coverage information)")
                               .short("p")
                               .long("prefix-dir")
                               .value_name("PATH")
                               .takes_value(true))

                          .arg(Arg::with_name("ignore_not_existing")
                               .help("Ignore source files that can't be found on the disk")
                               .long("ignore-not-existing"))

                          .arg(Arg::with_name("ignore_dir")
                               .help("Ignore files/directories specified as globs")
                               .long("ignore")
                               .value_name("PATH")
                               .multiple(true)
                               .number_of_values(1)
                               .takes_value(true))

                          .arg(Arg::with_name("path_mapping")
                               .long("path-mapping")
                               .value_name("PATH")
                               .multiple(true)
                               .number_of_values(1)
                               .takes_value(true))

                          .arg(Arg::with_name("branch")
                               .help("Enables parsing branch coverage information")
                               .long("branch"))

                          .arg(Arg::with_name("filter")
                               .help("Filters out covered/uncovered files. Use 'covered' to only return covered files, 'uncovered' to only return uncovered files")
                               .long("filter")
                               .possible_values(&["covered", "uncovered"])
                               .takes_value(true))

                          .arg(Arg::with_name("llvm")
                               .help("Speeds-up parsing, when the code coverage information is exclusively coming from a llvm build")
                               .long("llvm"))

                          .arg(Arg::with_name("token")
                               .help("Sets the repository token from Coveralls, required for the 'coveralls' and 'coveralls+' formats")
                               .long("token")
                               .value_name("TOKEN")
                               .takes_value(true))

                          .arg(Arg::with_name("commit_sha")
                               .help("Sets the hash of the commit used to generate the code coverage data")
                               .long("commit-sha")
                               .value_name("COMMIT HASH")
                               .takes_value(true))

                          .arg(Arg::with_name("service_name")
                               .help("Sets the service name")
                               .long("service-name")
                               .value_name("SERVICE NAME")
                               .takes_value(true))

                          .arg(Arg::with_name("service_number")
                               .help("Sets the service number")
                               .long("service-number")
                               .value_name("SERVICE NUMBER")
                               .takes_value(true))

                          .arg(Arg::with_name("service_job_id")
                               .help("Sets the service job id")
                               .long("service-job-id")
                               .value_name("SERVICE JOB ID")
                               .takes_value(true)
                               .visible_alias("service-job-number")
                               .requires("service_name"))

                          .arg(Arg::with_name("service_pull_request")
                               .help("Sets the service pull request number")
                               .long("service-pull-request")
                               .value_name("SERVICE PULL REQUEST")
                               .takes_value(true))

                          .arg(Arg::with_name("parallel")
                               .help("Sets the build type to be parallel for 'coveralls' and 'coveralls+' formats")
                               .long("parallel"))

                          .arg(Arg::with_name("threads")
                               .long("threads")
                               .value_name("NUMBER")
                               .default_value(&default_num_threads)
                               .takes_value(true))

                          .arg(Arg::with_name("guess_directory")
                               .long("guess-directory-when-missing"))

                          .arg(Arg::with_name("vcs_branch")
                               .help("Set the branch for coveralls report. Defaults to 'master'")
                               .long("vcs-branch")
                               .default_value("master")
                               .value_name("VCS BRANCH")
                               .takes_value(true))

                          .arg(Arg::with_name("log")
                               .help("Set the file where to log (or stderr or stdout). Defaults to 'stderr'")
                               .long("log")
                               .default_value("stderr")
                               .value_name("LOG")
                               .takes_value(true))

                          .arg(Arg::with_name("excl-line")
                               .help("Lines in covered files containing this marker will be excluded.")
                               .long("excl_line")
                               .value_name("regex")
                               .takes_value(true))

                            .arg(Arg::with_name("excl-start")
                                .help("Marks the beginning of an excluded section. The current line is part of this section.")
                                .long("excl_start")
                                .value_name("regex")
                                .takes_value(true))

                            .arg(Arg::with_name("excl-stop")
                                .help("Marks the end of an excluded section. The current line is part of this section.")
                                .long("excl_stop")
                                .value_name("regex")
                                .takes_value(true))

                          .arg(Arg::with_name("excl-br-line")
                               .help("Lines in covered files containing this marker will be excluded.")
                               .long("excl_br_line")
                               .value_name("regex")
                               .takes_value(true))

                            .arg(Arg::with_name("excl-br-start")
                                .help("Marks the beginning of an excluded section. The current line is part of this section.")
                                .long("excl_br_start")
                                .value_name("regex")
                                .takes_value(true))

                            .arg(Arg::with_name("excl-br-stop")
                                .help("Marks the end of an excluded section. The current line is part of this section.")
                                .long("excl_br_stop")
                                .value_name("regex")
                                .takes_value(true))

                          // This group requires that at least one of --token and --service-job-id
                          // be present. --service-job-id requires --service-name, so this
                          // effectively means we accept the following combinations:
                          // - --token
                          // - --token --service-job-id --service-name
                          // - --service-job-id --service-name
                          .group(ArgGroup::with_name("coveralls_auth").args(&["token", "service_job_id"]).multiple(true))

                          .get_matches();

    let paths: Vec<_> = matches.values_of("paths").unwrap().collect();
    let paths: Vec<String> = paths.iter().map(|s| s.to_string()).collect();
    let output_type = matches.value_of("output_type").unwrap();
    let output_file_path = matches.value_of("output_file");
    let source_dir = matches.value_of("source_dir").unwrap_or("");
    let prefix_dir = matches.value_of("prefix_dir").unwrap_or("");
    let ignore_not_existing = matches.is_present("ignore_not_existing");
    let mut to_ignore_dirs: Vec<_> = if let Some(to_ignore_dirs) = matches.values_of("ignore_dir") {
        to_ignore_dirs.collect()
    } else {
        Vec::new()
    };
    let path_mapping_file = matches.value_of("path_mapping").unwrap_or("");
    let branch_enabled = matches.is_present("branch");
    let filter_option = if let Some(filter) = matches.value_of("filter") {
        if filter == "covered" {
            Some(true)
        } else {
            Some(false)
        }
    } else {
        None
    };
    let is_llvm = matches.is_present("llvm");
    let repo_token = matches.value_of("token");
    let commit_sha = matches.value_of("commit_sha").unwrap_or("");
    let service_name = matches.value_of("service_name");
    let is_parallel = matches.is_present("parallel");
    let service_number = matches.value_of("service_number").unwrap_or("");
    let service_job_id = matches.value_of("service_job_id");
    let service_pull_request = matches.value_of("service_pull_request").unwrap_or("");
    let vcs_branch = matches.value_of("vcs_branch").unwrap_or("");
    let log = matches.value_of("log").unwrap_or("");
    match log {
        "stdout" => {
            let _ = TermLogger::init(LevelFilter::Error, Config::default(), TerminalMode::Stdout);
        }
        "stderr" => {
            let _ = TermLogger::init(LevelFilter::Error, Config::default(), TerminalMode::Stderr);
        }
        log => {
            if let Ok(file) = File::create(log) {
                let _ = WriteLogger::init(LevelFilter::Error, Config::default(), file);
            } else {
                let _ =
                    TermLogger::init(LevelFilter::Error, Config::default(), TerminalMode::Stderr);
                error!("Enable to create log file: {}. Swtich to stderr", log);
            }
        }
    };

    let excl_line = matches.value_of("excl_line").map_or(None, |f| {
        Some(regex::Regex::new(f).expect("invalid regex for excl_line."))
    });
    let excl_start = matches.value_of("excl_start").map_or(None, |f| {
        Some(regex::Regex::new(f).expect("invalid regex for excl_start."))
    });
    let excl_stop = matches.value_of("excl_stop").map_or(None, |f| {
        Some(regex::Regex::new(f).expect("invalid regex for excl_stop."))
    });
    let excl_br_line = matches.value_of("excl_br_line").map_or(None, |f| {
        Some(regex::Regex::new(f).expect("invalid regex for excl_br_line."))
    });
    let excl_br_start = matches.value_of("excl_br_start").map_or(None, |f| {
        Some(regex::Regex::new(f).expect("invalid regex for excl_br_start."))
    });
    let excl_br_stop = matches.value_of("excl_br_stop").map_or(None, |f| {
        Some(regex::Regex::new(f).expect("invalid regex for excl_br_stop."))
    });
    let file_filter = FileFilter::new(
        excl_line,
        excl_start,
        excl_stop,
        excl_br_line,
        excl_br_start,
        excl_br_stop,
    );

    panic::set_hook(Box::new(|panic_info| {
        let (filename, line) = panic_info
            .location()
            .map(|loc| (loc.file(), loc.line()))
            .unwrap_or(("<unknown>", 0));
        let cause = panic_info
            .payload()
            .downcast_ref::<String>()
            .map(String::deref);
        let cause = cause.unwrap_or_else(|| {
            panic_info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| *s)
                .unwrap_or("<cause unknown>")
        });
        error!("A panic occurred at {}:{}: {}", filename, line, cause);
    }));

    let num_threads: usize = matches
        .value_of("threads")
        .unwrap()
        .parse()
        .expect("Number of threads should be a number");
    let guess_directory = matches.is_present("guess_directory");

    let source_root = if source_dir != "" {
        Some(canonicalize_path(&source_dir).expect("Source directory does not exist."))
    } else {
        None
    };

    let prefix_dir = if prefix_dir == "" {
        source_root.clone()
    } else {
        Some(PathBuf::from(prefix_dir))
    };

    let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
    let tmp_path = tmp_dir.path().to_owned();
    assert!(tmp_path.exists());

    let result_map: Arc<SyncCovResultMap> = Arc::new(Mutex::new(
        FxHashMap::with_capacity_and_hasher(20_000, Default::default()),
    ));
    let (sender, receiver) = bounded(2 * num_threads);
    let path_mapping: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));

    let producer = {
        let sender: JobSender = sender.clone();
        let tmp_path = tmp_path.clone();
        let path_mapping_file = path_mapping_file.to_owned();
        let path_mapping = Arc::clone(&path_mapping);

        thread::Builder::new()
            .name(String::from("Producer"))
            .spawn(move || {
                let producer_path_mapping_buf = producer(
                    &tmp_path,
                    &paths,
                    &sender,
                    filter_option.is_some() && filter_option.unwrap(),
                    is_llvm,
                );

                let mut path_mapping = path_mapping.lock().unwrap();
                *path_mapping = if path_mapping_file != "" {
                    let file = File::open(path_mapping_file).unwrap();
                    Some(serde_json::from_reader(file).unwrap())
                } else if let Some(producer_path_mapping_buf) = producer_path_mapping_buf {
                    Some(serde_json::from_slice(&producer_path_mapping_buf).unwrap())
                } else {
                    None
                };
            })
            .unwrap()
    };

    let mut parsers = Vec::new();

    for i in 0..num_threads {
        let receiver = receiver.clone();
        let result_map = Arc::clone(&result_map);
        let working_dir = tmp_path.join(format!("{}", i));
        let source_root = source_root.clone();

        let t = thread::Builder::new()
            .name(format!("Consumer {}", i))
            .spawn(move || {
                fs::create_dir(&working_dir).expect("Failed to create working directory");
                consumer(
                    &working_dir,
                    &source_root,
                    &result_map,
                    receiver,
                    branch_enabled,
                    guess_directory,
                );
            })
            .unwrap();

        parsers.push(t);
    }

    if let Err(_) = producer.join() {
        process::exit(1);
    }

    // Poison the receiver, now that the producer is finished.
    for _ in 0..num_threads {
        sender.send(None).unwrap();
    }

    for parser in parsers {
        if let Err(_) = parser.join() {
            process::exit(1);
        }
    }

    let result_map_mutex = Arc::try_unwrap(result_map).unwrap();
    let result_map = result_map_mutex.into_inner().unwrap();

    let path_mapping_mutex = Arc::try_unwrap(path_mapping).unwrap();
    let path_mapping = path_mapping_mutex.into_inner().unwrap();

    let iterator = rewrite_paths(
        result_map,
        path_mapping,
        source_root,
        prefix_dir,
        ignore_not_existing,
        &mut to_ignore_dirs,
        filter_option,
        file_filter,
    );

    if output_type == "ade" {
        output_activedata_etl(iterator, output_file_path);
    } else if output_type == "lcov" {
        output_lcov(iterator, output_file_path);
    } else if output_type == "coveralls" {
        output_coveralls(
            iterator,
            repo_token,
            service_name,
            service_number,
            service_job_id,
            service_pull_request,
            commit_sha,
            false,
            output_file_path,
            vcs_branch,
            is_parallel,
        );
    } else if output_type == "coveralls+" {
        output_coveralls(
            iterator,
            repo_token,
            service_name,
            service_number,
            service_job_id,
            service_pull_request,
            commit_sha,
            true,
            output_file_path,
            vcs_branch,
            is_parallel,
        );
    } else if output_type == "files" {
        output_files(iterator, output_file_path);
    } else if output_type == "covdir" {
        output_covdir(iterator, output_file_path);
    } else if output_type == "html" {
        output_html(iterator, output_file_path, num_threads);
    } else {
        assert!(false, "{} is not a supported output type", output_type);
    }
}
