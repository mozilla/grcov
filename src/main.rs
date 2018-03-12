#![cfg_attr(feature="alloc_system",feature(alloc_system))]
#[cfg(feature="alloc_system")]
extern crate alloc_system;
extern crate serde_json;
extern crate crossbeam;
extern crate num_cpus;
extern crate tempdir;
extern crate grcov;

use std::collections::HashMap;
use std::{env, thread, process};
use std::fs::{self, File};
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use crossbeam::sync::MsQueue;
use serde_json::Value;
use tempdir::TempDir;

use grcov::*;

macro_rules! eprintln_stderr(
    ($($arg:tt)*) => { {
        writeln!(&mut io::stderr(), $($arg)*).unwrap();
    } }
);

fn print_usage(program: &str) {
    eprintln!("Usage: {} DIRECTORY_OR_ZIP_FILE[...] [-t OUTPUT_TYPE] [-s SOURCE_ROOT] [-p PREFIX_PATH] [--token COVERALLS_REPO_TOKEN] [--commit-sha COVERALLS_COMMIT_SHA] [--keep-global-includes] [--ignore-not-existing] [--ignore-dir DIRECTORY] [--llvm] [--path-mapping PATH_MAPPING_FILE] [--branch]", program);
    eprintln!("You can specify one or more directories, separated by a space.");
    eprintln!("OUTPUT_TYPE can be one of:");
    eprintln!(" - (DEFAULT) ade for the ActiveData-ETL specific format;");
    eprintln!(" - lcov for the lcov INFO format;");
    eprintln!(" - coveralls for the Coveralls specific format.");
    eprintln!(" - coveralls+ for the Coveralls specific format with function information.");
    eprintln!("SOURCE_ROOT is the root directory of the source files.");
    eprintln!("PREFIX_PATH is a prefix to remove from the paths (e.g. if grcov is run on a different machine than the one that generated the code coverage information).");
    eprintln!("COVERALLS_REPO_TOKEN is the repository token from Coveralls, required for the 'coveralls' and 'coveralls+' format.");
    eprintln!("COVERALLS_COMMIT_SHA is the SHA of the commit used to generate the code coverage data.");
    eprintln!("By default global includes are ignored. Use --keep-global-includes to keep them.");
    eprintln!("By default source files that can't be found on the disk are not ignored. Use --ignore-not-existing to ignore them.");
    eprintln!("The --llvm option must be used when the code coverage information is coming from a llvm build.");
    eprintln!("The --ignore-dir option can be used to ignore a directory.");
    eprintln!("The --branch option enables parsing branch coverage information.");
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln_stderr!("[ERROR]: Missing required directory argument.\n");
        print_usage(&args[0]);
        process::exit(1);
    }
    let mut output_type = "ade";
    let mut source_dir = "";
    let mut prefix_dir = "";
    let mut repo_token = "";
    let mut commit_sha = "";
    let mut service_name = "";
    let mut service_number = "";
    let mut service_job_number = "";
    let mut ignore_global = true;
    let mut ignore_not_existing = false;
    let mut to_ignore_dir = "";
    let mut is_llvm = false;
    let mut branch_enabled = false;
    let mut paths = Vec::new();
    let mut i = 1;
    let mut path_mapping_file = "";
    let mut filter_covered = true;
    let mut num_threads = num_cpus::get() * 2;
    while i < args.len() {
        if args[i] == "-t" {
            if args.len() <= i + 1 {
                eprintln_stderr!("[ERROR]: Output format not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            output_type = &args[i + 1];
            i += 1;
        } else if args[i] == "-s" {
            if args.len() <= i + 1 {
                eprintln_stderr!("[ERROR]: Source root directory not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            source_dir = &args[i + 1];
            i += 1;
        } else if args[i] == "-p" {
            if args.len() <= i + 1 {
                eprintln_stderr!("[ERROR]: Prefix path not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            prefix_dir = &args[i + 1];
            i += 1;
        } else if args[i] == "--token" {
            if args.len() <= i + 1 {
                eprintln_stderr!("[ERROR]: Repository token not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            repo_token = &args[i + 1];
            i += 1;
        } else if args[i] == "--service-name" {
            if args.len() <= i + 1 {
                eprintln_stderr!("[ERROR]: Service name not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            service_name = &args[i + 1];
            i += 1;
        } else if args[i] == "--service-number" {
            if args.len() <= i + 1 {
                eprintln_stderr!("[ERROR]: Service number not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            service_number = &args[i + 1];
            i += 1;
        } else if args[i] == "--service-job-number" {
            if args.len() <= i + 1 {
                eprintln_stderr!("[ERROR]: Service job number not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            service_job_number = &args[i + 1];
            i += 1;
        } else if args[i] == "--commit-sha" {
            if args.len() <= i + 1 {
                eprintln_stderr!("[ERROR]: Commit SHA not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            commit_sha = &args[i + 1];
            i += 1;
        } else if args[i] == "--keep-global-includes" {
            ignore_global = false;
        } else if args[i] == "--ignore-not-existing" {
            ignore_not_existing = true;
        } else if args[i] == "--ignore-dir" {
            if args.len() <= i + 1 {
                eprintln_stderr!("[ERROR]: Directory to ignore not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            to_ignore_dir = &args[i + 1];
            i += 1;
        } else if args[i] == "--llvm" {
            is_llvm = true;
        } else if args[i] == "--path-mapping" {
            if args.len() <= i + 1 {
                eprintln_stderr!("[ERROR]: Path mapping file not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            path_mapping_file = &args[i + 1];
            i += 1;
        } else if args[i] == "--branch" {
            branch_enabled = true;
        } else if args[i] == "--filter-covered" {
            filter_covered = true;
        } else if args[i] == "--filter-uncovered" {
            filter_covered = false;
        } else if args[i] == "--threads" {
            if args.len() <= i + 1 {
                eprintln_stderr!("[ERROR]: Number of threads not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            num_threads = args[i + 1].parse().expect("Number of threads should be a number");
            i += 1;
        } else {
            paths.push(args[i].clone());
        }

        i += 1;
    }

    if !is_llvm && !check_gcov_version() {
        eprintln_stderr!("[ERROR]: gcov (bundled with GCC) >= 4.9 is required.\n");
        process::exit(1);
    }

    if output_type != "ade" && output_type != "lcov" && output_type != "coveralls" && output_type != "coveralls+" && output_type != "files" {
        eprintln_stderr!("[ERROR]: '{}' output format is not supported.\n", output_type);
        print_usage(&args[0]);
        process::exit(1);
    }

    if output_type == "coveralls" || output_type == "coveralls+" {
        if repo_token == "" {
            eprintln_stderr!("[ERROR]: Repository token is needed when the output format is 'coveralls'.\n");
            print_usage(&args[0]);
            process::exit(1);
        }

        if commit_sha == "" {
            eprintln_stderr!("[ERROR]: Commit SHA is needed when the output format is 'coveralls'.\n");
            print_usage(&args[0]);
            process::exit(1);
        }
    }

    if prefix_dir == "" {
        prefix_dir = source_dir;
    }

    let to_ignore_dir = if to_ignore_dir == "" {
        None
    } else {
        Some(to_ignore_dir.to_owned())
    };

    let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
    let tmp_path = tmp_dir.path().to_owned();

    let result_map: Arc<SyncCovResultMap> = Arc::new(Mutex::new(HashMap::with_capacity(20_000)));
    let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());
    let path_mapping: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));

    let producer = {
        let queue = Arc::clone(&queue);
        let tmp_path = tmp_path.clone();
        let path_mapping_file = path_mapping_file.to_owned();
        let path_mapping = Arc::clone(&path_mapping);

        thread::spawn(move || {
            let producer_path_mapping_buf = producer(&tmp_path, &paths, &queue);

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
    };

    let mut parsers = Vec::new();

    for i in 0..num_threads {
        let queue = Arc::clone(&queue);
        let result_map = Arc::clone(&result_map);
        let working_dir = tmp_path.join(format!("{}", i));

        let t = thread::spawn(move || {
            fs::create_dir(&working_dir).expect("Failed to create working directory");
            consumer(&working_dir, &result_map, &queue, is_llvm, branch_enabled);
        });

        parsers.push(t);
    }

    let _ = producer.join();

    // Poison the queue, now that the producer is finished.
    for _ in 0..num_threads {
        queue.push(None);
    }

    for parser in parsers {
        parser.join().unwrap();
    }

    let result_map_mutex = Arc::try_unwrap(result_map).unwrap();
    let result_map = result_map_mutex.into_inner().unwrap();

    let path_mapping_mutex = Arc::try_unwrap(path_mapping).unwrap();
    let path_mapping = path_mapping_mutex.into_inner().unwrap();

    let iterator = rewrite_paths(result_map, path_mapping, source_dir, prefix_dir, ignore_global, ignore_not_existing, to_ignore_dir);

    if output_type == "ade" {
        output_activedata_etl(iterator);
    } else if output_type == "lcov" {
        output_lcov(iterator);
    } else if output_type == "coveralls" {
        output_coveralls(iterator, repo_token, service_name, service_number, service_job_number, commit_sha, false);
    } else if output_type == "coveralls+" {
        output_coveralls(iterator, repo_token, service_name, service_number, service_job_number, commit_sha, true);
    } else if output_type == "files" {
        output_files(iterator, filter_covered);
    } else {
        assert!(false, "{} is not a supported output type", output_type);
    }
}
