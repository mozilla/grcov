#![cfg_attr(feature="alloc_system",feature(alloc_system))]
#[cfg(feature="alloc_system")]
extern crate alloc_system;
#[macro_use]
extern crate serde_json;
extern crate crossbeam;
extern crate walkdir;
extern crate num_cpus;
extern crate semver;
extern crate crypto;
extern crate zip;
extern crate tempdir;
extern crate uuid;
extern crate libc;

use std::cmp;
use std::collections::{HashSet,HashMap};
use std::collections::hash_map::Entry;
use std::env;
use std::path::{Path, PathBuf};
use std::ffi::OsStr;
use std::fs;
use std::fs::File;
use zip::ZipArchive;
use std::io;
use std::io::{Read, BufRead, BufReader, Write, BufWriter};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::process;
use crossbeam::sync::MsQueue;
use walkdir::WalkDir;
use serde_json::Value;
use semver::Version;
use crypto::md5::Md5;
use crypto::digest::Digest;
use tempdir::TempDir;
use uuid::Uuid;
use std::ffi::CString;

/*
use std::os::raw::c_char;

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

type WorkQueue = MsQueue<Option<PathBuf>>;

struct Function {
    start: u32,
    executed: bool,
}

struct CovResult {
    covered: Vec<u32>,
    uncovered: Vec<u32>,
    functions: HashMap<String,Function>,
}

type CovResultMap = Mutex<HashMap<String,CovResult>>;

macro_rules! println_stderr(
    ($($arg:tt)*) => { {
        writeln!(&mut io::stderr(), $($arg)*).unwrap();
    } }
);

fn mkfifo(path: &str) {
    let filename = CString::new(path).unwrap();
    unsafe {
        if libc::mkfifo(filename.as_ptr(), 0o644) != 0 {
            panic!("mkfifo fail!");
        }
    }
}

#[test]
fn test_mkfifo() {
    let test_path = "/tmp/grcov_mkfifo_test";
    mkfifo(test_path);
    assert!(Path::new(test_path).exists());
    fs::remove_file(test_path).unwrap();
}

fn producer(directories: &[String], queue: &WorkQueue) {
    let gcda_ext = Some(OsStr::new("gcda"));
    let info_ext = Some(OsStr::new("info"));

    for directory in directories {
        for entry in WalkDir::new(&directory) {
            let entry = entry.expect(format!("Failed to open directory '{}'.", directory).as_str());
            let path = entry.path();
            if path.is_file() && (path.extension() == gcda_ext || path.extension() == info_ext) {
                queue.push(Some(fs::canonicalize(&path).unwrap()));
            }
        }
    }
}

#[test]
fn test_producer() {
    let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());
    let queue_consumer = queue.clone();

    producer(&vec!["test".to_string()], &queue);

    let endswith_strings: Vec<String> = vec![
        "grcov/test/Platform.gcda".to_string(),
        "grcov/test/sub2/RootAccessibleWrap.gcda".to_string(),
        "grcov/test/nsMaiInterfaceValue.gcda".to_string(),
        "grcov/test/sub/prova2.gcda".to_string(),
        "grcov/test/nsMaiInterfaceDocument.gcda".to_string(),
        "grcov/test/Unified_cpp_netwerk_base0.gcda".to_string(),
        "grcov/test/prova.gcda".to_string(),
        "grcov/test/nsGnomeModule.gcda".to_string(),
        "grcov/test/negative_counts.gcda".to_string(),
        "grcov/test/64bit_count.gcda".to_string(),
        "grcov/test/1494603973-2977-7.info".to_string(),
        "grcov/test/prova.info".to_string(),
    ];

    let mut vec: Vec<Option<PathBuf>> = Vec::new();
    for _ in 0..endswith_strings.len() {
        vec.push(queue_consumer.pop());
    }

    assert_eq!(vec.len(), 12);

    for endswith_string in endswith_strings.iter() {
        assert!(vec.iter().any(|x| x.clone().unwrap().ends_with(endswith_string)), "Missing {}", endswith_string);
    }

    assert_eq!(queue_consumer.try_pop(), None);

    let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());
    let queue_consumer = queue.clone();

    producer(&vec!["test/sub".to_string(), "test/sub2".to_string()], &queue);

    let endswith_strings: Vec<String> = vec![
        "grcov/test/sub2/RootAccessibleWrap.gcda".to_string(),
        "grcov/test/sub/prova2.gcda".to_string(),
    ];

    let mut vec: Vec<Option<PathBuf>> = Vec::new();
    for _ in 0..endswith_strings.len() {
        vec.push(queue_consumer.pop());
    }

    assert_eq!(vec.len(), 2);

    for endswith_string in endswith_strings.iter() {
        assert!(vec.iter().any(|x| x.clone().unwrap().ends_with(endswith_string)), "Missing {}", endswith_string);
    }

    assert_eq!(queue_consumer.try_pop(), None);
}

fn open_archive(path: &str) -> ZipArchive<File> {
    let file = File::open(&path).expect(format!("Failed to open ZIP file '{}'.", path).as_str());
    ZipArchive::new(file).expect(format!("Failed to parse ZIP file: {}", path).as_str())
}

fn extract_file(zip_file: &mut zip::read::ZipFile, path: &PathBuf) {
    let mut file = File::create(&path).expect("Failed to create file");
    io::copy(zip_file, &mut file).expect("Failed to copy file from ZIP");
}

fn zip_producer(tmp_dir: &Path, zip_files: &[String], queue: &WorkQueue) {
    let mut gcno_archive: Option<ZipArchive<File>> = None;
    let mut gcda_archives: Vec<ZipArchive<File>> = Vec::new();
    let mut info_archives: Vec<ZipArchive<File>> = Vec::new();

    for zip_file in zip_files.iter() {
        let archive = open_archive(zip_file);
        if zip_file.contains("gcno") {
            gcno_archive = Some(archive);
        } else if zip_file.contains("gcda") {
            gcda_archives.push(archive);
        } else if zip_file.contains("info") {
            info_archives.push(archive);
        } else {
            panic!("Unsupported archive type.");
        }
    }

    if let Some(mut gcno_archive) = gcno_archive {
        for i in 0..gcno_archive.len() {
            let mut gcno_file = gcno_archive.by_index(i).unwrap();
            let gcno_path_in_zip = PathBuf::from(gcno_file.name());
            let gcda_path_in_zip = gcno_path_in_zip.with_extension("gcda");

            let path = tmp_dir.join(gcno_path_in_zip);
            let stem = path.file_stem().unwrap().to_str().unwrap();

            fs::create_dir_all(path.parent().unwrap()).expect("Failed to create directory");

            if gcno_file.name().ends_with('/') {
                fs::create_dir_all(&path).expect("Failed to create directory");
            }
            else {
                let gcno_path = path.with_file_name(format!("{}_{}.gcno", stem, 1));
                extract_file(&mut gcno_file, &gcno_path);

                for (num, gcda_archive) in gcda_archives.iter_mut().enumerate() {
                    if let Ok(mut gcda_file) = gcda_archive.by_name(gcda_path_in_zip.to_str().unwrap()) {
                        // Create symlinks.
                        if num != 0 {
                            let link_path = path.with_file_name(format!("{}_{}.gcno", stem, num + 1));
                            fs::hard_link(&gcno_path, &link_path).expect(format!("Failed to create hardlink {}", link_path.display()).as_str());
                        }

                        let gcda_path = path.with_file_name(format!("{}_{}.gcda", stem, num + 1));

                        extract_file(&mut gcda_file, &gcda_path);

                        queue.push(Some(gcda_path));
                    }
                }
            }
        }
    }

    for (num, archive) in info_archives.iter_mut().enumerate() {
        for i in 0..archive.len() {
            let mut file = archive.by_index(i).unwrap();

            let path = tmp_dir.join(PathBuf::from(file.name()));

            fs::create_dir_all(path.parent().unwrap()).expect("Failed to create directory");

            if file.name().ends_with('/') {
                fs::create_dir_all(path).expect("Failed to create directory");
            }
            else {
                let new_path = path.with_file_name(format!("{}_{}.info", path.file_stem().unwrap().to_str().unwrap(), num));
                extract_file(&mut file, &new_path);
                queue.push(Some(new_path));
            }
        }
    }
}

#[test]
fn test_zip_producer() {
    let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());
    let queue_consumer = queue.clone();

    let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
    let tmp_path = tmp_dir.path().to_owned();
    zip_producer(&tmp_path, &vec!["test/gcno.zip".to_string(), "test/gcda1.zip".to_string(), "test/gcda2.zip".to_string()], &queue);

    let endswith_strings: Vec<String> = vec![
        "Platform_1.gcda".to_string(),
        "sub2/RootAccessibleWrap_1.gcda".to_string(),
        "nsMaiInterfaceValue_1.gcda".to_string(),
        "sub/prova2_1.gcda".to_string(),
        "nsMaiInterfaceDocument_1.gcda".to_string(),
        "nsGnomeModule_1.gcda".to_string(),
        "nsMaiInterfaceValue_2.gcda".to_string(),
        "nsMaiInterfaceDocument_2.gcda".to_string(),
        "nsGnomeModule_2.gcda".to_string(),
        "sub/prova2_2.gcda".to_string(),
    ];

    let mut vec: Vec<Option<PathBuf>> = Vec::new();
    for _ in 0..endswith_strings.len() {
        vec.push(queue_consumer.pop());
    }

    assert_eq!(vec.len(), 10);

    for endswith_string in endswith_strings.iter() {
        assert!(vec.iter().any(|x| x.clone().unwrap().ends_with(endswith_string)), "Missing {}", endswith_string);
    }

    // Assert file exists and file with the same name but with extension .gcno exists.
    for f in vec.iter() {
        let f = f.clone().unwrap();
        assert!(f.exists(), "{} doesn't exist", f.display());
        let gcno = f.with_file_name(format!("{}.gcno", f.file_stem().unwrap().to_str().unwrap()));
        assert!(gcno.exists(), "{} doesn't exist", gcno.display());
    }

    assert_eq!(queue_consumer.try_pop(), None);

    let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());
    let queue_consumer = queue.clone();

    zip_producer(&tmp_path, &vec!["test/info1.zip".to_string(), "test/info2.zip".to_string()], &queue);

    let endswith_strings: Vec<String> = vec![
        "1494603967-2977-2_0.info".to_string(),
        "1494603967-2977-3_0.info".to_string(),
        "1494603967-2977-4_0.info".to_string(),
        "1494603968-2977-5_0.info".to_string(),
        "1494603972-2977-6_0.info".to_string(),
        "1494603973-2977-7_0.info".to_string(),
        "1494603967-2977-2_1.info".to_string(),
        "1494603967-2977-3_1.info".to_string(),
        "1494603967-2977-4_1.info".to_string(),
        "1494603968-2977-5_1.info".to_string(),
        "1494603972-2977-6_1.info".to_string(),
        "1494603973-2977-7_1.info".to_string(),
    ];

    let mut vec: Vec<Option<PathBuf>> = Vec::new();
    for _ in 0..endswith_strings.len() {
        vec.push(queue_consumer.pop());
    }

    assert_eq!(vec.len(), 12);

    for endswith_string in endswith_strings.iter() {
        assert!(vec.iter().any(|x| x.clone().unwrap().ends_with(endswith_string)), "Missing {}", endswith_string);
    }

    for f in vec.iter() {
        let f = f.clone().unwrap();
        assert!(f.exists(), "{} doesn't exist", f.display());
    }

    assert_eq!(queue_consumer.try_pop(), None);
}

fn run_gcov(gcda_path: &PathBuf, working_dir: &PathBuf) {
    let status = Command::new("gcov")
                         .arg(gcda_path)
                         .arg("-i") // Generate intermediate gcov format, faster to parse.
                         .current_dir(working_dir)
                         .stdout(Stdio::null())
                         .stderr(Stdio::null())
                         .status()
                         .expect("Failed to execute gcov process");

    assert!(status.success(), "gcov wasn't successfully executed");
}

fn run_llvm_gcov(gcda_path: &PathBuf, working_dir: &PathBuf) {
    let status = Command::new("llvm-cov")
                         .arg("gcov")
                         .arg("-l") // Generate unique names for gcov files.
                         .arg("-b") // Generate function call information.
                         .arg(gcda_path)
                         .current_dir(working_dir)
                         .stdout(Stdio::null())
                         .stderr(Stdio::null())
                         .status()
                         .expect("Failed to execute llvm-cov process");

    assert!(status.success(), "llvm-cov wasn't successfully executed");
}

fn parse_lcov(lcov_path: &Path) -> Vec<(String,CovResult)> {
    let mut cur_file = String::new();
    let mut cur_lines_covered: Vec<u32> = Vec::new();
    let mut cur_lines_uncovered: Vec<u32> = Vec::new();
    let mut cur_functions: HashMap<String,Function> = HashMap::new();

    let mut results = Vec::new();

    let f = File::open(&lcov_path).expect("Failed to open gcov file");
    let file = BufReader::new(&f);
    for line in file.lines() {
        let l = line.unwrap();

        if l == "end_of_record" {
            results.push((cur_file, CovResult {
                covered: cur_lines_covered,
                uncovered: cur_lines_uncovered,
                functions: cur_functions,
            }));

            cur_file = String::new();
            cur_lines_covered = Vec::new();
            cur_lines_uncovered = Vec::new();
            cur_functions = HashMap::new();
        } else {
            let mut key_value = l.splitn(2, ':');
            let key = key_value.next().unwrap();
            let value = key_value.next();
            if value.is_none() {
                // Ignore lines without a ':' character.
                continue;
            }
            let value = value.unwrap();
            match key {
                "SF" => {
                    cur_file = value.to_string();
                },
                "DA" => {
                    let mut values = value.splitn(3, ',');
                    let line_no = values.next().unwrap().parse().unwrap();
                    let execution_count = values.next().unwrap();
                    if execution_count == "0" || execution_count.starts_with('-') {
                        cur_lines_uncovered.push(line_no);
                    } else {
                        cur_lines_covered.push(line_no);
                    }
                },
                "FN" => {
                    let mut f_splits = value.splitn(2, ',');
                    let start = f_splits.next().unwrap().parse().unwrap();
                    let f_name = f_splits.next().unwrap();
                    cur_functions.insert(f_name.to_string(), Function {
                      start: start,
                      executed: false,
                    });
                },
                "FNDA" => {
                    let mut f_splits = value.splitn(2, ',');
                    let executed = f_splits.next().unwrap() != "0";
                    let f_name = f_splits.next().unwrap();
                    let f = cur_functions.get_mut(f_name).expect(format!("FN record missing for function {}", f_name).as_str());
                    f.executed = executed;
                },
                _ => {}
            }
        }
    }

    results
}

#[test]
fn test_lcov_parser() {
    let results = parse_lcov(Path::new("./test/prova.info"));

    assert_eq!(results.len(), 603);

    let ref result1 = results[0];
    assert_eq!(result1.0, "resource://gre/components/MainProcessSingleton.js");
    assert_eq!(result1.1.covered, vec![7,9,10,12,13,12,16,17,18,19,18,21,28,67,90,68,70,74,75,76,77,78,83,84]);
    assert_eq!(result1.1.uncovered, vec![22,23,24,29,30,32,33,34,35,37,39,41,42,44,45,46,47,49,50,51,52,53,54,53,55,56,65,59,60,61,63]);
    assert!(result1.1.functions.contains_key("MainProcessSingleton"));
    let func = result1.1.functions.get("MainProcessSingleton").unwrap();
    assert_eq!(func.start, 15);
    assert_eq!(func.executed, true);
    assert!(result1.1.functions.contains_key("logConsoleMessage"));
    let func = result1.1.functions.get("logConsoleMessage").unwrap();
    assert_eq!(func.start, 21);
    assert_eq!(func.executed, false);
}

fn parse_old_gcov(gcov_path: &Path) -> Vec<(String,CovResult)> {
    let mut lines_covered = Vec::new();
    let mut lines_uncovered = Vec::new();
    let mut functions: HashMap<String,Function> = HashMap::new();

    let f = File::open(gcov_path).expect("Failed to open gcov file");
    let mut file = BufReader::new(&f);
    let mut line_no: u32 = 0;

    let mut first_line = String::new();
    file.read_line(&mut first_line).unwrap();
    // TODO: Don't collect in a Vec when parsing to avoid malloc overhead, both here and next.
    let splits: Vec<&str> = first_line.splitn(4, ':').collect();
    let mut source_name = splits[3].to_string();
    let len = source_name.len();
    source_name.truncate(len - 1);

    for line in file.lines() {
        let l = line.unwrap();
        let splits: Vec<&str> = l.splitn(3, ':').collect();
        if splits.len() == 1 {
            if !l.starts_with("function ") {
                continue;
            }

            let f_splits: Vec<&str> = l.splitn(5, ' ').collect();
            let execution_count: u64 = f_splits[3].parse().expect(&format!("Failed parsing execution count: {:?}", f_splits));
            functions.insert(f_splits[1].to_string(), Function {
              start: line_no + 1,
              executed: execution_count > 0,
            });
        } else {
            if splits.len() != 3 {
                println!("{:?}", splits);
                panic!("GCOV lines should be in the format STRING:STRING:STRING");
            }

            line_no = splits[1].trim().parse().unwrap();

            let cover = splits[0].trim();
            if cover == "-" {
                continue;
            }

            if cover == "#####" || cover.starts_with('-') {
                lines_uncovered.push(line_no);
            } else {
                lines_covered.push(line_no);
            }
        }
    }

    vec![(source_name, CovResult {
      covered: lines_covered,
      uncovered: lines_uncovered,
      functions: functions,
    })]
}

fn parse_gcov(gcov_path: &Path) -> Vec<(String,CovResult)> {
    let mut cur_file = String::new();
    let mut cur_lines_covered: Vec<u32> = Vec::new();
    let mut cur_lines_uncovered: Vec<u32> = Vec::new();
    let mut cur_functions: HashMap<String,Function> = HashMap::new();

    let mut results = Vec::new();

    let f = File::open(&gcov_path).expect("Failed to open gcov file");
    let file = BufReader::new(&f);
    for line in file.lines() {
        let l = line.unwrap();
        let mut key_value = l.splitn(2, ':');
        let key = key_value.next().unwrap();
        let value = key_value.next().unwrap();
        match key {
            "file" => {
                if !cur_file.is_empty() && (!cur_lines_covered.is_empty() || !cur_lines_uncovered.is_empty()) {
                    // println!("{} {} {:?} {:?}", gcov_path.display(), cur_file, cur_lines_covered, cur_lines_uncovered);
                    results.push((cur_file, CovResult {
                        covered: cur_lines_covered,
                        uncovered: cur_lines_uncovered,
                        functions: cur_functions,
                    }));
                }

                cur_file = value.to_string();
                cur_lines_covered = Vec::new();
                cur_lines_uncovered = Vec::new();
                cur_functions = HashMap::new();
            },
            "function" => {
                let mut f_splits = value.splitn(3, ',');
                let start = f_splits.next().unwrap().parse().unwrap();
                let executed = f_splits.next().unwrap() != "0";
                let f_name = f_splits.next().unwrap();
                cur_functions.insert(f_name.to_string(), Function {
                  start: start,
                  executed: executed,
                });
            },
            "lcount" => {
                let mut values = value.splitn(2, ',');
                let line_no = values.next().unwrap().parse().unwrap();
                let execution_count = values.next().unwrap();
                if execution_count == "0" || execution_count.starts_with('-') {
                    cur_lines_uncovered.push(line_no);
                } else {
                    cur_lines_covered.push(line_no);
                }
            },
            _ => {}
        }
    }

    if !cur_lines_covered.is_empty() || !cur_lines_uncovered.is_empty() {
        results.push((cur_file, CovResult {
            covered: cur_lines_covered,
            uncovered: cur_lines_uncovered,
            functions: cur_functions,
        }));
    }

    results
}

#[test]
fn test_parser() {
    let results = parse_gcov(Path::new("./test/prova.gcov"));

    assert_eq!(results.len(), 10);

    let ref result1 = results[0];
    assert_eq!(result1.0, "/home/marco/Documenti/FD/mozilla-central/build-cov-gcc/dist/include/nsExpirationTracker.h");
    assert!(result1.1.covered.is_empty());
    assert_eq!(result1.1.uncovered, vec![393,397,399,401,402,403,405]);
    assert!(result1.1.functions.contains_key("_ZN19nsExpirationTrackerIN11nsIDocument16SelectorCacheKeyELj4EE25ExpirationTrackerObserver7ReleaseEv"));
    let mut func = result1.1.functions.get("_ZN19nsExpirationTrackerIN11nsIDocument16SelectorCacheKeyELj4EE25ExpirationTrackerObserver7ReleaseEv").unwrap();
    assert_eq!(func.start, 393);
    assert_eq!(func.executed, false);

    let ref result5 = results[5];
    assert_eq!(result5.0, "/home/marco/Documenti/FD/mozilla-central/accessible/atk/Platform.cpp");
    assert_eq!(result5.1.covered, vec![136, 138, 216, 218, 226, 253, 261, 265, 268, 274, 277, 278, 281, 288, 289, 293, 294, 295, 298, 303, 306, 307, 309, 311, 312, 316, 317, 321, 322, 323, 324, 327, 328, 329, 330, 331, 332, 333, 338, 339, 340, 352, 353, 354, 355, 361, 362, 364, 365]);
    assert_eq!(result5.1.uncovered, vec![81, 83, 85, 87, 88, 90, 94, 96, 97, 98, 99, 100, 101, 103, 104, 108, 110, 111, 112, 115, 117, 118, 122, 123, 124, 128, 129, 130, 141, 142, 146, 147, 148, 151, 152, 153, 154, 155, 156, 157, 161, 162, 165, 166, 167, 168, 169, 170, 171, 172, 184, 187, 189, 190, 194, 195, 196, 200, 201, 202, 203, 207, 208, 219, 220, 221, 222, 223, 232, 233, 234, 313, 318, 343, 344, 345, 346, 347, 370, 372, 373, 374, 376]);
    assert!(result5.1.functions.contains_key("_ZL13LoadGtkModuleR24GnomeAccessibilityModule"));
    func = result5.1.functions.get("_ZL13LoadGtkModuleR24GnomeAccessibilityModule").unwrap();
    assert_eq!(func.start, 81);
    assert_eq!(func.executed, false);
    assert!(result5.1.functions.contains_key("_ZN7mozilla4a11y12PlatformInitEv"));
    func = result5.1.functions.get("_ZN7mozilla4a11y12PlatformInitEv").unwrap();
    assert_eq!(func.start, 136);
    assert_eq!(func.executed, true);
    assert!(result5.1.functions.contains_key("_ZN7mozilla4a11y16PlatformShutdownEv"));
    func = result5.1.functions.get("_ZN7mozilla4a11y16PlatformShutdownEv").unwrap();
    assert_eq!(func.start, 216);
    assert_eq!(func.executed, true);
    assert!(result5.1.functions.contains_key("_ZN7mozilla4a11y7PreInitEv"));
    func = result5.1.functions.get("_ZN7mozilla4a11y7PreInitEv").unwrap();
    assert_eq!(func.start, 261);
    assert_eq!(func.executed, true);
    assert!(result5.1.functions.contains_key("_ZN7mozilla4a11y19ShouldA11yBeEnabledEv"));
    func = result5.1.functions.get("_ZN7mozilla4a11y19ShouldA11yBeEnabledEv").unwrap();
    assert_eq!(func.start, 303);
    assert_eq!(func.executed, true);

    let results = parse_gcov(Path::new("./test/negative_counts.gcov"));
    assert_eq!(results.len(), 118);
    let ref negative_count_result = results[14];
    assert_eq!(negative_count_result.0, "/home/marco/Documenti/FD/mozilla-central/build-cov-gcc/dist/include/mozilla/Assertions.h");
    assert!(negative_count_result.1.covered.is_empty());
    assert_eq!(negative_count_result.1.uncovered, vec![40]);

    let results = parse_gcov(Path::new("./test/64bit_count.gcov"));
    assert_eq!(results.len(), 46);
    let ref a64bit_count_result = results[8];
    assert_eq!(a64bit_count_result.0, "/home/marco/Documenti/FD/mozilla-central/build-cov-gcc/dist/include/js/HashTable.h");
    assert_eq!(a64bit_count_result.1.covered, vec![324, 343, 344, 345, 357, 361, 399, 402, 403, 420, 709, 715, 801, 834, 835, 838, 840, 841, 842, 843, 845, 846, 847, 853, 854, 886, 887, 904, 908, 913, 916, 917, 940, 945, 960, 989, 990, 1019, 1029, 1038, 1065, 1075, 1076, 1090, 1112, 1113, 1118, 1119, 1120, 1197, 1202, 1207, 1210, 1211, 1212, 1222, 1223, 1225, 1237, 1238, 1240, 1244, 1250, 1257, 1264, 1278, 1279, 1283, 1284, 1285, 1286, 1289, 1293, 1294, 1297, 1299, 1309, 1310, 1316, 1327, 1329, 1330, 1331, 1337, 1344, 1345, 1353, 1354, 1364, 1372, 1381, 1382, 1385, 1391, 1397, 1400, 1403, 1404, 1405, 1407, 1408, 1412, 1414, 1415, 1417, 1420, 1433, 1442, 1443, 1446, 1452, 1456, 1459, 1461, 1462, 1471, 1474, 1475, 1476, 1477, 1478, 1484, 1485, 1489, 1490, 1491, 1492, 1495, 1496, 1497, 1498, 1499, 1500, 1506, 1507, 1513, 1516, 1518, 1522, 1527, 1530, 1547, 1548, 1549, 1552, 1554, 1571, 1573, 1574, 1575, 1576, 1577, 1580, 1581, 1582, 1693, 1711, 1730, 1732, 1733, 1735, 1736, 1739, 1741, 1743, 1744, 1747, 1749, 1750, 1752, 1753, 1754, 1755, 1759, 1761, 1767, 1772, 1773, 1776, 1777, 1780, 1781, 1785, 1786, 1789, 1790, 1796]);
    assert_eq!(a64bit_count_result.1.uncovered, vec![822, 825, 826, 828, 829, 831, 844, 1114, 1115, 1280, 1534, 1536, 1537, 1538, 1540, 1589, 1592, 1593,1594,1596,1597,1599,1600, 1601, 1604, 1605, 1606, 1607, 1609, 1610, 1611, 1615, 1616, 1625]);

    // Assert more stuff.
}

// Merge results, without caring about duplicate lines (they will be removed at the end).
fn merge_results(result: &mut CovResult, result2: &mut CovResult) {
    result.covered.append(&mut result2.covered);
    result.uncovered.append(&mut result2.uncovered);
    for (name, function) in result2.functions.drain() {
        match result.functions.entry(name) {
            Entry::Occupied(f) => f.into_mut().executed |= function.executed,
            Entry::Vacant(v) => { v.insert(function); }
        };
    }
}

#[test]
fn test_merge_results() {
    let mut functions1: HashMap<String,Function> = HashMap::new();
    functions1.insert("f1".to_string(), Function {
        start: 1,
        executed: false,
    });
    functions1.insert("f2".to_string(), Function {
        start: 2,
        executed: false,
    });
    let mut result = CovResult {
        covered: vec![1, 2],
        uncovered: vec![1, 7],
        functions: functions1,
    };
    let mut functions2: HashMap<String,Function> = HashMap::new();
    functions2.insert("f1".to_string(), Function {
        start: 1,
        executed: false,
    });
    functions2.insert("f2".to_string(), Function {
        start: 2,
        executed: true,
    });
    let mut result2 = CovResult {
        covered: vec![3, 4],
        uncovered: vec![1, 2, 8],
        functions: functions2,
    };

    merge_results(&mut result, &mut result2);
    assert_eq!(result.covered, vec![1, 2, 3, 4]);
    assert_eq!(result.uncovered, vec![1, 7, 1, 2, 8]);
    assert!(result.functions.contains_key("f1"));
    assert!(result.functions.contains_key("f2"));
    let mut func = result.functions.get("f1").unwrap();
    assert_eq!(func.start, 1);
    assert_eq!(func.executed, false);
    func = result.functions.get("f2").unwrap();
    assert_eq!(func.start, 2);
    assert_eq!(func.executed, true);
}

fn add_result(mut result: (String,CovResult), map: &mut HashMap<String,CovResult>) {
    match map.entry(result.0) {
        Entry::Occupied(obj) => {
            merge_results(obj.into_mut(), &mut result.1);
        },
        Entry::Vacant(v) => {
            v.insert(result.1);
        }
    };
}

fn process_gcov(gcov_path: &Path, is_llvm: bool, results_map: &CovResultMap) {
    let mut results = if is_llvm {
        parse_old_gcov(gcov_path)
    } else {
        parse_gcov(gcov_path)
    };

    fs::remove_file(gcov_path).unwrap();

    let mut map = results_map.lock().unwrap();
    for result in results.drain(..) {
        add_result(result, &mut map);
    }
}

fn clean_covered_lines(results: &mut HashMap<String,CovResult>) {
    for result in results.values_mut() {
        let result = &mut (*result);
        result.covered.sort();
        result.covered.dedup();

        result.uncovered.sort();
        result.uncovered.dedup();

        let set: HashSet<_> = result.covered.iter().collect();
        result.uncovered.retain(|x| !set.contains(x));
    }
}

fn to_activedata_etl_vec(normal_vec: &[u32]) -> Vec<Value> {
    normal_vec.iter().map(|&x| json!({"line": x})).collect()
}

fn output_activedata_etl(results: &mut HashMap<String,CovResult>) {
    for (key, result) in results {
        let mut orphan_covered: HashSet<u32> = result.covered.iter().cloned().collect();
        let mut orphan_uncovered: HashSet<u32> = result.uncovered.iter().cloned().collect();

        let end: u32 = cmp::max(result.covered.last().unwrap_or(&0), result.uncovered.last().unwrap_or(&0)) + 1;

        let mut start_indexes: Vec<u32> = Vec::new();
        for function in result.functions.values() {
            start_indexes.push(function.start);
        }
        start_indexes.sort();

        for (name, function) in result.functions.drain() {
            // println!("{} {} {}", name, function.executed, function.start);

            let mut func_end = end;

            for start in &start_indexes {
                if *start > function.start {
                    func_end = *start;
                    break;
                }
            }

            let mut lines_covered: Vec<Value> = Vec::new();
            for line in result.covered.iter().filter(|&&x| x >= function.start && x < func_end) {
                lines_covered.push(json!({
                    "line": *line
                }));
                orphan_covered.remove(line);
            }

            let mut lines_uncovered: Vec<u32> = Vec::new();
            for line in result.uncovered.iter().filter(|&&x| x >= function.start && x < func_end) {
                lines_uncovered.push(*line);
                orphan_uncovered.remove(line);
            }

            println!("{}", json!({
                "language": "c/c++",
                "file": {
                    "name": key,
                },
                "method": {
                    "name": name,
                    "covered": lines_covered,
                    "uncovered": lines_uncovered,
                    "total_covered": lines_covered.len(),
                    "total_uncovered": lines_uncovered.len(),
                    "percentage_covered": lines_covered.len() as f32 / (lines_covered.len() + lines_uncovered.len()) as f32,
                }
            }));
        }

        let mut orphan_covered: Vec<u32> = orphan_covered.into_iter().collect();
        orphan_covered.sort();
        let mut orphan_uncovered: Vec<u32> = orphan_uncovered.into_iter().collect();
        orphan_uncovered.sort();

        // The orphan lines will represent the file as a whole.
        println!("{}", json!({
            "language": "c/c++",
            "is_file": true,
            "file": {
                "name": key,
                "covered": to_activedata_etl_vec(&result.covered),
                "uncovered": result.uncovered,
                "total_covered": result.covered.len(),
                "total_uncovered": result.uncovered.len(),
                "percentage_covered": result.covered.len() as f32 / (result.covered.len() + result.uncovered.len()) as f32,
            },
            "method": {
                "covered": to_activedata_etl_vec(&orphan_covered),
                "uncovered": orphan_uncovered,
                "total_covered": orphan_covered.len(),
                "total_uncovered": orphan_uncovered.len(),
                "percentage_covered": orphan_covered.len() as f32 / (orphan_covered.len() + orphan_uncovered.len()) as f32,
            }
        }));
    }
}

fn output_lcov(results: &mut HashMap<String,CovResult>, source_dir: &str) {
    let stdout = io::stdout();
    let mut writer = BufWriter::new(stdout.lock());

    writer.write_all(b"TN:\n").unwrap();

    for (key, result) in results {
        // println!("{} {:?} {:?}", key, result.covered, result.uncovered);

        if source_dir != "" {
            let path = PathBuf::from(key);
            let unprefixed = if path.starts_with(source_dir) {
                path.strip_prefix(source_dir).unwrap().to_path_buf()
            } else {
                path
            };
            write!(writer, "SF:{}\n", unprefixed.display()).unwrap();
        } else {
            write!(writer, "SF:{}\n", key).unwrap();
        }

        for (name, function) in &result.functions {
            write!(writer, "FN:{},{}\n", function.start, name).unwrap();
        }
        for (name, function) in &result.functions {
            write!(writer, "FNDA:{},{}\n", if function.executed { 1 } else { 0 }, name).unwrap();
        }
        if !result.functions.is_empty() {
            write!(writer, "FNF:{}\n", result.functions.len()).unwrap();
            write!(writer, "FNF:{}\n", result.functions.values().filter(|x| x.executed).count()).unwrap();
        }

        let mut lines_map: HashMap<u32,u8> = HashMap::new();
        for line in &result.covered {
            lines_map.insert(*line, 1);
        }
        for line in &result.uncovered {
            lines_map.insert(*line, 0);
        }
        let mut all_lines: Vec<u32> = result.covered.clone();
        all_lines.append(&mut result.uncovered.clone());
        all_lines.sort();
        for line in &all_lines {
            write!(writer, "DA:{},{}\n", line, lines_map[line]).unwrap();
        }
        write!(writer, "LF:{}\n", all_lines.len()).unwrap();
        write!(writer, "LH:{}\n", result.covered.len()).unwrap();
        writer.write_all(b"end_of_record\n").unwrap();
    }
}

fn get_digest(path: PathBuf) -> String {
    match File::open(path) {
        Ok(mut f) => {
            let mut buffer = Vec::new();
            f.read_to_end(&mut buffer).unwrap();

            let mut hasher = Md5::new();
            hasher.input(buffer.as_slice());

            hasher.result_str()
        },
        Err(_) => {
            Uuid::new_v4().simple().to_string()
        }
    }
}

fn output_coveralls(results: &mut HashMap<String,CovResult>, source_dir: &str, prefix_dir: &str, repo_token: &str, service_name: &str, service_number: &str, service_job_number: &str, commit_sha: &str, ignore_global: bool, ignore_not_existing: bool, to_ignore_dir: &Option<String>) {
    let source_dir = fs::canonicalize(&source_dir).expect("Source directory does not exist.");

    let mut source_files = Vec::new();

    for (key, result) in results {
        let path = PathBuf::from(key);

        // Remove prefix from path.
        let unprefixed = if path.starts_with(prefix_dir) {
            path.strip_prefix(prefix_dir).unwrap().to_path_buf()
        } else {
            path
        };

        if ignore_global && !unprefixed.is_relative() {
            continue;
        }

        // Get absolute path to source file.
        let path = if unprefixed.is_relative() {
            PathBuf::from(&source_dir).join(&unprefixed)
        } else {
            unprefixed
        };

        // Canonicalize, if possible.
        let path = match fs::canonicalize(&path) {
            Ok(p) => p,
            Err(_) => path,
        };

        // Remove source dir from path.
        let unprefixed = if path.starts_with(&source_dir) {
            path.strip_prefix(&source_dir).unwrap().to_path_buf()
        } else {
            path.clone()
        };

        if to_ignore_dir.is_some() && unprefixed.starts_with(to_ignore_dir.as_ref().unwrap()) {
            continue;
        }

        if ignore_not_existing && !path.exists() {
            continue;
        }

        let end: u32 = cmp::max(result.covered.last().unwrap_or(&0), result.uncovered.last().unwrap_or(&0)) + 1;

        let mut lines_map: HashMap<u32,u8> = HashMap::new();
        for line in &result.covered {
            lines_map.insert(*line, 1);
        }
        for line in &result.uncovered {
            lines_map.insert(*line, 0);
        }

        let mut coverage = Vec::new();
        for line in 1..end {
            match lines_map.entry(line) {
                Entry::Occupied(covered) => {
                    coverage.push(Value::from(*covered.get()));
                },
                Entry::Vacant(_) => {
                    coverage.push(Value::Null);
                }
            };
        }

        source_files.push(json!({
            "name": unprefixed,
            "source_digest": get_digest(path),
            "coverage": coverage,
        }));
    }

    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    serde_json::to_writer(&mut stdout, &json!({
        "repo_token": repo_token,
        "git": {
          "head": {
            "id": commit_sha,
          },
          "branch": "master",
        },
        "source_files": source_files,
        "service_name": service_name,
        "service_number": service_number,
        "service_job_number": service_job_number,
    })).unwrap();
}

fn print_usage(program: &str) {
    println!("Usage: {} DIRECTORY[...] [-t OUTPUT_TYPE] [-s SOURCE_ROOT] [-p PREFIX_PATH] [--token COVERALLS_REPO_TOKEN] [--commit-sha COVERALLS_COMMIT_SHA] [-z] [--keep-global-includes] [--ignore-not-existing] [--ignore-dir DIRECTORY] [--llvm]", program);
    println!("You can specify one or more directories, separated by a space.");
    println!("OUTPUT_TYPE can be one of:");
    println!(" - (DEFAULT) ade for the ActiveData-ETL specific format;");
    println!(" - lcov for the lcov INFO format;");
    println!(" - coveralls for the Coveralls specific format.");
    println!("SOURCE_ROOT is the root directory of the source files.");
    println!("PREFIX_PATH is a prefix to remove from the paths (e.g. if grcov is run on a different machine than the one that generated the code coverage information).");
    println!("COVERALLS_REPO_TOKEN is the repository token from Coveralls, required for the 'coveralls' format.");
    println!("COVERALLS_COMMIT_SHA is the SHA of the commit used to generate the code coverage data.");
    println!("Use -z to use ZIP files instead of directories (the first ZIP file must contain the GCNO files, the following ones must contain the GCDA files).");
    println!("By default global includes are ignored. Use --keep-global-includes to keep them.");
    println!("By default source files that can't be found on the disk are not ignored. Use --ignore-not-existing to ignore them.");
    println!("The --llvm option must be used when the code coverage information is coming from a llvm build.");
    println!("The --ignore-dir option can be used to ignore a directory.");
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

#[test]
fn test_is_recent_version() {
    assert!(!is_recent_version("gcov (Ubuntu 4.3.0-12ubuntu2) 4.3.0 20170406"));
    assert!(is_recent_version("gcov (Ubuntu 4.9.0-12ubuntu2) 4.9.0 20170406"));
    assert!(is_recent_version("gcov (Ubuntu 6.3.0-12ubuntu2) 6.3.0 20170406"));
}

fn check_gcov_version() -> bool {
    let output = Command::new("gcov")
                         .arg("--version")
                         .output()
                         .expect("Failed to execute `gcov`. `gcov` is required (it is part of GCC).");

    assert!(output.status.success(), "`gcov` failed to execute.");

    is_recent_version(&String::from_utf8(output.stdout).unwrap())
}

fn main() {
    if !check_gcov_version() {
        println_stderr!("[ERROR]: gcov (bundled with GCC) >= 4.9 is required.\n");
        process::exit(1);
    }

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        println_stderr!("[ERROR]: Missing required directory argument.\n");
        print_usage(&args[0]);
        process::exit(1);
    }
    let mut output_type: &String = &"ade".to_string();
    let mut source_dir: &String = &String::new();
    let mut prefix_dir: &String = &String::new();
    let mut repo_token: &String = &String::new();
    let mut commit_sha: &String = &String::new();
    let mut service_name: &String = &String::new();
    let mut service_number: &String = &String::new();
    let mut service_job_number: &String = &String::new();
    let mut ignore_global: bool = true;
    let mut ignore_not_existing: bool = false;
    let mut to_ignore_dir: &String = &"".to_string();
    let mut is_llvm: bool = false;
    let mut directories: Vec<String> = Vec::new();
    let mut i = 1;
    let mut is_zip = false;
    while i < args.len() {
        if args[i] == "-t" {
            if args.len() <= i + 1 {
                println_stderr!("[ERROR]: Output format not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            output_type = &args[i + 1];
            i += 1;
        } else if args[i] == "-s" {
            if args.len() <= i + 1 {
                println_stderr!("[ERROR]: Source root directory not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            source_dir = &args[i + 1];
            i += 1;
        } else if args[i] == "-p" {
            if args.len() <= i + 1 {
                println_stderr!("[ERROR]: Prefix path not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            prefix_dir = &args[i + 1];
            i += 1;
        } else if args[i] == "--token" {
            if args.len() <= i + 1 {
                println_stderr!("[ERROR]: Repository token not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            repo_token = &args[i + 1];
            i += 1;
        } else if args[i] == "--service-name" {
            if args.len() <= i + 1 {
                println_stderr!("[ERROR]: Service name not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            service_name = &args[i + 1];
            i += 1;
        } else if args[i] == "--service-number" {
            if args.len() <= i + 1 {
                println_stderr!("[ERROR]: Service number not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            service_number = &args[i + 1];
            i += 1;
        } else if args[i] == "--service-job-number" {
            if args.len() <= i + 1 {
                println_stderr!("[ERROR]: Service job number not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            service_job_number = &args[i + 1];
            i += 1;
        } else if args[i] == "--commit-sha" {
            if args.len() <= i + 1 {
                println_stderr!("[ERROR]: Commit SHA not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            commit_sha = &args[i + 1];
            i += 1;
        } else if args[i] == "-z" {
            is_zip = true;
        } else if args[i] == "--keep-global-includes" {
            ignore_global = false;
        } else if args[i] == "--ignore-not-existing" {
            ignore_not_existing = true;
        } else if args[i] == "--ignore-dir" {
            if args.len() <= i + 1 {
                println_stderr!("[ERROR]: Directory to ignore not specified.\n");
                print_usage(&args[0]);
                process::exit(1);
            }

            to_ignore_dir = &args[i + 1];
            i += 1;
        } else if args[i] == "--llvm" {
            is_llvm = true;
        } else {
            directories.push(args[i].to_owned());
        }

        i += 1;
    }

    if output_type != "ade" && output_type != "lcov" && output_type != "coveralls" {
        println_stderr!("[ERROR]: '{}' output format is not supported.\n", output_type);
        print_usage(&args[0]);
        process::exit(1);
    }

    if output_type == "coveralls" {
        if repo_token == "" {
            println_stderr!("[ERROR]: Repository token is needed when the output format is 'coveralls'.\n");
            print_usage(&args[0]);
            process::exit(1);
        }

        if commit_sha == "" {
            println_stderr!("[ERROR]: Commit SHA is needed when the output format is 'coveralls'.\n");
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
        Some(to_ignore_dir.clone())
    };

    let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
    let tmp_path = tmp_dir.path().to_owned();

    let results: Arc<CovResultMap> = Arc::new(Mutex::new(HashMap::new()));
    let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

    let producer = {
        let queue = queue.clone();
        let tmp_path = tmp_path.clone();

        thread::spawn(move || {
            if is_zip {
                zip_producer(&tmp_path, &directories, &queue);
            } else {
                producer(&directories, &queue);
            }
        })
    };

    let mut parsers = Vec::new();

    let num_threads = num_cpus::get() * 2;

    for i in 0..num_threads {
        let queue = queue.clone();
        let results = results.clone();
        let tmp_path = tmp_path.clone();

        let t = thread::spawn(move || {
            let working_dir = tmp_path.join(format!("{}", i));
            fs::create_dir(&working_dir).expect("Failed to create working directory");

            while let Some(gcda_path) = queue.pop() {
                if is_llvm {
                    run_llvm_gcov(&gcda_path, &working_dir);
                    for entry in WalkDir::new(&working_dir).min_depth(1) {
                        process_gcov(entry.unwrap().path(), is_llvm, &results);
                    }
                } else {
                    run_gcov(&gcda_path, &working_dir);
                    process_gcov(working_dir.join(gcda_path.file_name().unwrap().to_str().unwrap().to_string() + ".gcov").as_path(), is_llvm, &results);
                }
            }
        });

        parsers.push(t);
    }

    let _ = producer.join();

    // Poison the queue, now that the producer is finished.
    for _ in 0..num_threads {
        queue.push(None);
    }

    for parser in parsers {
        let _ = parser.join();
    }

    let results_obj = &mut (*results.lock().unwrap());

    clean_covered_lines(results_obj);

    if output_type == "ade" {
        output_activedata_etl(results_obj);
    } else if output_type == "lcov" {
        output_lcov(results_obj, source_dir);
    } else if output_type == "coveralls" {
        output_coveralls(results_obj, source_dir, prefix_dir, repo_token, service_name, service_number, service_job_number, commit_sha, ignore_global, ignore_not_existing, &to_ignore_dir);
    }
}
