extern crate tempfile;

use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::env;
use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::os;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use zip::ZipArchive;

use crate::defs::*;

#[derive(Debug)]
pub enum ArchiveType {
    Zip(RefCell<ZipArchive<BufReader<File>>>),
    Dir(PathBuf),
    Plain(Vec<PathBuf>),
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

#[cfg(not(windows))]
fn clean_path(path: &PathBuf) -> String {
    path.to_str().unwrap().to_string()
}

#[cfg(windows)]
fn clean_path(path: &PathBuf) -> String {
    path.replace("\\", "/");
}

impl Archive {
    fn insert_vec<'a>(
        &'a self,
        filename: String,
        map: &RefCell<FxHashMap<String, Vec<&'a Archive>>>,
    ) {
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

    fn handle_file<'a>(
        &'a self,
        file: FilePath,
        path: &PathBuf,
        gcno_stem_archives: &RefCell<FxHashMap<GCNOStem, &'a Archive>>,
        gcda_stem_archives: &RefCell<FxHashMap<String, Vec<&'a Archive>>>,
        infos: &RefCell<FxHashMap<String, Vec<&'a Archive>>>,
        xmls: &RefCell<FxHashMap<String, Vec<&'a Archive>>>,
        linked_files_maps: &RefCell<FxHashMap<String, &'a Archive>>,
        is_llvm: bool,
    ) {
        if let Some(ext) = path.extension() {
            match ext.to_str().unwrap() {
                "gcno" => {
                    let llvm = is_llvm || Archive::check_file(file, &Archive::is_gcno_llvm);
                    let filename = clean_path(&path.with_extension(""));
                    gcno_stem_archives.borrow_mut().insert(
                        GCNOStem {
                            stem: filename,
                            llvm,
                        },
                        self,
                    );
                }
                "gcda" => {
                    let filename = clean_path(&path.with_extension(""));
                    self.insert_vec(filename, gcda_stem_archives);
                }
                "info" => {
                    if Archive::check_file(file, &Archive::is_info) {
                        let filename = clean_path(path);
                        self.insert_vec(filename, infos);
                    }
                }
                "xml" => {
                    if Archive::check_file(file, &Archive::is_jacoco) {
                        let filename = clean_path(path);
                        self.insert_vec(filename, xmls);
                    }
                }
                "json" => {
                    let filename = path.file_name().unwrap();
                    if filename == "linked-files-map.json" {
                        let filename = clean_path(path);
                        linked_files_maps.borrow_mut().insert(filename, self);
                    }
                }
                _ => {}
            }
        }
    }

    fn is_gcno_llvm(reader: &mut Read) -> bool {
        let mut bytes: [u8; 8] = [0; 8];
        reader.read_exact(&mut bytes).is_ok()
            && bytes == [b'o', b'n', b'c', b'g', b'*', b'2', b'0', b'4']
    }

    fn is_jacoco(reader: &mut Read) -> bool {
        let mut bytes: [u8; 256] = [0; 256];
        if reader.read_exact(&mut bytes).is_ok() {
            return match String::from_utf8(bytes.to_vec()) {
                Ok(s) => s.contains("-//JACOCO//DTD"),
                Err(_) => false,
            };
        }
        false
    }

    fn is_info(reader: &mut Read) -> bool {
        let mut bytes: [u8; 3] = [0; 3];
        reader.read_exact(&mut bytes).is_ok()
            && (bytes == [b'T', b'N', b':'] || bytes == [b'S', b'F', b':'])
    }

    fn check_file(file: FilePath, checker: &Fn(&mut Read) -> bool) -> bool {
        match file {
            FilePath::File(reader) => checker(reader),
            FilePath::Path(path) => match File::open(path) {
                Ok(mut f) => checker(&mut f),
                Err(_) => false,
            },
        }
    }

    pub fn get_name(&self) -> &String {
        &self.name
    }

    pub fn explore<'a>(
        &'a mut self,
        gcno_stem_archives: &RefCell<FxHashMap<GCNOStem, &'a Archive>>,
        gcda_stem_archives: &RefCell<FxHashMap<String, Vec<&'a Archive>>>,
        infos: &RefCell<FxHashMap<String, Vec<&'a Archive>>>,
        xmls: &RefCell<FxHashMap<String, Vec<&'a Archive>>>,
        linked_files_maps: &RefCell<FxHashMap<String, &'a Archive>>,
        is_llvm: bool,
    ) {
        match *self.item.borrow() {
            ArchiveType::Zip(ref zip) => {
                let mut zip = zip.borrow_mut();
                for i in 0..zip.len() {
                    let mut file = zip.by_index(i).unwrap();
                    let path = PathBuf::from(file.name());
                    self.handle_file(
                        FilePath::File(&mut file),
                        &path,
                        gcno_stem_archives,
                        gcda_stem_archives,
                        infos,
                        xmls,
                        linked_files_maps,
                        is_llvm,
                    );
                }
            }
            ArchiveType::Dir(ref dir) => {
                for entry in WalkDir::new(&dir) {
                    let entry =
                        entry.unwrap_or_else(|_| panic!("Failed to open directory '{:?}'.", dir));
                    let full_path = entry.path();
                    if full_path.is_file() {
                        let path = full_path.strip_prefix(dir).unwrap();
                        self.handle_file(
                            FilePath::Path(full_path),
                            &path.to_path_buf(),
                            gcno_stem_archives,
                            gcda_stem_archives,
                            infos,
                            xmls,
                            linked_files_maps,
                            is_llvm,
                        );
                    }
                }
            }
            ArchiveType::Plain(ref plain) => {
                // All the paths are absolutes
                for full_path in plain {
                    self.handle_file(
                        FilePath::Path(full_path),
                        &full_path,
                        gcno_stem_archives,
                        gcda_stem_archives,
                        infos,
                        xmls,
                        linked_files_maps,
                        is_llvm,
                    );
                }
            }
        }
    }

    pub fn read_in_buffer(&self, name: &str, buf: &mut Vec<u8>) -> bool {
        match *self.item.borrow_mut() {
            ArchiveType::Zip(ref mut zip) => {
                let mut zip = zip.borrow_mut();
                let zipfile = zip.by_name(&name);
                match zipfile {
                    Ok(mut f) => {
                        f.read_to_end(buf).expect("Failed to read gcda file");
                        true
                    }
                    Err(_) => false,
                }
            }
            ArchiveType::Dir(ref dir) => match File::open(dir.join(name)) {
                Ok(mut f) => {
                    f.read_to_end(buf).expect("Failed to read gcda file");
                    true
                }
                Err(_) => false,
            },
            ArchiveType::Plain(_) => match File::open(name) {
                Ok(mut f) => {
                    f.read_to_end(buf)
                        .expect(&format!("Failed to read file: {}.", name));
                    true
                }
                Err(_) => false,
            },
        }
    }

    pub fn extract(&self, name: &str, path: &PathBuf) -> bool {
        let dest_parent = path.parent().unwrap();
        if !dest_parent.exists() {
            fs::create_dir_all(dest_parent).expect("Cannot create parent directory");
        }

        match *self.item.borrow_mut() {
            ArchiveType::Zip(ref mut zip) => {
                let mut zip = zip.borrow_mut();
                let zipfile = zip.by_name(&name);
                if let Ok(mut f) = zipfile {
                    let mut file = File::create(&path).expect("Failed to create file");
                    io::copy(&mut f, &mut file).expect("Failed to copy file from ZIP");
                    true
                } else {
                    false
                }
            }
            ArchiveType::Dir(ref dir) => {
                // don't use a hard link here because it can fail when src and dst are not on the same device
                let src_path = dir.join(name);

                #[cfg(unix)]
                os::unix::fs::symlink(&src_path, path).unwrap_or_else(|_| {
                    panic!("Failed to create a symlink {:?} -> {:?}", src_path, path)
                });

                #[cfg(windows)]
                os::windows::fs::symlink_file(&src_path, path).unwrap_or_else(|_| {
                    panic!("Failed to create a symlink {:?} -> {:?}", src_path, path)
                });

                true
            }
            ArchiveType::Plain(_) => {
                panic!("We shouldn't be there !!");
            }
        }
    }
}

fn gcno_gcda_producer(
    tmp_dir: &Path,
    gcno_stem_archives: &FxHashMap<GCNOStem, &Archive>,
    gcda_stem_archives: &FxHashMap<String, Vec<&Archive>>,
    sender: &JobSender,
    ignore_orphan_gcno: bool,
) {
    let send_job = |item, name| {
        sender
            .send(Some(WorkItem {
                format: ItemFormat::GCNO,
                item,
                name,
            }))
            .unwrap()
    };

    for (gcno_stem, gcno_archive) in gcno_stem_archives {
        let stem = &gcno_stem.stem;
        if let Some(gcda_archives) = gcda_stem_archives.get(stem) {
            let gcno_archive = *gcno_archive;
            let gcno = format!("{}.gcno", stem).to_string();
            let physical_gcno_path = tmp_dir.join(format!("{}_{}.gcno", stem, 1));
            if gcno_stem.llvm {
                let mut gcno_buffer: Vec<u8> = Vec::new();
                let mut gcda_buffers: Vec<Vec<u8>> = Vec::with_capacity(gcda_archives.len());
                gcno_archive.read_in_buffer(&gcno, &mut gcno_buffer);
                for gcda_archive in gcda_archives {
                    let mut gcda_buf: Vec<u8> = Vec::new();
                    let gcda = format!("{}.gcda", stem).to_string();
                    if gcda_archive.read_in_buffer(&gcda, &mut gcda_buf) {
                        gcda_buffers.push(gcda_buf);
                    }
                }
                send_job(
                    ItemType::Buffers(GcnoBuffers {
                        stem: stem.clone(),
                        gcno_buf: gcno_buffer,
                        gcda_buf: gcda_buffers,
                    }),
                    "".to_string(),
                );
            } else {
                gcno_archive.extract(&gcno, &physical_gcno_path);
                for (num, &gcda_archive) in gcda_archives.iter().enumerate() {
                    let gcno_path = tmp_dir.join(format!("{}_{}.gcno", stem, num + 1));
                    let gcda = format!("{}.gcda", stem).to_string();

                    // Create symlinks.
                    if num != 0 {
                        fs::hard_link(&physical_gcno_path, &gcno_path).unwrap_or_else(|_| {
                            panic!("Failed to create hardlink {:?}", gcno_path)
                        });
                    }

                    let gcda_path = tmp_dir.join(format!("{}_{}.gcda", stem, num + 1));
                    if gcda_archive.extract(&gcda, &gcda_path) || (num == 0 && !ignore_orphan_gcno)
                    {
                        send_job(
                            ItemType::Path((stem.clone(), gcno_path)),
                            gcda_archive.get_name().to_string(),
                        );
                    }
                }
            }
        } else if !ignore_orphan_gcno {
            let gcno_archive = *gcno_archive;
            let gcno = format!("{}.gcno", stem).to_string();
            if gcno_stem.llvm {
                let mut buffer: Vec<u8> = Vec::new();
                gcno_archive.read_in_buffer(&gcno, &mut buffer);

                send_job(
                    ItemType::Buffers(GcnoBuffers {
                        stem: stem.clone(),
                        gcno_buf: buffer,
                        gcda_buf: Vec::new(),
                    }),
                    gcno_archive.get_name().to_string(),
                );
            } else {
                let physical_gcno_path = tmp_dir.join(format!("{}_{}.gcno", stem, 1));
                if gcno_archive.extract(&gcno, &physical_gcno_path) {
                    send_job(
                        ItemType::Path((stem.clone(), physical_gcno_path)),
                        gcno_archive.get_name().to_string(),
                    );
                }
            }
        }
    }
}

fn file_content_producer(
    files: &FxHashMap<String, Vec<&Archive>>,
    sender: &JobSender,
    item_format: ItemFormat,
) {
    for (name, archives) in files {
        for archive in archives {
            let mut buffer = Vec::new();
            archive.read_in_buffer(name, &mut buffer);
            sender
                .send(Some(WorkItem {
                    format: item_format,
                    item: ItemType::Content(buffer),
                    name: archive.get_name().to_string(),
                }))
                .unwrap();
        }
    }
}

pub fn get_mapping(linked_files_maps: &FxHashMap<String, &Archive>) -> Option<Vec<u8>> {
    if let Some((ref name, archive)) = linked_files_maps.iter().next() {
        let mut buffer = Vec::new();
        archive.read_in_buffer(name, &mut buffer);
        Some(buffer)
    } else {
        None
    }
}

fn open_archive(path: &str) -> ZipArchive<BufReader<File>> {
    let file = File::open(&path).unwrap_or_else(|_| panic!("Failed to open ZIP file '{}'.", path));
    let reader = BufReader::new(file);
    ZipArchive::new(reader).unwrap_or_else(|_| panic!("Failed to parse ZIP file: {}", path))
}

pub fn producer(
    tmp_dir: &Path,
    paths: &[String],
    sender: &JobSender,
    ignore_orphan_gcno: bool,
    is_llvm: bool,
) -> Option<Vec<u8>> {
    let mut archives: Vec<Archive> = Vec::new();
    let mut plain_files: Vec<PathBuf> = Vec::new();

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
            if full_path.is_dir() {
                archives.push(Archive {
                    name: path.to_string(),
                    item: RefCell::new(ArchiveType::Dir(full_path)),
                });
            } else if let Some(ext) = full_path.clone().extension() {
                let ext = ext.to_str().unwrap();
                if ext == "info" || ext == "json" || ext == "xml" {
                    plain_files.push(full_path);
                } else {
                    panic!(
                        "Cannot load file '{:?}': it isn't a .info, a .json or a .xml file.",
                        full_path
                    );
                }
            } else {
                panic!("Cannot load file '{:?}': it isn't a directory, a .info, a .json or a .xml file.", full_path);
            }
        }
    }

    if !plain_files.is_empty() {
        archives.push(Archive {
            name: "plain files".to_string(),
            item: RefCell::new(ArchiveType::Plain(plain_files)),
        });
    }

    let gcno_stems_archives: RefCell<FxHashMap<GCNOStem, &Archive>> =
        RefCell::new(FxHashMap::default());
    let gcda_stems_archives: RefCell<FxHashMap<String, Vec<&Archive>>> =
        RefCell::new(FxHashMap::default());
    let infos: RefCell<FxHashMap<String, Vec<&Archive>>> = RefCell::new(FxHashMap::default());
    let xmls: RefCell<FxHashMap<String, Vec<&Archive>>> = RefCell::new(FxHashMap::default());
    let linked_files_maps: RefCell<FxHashMap<String, &Archive>> =
        RefCell::new(FxHashMap::default());

    for archive in &mut archives {
        archive.explore(
            &gcno_stems_archives,
            &gcda_stems_archives,
            &infos,
            &xmls,
            &linked_files_maps,
            is_llvm,
        );
    }

    assert!(
        !(gcno_stems_archives.borrow().is_empty()
            && infos.borrow().is_empty()
            && xmls.borrow().is_empty()),
        "No input files found"
    );

    file_content_producer(&infos.into_inner(), sender, ItemFormat::INFO);
    file_content_producer(&xmls.into_inner(), sender, ItemFormat::JACOCO_XML);
    gcno_gcda_producer(
        tmp_dir,
        &gcno_stems_archives.into_inner(),
        &gcda_stems_archives.into_inner(),
        sender,
        ignore_orphan_gcno,
    );

    get_mapping(&linked_files_maps.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam::crossbeam_channel::unbounded;
    use serde_json::{self, Value};

    fn check_produced(
        directory: PathBuf,
        receiver: &JobReceiver,
        expected: Vec<(ItemFormat, bool, &str, bool)>,
    ) {
        let mut vec: Vec<Option<WorkItem>> = Vec::new();

        while let Ok(elem) = receiver.try_recv() {
            vec.push(elem);
        }

        for elem in &expected {
            assert!(
                vec.iter().any(|x| {
                    if !x.is_some() {
                        return false;
                    }

                    let x = x.as_ref().unwrap();

                    if x.format != elem.0 {
                        return false;
                    }

                    match x.item {
                        ItemType::Content(_) => !elem.1,
                        ItemType::Path((_, ref p)) => elem.1 && p.ends_with(elem.2),
                        ItemType::Buffers(ref b) => b.stem.replace("\\", "/").ends_with(elem.2),
                    }
                }),
                "Missing {:?}",
                elem
            );
        }

        for v in &vec {
            let v = v.as_ref().unwrap();
            assert!(
                expected.iter().any(|x| {
                    if v.format != x.0 {
                        return false;
                    }

                    match v.item {
                        ItemType::Content(_) => !x.1,
                        ItemType::Path((_, ref p)) => x.1 && p.ends_with(x.2),
                        ItemType::Buffers(ref b) => b.stem.replace("\\", "/").ends_with(x.2),
                    }
                }),
                "Unexpected {:?}",
                v
            );
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
                let gcda =
                    p.with_file_name(format!("{}.gcda", p.file_stem().unwrap().to_str().unwrap()));
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
        let (sender, receiver) = unbounded();

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        let mapping = producer(&tmp_path, &["test".to_string()], &sender, false, false);

        let expected = vec![
            (ItemFormat::GCNO, true, "Platform_1.gcno", true),
            (
                ItemFormat::GCNO,
                true,
                "sub2/RootAccessibleWrap_1.gcno",
                true,
            ),
            (ItemFormat::GCNO, true, "nsMaiInterfaceValue_1.gcno", true),
            (ItemFormat::GCNO, true, "sub/prova2_1.gcno", true),
            (
                ItemFormat::GCNO,
                true,
                "nsMaiInterfaceDocument_1.gcno",
                true,
            ),
            (
                ItemFormat::GCNO,
                true,
                "Unified_cpp_netwerk_base0_1.gcno",
                true,
            ),
            (ItemFormat::GCNO, true, "prova_1.gcno", true),
            (ItemFormat::GCNO, true, "nsGnomeModule_1.gcno", true),
            (ItemFormat::GCNO, true, "negative_counts_1.gcno", true),
            (ItemFormat::GCNO, true, "64bit_count_1.gcno", true),
            (ItemFormat::GCNO, true, "no_gcda/main_1.gcno", false),
            (ItemFormat::GCNO, true, "only_one_gcda/main_1.gcno", true),
            (ItemFormat::GCNO, true, "only_one_gcda/orphan_1.gcno", false),
            (
                ItemFormat::GCNO,
                true,
                "gcno_symlink/gcda/main_1.gcno",
                true,
            ),
            (
                ItemFormat::GCNO,
                true,
                "gcno_symlink/gcno/main_1.gcno",
                false,
            ),
            (
                ItemFormat::GCNO,
                false,
                "rust/generics_with_two_parameters",
                true,
            ),
            (ItemFormat::INFO, false, "1494603973-2977-7.info", false),
            (ItemFormat::INFO, false, "prova.info", false),
            (ItemFormat::INFO, false, "prova_fn_with_commas.info", false),
            (ItemFormat::INFO, false, "empty_line.info", false),
            (ItemFormat::INFO, false, "invalid_DA_record.info", false),
            (
                ItemFormat::INFO,
                false,
                "relative_path/relative_path.info",
                false,
            ),
            (ItemFormat::GCNO, false, "llvm/file", true),
            (ItemFormat::GCNO, false, "llvm/file_branch", true),
            (ItemFormat::GCNO, false, "llvm/reader", true),
            (
                ItemFormat::JACOCO_XML,
                false,
                "jacoco/basic-jacoco.xml",
                false,
            ),
            (
                ItemFormat::JACOCO_XML,
                false,
                "jacoco/inner-classes.xml",
                false,
            ),
            (
                ItemFormat::JACOCO_XML,
                false,
                "jacoco/multiple-top-level-classes.xml",
                false,
            ),
            (
                ItemFormat::JACOCO_XML,
                false,
                "jacoco/full-junit4-report-multiple-top-level-classes.xml",
                false,
            ),
        ];

        check_produced(tmp_path, &receiver, expected);
        assert!(mapping.is_some());
        let mapping: Value = serde_json::from_slice(&mapping.unwrap()).unwrap();
        assert_eq!(
            mapping
                .get("dist/include/zlib.h")
                .unwrap()
                .as_str()
                .unwrap(),
            "modules/zlib/src/zlib.h"
        );
    }

    #[test]
    fn test_dir_producer_multiple_directories() {
        let (sender, receiver) = unbounded();

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        let mapping = producer(
            &tmp_path,
            &["test/sub".to_string(), "test/sub2".to_string()],
            &sender,
            false,
            false,
        );

        let expected = vec![
            (ItemFormat::GCNO, true, "RootAccessibleWrap_1.gcno", true),
            (ItemFormat::GCNO, true, "prova2_1.gcno", true),
        ];

        check_produced(tmp_path, &receiver, expected);
        assert!(mapping.is_none());
    }

    #[test]
    fn test_dir_producer_directory_with_gcno_symlinks() {
        let (sender, receiver) = unbounded();

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        let mapping = producer(
            &tmp_path,
            &["test/gcno_symlink/gcda".to_string()],
            &sender,
            false,
            false,
        );

        let expected = vec![(ItemFormat::GCNO, true, "main_1.gcno", true)];

        check_produced(tmp_path, &receiver, expected);
        assert!(mapping.is_none());
    }

    #[test]
    fn test_dir_producer_directory_with_no_gcda() {
        let (sender, receiver) = unbounded();

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        let mapping = producer(
            &tmp_path,
            &["test/only_one_gcda".to_string()],
            &sender,
            false,
            false,
        );

        let expected = vec![
            (ItemFormat::GCNO, true, "main_1.gcno", true),
            (ItemFormat::GCNO, true, "orphan_1.gcno", false),
        ];

        check_produced(tmp_path, &receiver, expected);
        assert!(mapping.is_none());
    }

    #[test]
    fn test_dir_producer_directory_with_no_gcda_ignore_orphan_gcno() {
        let (sender, receiver) = unbounded();

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        let mapping = producer(
            &tmp_path,
            &["test/only_one_gcda".to_string()],
            &sender,
            true,
            false,
        );

        let expected = vec![(ItemFormat::GCNO, true, "main_1.gcno", true)];

        check_produced(tmp_path, &receiver, expected);
        assert!(mapping.is_none());
    }

    #[test]
    fn test_zip_producer_with_gcda_dir() {
        let (sender, receiver) = unbounded();

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        let mapping = producer(
            &tmp_path,
            &[
                "test/zip_dir/gcno.zip".to_string(),
                "test/zip_dir".to_string(),
            ],
            &sender,
            false,
            false,
        );

        let expected = vec![
            (ItemFormat::GCNO, true, "Platform_1.gcno", true),
            (
                ItemFormat::GCNO,
                true,
                "sub2/RootAccessibleWrap_1.gcno",
                true,
            ),
            (ItemFormat::GCNO, true, "nsMaiInterfaceValue_1.gcno", true),
            (ItemFormat::GCNO, true, "sub/prova2_1.gcno", true),
            (
                ItemFormat::GCNO,
                true,
                "nsMaiInterfaceDocument_1.gcno",
                true,
            ),
            (ItemFormat::GCNO, true, "nsGnomeModule_1.gcno", true),
        ];

        check_produced(tmp_path, &receiver, expected);
        assert!(mapping.is_some());
        let mapping: Value = serde_json::from_slice(&mapping.unwrap()).unwrap();
        assert_eq!(
            mapping
                .get("dist/include/zlib.h")
                .unwrap()
                .as_str()
                .unwrap(),
            "modules/zlib/src/zlib.h"
        );
    }

    // Test extracting multiple gcda archives.
    #[test]
    fn test_zip_producer_multiple_gcda_archives() {
        let (sender, receiver) = unbounded();

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        let mapping = producer(
            &tmp_path,
            &[
                "test/gcno.zip".to_string(),
                "test/gcda1.zip".to_string(),
                "test/gcda2.zip".to_string(),
            ],
            &sender,
            false,
            false,
        );

        let expected = vec![
            (ItemFormat::GCNO, true, "Platform_1.gcno", true),
            (
                ItemFormat::GCNO,
                true,
                "sub2/RootAccessibleWrap_1.gcno",
                true,
            ),
            (ItemFormat::GCNO, true, "nsMaiInterfaceValue_1.gcno", true),
            (ItemFormat::GCNO, true, "sub/prova2_1.gcno", true),
            (
                ItemFormat::GCNO,
                true,
                "nsMaiInterfaceDocument_1.gcno",
                true,
            ),
            (ItemFormat::GCNO, true, "nsGnomeModule_1.gcno", true),
            (ItemFormat::GCNO, true, "nsMaiInterfaceValue_2.gcno", true),
            (
                ItemFormat::GCNO,
                true,
                "nsMaiInterfaceDocument_2.gcno",
                true,
            ),
            (ItemFormat::GCNO, true, "nsGnomeModule_2.gcno", true),
            (ItemFormat::GCNO, true, "sub/prova2_2.gcno", true),
        ];

        check_produced(tmp_path, &receiver, expected);
        assert!(mapping.is_some());
        let mapping: Value = serde_json::from_slice(&mapping.unwrap()).unwrap();
        assert_eq!(
            mapping
                .get("dist/include/zlib.h")
                .unwrap()
                .as_str()
                .unwrap(),
            "modules/zlib/src/zlib.h"
        );
    }

    // Test extracting gcno with no path mapping.
    #[test]
    fn test_zip_producer_gcno_with_no_path_mapping() {
        let (sender, receiver) = unbounded();

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        let mapping = producer(
            &tmp_path,
            &[
                "test/gcno_no_path_mapping.zip".to_string(),
                "test/gcda1.zip".to_string(),
            ],
            &sender,
            false,
            false,
        );

        let expected = vec![
            (ItemFormat::GCNO, true, "Platform_1.gcno", true),
            (
                ItemFormat::GCNO,
                true,
                "sub2/RootAccessibleWrap_1.gcno",
                true,
            ),
            (ItemFormat::GCNO, true, "nsMaiInterfaceValue_1.gcno", true),
            (ItemFormat::GCNO, true, "sub/prova2_1.gcno", true),
            (
                ItemFormat::GCNO,
                true,
                "nsMaiInterfaceDocument_1.gcno",
                true,
            ),
            (ItemFormat::GCNO, true, "nsGnomeModule_1.gcno", true),
        ];

        check_produced(tmp_path, &receiver, expected);
        assert!(mapping.is_none());
    }

    // Test calling zip_producer with a different order of zip files.
    #[test]
    fn test_zip_producer_different_order_of_zip_files() {
        let (sender, receiver) = unbounded();

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        producer(
            &tmp_path,
            &[
                "test/gcda1.zip".to_string(),
                "test/gcno.zip".to_string(),
                "test/gcda2.zip".to_string(),
            ],
            &sender,
            false,
            false,
        );

        let expected = vec![
            (ItemFormat::GCNO, true, "Platform_1.gcno", true),
            (
                ItemFormat::GCNO,
                true,
                "sub2/RootAccessibleWrap_1.gcno",
                true,
            ),
            (ItemFormat::GCNO, true, "nsMaiInterfaceValue_1.gcno", true),
            (ItemFormat::GCNO, true, "sub/prova2_1.gcno", true),
            (
                ItemFormat::GCNO,
                true,
                "nsMaiInterfaceDocument_1.gcno",
                true,
            ),
            (ItemFormat::GCNO, true, "nsGnomeModule_1.gcno", true),
            (ItemFormat::GCNO, true, "nsMaiInterfaceValue_2.gcno", true),
            (
                ItemFormat::GCNO,
                true,
                "nsMaiInterfaceDocument_2.gcno",
                true,
            ),
            (ItemFormat::GCNO, true, "nsGnomeModule_2.gcno", true),
            (ItemFormat::GCNO, true, "sub/prova2_2.gcno", true),
        ];

        check_produced(tmp_path, &receiver, expected);
    }

    // Test extracting info files.
    #[test]
    fn test_zip_producer_info_files() {
        let (sender, receiver) = unbounded();

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        producer(
            &tmp_path,
            &["test/info1.zip".to_string(), "test/info2.zip".to_string()],
            &sender,
            false,
            false,
        );

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

        check_produced(tmp_path, &receiver, expected);
    }

    // Test extracting jacoco report XML files.
    #[test]
    fn test_zip_producer_jacoco_xml_files() {
        let (sender, receiver) = unbounded();

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        producer(
            &tmp_path,
            &[
                "test/jacoco1.zip".to_string(),
                "test/jacoco2.zip".to_string(),
            ],
            &sender,
            false,
            false,
        );

        let expected = vec![
            (
                ItemFormat::JACOCO_XML,
                false,
                "jacoco/basic-jacoco.xml",
                true,
            ),
            (ItemFormat::JACOCO_XML, false, "inner-classes.xml", true),
        ];

        check_produced(tmp_path, &receiver, expected);
    }

    // Test extracting both jacoco xml and info files.
    #[test]
    fn test_zip_producer_both_info_and_jacoco_xml() {
        let (sender, receiver) = unbounded();

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        producer(
            &tmp_path,
            &[
                "test/jacoco1.zip".to_string(),
                "test/jacoco2.zip".to_string(),
                "test/info1.zip".to_string(),
                "test/info2.zip".to_string(),
            ],
            &sender,
            false,
            false,
        );

        let expected = vec![
            (
                ItemFormat::JACOCO_XML,
                false,
                "jacoco/basic-jacoco.xml",
                true,
            ),
            (ItemFormat::JACOCO_XML, false, "inner-classes.xml", true),
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

        check_produced(tmp_path, &receiver, expected);
    }

    // Test extracting both info and gcno/gcda files.
    #[test]
    fn test_zip_producer_both_info_and_gcnogcda_files() {
        let (sender, receiver) = unbounded();

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        producer(
            &tmp_path,
            &[
                "test/gcno.zip".to_string(),
                "test/gcda1.zip".to_string(),
                "test/info1.zip".to_string(),
                "test/info2.zip".to_string(),
            ],
            &sender,
            false,
            false,
        );

        let expected = vec![
            (ItemFormat::GCNO, true, "Platform_1.gcno", true),
            (
                ItemFormat::GCNO,
                true,
                "sub2/RootAccessibleWrap_1.gcno",
                true,
            ),
            (ItemFormat::GCNO, true, "nsMaiInterfaceValue_1.gcno", true),
            (ItemFormat::GCNO, true, "sub/prova2_1.gcno", true),
            (
                ItemFormat::GCNO,
                true,
                "nsMaiInterfaceDocument_1.gcno",
                true,
            ),
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

        check_produced(tmp_path, &receiver, expected);
    }

    // Test extracting gcno with no associated gcda.
    #[test]
    fn test_zip_producer_gcno_with_no_associated_gcda() {
        let (sender, receiver) = unbounded();

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        let mapping = producer(
            &tmp_path,
            &[
                "test/no_gcda/main.gcno.zip".to_string(),
                "test/no_gcda/empty.gcda.zip".to_string(),
            ],
            &sender,
            false,
            false,
        );

        let expected = vec![(ItemFormat::GCNO, true, "main_1.gcno", false)];

        check_produced(tmp_path, &receiver, expected);
        assert!(mapping.is_none());
    }

    // Test extracting gcno with an associated gcda file in only one zip file.
    #[test]
    fn test_zip_producer_gcno_with_associated_gcda_in_only_one_archive() {
        let (sender, receiver) = unbounded();

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        let mapping = producer(
            &tmp_path,
            &[
                "test/no_gcda/main.gcno.zip".to_string(),
                "test/no_gcda/empty.gcda.zip".to_string(),
                "test/no_gcda/main.gcda.zip".to_string(),
            ],
            &sender,
            false,
            false,
        );

        let expected = vec![(ItemFormat::GCNO, true, "main_1.gcno", true)];

        check_produced(tmp_path, &receiver, expected);
        assert!(mapping.is_none());
    }

    // Test passing a gcda archive with no gcno archive makes zip_producer fail.
    #[test]
    #[should_panic]
    fn test_zip_producer_with_gcda_archive_and_no_gcno_archive() {
        let (sender, _) = unbounded();

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        producer(
            &tmp_path,
            &["test/no_gcda/main.gcda.zip".to_string()],
            &sender,
            false,
            false,
        );
    }

    // Test extracting gcno/gcda archives, where a gcno file exist with no matching gcda file.
    #[test]
    fn test_zip_producer_no_matching_gcno() {
        let (sender, receiver) = unbounded();

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        producer(
            &tmp_path,
            &["test/gcno.zip".to_string(), "test/gcda2.zip".to_string()],
            &sender,
            false,
            false,
        );

        let expected = vec![
            (ItemFormat::GCNO, true, "Platform_1.gcno", false),
            (
                ItemFormat::GCNO,
                true,
                "sub2/RootAccessibleWrap_1.gcno",
                false,
            ),
            (ItemFormat::GCNO, true, "nsMaiInterfaceValue_1.gcno", true),
            (ItemFormat::GCNO, true, "sub/prova2_1.gcno", true),
            (
                ItemFormat::GCNO,
                true,
                "nsMaiInterfaceDocument_1.gcno",
                true,
            ),
            (ItemFormat::GCNO, true, "nsGnomeModule_1.gcno", true),
        ];

        check_produced(tmp_path, &receiver, expected);
    }

    // Test extracting gcno/gcda archives, where a gcno file exist with no matching gcda file.
    // The gcno file should be produced only once, not twice.
    #[test]
    fn test_zip_producer_no_matching_gcno_two_gcda_archives() {
        let (sender, receiver) = unbounded();

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        producer(
            &tmp_path,
            &[
                "test/gcno.zip".to_string(),
                "test/gcda2.zip".to_string(),
                "test/gcda2.zip".to_string(),
            ],
            &sender,
            false,
            false,
        );

        let expected = vec![
            (ItemFormat::GCNO, true, "Platform_1.gcno", false),
            (
                ItemFormat::GCNO,
                true,
                "sub2/RootAccessibleWrap_1.gcno",
                false,
            ),
            (ItemFormat::GCNO, true, "nsMaiInterfaceValue_1.gcno", true),
            (ItemFormat::GCNO, true, "nsMaiInterfaceValue_2.gcno", true),
            (ItemFormat::GCNO, true, "sub/prova2_1.gcno", true),
            (ItemFormat::GCNO, true, "sub/prova2_2.gcno", true),
            (
                ItemFormat::GCNO,
                true,
                "nsMaiInterfaceDocument_1.gcno",
                true,
            ),
            (
                ItemFormat::GCNO,
                true,
                "nsMaiInterfaceDocument_2.gcno",
                true,
            ),
            (ItemFormat::GCNO, true, "nsGnomeModule_1.gcno", true),
            (ItemFormat::GCNO, true, "nsGnomeModule_2.gcno", true),
        ];

        check_produced(tmp_path, &receiver, expected);
    }

    // Test extracting gcno/gcda archives, where a gcno file exist with no matching gcda file and ignore orphan gcno files.
    #[test]
    fn test_zip_producer_no_matching_gcno_ignore_orphan_gcno() {
        let (sender, receiver) = unbounded();

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        producer(
            &tmp_path,
            &["test/gcno.zip".to_string(), "test/gcda2.zip".to_string()],
            &sender,
            true,
            false,
        );

        let expected = vec![
            (ItemFormat::GCNO, true, "nsMaiInterfaceValue_1.gcno", true),
            (ItemFormat::GCNO, true, "sub/prova2_1.gcno", true),
            (
                ItemFormat::GCNO,
                true,
                "nsMaiInterfaceDocument_1.gcno",
                true,
            ),
            (ItemFormat::GCNO, true, "nsGnomeModule_1.gcno", true),
        ];

        check_produced(tmp_path, &receiver, expected);
    }

    // Test extracting gcno/gcda archives, where a gcno file exist with no matching gcda file and ignore orphan gcno files.
    #[test]
    fn test_zip_producer_no_matching_gcno_two_gcda_archives_ignore_orphan_gcno() {
        let (sender, receiver) = unbounded();

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        producer(
            &tmp_path,
            &[
                "test/gcno.zip".to_string(),
                "test/gcda2.zip".to_string(),
                "test/gcda2.zip".to_string(),
            ],
            &sender,
            true,
            false,
        );

        let expected = vec![
            (ItemFormat::GCNO, true, "nsMaiInterfaceValue_1.gcno", true),
            (ItemFormat::GCNO, true, "nsMaiInterfaceValue_2.gcno", true),
            (ItemFormat::GCNO, true, "sub/prova2_1.gcno", true),
            (ItemFormat::GCNO, true, "sub/prova2_2.gcno", true),
            (
                ItemFormat::GCNO,
                true,
                "nsMaiInterfaceDocument_1.gcno",
                true,
            ),
            (
                ItemFormat::GCNO,
                true,
                "nsMaiInterfaceDocument_2.gcno",
                true,
            ),
            (ItemFormat::GCNO, true, "nsGnomeModule_1.gcno", true),
            (ItemFormat::GCNO, true, "nsGnomeModule_2.gcno", true),
        ];

        check_produced(tmp_path, &receiver, expected);
    }

    #[test]
    fn test_zip_producer_llvm_buffers() {
        let (sender, receiver) = unbounded();

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        producer(
            &tmp_path,
            &[
                "test/llvm/gcno.zip".to_string(),
                "test/llvm/gcda1.zip".to_string(),
                "test/llvm/gcda2.zip".to_string(),
            ],
            &sender,
            true,
            true,
        );
        let gcno_buf: Vec<u8> = vec![
            111, 110, 99, 103, 42, 50, 48, 52, 74, 200, 254, 66, 0, 0, 0, 1, 9, 0, 0, 0, 0, 0, 0,
            0, 236, 217, 93, 255, 2, 0, 0, 0, 109, 97, 105, 110, 0, 0, 0, 0, 2, 0, 0, 0, 102, 105,
            108, 101, 46, 99, 0, 0, 1, 0, 0, 0, 0, 0, 65, 1, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 67, 1, 3, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 67, 1, 3,
            0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 69, 1, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 69, 1, 8, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 102,
            105, 108, 101, 46, 99, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0,
        ];
        let gcda1_buf: Vec<u8> = vec![
            97, 100, 99, 103, 42, 50, 48, 52, 74, 200, 254, 66, 0, 0, 0, 1, 5, 0, 0, 0, 0, 0, 0, 0,
            236, 217, 93, 255, 2, 0, 0, 0, 109, 97, 105, 110, 0, 0, 0, 0, 0, 0, 161, 1, 4, 0, 0, 0,
            1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 161, 9, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 163, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        let gcda2_buf: Vec<u8> = vec![
            97, 100, 99, 103, 42, 50, 48, 52, 74, 200, 254, 66, 0, 0, 0, 1, 5, 0, 0, 0, 0, 0, 0, 0,
            236, 217, 93, 255, 2, 0, 0, 0, 109, 97, 105, 110, 0, 0, 0, 0, 0, 0, 161, 1, 4, 0, 0, 0,
            1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 161, 9, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 163, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];

        while let Ok(elem) = receiver.try_recv() {
            let elem = elem.unwrap();
            if let ItemType::Buffers(buffers) = elem.item {
                let stem = PathBuf::from(buffers.stem);
                let stem = stem.file_stem().expect("Unable to get file_stem");
                if stem == "file" {
                    assert_eq!(buffers.gcno_buf, gcno_buf);
                    assert_eq!(buffers.gcda_buf, vec![gcda1_buf.clone(), gcda2_buf.clone()]);
                } else {
                    assert!(false, "Unexpected file: {:?}", stem);
                }
            } else {
                assert!(false, "Buffers expected");
            }
        }
    }

    #[test]
    fn test_plain_producer() {
        let (sender, receiver) = unbounded();

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        let json_path = "test/linked-files-map.json";
        let mapping = producer(
            &tmp_path,
            &["test/prova.info".to_string(), json_path.to_string()],
            &sender,
            true,
            false,
        );

        assert!(mapping.is_some());
        let mapping = mapping.unwrap();

        let expected = vec![(ItemFormat::INFO, false, "prova_1.info", true)];

        if let Ok(mut reader) = File::open(json_path) {
            let mut json = Vec::new();
            reader.read_to_end(&mut json).unwrap();
            assert_eq!(json, mapping);
        } else {
            assert!(false, format!("Failed to read the file: {}", json_path));
        }

        check_produced(tmp_path, &receiver, expected);
    }

    #[test]
    #[should_panic]
    fn test_plain_producer_with_gcno() {
        let (sender, _) = unbounded();

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        producer(
            &tmp_path,
            &["sub2/RootAccessibleWrap_1.gcno".to_string()],
            &sender,
            true,
            false,
        );
    }

    #[test]
    #[should_panic]
    fn test_plain_producer_with_gcda() {
        let (sender, _) = unbounded();

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        producer(
            &tmp_path,
            &["./test/llvm/file.gcda".to_string()],
            &sender,
            true,
            false,
        );
    }

    #[test]
    fn test_jacoco_files() {
        assert!(
            Archive::check_file(
                FilePath::Path(&PathBuf::from("./test/jacoco/basic-report.xml")),
                &Archive::is_jacoco
            ),
            "A Jacoco XML file expected"
        );
        assert!(
            Archive::check_file(
                FilePath::Path(&PathBuf::from(
                    "./test/jacoco/full-junit4-report-multiple-top-level-classes.xml"
                )),
                &Archive::is_jacoco
            ),
            "A Jacoco XML file expected"
        );
        assert!(
            Archive::check_file(
                FilePath::Path(&PathBuf::from("./test/jacoco/inner-classes.xml")),
                &Archive::is_jacoco
            ),
            "A Jacoco XML file expected"
        );
        assert!(
            Archive::check_file(
                FilePath::Path(&PathBuf::from(
                    "./test/jacoco/multiple-top-level-classes.xml"
                )),
                &Archive::is_jacoco
            ),
            "A Jacoco XML file expected"
        );
        assert!(
            !Archive::check_file(
                FilePath::Path(&PathBuf::from("./test/jacoco/not_jacoco_file.xml")),
                &Archive::is_jacoco
            ),
            "Not a Jacoco XML file expected"
        );
    }

    #[test]
    fn test_info_files() {
        assert!(
            Archive::check_file(
                FilePath::Path(&PathBuf::from("./test/1494603973-2977-7.info")),
                &Archive::is_info
            ),
            "An info file expected"
        );
        assert!(
            Archive::check_file(
                FilePath::Path(&PathBuf::from("./test/empty_line.info")),
                &Archive::is_info
            ),
            "An info file expected"
        );
        assert!(
            Archive::check_file(
                FilePath::Path(&PathBuf::from("./test/relative_path/relative_path.info")),
                &Archive::is_info
            ),
            "An info file expected"
        );
        assert!(
            !Archive::check_file(
                FilePath::Path(&PathBuf::from("./test/not_info_file.info")),
                &Archive::is_info
            ),
            "Not an info file expected"
        );
    }
}
