extern crate crossbeam;
extern crate grcov;
extern crate num_cpus;
extern crate rustc_hash;
extern crate serde_json;
extern crate tempfile;

use crossbeam::queue::MsQueue;
use rustc_hash::FxHashMap;
use serde_json::Value;
use std::alloc::System;
use std::fs::{self, File};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::{env, process, thread};

#[global_allocator]
static GLOBAL: System = System;

use grcov::*;

fn print_usage(program: &str) {
    println!("Usage: {} DIRECTORY_OR_ZIP_FILE[...] [-o OUTPUT FILENAME] [-t OUTPUT_TYPE] [-s SOURCE_ROOT] [-p PREFIX_PATH] [--token COVERALLS_REPO_TOKEN] [--commit-sha COVERALLS_COMMIT_SHA] [--ignore-not-existing] [--ignore-dir DIRECTORY] [--llvm] [--path-mapping PATH_MAPPING_FILE] [--branch] [--filter]", program);
    println!("You can specify one or more directories, separated by a space.");
    println!("OUTPUT_TYPE can be one of:");
    println!(" - (DEFAULT) lcov for the lcov INFO format;");
    println!(" - coveralls for the Coveralls specific format.");
    println!(" - coveralls+ for the Coveralls specific format with function information.");
    println!(" - ade for the ActiveData-ETL specific format;");
    println!(" - files to only return a list of files.");
    println!("SOURCE_ROOT is the root directory of the source files.");
    println!("PREFIX_PATH is a prefix to remove from the paths (e.g. if grcov is run on a different machine than the one that generated the code coverage information).");
    println!("COVERALLS_REPO_TOKEN is the repository token from Coveralls, required for the 'coveralls' and 'coveralls+' format.");
    println!(
        "COVERALLS_COMMIT_SHA is the SHA of the commit used to generate the code coverage data."
    );
    println!("By default source files that can't be found on the disk are not ignored. Use --ignore-not-existing to ignore them.");
    println!("The --llvm option can be used when the code coverage information is exclusively coming from a llvm build, to speed-up parsing.");
    println!("The --ignore-dir option can be used to ignore files/directories specified as globs.");
    println!("The --branch option enables parsing branch coverage information.");
    println!("The --filter option allows filtering out covered/uncovered files. Use 'covered' to only return covered files, 'uncovered' to only return uncovered files.");
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("[ERROR]: Missing required directory argument.\n");
        print_usage(&args[0]);
        process::exit(1);
    }
    let mut output_type = "lcov";
    let mut source_dir = "";
    let mut prefix_dir = "";
    let mut repo_token = "";
    let mut commit_sha = "";
    let mut service_name = "";
    let mut service_number = "";
    let mut service_job_number = "";
    let mut ignore_not_existing = false;
    let mut to_ignore_dirs = Vec::new();
    let mut is_llvm = false;
    let mut branch_enabled = false;
    let mut paths = Vec::new();
    let mut i = 1;
    let mut path_mapping_file = "";
    let mut filter_option = None;
    let mut num_threads = num_cpus::get() * 2;
    let mut output_file_path = None;

    while i < args.len() {
        if args[i] == "-t" {
            if args.len() <= i + 1 {
                eprintln!("[ERROR]: Output format not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            output_type = &args[i + 1];
            i += 1;
        } else if args[i] == "-s" {
            if args.len() <= i + 1 {
                eprintln!("[ERROR]: Source root directory not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            source_dir = &args[i + 1];
            i += 1;
        } else if args[i] == "-p" {
            if args.len() <= i + 1 {
                eprintln!("[ERROR]: Prefix path not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            prefix_dir = &args[i + 1];
            i += 1;
        } else if args[i] == "--token" {
            if args.len() <= i + 1 {
                eprintln!("[ERROR]: Repository token not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            repo_token = &args[i + 1];
            i += 1;
        } else if args[i] == "--service-name" {
            if args.len() <= i + 1 {
                eprintln!("[ERROR]: Service name not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            service_name = &args[i + 1];
            i += 1;
        } else if args[i] == "--service-number" {
            if args.len() <= i + 1 {
                eprintln!("[ERROR]: Service number not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            service_number = &args[i + 1];
            i += 1;
        } else if args[i] == "--service-job-number" {
            if args.len() <= i + 1 {
                eprintln!("[ERROR]: Service job number not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            service_job_number = &args[i + 1];
            i += 1;
        } else if args[i] == "--commit-sha" {
            if args.len() <= i + 1 {
                eprintln!("[ERROR]: Commit SHA not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            commit_sha = &args[i + 1];
            i += 1;
        } else if args[i] == "--ignore-not-existing" {
            ignore_not_existing = true;
        } else if args[i] == "--ignore-dir" {
            if args.len() <= i + 1 {
                eprintln!("[ERROR]: Directory to ignore not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            to_ignore_dirs.push(args[i + 1].clone());
            i += 1;
        } else if args[i] == "--llvm" {
            is_llvm = true;
        } else if args[i] == "--path-mapping" {
            if args.len() <= i + 1 {
                eprintln!("[ERROR]: Path mapping file not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            path_mapping_file = &args[i + 1];
            i += 1;
        } else if args[i] == "--branch" {
            branch_enabled = true;
        } else if args[i] == "--filter" {
            if args.len() <= i + 1 {
                eprintln!("[ERROR]: Filter option not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            if args[i + 1] == "covered" {
                filter_option = Some(true);
            } else if args[i + 1] == "uncovered" {
                filter_option = Some(false);
            } else {
                eprintln!(
                    "[ERROR]: Filter option invalid (should be either 'covered' or 'uncovered')\n"
                );
                print_usage(&args[0]);
                process::exit(1);
            }
            i += 1;
        } else if args[i] == "--threads" {
            if args.len() <= i + 1 {
                eprintln!("[ERROR]: Number of threads not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            num_threads = args[i + 1]
                .parse()
                .expect("Number of threads should be a number");
            i += 1;
        } else if args[i] == "-o" {
            if args.len() <= i + 1 {
                eprintln!("[ERROR]: Output file not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }
            output_file_path = Some(&args[i + 1]);
            i += 1;
        } else {
            paths.push(args[i].clone());
        }

        i += 1;
    }

    if output_type != "ade"
        && output_type != "lcov"
        && output_type != "coveralls"
        && output_type != "coveralls+"
        && output_type != "files"
    {
        eprintln!(
            "[ERROR]: '{}' output format is not supported.\n",
            output_type
        );
        print_usage(&args[0]);
        process::exit(1);
    }

    if output_type == "coveralls" || output_type == "coveralls+" {
        if repo_token == "" {
            eprintln!(
                "[ERROR]: Repository token is needed when the output format is 'coveralls'.\n"
            );
            print_usage(&args[0]);
            process::exit(1);
        }

        if commit_sha == "" {
            eprintln!("[ERROR]: Commit SHA is needed when the output format is 'coveralls'.\n");
            print_usage(&args[0]);
            process::exit(1);
        }
    }

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

    let result_map: Arc<SyncCovResultMap> = Arc::new(Mutex::new(FxHashMap::with_capacity_and_hasher(20_000, Default::default())));
    let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());
    let path_mapping: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));

    let producer = {
        let queue = Arc::clone(&queue);
        let tmp_path = tmp_path.clone();
        let path_mapping_file = path_mapping_file.to_owned();
        let path_mapping = Arc::clone(&path_mapping);

        thread::Builder::new()
            .name(String::from("Producer"))
            .spawn(move || {
                let producer_path_mapping_buf = producer(
                    &tmp_path,
                    &paths,
                    &queue,
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
        let queue = Arc::clone(&queue);
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
                    &queue,
                    branch_enabled,
                );
            })
            .unwrap();

        parsers.push(t);
    }

    if let Err(_) = producer.join() {
        process::exit(1);
    }

    // Poison the queue, now that the producer is finished.
    for _ in 0..num_threads {
        queue.push(None);
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
        to_ignore_dirs,
        filter_option,
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
            service_job_number,
            commit_sha,
            false,
            output_file_path,
        );
    } else if output_type == "coveralls+" {
        output_coveralls(
            iterator,
            repo_token,
            service_name,
            service_number,
            service_job_number,
            commit_sha,
            true,
            output_file_path,
        );
    } else if output_type == "files" {
        output_files(iterator, output_file_path);
    } else {
        assert!(false, "{} is not a supported output type", output_type);
    }
}
