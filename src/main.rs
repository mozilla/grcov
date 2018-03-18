#![cfg_attr(feature="alloc_system",feature(alloc_system))]
#[cfg(feature="alloc_system")]
#[cfg(feature = "yaml")]
extern crate alloc_system;
extern crate serde_json;
extern crate crossbeam;
extern crate num_cpus;
extern crate tempdir;
extern crate grcov;
#[macro_use]
extern crate clap;

use std::collections::HashMap;
use std::{thread, process};
use std::fs::{self, File};
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use crossbeam::sync::MsQueue;
use serde_json::Value;
use tempdir::TempDir;
use clap::App;

use grcov::*;


fn main() {
    
    let yaml = load_yaml!("cli.yml");
    let matches = App::from_yaml(yaml).get_matches();

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
    let mut path_mapping_file = "";
    let mut filter_covered = true;
    let mut num_threads = num_cpus::get() * 2;
       
    let path: Vec<&str> = matches.values_of("DIRECTORY_OR_ZIP_FILE").unwrap().collect(); 
   
    let n = path.len();
    
    for i in 1..n {
     paths.push(path[i].to_string().clone());
    }

    if matches.is_present("--branch") {
       branch_enabled = true; 
    }
    if matches.is_present("--filter-covered"){
        filter_covered = true;
    }
    if matches.is_present("--filter-uncovered"){
        filter_covered = false;
    }
    if matches.is_present("--llvm"){
        is_llvm = true;
    }
    if matches.is_present("--keep-global-includes"){
        ignore_global = false;
    }
    if matches.is_present("--ignore-not-existing"){
        ignore_not_existing = true;
    }
    if let Some(o) = matches.value_of("t"){
        output_type = o;
    }
    if let Some(o) = matches.value_of("s") {
           source_dir = o;
    }
    if let Some(o) = matches.value_of("p") {
           prefix_dir = o;
    }
    if let Some(o) = matches.value_of("token"){
        repo_token = o;
    }
    if let Some(o) = matches.value_of("service-name"){
        service_name = o;
    }
    if let Some(o) = matches.value_of("service-number"){
        service_number = o;
    }
    if let Some(o) = matches.value_of("service-job-number"){
        service_job_number = o;
    }
    if let Some(o) = matches.value_of("commit-sha"){
        commit_sha = o;
    }
    if let Some(o) = matches.value_of("ignore-dir"){
        to_ignore_dir = o;
    }
    if let Some(o) = matches.value_of("path-mapping"){
        path_mapping_file = o;
    }
    if let Some(o) = matches.value_of("threads"){
        num_threads = o.parse().expect("Number of threads should be a number");
    }
   

    if !is_llvm && !check_gcov_version() {
        eprintln!("[ERROR]: gcov (bundled with GCC) >= 4.9 is required.\n");
        process::exit(1);
    }

    if output_type == "coveralls" || output_type == "coveralls+" {
        if repo_token == "" {
            eprintln!("[ERROR]: Repository token is needed when the output format is 'coveralls'.\n");
            process::exit(1);
        }

        if commit_sha == "" {
            eprintln!("[ERROR]: Commit SHA is needed when the output format is 'coveralls'.\n");
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
            let producer_path_mapping_buf = producer(&tmp_path, paths.as_slice(), &queue);

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
