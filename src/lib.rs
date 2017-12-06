#![cfg_attr(feature="alloc_system",feature(alloc_system))]
#[cfg(feature="alloc_system")]
extern crate alloc_system;
#[macro_use]
extern crate serde_json;
extern crate crossbeam;
extern crate walkdir;
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
use std::fs::{self, File};
use std::io::{self, Read, Write, BufWriter};
use std::process::{Command, Stdio};
use std::sync::Arc;
use zip::ZipArchive;
use crossbeam::sync::MsQueue;
use walkdir::WalkDir;
use serde_json::Value;
use semver::Version;
use crypto::md5::Md5;
use crypto::digest::Digest;
use tempdir::TempDir;
use uuid::Uuid;
use std::ffi::CString;

mod defs;
pub use defs::*;

mod parser;
pub use parser::*;

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

#[link(name = "llvmgcov", kind="static")]
extern {
    fn parse_llvm_gcno(working_dir: *const libc::c_char, file_stem: *const libc::c_char);
}

pub fn call_parse_llvm_gcno(working_dir: &str, file_stem: &str) {
    let working_dir_c = CString::new(working_dir).unwrap();
    let file_stem_c = CString::new(file_stem).unwrap();
    unsafe {
        parse_llvm_gcno(working_dir_c.as_ptr(), file_stem_c.as_ptr());
    };
}

fn dir_producer(directories: &[&String], queue: &WorkQueue) -> Option<Vec<u8>> {
    let gcno_ext = Some(OsStr::new("gcno"));
    let info_ext = Some(OsStr::new("info"));
    let json_ext = Some(OsStr::new("json"));

    let mut path_mapping_file = None;

    for directory in directories {
        let is_dir_relative = PathBuf::from(directory).is_relative();
        let current_dir = env::current_dir().unwrap();

        for entry in WalkDir::new(&directory) {
            let entry = entry.expect(format!("Failed to open directory '{}'.", directory).as_str());
            let path = entry.path();
            if path.is_file() {
                let ext = path.extension();
                let format = if ext == gcno_ext {
                    ItemFormat::GCNO
                } else if ext == info_ext {
                    ItemFormat::INFO
                } else if ext == json_ext && path.file_name().unwrap() == "linked-files-map.json" {
                    let mut buffer = Vec::new();
                    File::open(path).unwrap().read_to_end(&mut buffer).unwrap();
                    path_mapping_file = Some(buffer);
                    continue
                } else {
                    continue
                };

                let abs_path = if is_dir_relative {
                    current_dir.join(path)
                } else {
                    path.to_path_buf()
                };

                queue.push(Some(WorkItem {
                    format: format,
                    item: ItemType::Path(abs_path),
                }));
            }
        }
    }

    path_mapping_file
}

#[cfg(test)]
fn check_produced(directory: PathBuf, queue: &WorkQueue, expected: Vec<(ItemFormat,bool,&str,bool)>) {
    let mut vec: Vec<Option<WorkItem>> = Vec::new();

    loop {
        let elem = queue.try_pop();
        if elem.is_none() {
            break;
        }
        vec.push(elem.unwrap());
    }

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

    for v in &vec {
        let v = v.as_ref().unwrap();
        assert!(expected.iter().any(|x| {
            if v.format != x.0 {
                return false;
            }

            match v.item {
                ItemType::Content(_) => {
                    !x.1
                },
                ItemType::Path(ref p) => {
                    x.1 && p.ends_with(x.2)
                }
            }
        }), "Unexpected {:?}", v);
    }

    // Make sure we haven't generated duplicated entries.
    assert_eq!(vec.len(), expected.len());

    // Assert file exists and file with the same name but with extension .gcda exists.
    for x in expected.iter() {
        if !x.1 {
            continue;
        }

        let p = directory.join(x.2);
        assert!(p.exists(), "{} doesn't exist", p.display());
        if x.0 == ItemFormat::GCNO {
            let gcda = p.with_file_name(format!("{}.gcda", p.file_stem().unwrap().to_str().unwrap()));
            if x.3 {
                assert!(gcda.exists(), "{} doesn't exist", gcda.display());
            } else {
                assert!(!gcda.exists(), "{} exists", gcda.display());
            }
        }
    }
}

#[test]
fn test_dir_producer() {
    let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

    let mapping = dir_producer(&vec![&"test".to_string()], &queue);

    let expected = vec![
        (ItemFormat::GCNO, true, "test/Platform.gcno", true),
        (ItemFormat::GCNO, true, "test/sub2/RootAccessibleWrap.gcno", true),
        (ItemFormat::GCNO, true, "test/nsMaiInterfaceValue.gcno", true),
        (ItemFormat::GCNO, true, "test/sub/prova2.gcno", true),
        (ItemFormat::GCNO, true, "test/nsMaiInterfaceDocument.gcno", true),
        (ItemFormat::GCNO, true, "test/Unified_cpp_netwerk_base0.gcno", true),
        (ItemFormat::GCNO, true, "test/prova.gcno", true),
        (ItemFormat::GCNO, true, "test/nsGnomeModule.gcno", true),
        (ItemFormat::GCNO, true, "test/negative_counts.gcno", true),
        (ItemFormat::GCNO, true, "test/64bit_count.gcno", true),
        (ItemFormat::GCNO, true, "test/no_gcda/main.gcno", false),
        (ItemFormat::GCNO, true, "test/gcno_symlink/gcda/main.gcno", true),
        (ItemFormat::GCNO, true, "test/gcno_symlink/gcno/main.gcno", false),
        (ItemFormat::INFO, true, "test/1494603973-2977-7.info", true),
        (ItemFormat::INFO, true, "test/prova.info", true),
        (ItemFormat::INFO, true, "test/prova_fn_with_commas.info", true),
    ];

    check_produced(PathBuf::from("."), &queue, expected);
    assert!(mapping.is_some());
    let mapping: Value = serde_json::from_slice(&mapping.unwrap()).unwrap();
    assert_eq!(mapping.get("dist/include/zlib.h").unwrap().as_str().unwrap(), "modules/zlib/src/zlib.h");
}

#[test]
fn test_dir_producer_multiple_directories() {
    let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

    let mapping = dir_producer(&vec![&"test/sub".to_string(), &"test/sub2".to_string()], &queue);

    let expected = vec![
        (ItemFormat::GCNO, true, "test/sub2/RootAccessibleWrap.gcno", true),
        (ItemFormat::GCNO, true, "test/sub/prova2.gcno", true),
    ];

    check_produced(PathBuf::from("."), &queue, expected);
    assert!(mapping.is_none());
}

#[test]
fn test_dir_producer_directory_with_gcno_symlinks() {
    let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

    let mapping = dir_producer(&vec![&"test/gcno_symlink/gcda".to_string()], &queue);

    let expected = vec![
        (ItemFormat::GCNO, true, "test/gcno_symlink/gcda/main.gcno", true),
    ];

    check_produced(PathBuf::from("."), &queue, expected);
    assert!(mapping.is_none());
}

fn open_archive(path: &str) -> ZipArchive<File> {
    let file = File::open(&path).expect(format!("Failed to open ZIP file '{}'.", path).as_str());
    ZipArchive::new(file).expect(format!("Failed to parse ZIP file: {}", path).as_str())
}

fn extract_file(zip_file: &mut zip::read::ZipFile, path: &PathBuf) {
    let mut file = File::create(&path).expect("Failed to create file");
    io::copy(zip_file, &mut file).expect("Failed to copy file from ZIP");
}

fn zip_producer(tmp_dir: &Path, zip_files: &[&String], queue: &WorkQueue) -> Option<Vec<u8>> {
    let mut gcno_archive: Option<ZipArchive<File>> = None;
    let mut gcda_archives: Vec<ZipArchive<File>> = Vec::new();
    let mut info_archives: Vec<ZipArchive<File>> = Vec::new();

    let mut path_mapping_file = None;

    for zip_file in zip_files.iter() {
        let archive = open_archive(zip_file);
        if zip_file.contains("gcno") {
            gcno_archive = Some(archive);
        } else if zip_file.contains("gcda") {
            gcda_archives.push(archive);
        } else if zip_file.contains("info") || zip_file.contains("grcov") {
            info_archives.push(archive);
        } else {
            panic!("Unsupported archive type.");
        }
    }

    if gcno_archive.is_some() {
        assert!(!gcda_archives.is_empty());
    }
    if !gcda_archives.is_empty() {
        assert!(gcno_archive.is_some());
    }

    if let Some(mut gcno_archive) = gcno_archive {
        for i in 0..gcno_archive.len() {
            let mut gcno_file = gcno_archive.by_index(i).unwrap();
            if gcno_file.name() == "linked-files-map.json" {
                let mut buffer = Vec::new();
                gcno_file.read_to_end(&mut buffer).unwrap();
                path_mapping_file = Some(buffer);
                continue;
            }

            let gcno_path_in_zip = PathBuf::from(gcno_file.name());

            let path = tmp_dir.join(&gcno_path_in_zip);

            fs::create_dir_all(path.parent().unwrap()).expect("Failed to create directory");

            if gcno_file.name().ends_with('/') {
                fs::create_dir_all(&path).expect("Failed to create directory");
            }
            else {
                let stem = path.file_stem().unwrap().to_str().unwrap();

                let physical_gcno_path = path.with_file_name(format!("{}_{}.gcno", stem, 1));
                extract_file(&mut gcno_file, &physical_gcno_path);

                let gcda_path_in_zip = gcno_path_in_zip.with_extension("gcda");

                for (num, gcda_archive) in gcda_archives.iter_mut().enumerate() {
                    let gcno_path = path.with_file_name(format!("{}_{}.gcno", stem, num + 1));

                    // Create symlinks.
                    if num != 0 {
                        fs::hard_link(&physical_gcno_path, &gcno_path).expect(format!("Failed to create hardlink {}", gcno_path.display()).as_str());
                    }

                    if let Ok(mut gcda_file) = gcda_archive.by_name(&gcda_path_in_zip.to_str().unwrap().replace("\\", "/")) {
                        let gcda_path = path.with_file_name(format!("{}_{}.gcda", stem, num + 1));

                        extract_file(&mut gcda_file, &gcda_path);
                    }

                    queue.push(Some(WorkItem {
                        format: ItemFormat::GCNO,
                        item: ItemType::Path(gcno_path),
                    }));
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

    path_mapping_file
}

// Test extracting multiple gcda archives.
#[test]
fn test_zip_producer_multiple_gcda_archives() {
    let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

    let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
    let tmp_path = tmp_dir.path().to_owned();
    let mapping = zip_producer(&tmp_path, &vec![&"test/gcno.zip".to_string(), &"test/gcda1.zip".to_string(), &"test/gcda2.zip".to_string()], &queue);

    let expected = vec![
        (ItemFormat::GCNO, true, "Platform_1.gcno", true),
        (ItemFormat::GCNO, true, "Platform_2.gcno", false),
        (ItemFormat::GCNO, true, "sub2/RootAccessibleWrap_1.gcno", true),
        (ItemFormat::GCNO, true, "sub2/RootAccessibleWrap_2.gcno", false),
        (ItemFormat::GCNO, true, "nsMaiInterfaceValue_1.gcno", true),
        (ItemFormat::GCNO, true, "sub/prova2_1.gcno", true),
        (ItemFormat::GCNO, true, "nsMaiInterfaceDocument_1.gcno", true),
        (ItemFormat::GCNO, true, "nsGnomeModule_1.gcno", true),
        (ItemFormat::GCNO, true, "nsMaiInterfaceValue_2.gcno", true),
        (ItemFormat::GCNO, true, "nsMaiInterfaceDocument_2.gcno", true),
        (ItemFormat::GCNO, true, "nsGnomeModule_2.gcno", true),
        (ItemFormat::GCNO, true, "sub/prova2_2.gcno", true),
    ];

    check_produced(tmp_path, &queue, expected);
    assert!(mapping.is_some());
    let mapping: Value = serde_json::from_slice(&mapping.unwrap()).unwrap();
    assert_eq!(mapping.get("dist/include/zlib.h").unwrap().as_str().unwrap(), "modules/zlib/src/zlib.h");
}

// Test extracting gcno with no path mapping.
#[test]
fn test_zip_producer_gcno_with_no_path_mapping() {
    let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

    let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
    let tmp_path = tmp_dir.path().to_owned();
    let mapping = zip_producer(&tmp_path, &vec![&"test/gcno_no_path_mapping.zip".to_string(), &"test/gcda1.zip".to_string()], &queue);

    let expected = vec![
        (ItemFormat::GCNO, true, "Platform_1.gcno", true),
        (ItemFormat::GCNO, true, "sub2/RootAccessibleWrap_1.gcno", true),
        (ItemFormat::GCNO, true, "nsMaiInterfaceValue_1.gcno", true),
        (ItemFormat::GCNO, true, "sub/prova2_1.gcno", true),
        (ItemFormat::GCNO, true, "nsMaiInterfaceDocument_1.gcno", true),
        (ItemFormat::GCNO, true, "nsGnomeModule_1.gcno", true),
    ];

    check_produced(tmp_path, &queue, expected);
    assert!(mapping.is_none());
}

// Test calling zip_producer with a different order of zip files.
#[test]
fn test_zip_producer_different_order_of_zip_files() {
    let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

    let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
    let tmp_path = tmp_dir.path().to_owned();
    zip_producer(&tmp_path, &vec![&"test/gcda1.zip".to_string(), &"test/gcno.zip".to_string(), &"test/gcda2.zip".to_string()], &queue);

    let expected = vec![
        (ItemFormat::GCNO, true, "Platform_1.gcno", true),
        (ItemFormat::GCNO, true, "Platform_2.gcno", false),
        (ItemFormat::GCNO, true, "sub2/RootAccessibleWrap_1.gcno", true),
        (ItemFormat::GCNO, true, "sub2/RootAccessibleWrap_2.gcno", false),
        (ItemFormat::GCNO, true, "nsMaiInterfaceValue_1.gcno", true),
        (ItemFormat::GCNO, true, "sub/prova2_1.gcno", true),
        (ItemFormat::GCNO, true, "nsMaiInterfaceDocument_1.gcno", true),
        (ItemFormat::GCNO, true, "nsGnomeModule_1.gcno", true),
        (ItemFormat::GCNO, true, "nsMaiInterfaceValue_2.gcno", true),
        (ItemFormat::GCNO, true, "nsMaiInterfaceDocument_2.gcno", true),
        (ItemFormat::GCNO, true, "nsGnomeModule_2.gcno", true),
        (ItemFormat::GCNO, true, "sub/prova2_2.gcno", true),
    ];

    check_produced(tmp_path, &queue, expected);
}

// Test extracting info files.
#[test]
fn test_zip_producer_info_files() {
    let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

    let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
    let tmp_path = tmp_dir.path().to_owned();
    zip_producer(&tmp_path, &vec![&"test/info1.zip".to_string(), &"test/info2.zip".to_string()], &queue);

    let expected = vec![
        (ItemFormat::INFO, false, "1494603967-2977-2_0.info", true),
        (ItemFormat::INFO, false, "1494603967-2977-3_0.info", true),
        (ItemFormat::INFO, false, "1494603967-2977-4_0.info", true),
        (ItemFormat::INFO, false, "1494603968-2977-5_0.info", true),
        (ItemFormat::INFO, false, "1494603972-2977-6_0.info", true),
        (ItemFormat::INFO, false, "1494603973-2977-7_0.info", true),
        (ItemFormat::INFO, false, "1494603967-2977-2_1.info", true),
        (ItemFormat::INFO, false, "1494603967-2977-3_1.info", true),
        (ItemFormat::INFO, false, "1494603967-2977-4_1.info", true),
        (ItemFormat::INFO, false, "1494603968-2977-5_1.info", true),
        (ItemFormat::INFO, false, "1494603972-2977-6_1.info", true),
        (ItemFormat::INFO, false, "1494603973-2977-7_1.info", true),
    ];

    check_produced(tmp_path, &queue, expected);
}

// Test extracting both info and gcno/gcda files.
#[test]
fn test_zip_producer_both_info_and_gcnogcda_files() {
    let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

    let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
    let tmp_path = tmp_dir.path().to_owned();
    zip_producer(&tmp_path, &vec![&"test/gcno.zip".to_string(), &"test/gcda1.zip".to_string(), &"test/info1.zip".to_string(), &"test/info2.zip".to_string()], &queue);

    let expected = vec![
        (ItemFormat::GCNO, true, "Platform_1.gcno", true),
        (ItemFormat::GCNO, true, "sub2/RootAccessibleWrap_1.gcno", true),
        (ItemFormat::GCNO, true, "nsMaiInterfaceValue_1.gcno", true),
        (ItemFormat::GCNO, true, "sub/prova2_1.gcno", true),
        (ItemFormat::GCNO, true, "nsMaiInterfaceDocument_1.gcno", true),
        (ItemFormat::GCNO, true, "nsGnomeModule_1.gcno", true),
        (ItemFormat::INFO, false, "1494603967-2977-2_0.info", true),
        (ItemFormat::INFO, false, "1494603967-2977-3_0.info", true),
        (ItemFormat::INFO, false, "1494603967-2977-4_0.info", true),
        (ItemFormat::INFO, false, "1494603968-2977-5_0.info", true),
        (ItemFormat::INFO, false, "1494603972-2977-6_0.info", true),
        (ItemFormat::INFO, false, "1494603973-2977-7_0.info", true),
        (ItemFormat::INFO, false, "1494603967-2977-2_1.info", true),
        (ItemFormat::INFO, false, "1494603967-2977-3_1.info", true),
        (ItemFormat::INFO, false, "1494603967-2977-4_1.info", true),
        (ItemFormat::INFO, false, "1494603968-2977-5_1.info", true),
        (ItemFormat::INFO, false, "1494603972-2977-6_1.info", true),
        (ItemFormat::INFO, false, "1494603973-2977-7_1.info", true),
    ];

    check_produced(tmp_path, &queue, expected);
}

// Test extracting gcno with no associated gcda.
#[test]
fn test_zip_producer_gcno_with_no_associated_gcda() {
    let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

    let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
    let tmp_path = tmp_dir.path().to_owned();
    let mapping = zip_producer(&tmp_path, &vec![&"test/no_gcda/main.gcno.zip".to_string(), &"test/no_gcda/empty.gcda.zip".to_string()], &queue);

    let expected = vec![
        (ItemFormat::GCNO, true, "main_1.gcno", false),
    ];

    check_produced(tmp_path, &queue, expected);
    assert!(mapping.is_none());
}

// Test extracting gcno with an associated gcda file in only one zip file.
#[test]
fn test_zip_producer_gcno_with_associated_gcda_in_only_one_archive() {
    let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

    let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
    let tmp_path = tmp_dir.path().to_owned();
    let mapping = zip_producer(&tmp_path, &vec![&"test/no_gcda/main.gcno.zip".to_string(), &"test/no_gcda/empty.gcda.zip".to_string(),  &"test/no_gcda/main.gcda.zip".to_string()], &queue);

    let expected = vec![
        (ItemFormat::GCNO, true, "main_1.gcno", false),
        (ItemFormat::GCNO, true, "main_2.gcno", true),
    ];

    check_produced(tmp_path, &queue, expected);
    assert!(mapping.is_none());
}

// Test passing a gcno archive with no gcda archive makes zip_producer fail.
#[test]
#[should_panic]
fn test_zip_producer_with_gcno_archive_and_no_gcda_archive() {
    let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

    let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
    let tmp_path = tmp_dir.path().to_owned();
    zip_producer(&tmp_path, &vec![&"test/no_gcda/main.gcno.zip".to_string()], &queue);
}

// Test passing a gcda archive with no gcno archive makes zip_producer fail.
#[test]
#[should_panic]
fn test_zip_producer_with_gcda_archive_and_no_gcno_archive() {
    let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

    let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
    let tmp_path = tmp_dir.path().to_owned();
    zip_producer(&tmp_path, &vec![&"test/no_gcda/main.gcda.zip".to_string()], &queue);
}

pub fn producer(tmp_dir: &Path, paths: &[String], queue: &WorkQueue) -> Option<Vec<u8>> {
    let mut zip_files = Vec::new();
    let mut directories = Vec::new();

    for path in paths {
        if path.ends_with(".zip") {
            zip_files.push(path);
        } else {
            directories.push(path);
        }
    }

    let ret1 = zip_producer(tmp_dir, &zip_files, queue);
    let ret2 = dir_producer(&directories, queue);

    if ret1.is_some() {
        ret1
    } else if ret2.is_some() {
        ret2
    } else {
        None
    }
}

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

// Merge results, without caring about duplicate lines (they will be removed at the end).
pub fn merge_results(result: &mut CovResult, result2: &mut CovResult) {
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

    for (&(line_no, number), &taken) in &result2.branches {
        match result.branches.entry((line_no, number)) {
            btree_map::Entry::Occupied(c) => {
                *c.into_mut() |= taken;
            },
            btree_map::Entry::Vacant(v) => {
                v.insert(taken);
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
        branches: [((1, 0), false), ((1, 1), false), ((2, 0), false), ((2, 1), true), ((4, 0), true)].iter().cloned().collect(),
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
        branches: [((1, 0), false), ((1, 1), false), ((2, 0), true), ((2, 1), false), ((3, 0), true)].iter().cloned().collect(),
        functions: functions2,
    };

    merge_results(&mut result, &mut result2);
    assert_eq!(result.lines, [(1,42),(2,7),(3,42),(4,7),(7,0),(8,0)].iter().cloned().collect());
    assert_eq!(result.branches, [((1, 0), false), ((1, 1), false), ((2, 0), true), ((2, 1), true), ((3, 0), true), ((4, 0), true)].iter().cloned().collect());
    assert!(result.functions.contains_key("f1"));
    assert!(result.functions.contains_key("f2"));
    let mut func = result.functions.get("f1").unwrap();
    assert_eq!(func.start, 1);
    assert_eq!(func.executed, false);
    func = result.functions.get("f2").unwrap();
    assert_eq!(func.start, 2);
    assert_eq!(func.executed, true);
}

pub fn add_results(mut results: Vec<(String,CovResult)>, result_map: &SyncCovResultMap) {
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

fn to_lowercase_first(s: &str) -> String {
    let mut c = s.chars();
    c.next().unwrap().to_lowercase().collect::<String>() + c.as_str()
}

#[test]
fn test_to_lowercase_first() {
  assert_eq!(to_lowercase_first("marco"), "marco");
  assert_eq!(to_lowercase_first("Marco"), "marco");
}

#[test]
#[should_panic]
fn test_to_lowercase_first_empty() {
    to_lowercase_first("");
}

fn to_uppercase_first(s: &str) -> String {
    let mut c = s.chars();
    c.next().unwrap().to_uppercase().collect::<String>() + c.as_str()
}

#[test]
fn test_to_uppercase_first() {
  assert_eq!(to_uppercase_first("marco"), "Marco");
  assert_eq!(to_uppercase_first("Marco"), "Marco");
}

#[test]
#[should_panic]
fn test_to_uppercase_first_empty() {
    to_uppercase_first("");
}

pub fn rewrite_paths(result_map: CovResultMap, path_mapping: Option<Value>, source_dir: &str, prefix_dir: &str, ignore_global: bool, ignore_not_existing: bool, to_ignore_dir: Option<String>) -> CovResultIter {
    let source_dir = if source_dir != "" {
        fs::canonicalize(&source_dir).expect("Source directory does not exist.")
    } else {
        PathBuf::from("")
    };

    let path_mapping = if path_mapping.is_some() {
        path_mapping.unwrap()
    } else {
        json!({})
    };

    let prefix_dir = prefix_dir.to_owned();

    Box::new(result_map.into_iter().filter_map(move |(path, result)| {
        let path = PathBuf::from(path.replace("\\", "/"));

        // Get path from the mapping, or remove prefix from path.
        let (rel_path, found_in_mapping) = if let Some(p) = path_mapping.get(to_lowercase_first(path.to_str().unwrap())) {
            (PathBuf::from(p.as_str().unwrap()), true)
        } else if let Some(p) = path_mapping.get(to_uppercase_first(path.to_str().unwrap())) {
            (PathBuf::from(p.as_str().unwrap()), true)
        } else if path.starts_with(&prefix_dir) {
            (path.strip_prefix(&prefix_dir).unwrap().to_path_buf(), false)
        } else if path.starts_with(&source_dir) {
            (path.strip_prefix(&source_dir).unwrap().to_path_buf(), false)
        } else {
            (path, false)
        };

        if ignore_global && !rel_path.is_relative() {
            return None;
        }

        // Get absolute path to source file.
        let abs_path = if rel_path.is_relative() {
            if !cfg!(windows) {
                PathBuf::from(&source_dir).join(&rel_path)
            } else {
                PathBuf::from(&source_dir).join(&rel_path.to_str().unwrap().replace("/", "\\"))
            }
        } else {
            rel_path.clone()
        };

        // Canonicalize, if possible.
        let abs_path = match fs::canonicalize(&abs_path) {
            Ok(p) => p,
            Err(_) => abs_path,
        };

        let rel_path = if found_in_mapping {
            rel_path
        } else if abs_path.starts_with(&source_dir) { // Remove source dir from path.
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

        let rel_path = PathBuf::from(rel_path.to_str().unwrap().replace("\\", "/"));

        Some((abs_path, rel_path, result))
    }))
}

#[allow(unused_macros)]
macro_rules! empty_result {
    () => {
        {
            CovResult {
                lines: BTreeMap::new(),
                branches: BTreeMap::new(),
                functions: HashMap::new(),
            }
        }
    };
}

#[test]
fn test_rewrite_paths_basic() {
    let mut result_map: CovResultMap = HashMap::new();
    result_map.insert("main.cpp".to_string(), empty_result!());
    let results = rewrite_paths(result_map, None, "", "", false, false, None);
    let mut count = 0;
    for (abs_path, rel_path, result) in results {
        count += 1;
        assert_eq!(abs_path, PathBuf::from("main.cpp"));
        assert_eq!(rel_path, PathBuf::from("main.cpp"));
        assert_eq!(result, empty_result!());
    }
    assert_eq!(count, 1);
}

#[cfg(unix)]
#[test]
fn test_rewrite_paths_ignore_global_files() {
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("main.cpp".to_string(), empty_result!());
        result_map.insert("/usr/include/prova.h".to_string(), empty_result!());
        let results = rewrite_paths(result_map, None, "", "", true, false, None);
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert_eq!(abs_path, PathBuf::from("main.cpp"));
            assert_eq!(rel_path, PathBuf::from("main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
}

#[cfg(windows)]
#[test]
fn test_rewrite_paths_ignore_global_files() {
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("main.cpp".to_string(), empty_result!());
        result_map.insert("C:\\usr\\include\\prova.h".to_string(), empty_result!());
        let results = rewrite_paths(result_map, None, "", "", true, false, None);
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert_eq!(abs_path, PathBuf::from("main.cpp"));
            assert_eq!(rel_path, PathBuf::from("main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
}

#[cfg(unix)]
#[test]
fn test_rewrite_paths_remove_prefix() {
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("/home/worker/src/workspace/main.cpp".to_string(), empty_result!());
        let results = rewrite_paths(result_map, None, "", "/home/worker/src/workspace/", false, false, None);
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert_eq!(abs_path, PathBuf::from("main.cpp"));
            assert_eq!(rel_path, PathBuf::from("main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
}

#[cfg(windows)]
#[test]
fn test_rewrite_paths_remove_prefix() {
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("C:\\Users\\worker\\src\\workspace\\main.cpp".to_string(), empty_result!());
        let results = rewrite_paths(result_map, None, "", "C:\\Users\\worker\\src\\workspace\\", false, false, None);
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert_eq!(abs_path, PathBuf::from("main.cpp"));
            assert_eq!(rel_path, PathBuf::from("main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
}

#[cfg(windows)]
#[test]
fn test_rewrite_paths_remove_prefix_with_slash() {
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("C:/Users/worker/src/workspace/main.cpp".to_string(), empty_result!());
        let results = rewrite_paths(result_map, None, "", "C:/Users/worker/src/workspace/", false, false, None);
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert_eq!(abs_path, PathBuf::from("main.cpp"));
            assert_eq!(rel_path, PathBuf::from("main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
}

#[cfg(windows)]
#[test]
fn test_rewrite_paths_remove_prefix_with_slash_longer_path() {
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("C:/Users/worker/src/workspace/main.cpp".to_string(), empty_result!());
        let results = rewrite_paths(result_map, None, "", "C:/Users/worker/src/", false, false, None);
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert_eq!(abs_path, PathBuf::from("workspace/main.cpp"));
            assert_eq!(rel_path.to_str().unwrap(), "workspace/main.cpp");
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
}

#[cfg(unix)]
#[test]
fn test_rewrite_paths_ignore_non_existing_files() {
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("tests/class/main.cpp".to_string(), empty_result!());
        result_map.insert("tests/class/doesntexist.cpp".to_string(), empty_result!());
        let results = rewrite_paths(result_map, None, "", "", false, true, None);
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests/class/main.cpp"));
            assert!(rel_path.ends_with("tests/class/main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
}

#[cfg(windows)]
#[test]
fn test_rewrite_paths_ignore_non_existing_files() {
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("tests\\class\\main.cpp".to_string(), empty_result!());
        result_map.insert("tests\\class\\doesntexist.cpp".to_string(), empty_result!());
        let results = rewrite_paths(result_map, None, "", "", false, true, None);
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests\\class\\main.cpp"));
            assert!(rel_path.ends_with("tests\\class\\main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
}

#[cfg(unix)]
#[test]
fn test_rewrite_paths_ignore_a_directory() {
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("main.cpp".to_string(), empty_result!());
        result_map.insert("mydir/prova.h".to_string(), empty_result!());
        let results = rewrite_paths(result_map, None, "", "", false, false, Some("mydir".to_string()));
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert_eq!(abs_path, PathBuf::from("main.cpp"));
            assert_eq!(rel_path, PathBuf::from("main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
}

#[cfg(windows)]
#[test]
fn test_rewrite_paths_ignore_a_directory() {
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("main.cpp".to_string(), empty_result!());
        result_map.insert("mydir\\prova.h".to_string(), empty_result!());
        let results = rewrite_paths(result_map, None, "", "", false, false, Some("mydir".to_string()));
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert_eq!(abs_path, PathBuf::from("main.cpp"));
            assert_eq!(rel_path, PathBuf::from("main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
}

#[cfg(unix)]
#[test]
fn test_rewrite_paths_rewrite_path_using_relative_source_directory() {
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("class/main.cpp".to_string(), empty_result!());
        let results = rewrite_paths(result_map, None, "tests", "", false, true, None);
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests/class/main.cpp"));
            assert_eq!(rel_path, PathBuf::from("class/main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
}

#[cfg(windows)]
#[test]
fn test_rewrite_paths_rewrite_path_using_relative_source_directory() {
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("class\\main.cpp".to_string(), empty_result!());
        let results = rewrite_paths(result_map, None, "tests", "", false, true, None);
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests\\class\\main.cpp"));
            assert_eq!(rel_path, PathBuf::from("class\\main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
}

#[cfg(unix)]
#[test]
fn test_rewrite_paths_rewrite_path_using_absolute_source_directory() {
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("class/main.cpp".to_string(), empty_result!());
        let results = rewrite_paths(result_map, None, fs::canonicalize("tests").unwrap().to_str().unwrap(), "", false, true, None);
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests/class/main.cpp"));
            assert_eq!(rel_path, PathBuf::from("class/main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
}

#[cfg(windows)]
#[test]
fn test_rewrite_paths_rewrite_path_using_absolute_source_directory() {
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("class\\main.cpp".to_string(), empty_result!());
        let results = rewrite_paths(result_map, None, fs::canonicalize("tests").unwrap().to_str().unwrap(), "", false, true, None);
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests\\class\\main.cpp"));
            assert_eq!(rel_path, PathBuf::from("class\\main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
}

#[cfg(unix)]
#[test]
fn test_rewrite_paths_rewrite_path_and_remove_prefix() {
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("/home/worker/src/workspace/class/main.cpp".to_string(), empty_result!());
        let results = rewrite_paths(result_map, None, "tests", "/home/worker/src/workspace", false, true, None);
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests/class/main.cpp"));
            assert_eq!(rel_path, PathBuf::from("class/main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
}

#[cfg(windows)]
#[test]
fn test_rewrite_paths_rewrite_path_and_remove_prefix() {
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("C:\\Users\\worker\\src\\workspace\\class\\main.cpp".to_string(), empty_result!());
        let results = rewrite_paths(result_map, None, "tests", "C:\\Users\\worker\\src\\workspace", false, true, None);
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests\\class\\main.cpp"));
            assert_eq!(rel_path, PathBuf::from("class\\main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
}

#[cfg(unix)]
#[test]
fn test_rewrite_paths_rewrite_path_using_mapping() {
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("class/main.cpp".to_string(), empty_result!());
        let results = rewrite_paths(result_map, Some(json!({"class/main.cpp": "rewritten/main.cpp"})), "", "", false, false, None);
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert_eq!(abs_path, PathBuf::from("rewritten/main.cpp"));
            assert_eq!(rel_path, PathBuf::from("rewritten/main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
}

#[cfg(windows)]
#[test]
fn test_rewrite_paths_rewrite_path_using_mapping() {
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("class\\main.cpp".to_string(), empty_result!());
        let results = rewrite_paths(result_map, Some(json!({"class/main.cpp": "rewritten/main.cpp"})), "", "", false, false, None);
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert_eq!(abs_path, PathBuf::from("rewritten\\main.cpp"));
            assert_eq!(rel_path, PathBuf::from("rewritten\\main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
}

#[cfg(unix)]
#[test]
fn test_rewrite_paths_rewrite_path_using_mapping_and_ignore_non_existing() {
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("rewritten/main.cpp".to_string(), empty_result!());
        result_map.insert("tests/class/main.cpp".to_string(), empty_result!());
        let results = rewrite_paths(result_map, Some(json!({"rewritten/main.cpp": "tests/class/main.cpp", "tests/class/main.cpp": "rewritten/main.cpp"})), "", "", false, true, None);
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests/class/main.cpp"));
            assert_eq!(rel_path, PathBuf::from("tests/class/main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
}

#[cfg(windows)]
#[test]
fn test_rewrite_paths_rewrite_path_using_mapping_and_ignore_non_existing() {
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("rewritten\\main.cpp".to_string(), empty_result!());
        result_map.insert("tests\\class\\main.cpp".to_string(), empty_result!());
        let results = rewrite_paths(result_map, Some(json!({"rewritten/main.cpp": "tests/class/main.cpp", "tests/class/main.cpp": "rewritten/main.cpp"})), "", "", false, true, None);
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests\\class\\main.cpp"));
            assert_eq!(rel_path, PathBuf::from("tests\\class\\main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
}

#[cfg(unix)]
#[test]
fn test_rewrite_paths_rewrite_path_using_mapping_and_remove_prefix() {
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("/home/worker/src/workspace/rewritten/main.cpp".to_string(), empty_result!());
        let results = rewrite_paths(result_map, Some(json!({"/home/worker/src/workspace/rewritten/main.cpp": "tests/class/main.cpp"})), "", "/home/worker/src/workspace", false, true, None);
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests/class/main.cpp"));
            assert_eq!(rel_path, PathBuf::from("tests/class/main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
}

#[cfg(windows)]
#[test]
fn test_rewrite_paths_rewrite_path_using_mapping_and_remove_prefix() {
        // Mapping with uppercase disk and prefix with uppercase disk.
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("C:\\Users\\worker\\src\\workspace\\rewritten\\main.cpp".to_string(), empty_result!());
        let results = rewrite_paths(result_map, Some(json!({"C:/Users/worker/src/workspace/rewritten/main.cpp": "tests/class/main.cpp"})), "", "C:\\Users\\worker\\src\\workspace", false, true, None);
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests\\class\\main.cpp"));
            assert_eq!(rel_path, PathBuf::from("tests\\class\\main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);

        // Mapping with lowercase disk and prefix with uppercase disk.
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("C:\\Users\\worker\\src\\workspace\\rewritten\\main.cpp".to_string(), empty_result!());
        let results = rewrite_paths(result_map, Some(json!({"c:/Users/worker/src/workspace/rewritten/main.cpp": "tests/class/main.cpp"})), "", "C:\\Users\\worker\\src\\workspace", false, true, None);
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests\\class\\main.cpp"));
            assert_eq!(rel_path, PathBuf::from("tests\\class\\main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);

        // Mapping with uppercase disk and prefix with lowercase disk.
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("C:\\Users\\worker\\src\\workspace\\rewritten\\main.cpp".to_string(), empty_result!());
        let results = rewrite_paths(result_map, Some(json!({"C:/Users/worker/src/workspace/rewritten/main.cpp": "tests/class/main.cpp"})), "", "c:\\Users\\worker\\src\\workspace", false, true, None);
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests\\class\\main.cpp"));
            assert_eq!(rel_path, PathBuf::from("tests\\class\\main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);

        // Mapping with lowercase disk and prefix with lowercase disk.
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("C:\\Users\\worker\\src\\workspace\\rewritten\\main.cpp".to_string(), empty_result!());
        let results = rewrite_paths(result_map, Some(json!({"c:/Users/worker/src/workspace/rewritten/main.cpp": "tests/class/main.cpp"})), "", "c:\\Users\\worker\\src\\workspace", false, true, None);
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests\\class\\main.cpp"));
            assert_eq!(rel_path, PathBuf::from("tests\\class\\main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
}

#[cfg(unix)]
#[test]
fn test_rewrite_paths_rewrite_path_using_mapping_and_source_directory_and_remove_prefix() {
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("/home/worker/src/workspace/rewritten/main.cpp".to_string(), empty_result!());
        let results = rewrite_paths(result_map, Some(json!({"/home/worker/src/workspace/rewritten/main.cpp": "class/main.cpp"})), "tests", "/home/worker/src/workspace", false, true, None);
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
             count += 1;
             assert!(abs_path.is_absolute());
             assert!(abs_path.ends_with("tests/class/main.cpp"));
             assert_eq!(rel_path, PathBuf::from("class/main.cpp"));
             assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
}

#[cfg(windows)]
#[test]
fn test_rewrite_paths_rewrite_path_using_mapping_and_source_directory_and_remove_prefix() {
        let mut result_map: CovResultMap = HashMap::new();
        result_map.insert("C:\\Users\\worker\\src\\workspace\\rewritten\\main.cpp".to_string(), empty_result!());
        let results = rewrite_paths(result_map, Some(json!({"C:/Users/worker/src/workspace/rewritten/main.cpp": "class/main.cpp"})), "tests", "C:\\Users\\worker\\src\\workspace", false, true, None);
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests\\class\\main.cpp"));
            assert_eq!(rel_path, PathBuf::from("class\\main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
}

fn to_activedata_etl_vec(normal_vec: &[u32]) -> Vec<Value> {
    normal_vec.iter().map(|&x| json!({"line": x})).collect()
}

pub fn output_activedata_etl(results: CovResultIter) {
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

pub fn output_lcov(results: CovResultIter) {
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
            write!(writer, "FNH:{}\n", result.functions.values().filter(|x| x.executed).count()).unwrap();
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

pub fn output_coveralls(results: CovResultIter, repo_token: &str, service_name: &str, service_number: &str, service_job_number: &str, commit_sha: &str, with_function_info: bool) {
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

        let mut branches = Vec::new();
        for (&(line, number), &taken) in &result.branches {
            branches.push(line);
            branches.push(0);
            branches.push(number);
            branches.push(if taken { 1 } else { 0 });
        }

        if !with_function_info {
            source_files.push(json!({
                "name": rel_path,
                "source_digest": get_digest(abs_path),
                "coverage": coverage,
                "branches": branches,
            }));
        } else {
            let mut functions = Vec::new();
            for (name, function) in &result.functions {
              functions.push(json!({
                  "name": name,
                  "start": function.start,
                  "exec": function.executed,
              }));
            }

            source_files.push(json!({
                "name": rel_path,
                "source_digest": get_digest(abs_path),
                "coverage": coverage,
                "branches": branches,
                "functions": functions,
            }));
        }
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

pub fn is_recent_version(gcov_output: &str) -> bool {
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

pub fn check_gcov_version() -> bool {
    let output = Command::new("gcov")
                         .arg("--version")
                         .output()
                         .expect("Failed to execute `gcov`. `gcov` is required (it is part of GCC).");

    assert!(output.status.success(), "`gcov` failed to execute.");

    is_recent_version(&String::from_utf8(output.stdout).unwrap())
}
