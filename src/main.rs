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

use std::collections::{BTreeSet, BTreeMap, btree_map, HashMap, hash_map};
use std::env;
use std::path::{Path, PathBuf};
use std::ffi::OsStr;
use std::fs;
use std::fs::File;
use zip::ZipArchive;
use std::io;
use std::io::{Cursor, Read, BufRead, BufReader, Write, BufWriter};
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
#[cfg(unix)]
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

#[derive(Debug,PartialEq)]
enum ItemFormat {
    GCDA,
    INFO,
}

#[derive(Debug)]
enum ItemType {
    Path(PathBuf),
    Content(Vec<u8>),
}

#[derive(Debug)]
struct WorkItem {
    format: ItemFormat,
    item: ItemType,
}

impl WorkItem {
    fn path(&self) -> &PathBuf {
        if let ItemType::Path(ref p) = self.item {
            p
        } else {
            panic!("Path expected");
        }
    }
}

type WorkQueue = MsQueue<Option<WorkItem>>;

#[derive(Debug,Clone,PartialEq)]
struct Function {
    start: u32,
    executed: bool,
}

#[derive(Debug,Clone,PartialEq)]
struct CovResult {
    lines: BTreeMap<u32,u64>,
    functions: HashMap<String,Function>,
}

type CovResultMap = HashMap<String,CovResult>;
type SyncCovResultMap = Mutex<CovResultMap>;
type CovResultIter = Box<Iterator<Item=(PathBuf,PathBuf,CovResult)>>;

macro_rules! println_stderr(
    ($($arg:tt)*) => { {
        writeln!(&mut io::stderr(), $($arg)*).unwrap();
    } }
);

#[cfg(unix)]
fn mkfifo<P: AsRef<Path>>(path: P) {
    let filename = CString::new(path.as_ref().as_os_str().to_str().unwrap()).unwrap();
    unsafe {
        if libc::mkfifo(filename.as_ptr(), 0o644) != 0 {
            panic!("mkfifo fail!");
        }
    }
}
#[cfg(windows)]
fn mkfifo<P: AsRef<Path>>(path: P) {
}

#[cfg(unix)]
#[test]
fn test_mkfifo() {
    let test_path = "/tmp/grcov_mkfifo_test";
    assert!(!Path::new(test_path).exists());
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
            if path.is_file() {
                let ext = path.extension();
                let format = if ext == gcda_ext {
                    ItemFormat::GCDA
                } else if ext == info_ext {
                    ItemFormat::INFO
                } else {
                    continue
                };

                queue.push(Some(WorkItem {
                    format: format,
                    item: ItemType::Path(fs::canonicalize(&path).unwrap()),
                }));
            }
        }
    }
}

#[cfg(test)]
fn check_produced(queue: &WorkQueue, expected: Vec<(ItemFormat,bool,&str)>) {
    let mut vec: Vec<Option<WorkItem>> = Vec::new();

    for _ in 0..expected.len() {
        vec.push(queue.pop());
    }

    assert!(queue.try_pop().is_none());

    for elem in &expected {
        assert!(vec.iter().any(|x| {
            if !x.is_some() {
                return false;
            }

            let x = x.as_ref().unwrap();

            if x.format != elem.0 {
                return false;
            }

            match x.item {
                ItemType::Content(_) => {
                    !elem.1
                },
                ItemType::Path(ref p) => {
                    elem.1 && p.ends_with(elem.2)
                }
            }
        }), "Missing {:?}", elem);
    }

    // Assert file exists and file with the same name but with extension .gcno exists.
    for x in vec.iter() {
        let x = x.as_ref().unwrap();

        match x.item {
            ItemType::Content(_) => {
                continue
            },
            ItemType::Path(ref p) => {
                assert!(p.exists(), "{} doesn't exist", p.display());
                if x.format == ItemFormat::GCDA {
                    let gcno = p.with_file_name(format!("{}.gcno", p.file_stem().unwrap().to_str().unwrap()));
                    assert!(gcno.exists(), "{} doesn't exist", gcno.display());
                }
            }
        }
    }
}

#[test]
fn test_producer() {
    let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

    producer(&vec!["test".to_string()], &queue);

    let expected: Vec<(ItemFormat,bool,&str)> = vec![
        (ItemFormat::GCDA, true, "grcov/test/Platform.gcda"),
        (ItemFormat::GCDA, true, "grcov/test/sub2/RootAccessibleWrap.gcda"),
        (ItemFormat::GCDA, true, "grcov/test/nsMaiInterfaceValue.gcda"),
        (ItemFormat::GCDA, true, "grcov/test/sub/prova2.gcda"),
        (ItemFormat::GCDA, true, "grcov/test/nsMaiInterfaceDocument.gcda"),
        (ItemFormat::GCDA, true, "grcov/test/Unified_cpp_netwerk_base0.gcda"),
        (ItemFormat::GCDA, true, "grcov/test/prova.gcda"),
        (ItemFormat::GCDA, true, "grcov/test/nsGnomeModule.gcda"),
        (ItemFormat::GCDA, true, "grcov/test/negative_counts.gcda"),
        (ItemFormat::GCDA, true, "grcov/test/64bit_count.gcda"),
        (ItemFormat::INFO, true, "grcov/test/1494603973-2977-7.info"),
        (ItemFormat::INFO, true, "grcov/test/prova.info"),
    ];

    check_produced(&queue, expected);


    let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

    producer(&vec!["test/sub".to_string(), "test/sub2".to_string()], &queue);

    let expected: Vec<(ItemFormat,bool,&str)> = vec![
        (ItemFormat::GCDA, true, "grcov/test/sub2/RootAccessibleWrap.gcda"),
        (ItemFormat::GCDA, true, "grcov/test/sub/prova2.gcda"),
    ];

    check_produced(&queue, expected);
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

            let path = tmp_dir.join(&gcno_path_in_zip);

            fs::create_dir_all(path.parent().unwrap()).expect("Failed to create directory");

            if gcno_file.name().ends_with('/') {
                fs::create_dir_all(&path).expect("Failed to create directory");
            }
            else {
                let stem = path.file_stem().unwrap().to_str().unwrap();

                let gcno_path = path.with_file_name(format!("{}_{}.gcno", stem, 1));
                extract_file(&mut gcno_file, &gcno_path);

                let gcda_path_in_zip = gcno_path_in_zip.with_extension("gcda");

                for (num, gcda_archive) in gcda_archives.iter_mut().enumerate() {
                    if let Ok(mut gcda_file) = gcda_archive.by_name(gcda_path_in_zip.to_str().unwrap()) {
                        // Create symlinks.
                        if num != 0 {
                            let link_path = path.with_file_name(format!("{}_{}.gcno", stem, num + 1));
                            fs::hard_link(&gcno_path, &link_path).expect(format!("Failed to create hardlink {}", link_path.display()).as_str());
                        }

                        let gcda_path = path.with_file_name(format!("{}_{}.gcda", stem, num + 1));

                        extract_file(&mut gcda_file, &gcda_path);

                        queue.push(Some(WorkItem {
                            format: ItemFormat::GCDA,
                            item: ItemType::Path(gcda_path),
                        }));
                    }
                }
            }
        }
    }

    for archive in &mut info_archives {
        for i in 0..archive.len() {
            let mut file = archive.by_index(i).unwrap();

            if file.name().ends_with('/') {
                continue;
            }

            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer).unwrap();
            queue.push(Some(WorkItem {
                format: ItemFormat::INFO,
                item: ItemType::Content(buffer),
            }));
        }
    }
}

#[test]
fn test_zip_producer() {
    // Test extracting multiple gcda archives.
    let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

    let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
    let tmp_path = tmp_dir.path().to_owned();
    zip_producer(&tmp_path, &vec!["test/gcno.zip".to_string(), "test/gcda1.zip".to_string(), "test/gcda2.zip".to_string()], &queue);

    let expected: Vec<(ItemFormat,bool,&str)> = vec![
        (ItemFormat::GCDA, true, "Platform_1.gcda"),
        (ItemFormat::GCDA, true, "sub2/RootAccessibleWrap_1.gcda"),
        (ItemFormat::GCDA, true, "nsMaiInterfaceValue_1.gcda"),
        (ItemFormat::GCDA, true, "sub/prova2_1.gcda"),
        (ItemFormat::GCDA, true, "nsMaiInterfaceDocument_1.gcda"),
        (ItemFormat::GCDA, true, "nsGnomeModule_1.gcda"),
        (ItemFormat::GCDA, true, "nsMaiInterfaceValue_2.gcda"),
        (ItemFormat::GCDA, true, "nsMaiInterfaceDocument_2.gcda"),
        (ItemFormat::GCDA, true, "nsGnomeModule_2.gcda"),
        (ItemFormat::GCDA, true, "sub/prova2_2.gcda"),
    ];

    check_produced(&queue, expected);

    // Test calling zip_producer with a different order of zip files.
    let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

    let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
    let tmp_path = tmp_dir.path().to_owned();
    zip_producer(&tmp_path, &vec!["test/gcda1.zip".to_string(), "test/gcno.zip".to_string(), "test/gcda2.zip".to_string()], &queue);

    let expected: Vec<(ItemFormat,bool,&str)> = vec![
        (ItemFormat::GCDA, true, "Platform_1.gcda"),
        (ItemFormat::GCDA, true, "sub2/RootAccessibleWrap_1.gcda"),
        (ItemFormat::GCDA, true, "nsMaiInterfaceValue_1.gcda"),
        (ItemFormat::GCDA, true, "sub/prova2_1.gcda"),
        (ItemFormat::GCDA, true, "nsMaiInterfaceDocument_1.gcda"),
        (ItemFormat::GCDA, true, "nsGnomeModule_1.gcda"),
        (ItemFormat::GCDA, true, "nsMaiInterfaceValue_2.gcda"),
        (ItemFormat::GCDA, true, "nsMaiInterfaceDocument_2.gcda"),
        (ItemFormat::GCDA, true, "nsGnomeModule_2.gcda"),
        (ItemFormat::GCDA, true, "sub/prova2_2.gcda"),
    ];

    check_produced(&queue, expected);

    // Test extracting info files.
    let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

    let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
    let tmp_path = tmp_dir.path().to_owned();
    zip_producer(&tmp_path, &vec!["test/info1.zip".to_string(), "test/info2.zip".to_string()], &queue);

    let expected: Vec<(ItemFormat,bool,&str)> = vec![
        (ItemFormat::INFO, false, "1494603967-2977-2_0.info"),
        (ItemFormat::INFO, false, "1494603967-2977-3_0.info"),
        (ItemFormat::INFO, false, "1494603967-2977-4_0.info"),
        (ItemFormat::INFO, false, "1494603968-2977-5_0.info"),
        (ItemFormat::INFO, false, "1494603972-2977-6_0.info"),
        (ItemFormat::INFO, false, "1494603973-2977-7_0.info"),
        (ItemFormat::INFO, false, "1494603967-2977-2_1.info"),
        (ItemFormat::INFO, false, "1494603967-2977-3_1.info"),
        (ItemFormat::INFO, false, "1494603967-2977-4_1.info"),
        (ItemFormat::INFO, false, "1494603968-2977-5_1.info"),
        (ItemFormat::INFO, false, "1494603972-2977-6_1.info"),
        (ItemFormat::INFO, false, "1494603973-2977-7_1.info"),
    ];

    check_produced(&queue, expected);

    // Test extracting both info and gcno/gcda files.
    let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

    let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
    let tmp_path = tmp_dir.path().to_owned();
    zip_producer(&tmp_path, &vec!["test/gcno.zip".to_string(), "test/gcda1.zip".to_string(), "test/info1.zip".to_string(), "test/info2.zip".to_string()], &queue);

    let expected: Vec<(ItemFormat,bool,&str)> = vec![
        (ItemFormat::GCDA, true, "Platform_1.gcda"),
        (ItemFormat::GCDA, true, "sub2/RootAccessibleWrap_1.gcda"),
        (ItemFormat::GCDA, true, "nsMaiInterfaceValue_1.gcda"),
        (ItemFormat::GCDA, true, "sub/prova2_1.gcda"),
        (ItemFormat::GCDA, true, "nsMaiInterfaceDocument_1.gcda"),
        (ItemFormat::GCDA, true, "nsGnomeModule_1.gcda"),
        (ItemFormat::INFO, false, "1494603967-2977-2_0.info"),
        (ItemFormat::INFO, false, "1494603967-2977-3_0.info"),
        (ItemFormat::INFO, false, "1494603967-2977-4_0.info"),
        (ItemFormat::INFO, false, "1494603968-2977-5_0.info"),
        (ItemFormat::INFO, false, "1494603972-2977-6_0.info"),
        (ItemFormat::INFO, false, "1494603973-2977-7_0.info"),
        (ItemFormat::INFO, false, "1494603967-2977-2_1.info"),
        (ItemFormat::INFO, false, "1494603967-2977-3_1.info"),
        (ItemFormat::INFO, false, "1494603967-2977-4_1.info"),
        (ItemFormat::INFO, false, "1494603968-2977-5_1.info"),
        (ItemFormat::INFO, false, "1494603972-2977-6_1.info"),
        (ItemFormat::INFO, false, "1494603973-2977-7_1.info"),
    ];

    check_produced(&queue, expected);
}

fn run_gcov(gcda_path: &PathBuf, working_dir: &PathBuf) {
    let mut command = Command::new("gcov");
    let status = command.arg(gcda_path)
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
        assert!(status.success(), "gcov wasn't successfully executed");
    //}
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

fn parse_lcov<T: Read>(lcov_reader: BufReader<T>) -> Vec<(String,CovResult)> {
    let mut cur_file = String::new();
    let mut cur_lines: BTreeMap<u32,u64> = BTreeMap::new();
    let mut cur_functions: HashMap<String,Function> = HashMap::new();

    let mut results = Vec::new();

    for line in lcov_reader.lines() {
        let l = line.unwrap();

        if l == "end_of_record" {
            results.push((cur_file, CovResult {
                lines: cur_lines,
                functions: cur_functions,
            }));

            cur_file = String::new();
            cur_lines = BTreeMap::new();
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
                        cur_lines.insert(line_no, 0);
                    } else {
                        cur_lines.insert(line_no, execution_count.parse().unwrap());
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
    let f = File::open("./test/prova.info").expect("Failed to open lcov file");
    let file = BufReader::new(&f);
    let results = parse_lcov(file);

    assert_eq!(results.len(), 603);

    let ref result = results[0];
    assert_eq!(result.0, "resource://gre/components/MainProcessSingleton.js");
    assert_eq!(result.1.lines, [(7,1),(9,1),(10,1),(12,1),(13,1),(16,1),(17,1),(18,1),(19,1),(21,1),(22,0),(23,0),(24,0),(28,1),(29,0),(30,0),(32,0),(33,0),(34,0),(35,0),(37,0),(39,0),(41,0),(42,0),(44,0),(45,0),(46,0),(47,0),(49,0),(50,0),(51,0),(52,0),(53,0),(54,0),(55,0),(56,0),(59,0),(60,0),(61,0),(63,0),(65,0),(67,1),(68,2),(70,1),(74,1),(75,1),(76,1),(77,1),(78,1),(83,1),(84,1),(90,1)].iter().cloned().collect());
    assert!(result.1.functions.contains_key("MainProcessSingleton"));
    let func = result.1.functions.get("MainProcessSingleton").unwrap();
    assert_eq!(func.start, 15);
    assert_eq!(func.executed, true);
    assert!(result.1.functions.contains_key("logConsoleMessage"));
    let func = result.1.functions.get("logConsoleMessage").unwrap();
    assert_eq!(func.start, 21);
    assert_eq!(func.executed, false);
}

fn parse_old_gcov(gcov_path: &Path) -> (String,CovResult) {
    let mut lines = BTreeMap::new();
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
                lines.insert(line_no, 0);
            } else {
                lines.insert(line_no, cover.parse().unwrap());
            }
        }
    }

    (source_name, CovResult {
      lines: lines,
      functions: functions,
    })
}

fn parse_gcov(gcov_path: &Path) -> Vec<(String,CovResult)> {
    let mut cur_file = String::new();
    let mut cur_lines: BTreeMap<u32,u64> = BTreeMap::new();
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
                if !cur_file.is_empty() && !cur_lines.is_empty() {
                    // println!("{} {} {:?}", gcov_path.display(), cur_file, cur_lines);
                    results.push((cur_file, CovResult {
                        lines: cur_lines,
                        functions: cur_functions,
                    }));
                }

                cur_file = value.to_string();
                cur_lines = BTreeMap::new();
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
                    cur_lines.insert(line_no, 0);
                } else {
                    cur_lines.insert(line_no, execution_count.parse().unwrap());
                }
            },
            _ => {}
        }
    }

    if !cur_lines.is_empty() {
        results.push((cur_file, CovResult {
            lines: cur_lines,
            functions: cur_functions,
        }));
    }

    results
}

#[test]
fn test_parser() {
    let results = parse_gcov(Path::new("./test/prova.gcov"));

    assert_eq!(results.len(), 10);

    let ref result = results[0];
    assert_eq!(result.0, "/home/marco/Documenti/FD/mozilla-central/build-cov-gcc/dist/include/nsExpirationTracker.h");
    assert_eq!(result.1.lines, [(393,0),(397,0),(399,0),(401,0),(402,0),(403,0),(405,0)].iter().cloned().collect());
    assert!(result.1.functions.contains_key("_ZN19nsExpirationTrackerIN11nsIDocument16SelectorCacheKeyELj4EE25ExpirationTrackerObserver7ReleaseEv"));
    let mut func = result.1.functions.get("_ZN19nsExpirationTrackerIN11nsIDocument16SelectorCacheKeyELj4EE25ExpirationTrackerObserver7ReleaseEv").unwrap();
    assert_eq!(func.start, 393);
    assert_eq!(func.executed, false);

    let ref result = results[5];
    assert_eq!(result.0, "/home/marco/Documenti/FD/mozilla-central/accessible/atk/Platform.cpp");
    assert_eq!(result.1.lines, [(81,0),(83,0),(85,0),(87,0),(88,0),(90,0),(94,0),(96,0),(97,0),(98,0),(99,0),(100,0),(101,0),(103,0),(104,0),(108,0),(110,0),(111,0),(112,0),(115,0),(117,0),(118,0),(122,0),(123,0),(124,0),(128,0),(129,0),(130,0),(136,17),(138,17),(141,0),(142,0),(146,0),(147,0),(148,0),(151,0),(152,0),(153,0),(154,0),(155,0),(156,0),(157,0),(161,0),(162,0),(165,0),(166,0),(167,0),(168,0),(169,0),(170,0),(171,0),(172,0),(184,0),(187,0),(189,0),(190,0),(194,0),(195,0),(196,0),(200,0),(201,0),(202,0),(203,0),(207,0),(208,0),(216,17),(218,17),(219,0),(220,0),(221,0),(222,0),(223,0),(226,17),(232,0),(233,0),(234,0),(253,17),(261,11390),(265,11390),(268,373),(274,373),(277,373),(278,373),(281,373),(288,373),(289,373),(293,373),(294,373),(295,373),(298,373),(303,5794),(306,5794),(307,5558),(309,236),(311,236),(312,236),(313,0),(316,236),(317,236),(318,0),(321,236),(322,236),(323,236),(324,236),(327,236),(328,236),(329,236),(330,236),(331,472),(332,472),(333,236),(338,236),(339,236),(340,236),(343,0),(344,0),(345,0),(346,0),(347,0),(352,236),(353,236),(354,236),(355,236),(361,236),(362,236),(364,236),(365,236),(370,0),(372,0),(373,0),(374,0),(376,0)].iter().cloned().collect());
    assert!(result.1.functions.contains_key("_ZL13LoadGtkModuleR24GnomeAccessibilityModule"));
    func = result.1.functions.get("_ZL13LoadGtkModuleR24GnomeAccessibilityModule").unwrap();
    assert_eq!(func.start, 81);
    assert_eq!(func.executed, false);
    assert!(result.1.functions.contains_key("_ZN7mozilla4a11y12PlatformInitEv"));
    func = result.1.functions.get("_ZN7mozilla4a11y12PlatformInitEv").unwrap();
    assert_eq!(func.start, 136);
    assert_eq!(func.executed, true);
    assert!(result.1.functions.contains_key("_ZN7mozilla4a11y16PlatformShutdownEv"));
    func = result.1.functions.get("_ZN7mozilla4a11y16PlatformShutdownEv").unwrap();
    assert_eq!(func.start, 216);
    assert_eq!(func.executed, true);
    assert!(result.1.functions.contains_key("_ZN7mozilla4a11y7PreInitEv"));
    func = result.1.functions.get("_ZN7mozilla4a11y7PreInitEv").unwrap();
    assert_eq!(func.start, 261);
    assert_eq!(func.executed, true);
    assert!(result.1.functions.contains_key("_ZN7mozilla4a11y19ShouldA11yBeEnabledEv"));
    func = result.1.functions.get("_ZN7mozilla4a11y19ShouldA11yBeEnabledEv").unwrap();
    assert_eq!(func.start, 303);
    assert_eq!(func.executed, true);

    let results = parse_gcov(Path::new("./test/negative_counts.gcov"));
    assert_eq!(results.len(), 118);
    let ref result = results[14];
    assert_eq!(result.0, "/home/marco/Documenti/FD/mozilla-central/build-cov-gcc/dist/include/mozilla/Assertions.h");
    assert_eq!(result.1.lines, [(40,0)].iter().cloned().collect());

    let results = parse_gcov(Path::new("./test/64bit_count.gcov"));
    assert_eq!(results.len(), 46);
    let ref result = results[8];
    assert_eq!(result.0, "/home/marco/Documenti/FD/mozilla-central/build-cov-gcc/dist/include/js/HashTable.h");
    assert_eq!(result.1.lines, [(324,8096),(343,12174),(344,6085),(345,23331),(357,10720),(361,313165934),(399,272539208),(402,31491125),(403,35509735),(420,434104),(709,313172766),(715,272542535),(801,584943263),(822,0),(825,0),(826,0),(828,0),(829,0),(831,0),(834,2210404897),(835,196249666),(838,3764974),(840,516370744),(841,1541684),(842,2253988941),(843,197245483),(844,0),(845,5306658),(846,821426720),(847,47096565),(853,82598134),(854,247796865),(886,272542256),(887,272542256),(904,599154437),(908,584933028),(913,584943263),(916,543534922),(917,584933028),(940,508959481),(945,1084660344),(960,545084512),(989,534593),(990,128435),(1019,427973453),(1029,504065334),(1038,1910289238),(1065,425402),(1075,10613316),(1076,5306658),(1090,392499332),(1112,48208),(1113,48208),(1114,0),(1115,0),(1118,48211),(1119,8009),(1120,48211),(1197,40347),(1202,585715301),(1207,1171430602),(1210,585715301),(1211,910968),(1212,585715301),(1222,30644),(1223,70165),(1225,1647),(1237,4048),(1238,4048),(1240,8096),(1244,6087),(1250,6087),(1257,6085),(1264,6085),(1278,6085),(1279,6085),(1280,0),(1283,6085),(1284,66935),(1285,30425),(1286,30425),(1289,6085),(1293,12171),(1294,6086),(1297,6087),(1299,6087),(1309,4048),(1310,4048),(1316,632104110),(1327,251893735),(1329,251893735),(1330,251893735),(1331,503787470),(1337,528619265),(1344,35325952),(1345,35325952),(1353,26236),(1354,13118),(1364,305520839),(1372,585099705),(1381,585099705),(1382,585099705),(1385,585099705),(1391,1135737600),(1397,242807686),(1400,242807686),(1403,1032741488),(1404,1290630),(1405,1042115),(1407,515080114),(1408,184996962),(1412,516370744),(1414,516370744),(1415,516370744),(1417,154330912),(1420,812664176),(1433,47004405),(1442,47004405),(1443,47004405),(1446,94008810),(1452,9086049),(1456,24497042),(1459,12248521),(1461,12248521),(1462,24497042),(1471,30642),(1474,30642),(1475,30642),(1476,30642),(1477,30642),(1478,30642),(1484,64904),(1485,34260),(1489,34260),(1490,34260),(1491,34260),(1492,34260),(1495,34260),(1496,69792911),(1497,139524496),(1498,94193130),(1499,47096565),(1500,47096565),(1506,61326),(1507,30663),(1513,58000),(1516,35325952),(1518,35325952),(1522,29000),(1527,29000),(1530,29000),(1534,0),(1536,0),(1537,0),(1538,0),(1540,0),(1547,10613316),(1548,1541684),(1549,1541684),(1552,3764974),(1554,5306658),(1571,8009),(1573,8009),(1574,8009),(1575,31345),(1576,5109),(1577,5109),(1580,8009),(1581,1647),(1582,8009),(1589,0),(1592,0),(1593,0),(1594,0),(1596,0),(1597,0),(1599,0),(1600,0),(1601,0),(1604,0),(1605,0),(1606,0),(1607,0),(1609,0),(1610,0),(1611,0),(1615,0),(1616,0),(1625,0),(1693,655507),(1711,35615006),(1730,10720),(1732,10720),(1733,10720),(1735,10720),(1736,10720),(1739,313162046),(1741,313162046),(1743,313162046),(1744,313162046),(1747,272542535),(1749,272542535),(1750,272542535),(1752,272542535),(1753,272542535),(1754,272542256),(1755,272542256),(1759,35509724),(1761,35509724),(1767,71019448),(1772,35505028),(1773,179105),(1776,179105),(1777,179105),(1780,35325923),(1781,35326057),(1785,35326058),(1786,29011),(1789,71010332),(1790,35505166),(1796,35505166)].iter().cloned().collect());

    // Assert more stuff.
}

// Merge results, without caring about duplicate lines (they will be removed at the end).
fn merge_results(result: &mut CovResult, result2: &mut CovResult) {
    for (&line_no, &execution_count) in &result2.lines {
        match result.lines.entry(line_no) {
            btree_map::Entry::Occupied(c) => {
                *c.into_mut() += execution_count;
            },
            btree_map::Entry::Vacant(v) => {
                v.insert(execution_count);
            }
        };
    }

    for (name, function) in result2.functions.drain() {
        match result.functions.entry(name) {
            hash_map::Entry::Occupied(f) => f.into_mut().executed |= function.executed,
            hash_map::Entry::Vacant(v) => {
                v.insert(function);
            }
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
        lines: [(1, 21),(2, 7),(7,0)].iter().cloned().collect(),
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
        lines: [(1,21),(3,42),(4,7),(2,0),(8,0)].iter().cloned().collect(),
        functions: functions2,
    };

    merge_results(&mut result, &mut result2);
    assert_eq!(result.lines, [(1,42),(2,7),(3,42),(4,7),(7,0),(8,0)].iter().cloned().collect());
    assert!(result.functions.contains_key("f1"));
    assert!(result.functions.contains_key("f2"));
    let mut func = result.functions.get("f1").unwrap();
    assert_eq!(func.start, 1);
    assert_eq!(func.executed, false);
    func = result.functions.get("f2").unwrap();
    assert_eq!(func.start, 2);
    assert_eq!(func.executed, true);
}

fn add_results(mut results: Vec<(String,CovResult)>, result_map: &SyncCovResultMap) {
    let mut map = result_map.lock().unwrap();
    for mut result in results.drain(..) {
        match map.entry(result.0) {
            hash_map::Entry::Occupied(obj) => {
                merge_results(obj.into_mut(), &mut result.1);
            },
            hash_map::Entry::Vacant(v) => {
                v.insert(result.1);
            }
        };
    }
}

fn rewrite_paths(result_map: CovResultMap, source_dir: &str, prefix_dir: &str, ignore_global: bool, ignore_not_existing: bool, to_ignore_dir: Option<String>) -> CovResultIter {
    let source_dir = if source_dir != "" {
        fs::canonicalize(&source_dir).expect("Source directory does not exist.")
    } else {
        PathBuf::from("")
    };

    let prefix_dir = prefix_dir.to_owned();

    Box::new(result_map.into_iter().filter_map(move |(path, result)| {
        let path = PathBuf::from(path);

        // Remove prefix from path.
        let rel_path = if path.starts_with(&prefix_dir) {
            path.strip_prefix(&prefix_dir).unwrap().to_path_buf()
        } else {
            path
        };

        if ignore_global && !rel_path.is_relative() {
            return None;
        }

        // Get absolute path to source file.
        let abs_path = if rel_path.is_relative() {
            PathBuf::from(&source_dir).join(&rel_path)
        } else {
            rel_path
        };

        // Canonicalize, if possible.
        let abs_path = match fs::canonicalize(&abs_path) {
            Ok(p) => p,
            Err(_) => abs_path,
        };

        // Remove source dir from path.
        let rel_path = if abs_path.starts_with(&source_dir) {
            abs_path.strip_prefix(&source_dir).unwrap().to_path_buf()
        } else {
            abs_path.clone()
        };

        if to_ignore_dir.is_some() && rel_path.starts_with(to_ignore_dir.as_ref().unwrap()) {
            return None;
        }

        if ignore_not_existing && !abs_path.exists() {
            return None;
        }

        Some((abs_path, rel_path, result))
    }))
}

#[test]
fn test_rewrite_paths() {
    let empty_result = CovResult {
        lines: BTreeMap::new(),
        functions: HashMap::new(),
    };

    // Basic test.
    let mut result_map: CovResultMap = HashMap::new();
    result_map.insert("main.cpp".to_string(), empty_result.clone());
    let results = rewrite_paths(result_map, "", "", false, false, None);
    let mut count = 0;
    for (abs_path, rel_path, result) in results {
       count += 1;
       assert_eq!(abs_path, PathBuf::from("main.cpp"));
       assert_eq!(rel_path, PathBuf::from("main.cpp"));
       assert_eq!(result, empty_result);
    }
    assert_eq!(count, 1);

    // Ignore global files.
    let mut result_map: CovResultMap = HashMap::new();
    result_map.insert("main.cpp".to_string(), empty_result.clone());
    result_map.insert("/usr/include/prova.h".to_string(), empty_result.clone());
    let results = rewrite_paths(result_map, "", "", true, false, None);
    let mut count = 0;
    for (abs_path, rel_path, result) in results {
       count += 1;
       assert_eq!(abs_path, PathBuf::from("main.cpp"));
       assert_eq!(rel_path, PathBuf::from("main.cpp"));
       assert_eq!(result, empty_result);
    }
    assert_eq!(count, 1);

    // Remove prefix.
    let mut result_map: CovResultMap = HashMap::new();
    result_map.insert("/home/worker/src/workspace/main.cpp".to_string(), empty_result.clone());
    let results = rewrite_paths(result_map, "", "/home/worker/src/workspace/", false, false, None);
    let mut count = 0;
    for (abs_path, rel_path, result) in results {
       count += 1;
       assert_eq!(abs_path, PathBuf::from("main.cpp"));
       assert_eq!(rel_path, PathBuf::from("main.cpp"));
       assert_eq!(result, empty_result);
    }
    assert_eq!(count, 1);

    // Ignore non existing files.
    let mut result_map: CovResultMap = HashMap::new();
    result_map.insert("tests/class/main.cpp".to_string(), empty_result.clone());
    result_map.insert("tests/class/doesntexist.cpp".to_string(), empty_result.clone());
    let results = rewrite_paths(result_map, "", "", false, true, None);
    let mut count = 0;
    for (abs_path, rel_path, result) in results {
       count += 1;
       assert!(abs_path.is_absolute());
       assert!(abs_path.ends_with("tests/class/main.cpp"));
       assert!(rel_path.ends_with("tests/class/main.cpp"));
       assert_eq!(result, empty_result);
    }
    assert_eq!(count, 1);

    // Ignore a directory.
    let mut result_map: CovResultMap = HashMap::new();
    result_map.insert("main.cpp".to_string(), empty_result.clone());
    result_map.insert("mydir/prova.h".to_string(), empty_result.clone());
    let results = rewrite_paths(result_map, "", "", false, false, Some("mydir".to_string()));
    let mut count = 0;
    for (abs_path, rel_path, result) in results {
       count += 1;
       assert_eq!(abs_path, PathBuf::from("main.cpp"));
       assert_eq!(rel_path, PathBuf::from("main.cpp"));
       assert_eq!(result, empty_result);
    }
    assert_eq!(count, 1);

    // Rewrite path using relative source directory.
    let mut result_map: CovResultMap = HashMap::new();
    result_map.insert("class/main.cpp".to_string(), empty_result.clone());
    let results = rewrite_paths(result_map, "tests", "", false, true, None);
    let mut count = 0;
    for (abs_path, rel_path, result) in results {
       count += 1;
       assert!(abs_path.is_absolute());
       assert!(abs_path.ends_with("tests/class/main.cpp"));
       assert_eq!(rel_path, PathBuf::from("class/main.cpp"));
       assert_eq!(result, empty_result);
    }
    assert_eq!(count, 1);

    // Rewrite path using absolute source directory.
    let mut result_map: CovResultMap = HashMap::new();
    result_map.insert("class/main.cpp".to_string(), empty_result.clone());
    let results = rewrite_paths(result_map, fs::canonicalize("tests").unwrap().to_str().unwrap(), "", false, true, None);
    let mut count = 0;
    for (abs_path, rel_path, result) in results {
       count += 1;
       assert!(abs_path.is_absolute());
       assert!(abs_path.ends_with("tests/class/main.cpp"));
       assert_eq!(rel_path, PathBuf::from("class/main.cpp"));
       assert_eq!(result, empty_result);
    }
    assert_eq!(count, 1);

    // Rewrite path and remove prefix.
    let mut result_map: CovResultMap = HashMap::new();
    result_map.insert("/home/worker/src/workspace/class/main.cpp".to_string(), empty_result.clone());
    let results = rewrite_paths(result_map, "tests", "/home/worker/src/workspace", false, true, None);
    let mut count = 0;
    for (abs_path, rel_path, result) in results {
       count += 1;
       assert!(abs_path.is_absolute());
       assert!(abs_path.ends_with("tests/class/main.cpp"));
       assert_eq!(rel_path, PathBuf::from("class/main.cpp"));
       assert_eq!(result, empty_result);
    }
    assert_eq!(count, 1);
}

fn to_activedata_etl_vec(normal_vec: &[u32]) -> Vec<Value> {
    normal_vec.iter().map(|&x| json!({"line": x})).collect()
}

fn output_activedata_etl(results: CovResultIter) {
    for (_, rel_path, result) in results {
        let covered: Vec<u32> = result.lines.iter().filter(|&(_,v)| *v > 0).map(|(k,_)| k).cloned().collect();
        let uncovered: Vec<u32> = result.lines.iter().filter(|&(_,v)| *v == 0).map(|(k,_)| k).cloned().collect();

        let mut orphan_covered: BTreeSet<u32> = covered.iter().cloned().collect();
        let mut orphan_uncovered: BTreeSet<u32> = uncovered.iter().cloned().collect();

        let end: u32 = result.lines.keys().last().unwrap_or(&0) + 1;

        let mut start_indexes: Vec<u32> = Vec::new();
        for function in result.functions.values() {
            start_indexes.push(function.start);
        }
        start_indexes.sort();

        for (name, function) in &result.functions {
            // println!("{} {} {}", name, function.executed, function.start);

            let mut func_end = end;

            for start in &start_indexes {
                if *start > function.start {
                    func_end = *start;
                    break;
                }
            }

            let mut lines_covered: Vec<Value> = Vec::new();
            for line in covered.iter().filter(|&&x| x >= function.start && x < func_end) {
                lines_covered.push(json!({
                    "line": *line
                }));
                orphan_covered.remove(line);
            }

            let mut lines_uncovered: Vec<u32> = Vec::new();
            for line in uncovered.iter().filter(|&&x| x >= function.start && x < func_end) {
                lines_uncovered.push(*line);
                orphan_uncovered.remove(line);
            }

            println!("{}", json!({
                "language": "c/c++",
                "file": {
                    "name": rel_path,
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

        let orphan_covered: Vec<u32> = orphan_covered.into_iter().collect();
        let orphan_uncovered: Vec<u32> = orphan_uncovered.into_iter().collect();

        // The orphan lines will represent the file as a whole.
        println!("{}", json!({
            "language": "c/c++",
            "is_file": true,
            "file": {
                "name": rel_path,
                "covered": to_activedata_etl_vec(&covered),
                "uncovered": uncovered,
                "total_covered": covered.len(),
                "total_uncovered": uncovered.len(),
                "percentage_covered": covered.len() as f32 / (covered.len() + uncovered.len()) as f32,
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

fn output_lcov(results: CovResultIter) {
    let stdout = io::stdout();
    let mut writer = BufWriter::new(stdout.lock());

    writer.write_all(b"TN:\n").unwrap();

    for (_, rel_path, result) in results {
        // println!("{} {:?}", rel_path, result.lines);

        write!(writer, "SF:{}\n", rel_path.display()).unwrap();

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

        for (line, execution_count) in &result.lines {
            write!(writer, "DA:{},{}\n", line, execution_count).unwrap();
        }
        write!(writer, "LF:{}\n", result.lines.len()).unwrap();
        write!(writer, "LH:{}\n", result.lines.values().filter(|&v| *v > 0).count()).unwrap();
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

fn output_coveralls(results: CovResultIter, repo_token: &str, service_name: &str, service_number: &str, service_job_number: &str, commit_sha: &str) {
    let mut source_files = Vec::new();

    for (abs_path, rel_path, result) in results {
        let end: u32 = result.lines.keys().last().unwrap_or(&0) + 1;

        let mut coverage = Vec::new();
        for line in 1..end {
            let entry = result.lines.get(&line);
            if let Some(c) = entry {
                coverage.push(Value::from(*c));
            } else {
                coverage.push(Value::Null);
            }
        }

        source_files.push(json!({
            "name": rel_path,
            "source_digest": get_digest(abs_path),
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
    let mut directories = Vec::new();
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
            directories.push(args[i].clone());
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
        Some(to_ignore_dir.to_owned())
    };

    let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
    let tmp_path = tmp_dir.path().to_owned();

    let result_map: Arc<SyncCovResultMap> = Arc::new(Mutex::new(HashMap::with_capacity(20000)));
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
        let result_map = result_map.clone();
        let working_dir = tmp_path.join(format!("{}", i));

        let t = thread::spawn(move || {
            fs::create_dir(&working_dir).expect("Failed to create working directory");

            while let Some(work_item) = queue.pop() {
                let new_results = match work_item.format {
                    ItemFormat::GCDA => {
                        let gcda_path = work_item.path();

                        if !is_llvm {
                            let gcov_path = working_dir.join(gcda_path.file_name().unwrap().to_str().unwrap().to_string() + ".gcov");

                            /*if cfg!(unix) {
                                mkfifo(&gcov_path);
                            }*/
                            run_gcov(gcda_path, &working_dir);

                            let new_results = parse_gcov(&gcov_path);
                            fs::remove_file(gcov_path).unwrap();

                            new_results
                        } else {
                            run_llvm_gcov(gcda_path, &working_dir);

                            let mut new_results: Vec<(String,CovResult)> = Vec::new();

                            for entry in WalkDir::new(&working_dir).min_depth(1) {
                                let gcov_path = entry.unwrap();
                                let gcov_path = gcov_path.path();

                                new_results.push(parse_old_gcov(gcov_path));
                                fs::remove_file(gcov_path).unwrap();
                            }

                            new_results
                        }
                    },
                    ItemFormat::INFO => {
                        match work_item.item {
                            ItemType::Path(info_path) => {
                                let f = File::open(&info_path).expect("Failed to open lcov file");
                                let file = BufReader::new(&f);
                                parse_lcov(file)
                            },
                            ItemType::Content(info_content) => {
                                let buffer = BufReader::new(Cursor::new(info_content));
                                parse_lcov(buffer)
                            }
                        }
                    }
                };

                add_results(new_results, &result_map);
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

    let result_map_mutex = Arc::try_unwrap(result_map).unwrap();
    let result_map = result_map_mutex.into_inner().unwrap();

    let iterator = rewrite_paths(result_map, source_dir, prefix_dir, ignore_global, ignore_not_existing, to_ignore_dir);

    if output_type == "ade" {
        output_activedata_etl(iterator);
    } else if output_type == "lcov" {
        output_lcov(iterator);
    } else if output_type == "coveralls" {
        output_coveralls(iterator, repo_token, service_name, service_number, service_job_number, commit_sha);
    }
}
