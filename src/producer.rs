use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::fs::{self, File};
use std::io::{self, Read};
use std::os;
use std::sync::Arc;
use zip::ZipArchive;
use walkdir::WalkDir;

use defs::*;

#[derive(Debug)]
pub enum ArchiveType {
    Zip(RefCell<ZipArchive<File>>),
    Dir(PathBuf),
}

pub enum FilePath<'a> {
    File(&'a mut Read),
    Path(&'a Path),
}

#[derive(Debug)]
pub struct Archive {
    pub name: String,
    pub item: RefCell<ArchiveType>,
}

#[derive(Debug, PartialEq, Eq, Hash)]
pub struct GCNOStem {
    pub stem: String,
    pub llvm: bool,
}

impl Archive {
    fn insert_vec<'a>(&'a self, filename: String, map: &RefCell<HashMap<String, Vec<&'a Archive>>>) {
        let mut map = map.borrow_mut();
        if map.contains_key(&filename) {
            let vec = map.get_mut(&filename).unwrap();
            vec.push(self);
        } else {
            let mut vec = Vec::new();
            vec.push(self);
            map.insert(filename, vec);
        }
    }

    fn handle_file<'a>(&'a self, file: FilePath, path: PathBuf,
                       gcno_stem_archives: &RefCell<HashMap<GCNOStem, &'a Archive>>,
                       gcda_stem_archives: &RefCell<HashMap<String, Vec<&'a Archive>>>,
                       infos: &RefCell<HashMap<String, Vec<&'a Archive>>>,
                       linked_files_maps: &RefCell<HashMap<String, &'a Archive>>,
                       is_llvm: bool) {
        match path.extension() {
            Some(ext) => match ext.to_str().unwrap() {
                "gcno" => {
                    let llvm = is_llvm || match file {
                        FilePath::File(reader) => {
                            Archive::is_gcno_llvm(reader)
                        },
                        FilePath::Path(path) => {
                            match File::open(path) {
                                Ok(mut f) => {
                                    Archive::is_gcno_llvm(&mut f)
                                },
                                Err(_) => false,
                            }
                        }
                    };
                    let filename = path.with_extension("").to_str().unwrap().to_string();
                    gcno_stem_archives.borrow_mut().insert(GCNOStem {
                        stem: filename,
                        llvm: llvm,
                    }, self);
                }
                "gcda" => {
                    let filename = path.with_extension("").to_str().unwrap().to_string();
                    self.insert_vec(filename, gcda_stem_archives);
                },
                "info" => {
                    let filename = path.to_str().unwrap().to_string();
                    self.insert_vec(filename, infos);
                },
                "json" => {
                    let filename = path.file_name().unwrap();
                    if filename == "linked-files-map.json" {
                        let filename = path.to_str().unwrap().to_string();
                        linked_files_maps.borrow_mut().insert(filename, self);
                    }
                },
                _ => { },
            },
            None => { },
        }
    }

    fn is_gcno_llvm(reader: &mut Read) -> bool {
        let mut bytes: [u8; 8] = [0; 8];
        reader.read_exact(&mut bytes).is_ok() && bytes == ['o' as u8,
                                                           'n' as u8,
                                                           'c' as u8,
                                                           'g' as u8,
                                                           '*' as u8,
                                                           '2' as u8,
                                                           '0' as u8,
                                                           '4' as u8]
    }

    pub fn get_name(&self) -> &String {
        &self.name
    }

    pub fn explore<'a>(&'a mut self,
                       gcno_stem_archives: &RefCell<HashMap<GCNOStem, &'a Archive>>,
                       gcda_stem_archives: &RefCell<HashMap<String, Vec<&'a Archive>>>,
                       infos: &RefCell<HashMap<String, Vec<&'a Archive>>>,
                       linked_files_maps: &RefCell<HashMap<String, &'a Archive>>,
                       is_llvm: bool) {
        match *self.item.borrow() {
            ArchiveType::Zip(ref zip) => {
                let mut zip = zip.borrow_mut();
                for i in 0..zip.len() {
                    let mut file = zip.by_index(i).unwrap();
                    let path = PathBuf::from(file.name());
                    self.handle_file(FilePath::File(&mut file), path, gcno_stem_archives, gcda_stem_archives, infos, linked_files_maps, is_llvm);
                }
            },
            ArchiveType::Dir(ref dir) => {
                for entry in WalkDir::new(&dir) {
                    let entry = entry.expect(&format!("Failed to open directory '{:?}'.", dir));
                    let full_path = entry.path();
                    if full_path.is_file() {
                        let path = full_path.strip_prefix(dir).unwrap();
                        self.handle_file(FilePath::Path(full_path), path.to_path_buf(), gcno_stem_archives, gcda_stem_archives, infos, linked_files_maps, is_llvm);
                    }
                }
            }
        }
    }

    pub fn read_in_buffer(&self, name: &str, buf: &mut Vec<u8>) -> bool {
        match *self.item.borrow_mut() {
            ArchiveType::Zip(ref mut zip) => {
                let name = name.replace("\\", "/");
                match zip.borrow_mut().by_name(&name) {
                    Ok(mut f) => {
                        f.read_to_end(buf).expect("Failed to read gcda file");
                        true
                    },
                    Err(_) => false,
                }
            },
            ArchiveType::Dir(ref dir) => {
                match File::open(dir.join(name)) {
                    Ok(mut f) => {
                        f.read_to_end(buf).expect("Failed to read gcda file");
                        true
                    },
                    Err(_) => false,
                }
            }
        }
    }

    pub fn extract(&self, name: &str, path: &PathBuf) -> bool {
        let dest_parent = path.parent().unwrap();
        if !dest_parent.exists() {
            match fs::create_dir_all(dest_parent) {
                Ok(_) => { },
                Err(_) => panic!("Cannot create parent directory"),
            };
        }

        match *self.item.borrow_mut() {
            ArchiveType::Zip(ref mut zip) => {
                let name = name.replace("\\", "/");
                match zip.borrow_mut().by_name(&name) {
                    Ok(mut f) => {
                        let mut file = File::create(&path).expect("Failed to create file");
                        io::copy(&mut f, &mut file).expect("Failed to copy file from ZIP");
                        true
                    },
                    Err(_) => {
                        false
                    },
                }
            },
            ArchiveType::Dir(ref dir) => {
                // don't use a hard link here because it can fail when src and dst are not on the same device
                let src_path = dir.join(name);

                #[cfg(unix)]
                os::unix::fs::symlink(&src_path, path).expect(&format!("Failed to create a symlink {:?} -> {:?}", src_path, path));

                #[cfg(windows)]
                os::windows::fs::symlink_file(&src_path, path).expect(&format!("Failed to create a symlink {:?} -> {:?}", src_path, path));

                true
            },
        }
    }
}

fn gcno_gcda_producer(tmp_dir: &Path,
                      gcno_stem_archives: HashMap<GCNOStem, &Archive>,
                      gcda_stem_archives: HashMap<String, Vec<&Archive>>,
                      queue: &WorkQueue,
                      ignore_orphan_gcno: bool) {
    let push_to_queue  = |item, name| queue.push(Some(WorkItem {
        format: ItemFormat::GCNO,
        item: item,
        name: name,
    }));;

    for (gcno_stem, gcno_archive) in gcno_stem_archives.iter() {
        let stem = &gcno_stem.stem;
        match gcda_stem_archives.get(stem) {
            Some(gcda_archives) => {
                let gcno_archive = *gcno_archive;
                let gcno = format!("{}.gcno", stem).to_string();
                let physical_gcno_path = tmp_dir.join(format!("{}_{}.gcno", stem, 1));
                let mut gcno_buf_opt: Option<Arc<Vec<u8>>> = if gcno_stem.llvm {
                    let mut buffer: Vec<u8> = Vec::new();
                    gcno_archive.read_in_buffer(&gcno, &mut buffer);
                    Some(Arc::new(buffer))
                } else {
                    gcno_archive.extract(&gcno, &physical_gcno_path);
                    None
                };

                for (num, &gcda_archive) in gcda_archives.iter().enumerate() {
                    let gcno_path = tmp_dir.join(format!("{}_{}.gcno", stem, num + 1));
                    let gcda = format!("{}.gcda", stem).to_string();

                    match gcno_buf_opt {
                        Some(ref gcno_buf) => {
                            let mut gcda_buf: Vec<u8> = Vec::new();
                            let gcno_stem = tmp_dir.join(format!("{}_{}", stem, num + 1));
                            let gcno_stem = gcno_stem.to_str().expect("Failed to create stem file string");

                            if gcda_archive.read_in_buffer(&gcda, &mut gcda_buf) || (num == 0 && !ignore_orphan_gcno) {
                                push_to_queue(ItemType::Buffers(GcnoBuffers {
                                    stem: gcno_stem.to_string(),
                                    gcno_buf: Arc::clone(gcno_buf),
                                    gcda_buf: gcda_buf
                                }), gcda_archive.get_name().to_string());
                            }
                        },
                        None => {
                            // Create symlinks.
                            if num != 0 {
                                fs::hard_link(&physical_gcno_path, &gcno_path).expect(&format!("Failed to create hardlink {:?}", gcno_path));
                            }

                            let gcda_path = tmp_dir.join(format!("{}_{}.gcda", stem, num + 1));

                            if gcda_archive.extract(&gcda, &gcda_path) || (num == 0 && !ignore_orphan_gcno) {
                                push_to_queue(ItemType::Path(gcno_path), gcda_archive.get_name().to_string());
                            }
                        }
                    };
                }
            },
            None => {
                if !ignore_orphan_gcno {
                    let gcno_archive = *gcno_archive;
                    let gcno = format!("{}.gcno", stem).to_string();
                    if gcno_stem.llvm {
                        let mut buffer: Vec<u8> = Vec::new();
                        gcno_archive.read_in_buffer(&gcno, &mut buffer);

                        push_to_queue(ItemType::Buffers(GcnoBuffers {
                            stem: gcno,
                            gcno_buf: Arc::new(buffer),
                            gcda_buf: Vec::new()
                        }), gcno_archive.get_name().to_string());
                    } else {
                        let physical_gcno_path = tmp_dir.join(format!("{}_{}.gcno", stem, 1));
                        if gcno_archive.extract(&gcno, &physical_gcno_path) {
                            push_to_queue(ItemType::Path(physical_gcno_path), gcno_archive.get_name().to_string());
                        }
                    }
                }
            }
        }
    }
}

pub fn info_producer(infos: HashMap<String, Vec<&Archive>>, queue: &WorkQueue) {
    for (name, archives) in infos.iter() {
        for archive in archives {
            let mut buffer = Vec::new();
            archive.read_in_buffer(name, &mut buffer);
            queue.push(Some(WorkItem {
                format: ItemFormat::INFO,
                item: ItemType::Content(buffer),
                name: archive.get_name().to_string(),
            }));
        }
    }
}

pub fn get_mapping(linked_files_maps: HashMap<String, &Archive>) -> Option<Vec<u8>> {
    let mut mapping: Option<Vec<u8>> = None;
    for (name, archive) in linked_files_maps.iter() {
        let mut buffer = Vec::new();
        archive.read_in_buffer(name, &mut buffer);
        mapping = Some(buffer);
        break;
    }
    mapping
}

fn open_archive(path: &str) -> ZipArchive<File> {
    let file = File::open(&path).expect(&format!("Failed to open ZIP file '{}'.", path));
    ZipArchive::new(file).expect(&format!("Failed to parse ZIP file: {}", path))
}

pub fn producer(tmp_dir: &Path, paths: &[String], queue: &WorkQueue, ignore_orphan_gcno: bool, is_llvm: bool) -> Option<Vec<u8>> {
    let mut archives: Vec<Archive> = Vec::new();
    let current_dir = env::current_dir().unwrap();

    for path in paths {
        if path.ends_with(".zip") {
            let archive = open_archive(path);
            archives.push(Archive {
                name: path.to_string(),
                item: RefCell::new(ArchiveType::Zip(RefCell::new(archive))),
            });
        } else {
            let path_dir = PathBuf::from(path);
            let full_path = if path_dir.is_relative() {
                current_dir.join(path_dir)
            } else {
                path_dir
            };
            archives.push(Archive {
                name: path.to_string(),
                item: RefCell::new(ArchiveType::Dir(full_path)),
            });
        }
    }

    let gcno_stems_archives: RefCell<HashMap<GCNOStem, &Archive>> = RefCell::new(HashMap::new());
    let gcda_stems_archives: RefCell<HashMap<String, Vec<&Archive>>> = RefCell::new(HashMap::new());
    let infos: RefCell<HashMap<String, Vec<&Archive>>> = RefCell::new(HashMap::new());
    let linked_files_maps: RefCell<HashMap<String, &Archive>> = RefCell::new(HashMap::new());

    for archive in archives.iter_mut() {
        archive.explore(&gcno_stems_archives, &gcda_stems_archives, &infos, &linked_files_maps, is_llvm);
    }

    if gcno_stems_archives.borrow().is_empty() {
        assert!(!infos.borrow().is_empty());
    }

    info_producer(infos.into_inner(), queue);
    gcno_gcda_producer(tmp_dir, gcno_stems_archives.into_inner(), gcda_stems_archives.into_inner(), queue, ignore_orphan_gcno);

    get_mapping(linked_files_maps.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use crossbeam::sync::MsQueue;
    use serde_json::{self, Value};
    use tempdir::TempDir;

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
                    },
                    ItemType::Buffers(ref b) => {
                        b.stem.ends_with(elem.2)
                    },
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
                    },
                    ItemType::Buffers(ref b) => {
                        b.stem.ends_with(x.2)
                    },
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

        let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        let mapping = producer(&tmp_path, &["test".to_string()], &queue, false, false);

        let expected = vec![
            (ItemFormat::GCNO, true, "Platform_1.gcno", true),
            (ItemFormat::GCNO, true, "sub2/RootAccessibleWrap_1.gcno", true),
            (ItemFormat::GCNO, true, "nsMaiInterfaceValue_1.gcno", true),
            (ItemFormat::GCNO, true, "sub/prova2_1.gcno", true),
            (ItemFormat::GCNO, true, "nsMaiInterfaceDocument_1.gcno", true),
            (ItemFormat::GCNO, true, "Unified_cpp_netwerk_base0_1.gcno", true),
            (ItemFormat::GCNO, true, "prova_1.gcno", true),
            (ItemFormat::GCNO, true, "nsGnomeModule_1.gcno", true),
            (ItemFormat::GCNO, true, "negative_counts_1.gcno", true),
            (ItemFormat::GCNO, true, "64bit_count_1.gcno", true),
            (ItemFormat::GCNO, true, "no_gcda/main_1.gcno", false),
            (ItemFormat::GCNO, true, "only_one_gcda/main_1.gcno", true),
            (ItemFormat::GCNO, true, "only_one_gcda/orphan_1.gcno", false),
            (ItemFormat::GCNO, true, "gcno_symlink/gcda/main_1.gcno", true),
            (ItemFormat::GCNO, true, "gcno_symlink/gcno/main_1.gcno", false),
            (ItemFormat::GCNO, false, "rust/generics_with_two_parameters_1", true),
            (ItemFormat::INFO, false, "1494603973-2977-7.info", false),
            (ItemFormat::INFO, false, "prova.info", false),
            (ItemFormat::INFO, false, "prova_fn_with_commas.info", false),
            (ItemFormat::INFO, false, "empty_line.info", false),
            (ItemFormat::INFO, false, "invalid_DA_record.info", false),
            (ItemFormat::INFO, false, "relative_path/relative_path.info", false),
            (ItemFormat::GCNO, false, "llvm/file_1", true),
        ];

        check_produced(tmp_path, &queue, expected);
        assert!(mapping.is_some());
        let mapping: Value = serde_json::from_slice(&mapping.unwrap()).unwrap();
        assert_eq!(mapping.get("dist/include/zlib.h").unwrap().as_str().unwrap(), "modules/zlib/src/zlib.h");
    }

    #[test]
    fn test_dir_producer_multiple_directories() {
        let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

        let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        let mapping = producer(&tmp_path, &["test/sub".to_string(),
                                            "test/sub2".to_string()], &queue, false, false);

        let expected = vec![
            (ItemFormat::GCNO, true, "RootAccessibleWrap_1.gcno", true),
            (ItemFormat::GCNO, true, "prova2_1.gcno", true),
        ];

        check_produced(tmp_path, &queue, expected);
        assert!(mapping.is_none());
    }

    #[test]
    fn test_dir_producer_directory_with_gcno_symlinks() {
        let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

        let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        let mapping = producer(&tmp_path, &["test/gcno_symlink/gcda".to_string()], &queue, false, false);

        let expected = vec![
            (ItemFormat::GCNO, true, "main_1.gcno", true),
        ];

        check_produced(tmp_path, &queue, expected);
        assert!(mapping.is_none());
    }

    #[test]
    fn test_dir_producer_directory_with_no_gcda() {
        let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

        let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        let mapping = producer(&tmp_path, &["test/only_one_gcda".to_string()], &queue, false, false);

        let expected = vec![
            (ItemFormat::GCNO, true, "main_1.gcno", true),
            (ItemFormat::GCNO, true, "orphan_1.gcno", false),
        ];

        check_produced(tmp_path, &queue, expected);
        assert!(mapping.is_none());
    }

    #[test]
    fn test_dir_producer_directory_with_no_gcda_ignore_orphan_gcno() {
        let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

        let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        let mapping = producer(&tmp_path, &["test/only_one_gcda".to_string()], &queue, true, false);

        let expected = vec![
            (ItemFormat::GCNO, true, "main_1.gcno", true),
        ];

        check_produced(tmp_path, &queue, expected);
        assert!(mapping.is_none());
    }

    #[test]
    fn test_zip_producer_with_gcda_dir() {
        let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

        let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        let mapping = producer(&tmp_path, &["test/zip_dir/gcno.zip".to_string(),
                                            "test/zip_dir".to_string()], &queue, false, false);

        let expected = vec![
            (ItemFormat::GCNO, true, "Platform_1.gcno", true),
            (ItemFormat::GCNO, true, "sub2/RootAccessibleWrap_1.gcno", true),
            (ItemFormat::GCNO, true, "nsMaiInterfaceValue_1.gcno", true),
            (ItemFormat::GCNO, true, "sub/prova2_1.gcno", true),
            (ItemFormat::GCNO, true, "nsMaiInterfaceDocument_1.gcno", true),
            (ItemFormat::GCNO, true, "nsGnomeModule_1.gcno", true)
        ];

        check_produced(tmp_path, &queue, expected);
        assert!(mapping.is_some());
        let mapping: Value = serde_json::from_slice(&mapping.unwrap()).unwrap();
        assert_eq!(mapping.get("dist/include/zlib.h").unwrap().as_str().unwrap(), "modules/zlib/src/zlib.h");
    }

    // Test extracting multiple gcda archives.
    #[test]
    fn test_zip_producer_multiple_gcda_archives() {
        let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

        let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        let mapping = producer(&tmp_path, &["test/gcno.zip".to_string(),
                                            "test/gcda1.zip".to_string(),
                                            "test/gcda2.zip".to_string()], &queue, false, false);

        let expected = vec![
            (ItemFormat::GCNO, true, "Platform_1.gcno", true),
            (ItemFormat::GCNO, true, "sub2/RootAccessibleWrap_1.gcno", true),
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
        let mapping = producer(&tmp_path, &["test/gcno_no_path_mapping.zip".to_string(),
                                            "test/gcda1.zip".to_string()], &queue, false, false);

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
        producer(&tmp_path, &["test/gcda1.zip".to_string(),
                              "test/gcno.zip".to_string(),
                              "test/gcda2.zip".to_string()], &queue, false, false);

        let expected = vec![
            (ItemFormat::GCNO, true, "Platform_1.gcno", true),
            (ItemFormat::GCNO, true, "sub2/RootAccessibleWrap_1.gcno", true),
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
        producer(&tmp_path, &["test/info1.zip".to_string(),
                              "test/info2.zip".to_string()], &queue, false, false);

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
        producer(&tmp_path, &["test/gcno.zip".to_string(),
                              "test/gcda1.zip".to_string(),
                              "test/info1.zip".to_string(),
                              "test/info2.zip".to_string()], &queue, false, false);

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
        let mapping = producer(&tmp_path, &["test/no_gcda/main.gcno.zip".to_string(),
                                            "test/no_gcda/empty.gcda.zip".to_string()], &queue, false, false);

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
        let mapping = producer(&tmp_path, &["test/no_gcda/main.gcno.zip".to_string(),
                                            "test/no_gcda/empty.gcda.zip".to_string(),
                                            "test/no_gcda/main.gcda.zip".to_string()], &queue, false, false);

        let expected = vec![
            (ItemFormat::GCNO, true, "main_1.gcno", true),
        ];

        check_produced(tmp_path, &queue, expected);
        assert!(mapping.is_none());
    }

    // Test passing a gcda archive with no gcno archive makes zip_producer fail.
    #[test]
    #[should_panic]
    fn test_zip_producer_with_gcda_archive_and_no_gcno_archive() {
        let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

        let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        producer(&tmp_path, &["test/no_gcda/main.gcda.zip".to_string()], &queue, false, false);
    }

    // Test extracting gcno/gcda archives, where a gcno file exist with no matching gcda file.
    #[test]
    fn test_zip_producer_no_matching_gcno() {
        let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

        let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        producer(&tmp_path, &["test/gcno.zip".to_string(),
                              "test/gcda2.zip".to_string()], &queue, false, false);

        let expected = vec![
            (ItemFormat::GCNO, true, "Platform_1.gcno", false),
            (ItemFormat::GCNO, true, "sub2/RootAccessibleWrap_1.gcno", false),
            (ItemFormat::GCNO, true, "nsMaiInterfaceValue_1.gcno", true),
            (ItemFormat::GCNO, true, "sub/prova2_1.gcno", true),
            (ItemFormat::GCNO, true, "nsMaiInterfaceDocument_1.gcno", true),
            (ItemFormat::GCNO, true, "nsGnomeModule_1.gcno", true),
        ];

        check_produced(tmp_path, &queue, expected);
    }

    // Test extracting gcno/gcda archives, where a gcno file exist with no matching gcda file.
    // The gcno file should be produced only once, not twice.
    #[test]
    fn test_zip_producer_no_matching_gcno_two_gcda_archives() {
        let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

        let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        producer(&tmp_path, &["test/gcno.zip".to_string(),
                              "test/gcda2.zip".to_string(),
                              "test/gcda2.zip".to_string()], &queue, false, false);

        let expected = vec![
            (ItemFormat::GCNO, true, "Platform_1.gcno", false),
            (ItemFormat::GCNO, true, "sub2/RootAccessibleWrap_1.gcno", false),
            (ItemFormat::GCNO, true, "nsMaiInterfaceValue_1.gcno", true),
            (ItemFormat::GCNO, true, "nsMaiInterfaceValue_2.gcno", true),
            (ItemFormat::GCNO, true, "sub/prova2_1.gcno", true),
            (ItemFormat::GCNO, true, "sub/prova2_2.gcno", true),
            (ItemFormat::GCNO, true, "nsMaiInterfaceDocument_1.gcno", true),
            (ItemFormat::GCNO, true, "nsMaiInterfaceDocument_2.gcno", true),
            (ItemFormat::GCNO, true, "nsGnomeModule_1.gcno", true),
            (ItemFormat::GCNO, true, "nsGnomeModule_2.gcno", true),
        ];

        check_produced(tmp_path, &queue, expected);
    }

    // Test extracting gcno/gcda archives, where a gcno file exist with no matching gcda file and ignore orphan gcno files.
    #[test]
    fn test_zip_producer_no_matching_gcno_ignore_orphan_gcno() {
        let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

        let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        producer(&tmp_path, &["test/gcno.zip".to_string(),
                              "test/gcda2.zip".to_string()], &queue, true, false);

        let expected = vec![
            (ItemFormat::GCNO, true, "nsMaiInterfaceValue_1.gcno", true),
            (ItemFormat::GCNO, true, "sub/prova2_1.gcno", true),
            (ItemFormat::GCNO, true, "nsMaiInterfaceDocument_1.gcno", true),
            (ItemFormat::GCNO, true, "nsGnomeModule_1.gcno", true),
        ];

        check_produced(tmp_path, &queue, expected);
    }

    // Test extracting gcno/gcda archives, where a gcno file exist with no matching gcda file and ignore orphan gcno files.
    #[test]
    fn test_zip_producer_no_matching_gcno_two_gcda_archives_ignore_orphan_gcno() {
        let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

        let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        producer(&tmp_path, &["test/gcno.zip".to_string(),
                              "test/gcda2.zip".to_string(),
                              "test/gcda2.zip".to_string()], &queue, true, false);

        let expected = vec![
            (ItemFormat::GCNO, true, "nsMaiInterfaceValue_1.gcno", true),
            (ItemFormat::GCNO, true, "nsMaiInterfaceValue_2.gcno", true),
            (ItemFormat::GCNO, true, "sub/prova2_1.gcno", true),
            (ItemFormat::GCNO, true, "sub/prova2_2.gcno", true),
            (ItemFormat::GCNO, true, "nsMaiInterfaceDocument_1.gcno", true),
            (ItemFormat::GCNO, true, "nsMaiInterfaceDocument_2.gcno", true),
            (ItemFormat::GCNO, true, "nsGnomeModule_1.gcno", true),
            (ItemFormat::GCNO, true, "nsGnomeModule_2.gcno", true),
        ];

        check_produced(tmp_path, &queue, expected);
    }

    #[test]
    fn test_zip_producer_llvm_buffers() {
        let queue: Arc<WorkQueue> = Arc::new(MsQueue::new());

        let tmp_dir = TempDir::new("grcov").expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        producer(&tmp_path, &["test/llvm/gcno.zip".to_string(),
                              "test/llvm/gcda1.zip".to_string(),
                              "test/llvm/gcda2.zip".to_string()], &queue, true, true);
        let gcno_buf: Arc<Vec<u8>> = Arc::new(vec![111, 110, 99, 103, 42, 50, 48, 52, 74, 200, 254, 66, 0, 0, 0, 1, 9, 0, 0, 0, 0, 0, 0, 0, 236, 217, 93, 255, 2, 0, 0, 0, 109, 97, 105, 110, 0, 0, 0, 0, 2, 0, 0, 0, 102, 105, 108, 101, 46, 99, 0, 0, 1, 0, 0, 0, 0, 0, 65, 1, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 67, 1, 3, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 67, 1, 3, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 69, 1, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 69, 1, 8, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 102, 105, 108, 101, 46, 99, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let gcda1_buf: Vec<u8> = vec![97, 100, 99, 103, 42, 50, 48, 52, 74, 200, 254, 66, 0, 0, 0, 1, 5, 0, 0, 0, 0, 0, 0, 0, 236, 217, 93, 255, 2, 0, 0, 0, 109, 97, 105, 110, 0, 0, 0, 0, 0, 0, 161, 1, 4, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 161, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 163, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let gcda2_buf: Vec<u8> = vec![97, 100, 99, 103, 42, 50, 48, 52, 74, 200, 254, 66, 0, 0, 0, 1, 5, 0, 0, 0, 0, 0, 0, 0, 236, 217, 93, 255, 2, 0, 0, 0, 109, 97, 105, 110, 0, 0, 0, 0, 0, 0, 161, 1, 4, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 161, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 163, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

        loop {
            let elem = queue.try_pop();
            if elem.is_none() {
                break;
            }
            let elem = elem.unwrap().unwrap();
            if let ItemType::Buffers(buffers) = elem.item {
                let stem = PathBuf::from(buffers.stem);
                let stem = stem.file_stem().expect("Unable to get file_stem");
                if stem == "file_1" {
                    assert_eq!(buffers.gcno_buf, gcno_buf);
                    assert_eq!(buffers.gcda_buf, gcda1_buf);
                } else if stem == "file_2" {
                    assert_eq!(buffers.gcno_buf, gcno_buf);
                    assert_eq!(buffers.gcda_buf, gcda2_buf);
                } else {
                    assert!(false, "Unexpected file: {:?}", stem);
                }
            } else {
                assert!(false, "Buffers expected");
            }
        }
    }
}
