use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::env;
use std::fs::{self, File};
use std::io::{self, BufReader, Read};
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
fn clean_path(path: &Path) -> String {
    path.to_str().unwrap().to_string()
}

#[cfg(windows)]
fn clean_path(path: &Path) -> String {
    path.to_str().unwrap().to_string().replace("\\", "/")
}

impl Archive {
    fn insert_vec<'a>(
        &'a self,
        filename: String,
        map: &RefCell<FxHashMap<String, Vec<&'a Archive>>>,
    ) {
        let mut map = map.borrow_mut();
        map.entry(filename)
            .or_insert_with(|| Vec::with_capacity(1))
            .push(self);
    }

    fn handle_file<'a>(
        &'a self,
        file: Option<&mut impl Read>,
        path: &Path,
        gcno_stem_archives: &RefCell<FxHashMap<GCNOStem, &'a Archive>>,
        gcda_stem_archives: &RefCell<FxHashMap<String, Vec<&'a Archive>>>,
        profraws: &RefCell<FxHashMap<String, Vec<&'a Archive>>>,
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
                "profraw" => {
                    let filename = clean_path(path);
                    self.insert_vec(filename, profraws);
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

    fn is_gcno_llvm(reader: &mut dyn Read) -> bool {
        let mut bytes: [u8; 8] = [0; 8];
        reader.read_exact(&mut bytes).is_ok()
            && &bytes[..5] == b"oncg*"
            && (&bytes[5..] == b"204" || &bytes[5..] == b"804")
    }

    fn is_jacoco(reader: &mut dyn Read) -> bool {
        let mut bytes: [u8; 256] = [0; 256];
        if reader.read_exact(&mut bytes).is_ok() {
            return match String::from_utf8(bytes.to_vec()) {
                Ok(s) => s.contains("-//JACOCO//DTD"),
                Err(_) => false,
            };
        }
        false
    }

    fn is_info(reader: &mut dyn Read) -> bool {
        let mut bytes: [u8; 3] = [0; 3];
        reader.read_exact(&mut bytes).is_ok()
            && (bytes == [b'T', b'N', b':'] || bytes == [b'S', b'F', b':'])
    }

    fn check_file(file: Option<&mut impl Read>, checker: &dyn Fn(&mut dyn Read) -> bool) -> bool {
        file.map_or(false, |f| checker(f))
    }

    pub fn get_name(&self) -> &String {
        &self.name
    }

    pub fn explore<'a>(
        &'a mut self,
        gcno_stem_archives: &RefCell<FxHashMap<GCNOStem, &'a Archive>>,
        gcda_stem_archives: &RefCell<FxHashMap<String, Vec<&'a Archive>>>,
        profraws: &RefCell<FxHashMap<String, Vec<&'a Archive>>>,
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
                        Some(&mut file),
                        &path,
                        gcno_stem_archives,
                        gcda_stem_archives,
                        profraws,
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
                        let mut file = File::open(full_path).ok();
                        let path = full_path.strip_prefix(dir).unwrap();
                        self.handle_file(
                            file.as_mut(),
                            &path.to_path_buf(),
                            gcno_stem_archives,
                            gcda_stem_archives,
                            profraws,
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
                    let mut file = File::open(full_path).ok();
                    self.handle_file(
                        file.as_mut(),
                        full_path,
                        gcno_stem_archives,
                        gcda_stem_archives,
                        profraws,
                        infos,
                        xmls,
                        linked_files_maps,
                        is_llvm,
                    );
                }
            }
        }
    }

    pub fn read(&self, name: &str) -> Option<Vec<u8>> {
        match *self.item.borrow_mut() {
            ArchiveType::Zip(ref mut zip) => {
                let mut zip = zip.borrow_mut();
                let zipfile = zip.by_name(name);
                match zipfile {
                    Ok(mut f) => {
                        let mut buf = Vec::with_capacity(f.size() as usize + 1);
                        f.read_to_end(&mut buf).expect("Failed to read gcda file");
                        Some(buf)
                    }
                    Err(_) => None,
                }
            }
            ArchiveType::Dir(ref dir) => {
                let path = dir.join(name);
                if let Ok(metadata) = fs::metadata(&path) {
                    match File::open(path) {
                        Ok(mut f) => {
                            let mut buf = Vec::with_capacity(metadata.len() as usize + 1);
                            f.read_to_end(&mut buf).expect("Failed to read gcda file");
                            Some(buf)
                        }
                        Err(_) => None,
                    }
                } else {
                    None
                }
            }
            ArchiveType::Plain(_) => {
                if let Ok(metadata) = fs::metadata(name) {
                    match File::open(name) {
                        Ok(mut f) => {
                            let mut buf = Vec::with_capacity(metadata.len() as usize + 1);
                            f.read_to_end(&mut buf)
                                .unwrap_or_else(|_| panic!("Failed to read file: {}.", name));
                            Some(buf)
                        }
                        Err(_) => None,
                    }
                } else {
                    None
                }
            }
        }
    }

    pub fn extract(&self, name: &str, path: &Path) -> bool {
        let dest_parent = path.parent().unwrap();
        if !dest_parent.exists() {
            fs::create_dir_all(dest_parent).expect("Cannot create parent directory");
        }

        match *self.item.borrow_mut() {
            ArchiveType::Zip(ref mut zip) => {
                let mut zip = zip.borrow_mut();
                let zipfile = zip.by_name(name);
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

                crate::symlink::symlink_file(&src_path, path).unwrap_or_else(|_| {
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
                format: ItemFormat::Gcno,
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
                let mut gcda_buffers: Vec<Vec<u8>> = Vec::with_capacity(gcda_archives.len());
                if let Some(gcno_buffer) = gcno_archive.read(&gcno) {
                    for gcda_archive in gcda_archives {
                        let gcda = format!("{}.gcda", stem).to_string();
                        if let Some(gcda_buf) = gcda_archive.read(&gcda) {
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
                }
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
                if let Some(gcno_buf) = gcno_archive.read(&gcno) {
                    send_job(
                        ItemType::Buffers(GcnoBuffers {
                            stem: stem.clone(),
                            gcno_buf,
                            gcda_buf: Vec::new(),
                        }),
                        gcno_archive.get_name().to_string(),
                    );
                }
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

fn profraw_producer(
    tmp_dir: &Path,
    profraws: &FxHashMap<String, Vec<&Archive>>,
    sender: &JobSender,
) {
    if profraws.is_empty() {
        return;
    }

    let mut profraw_paths = Vec::new();

    for (name, archives) in profraws {
        let path = PathBuf::from(name);
        let stem = clean_path(&path.with_extension(""));

        // TODO: If there is only one archive and it is not a zip, we don't need to "extract".

        for (num, &archive) in archives.iter().enumerate() {
            let profraw_path = if let ArchiveType::Plain(_) = *archive.item.borrow() {
                Some(path.clone())
            } else {
                None
            };

            let profraw_path = if let Some(profraw_path) = profraw_path {
                profraw_path
            } else {
                let tmp_path = tmp_dir.join(format!("{}_{}.profraw", stem, num + 1));
                archive.extract(name, &tmp_path);
                tmp_path
            };

            profraw_paths.push(profraw_path);
        }
    }

    sender
        .send(Some(WorkItem {
            format: ItemFormat::Profraw,
            item: ItemType::Paths(profraw_paths),
            name: "profraws".to_string(),
        }))
        .unwrap()
}

fn file_content_producer(
    files: &FxHashMap<String, Vec<&Archive>>,
    sender: &JobSender,
    item_format: ItemFormat,
) {
    for (name, archives) in files {
        for archive in archives {
            if let Some(buffer) = archive.read(name) {
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
}

pub fn get_mapping(linked_files_maps: &FxHashMap<String, &Archive>) -> Option<Vec<u8>> {
    if let Some((name, archive)) = linked_files_maps.iter().next() {
        archive.read(name)
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
                if ext == "info" || ext == "json" || ext == "xml" || ext == "profraw" {
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
    let profraws: RefCell<FxHashMap<String, Vec<&Archive>>> = RefCell::new(FxHashMap::default());
    let infos: RefCell<FxHashMap<String, Vec<&Archive>>> = RefCell::new(FxHashMap::default());
    let xmls: RefCell<FxHashMap<String, Vec<&Archive>>> = RefCell::new(FxHashMap::default());
    let linked_files_maps: RefCell<FxHashMap<String, &Archive>> =
        RefCell::new(FxHashMap::default());

    for archive in &mut archives {
        archive.explore(
            &gcno_stems_archives,
            &gcda_stems_archives,
            &profraws,
            &infos,
            &xmls,
            &linked_files_maps,
            is_llvm,
        );
    }

    assert!(
        !(gcno_stems_archives.borrow().is_empty()
            && profraws.borrow().is_empty()
            && infos.borrow().is_empty()
            && xmls.borrow().is_empty()),
        "No input files found"
    );

    file_content_producer(&infos.into_inner(), sender, ItemFormat::Info);
    file_content_producer(&xmls.into_inner(), sender, ItemFormat::JacocoXml);
    profraw_producer(tmp_dir, &profraws.into_inner(), sender);
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
    use crossbeam::channel::unbounded;
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
                        ItemType::Paths(ref paths) => paths.iter().any(|p| p.ends_with(elem.2)),
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
                        ItemType::Paths(ref paths) => paths.iter().any(|p| p.ends_with(x.2)),
                        ItemType::Buffers(ref b) => b.stem.replace("\\", "/").ends_with(x.2),
                    }
                }),
                "Unexpected {:?}",
                v
            );
        }

        // Make sure we haven't generated duplicated entries.
        assert!(vec.len() <= expected.len());

        // Assert file exists and file with the same name but with extension .gcda exists.
        for x in expected.iter() {
            if !x.1 {
                continue;
            }

            let p = directory.join(x.2);
            assert!(p.exists(), "{} doesn't exist", p.display());
            if x.0 == ItemFormat::Gcno {
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
            (ItemFormat::Gcno, true, "Platform_1.gcno", true),
            (
                ItemFormat::Gcno,
                true,
                "sub2/RootAccessibleWrap_1.gcno",
                true,
            ),
            (ItemFormat::Gcno, true, "nsMaiInterfaceValue_1.gcno", true),
            (ItemFormat::Gcno, true, "sub/prova2_1.gcno", true),
            (
                ItemFormat::Gcno,
                true,
                "nsMaiInterfaceDocument_1.gcno",
                true,
            ),
            (
                ItemFormat::Gcno,
                true,
                "Unified_cpp_netwerk_base0_1.gcno",
                true,
            ),
            (ItemFormat::Gcno, true, "prova_1.gcno", true),
            (ItemFormat::Gcno, true, "nsGnomeModule_1.gcno", true),
            (ItemFormat::Gcno, true, "negative_counts_1.gcno", true),
            (ItemFormat::Gcno, true, "64bit_count_1.gcno", true),
            (ItemFormat::Gcno, true, "no_gcda/main_1.gcno", false),
            (ItemFormat::Gcno, true, "only_one_gcda/main_1.gcno", true),
            (ItemFormat::Gcno, true, "only_one_gcda/orphan_1.gcno", false),
            (
                ItemFormat::Gcno,
                true,
                "gcno_symlink/gcda/main_1.gcno",
                true,
            ),
            (
                ItemFormat::Gcno,
                true,
                "gcno_symlink/gcno/main_1.gcno",
                false,
            ),
            (
                ItemFormat::Gcno,
                false,
                "rust/generics_with_two_parameters",
                true,
            ),
            (ItemFormat::Gcno, true, "reader_gcc-6_1.gcno", true),
            (ItemFormat::Gcno, true, "reader_gcc-7_1.gcno", true),
            (ItemFormat::Gcno, true, "reader_gcc-8_1.gcno", true),
            (ItemFormat::Gcno, true, "reader_gcc-9_1.gcno", true),
            (ItemFormat::Gcno, true, "reader_gcc-10_1.gcno", true),
            (ItemFormat::Info, false, "1494603973-2977-7.info", false),
            (ItemFormat::Info, false, "prova.info", false),
            (ItemFormat::Info, false, "prova_fn_with_commas.info", false),
            (ItemFormat::Info, false, "empty_line.info", false),
            (ItemFormat::Info, false, "invalid_DA_record.info", false),
            (
                ItemFormat::Info,
                false,
                "relative_path/relative_path.info",
                false,
            ),
            (ItemFormat::Gcno, false, "llvm/file", true),
            (ItemFormat::Gcno, false, "llvm/file_branch", true),
            (ItemFormat::Gcno, false, "llvm/reader", true),
            (
                ItemFormat::JacocoXml,
                false,
                "jacoco/basic-jacoco.xml",
                false,
            ),
            (
                ItemFormat::JacocoXml,
                false,
                "jacoco/inner-classes.xml",
                false,
            ),
            (
                ItemFormat::JacocoXml,
                false,
                "jacoco/multiple-top-level-classes.xml",
                false,
            ),
            (
                ItemFormat::JacocoXml,
                false,
                "jacoco/full-junit4-report-multiple-top-level-classes.xml",
                false,
            ),
            (ItemFormat::Profraw, true, "default_1.profraw", false),
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
            (ItemFormat::Gcno, true, "RootAccessibleWrap_1.gcno", true),
            (ItemFormat::Gcno, true, "prova2_1.gcno", true),
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

        let expected = vec![(ItemFormat::Gcno, true, "main_1.gcno", true)];

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
            (ItemFormat::Gcno, true, "main_1.gcno", true),
            (ItemFormat::Gcno, true, "orphan_1.gcno", false),
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

        let expected = vec![(ItemFormat::Gcno, true, "main_1.gcno", true)];

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
            (ItemFormat::Gcno, true, "Platform_1.gcno", true),
            (
                ItemFormat::Gcno,
                true,
                "sub2/RootAccessibleWrap_1.gcno",
                true,
            ),
            (ItemFormat::Gcno, true, "nsMaiInterfaceValue_1.gcno", true),
            (ItemFormat::Gcno, true, "sub/prova2_1.gcno", true),
            (
                ItemFormat::Gcno,
                true,
                "nsMaiInterfaceDocument_1.gcno",
                true,
            ),
            (ItemFormat::Gcno, true, "nsGnomeModule_1.gcno", true),
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
            (ItemFormat::Gcno, true, "Platform_1.gcno", true),
            (
                ItemFormat::Gcno,
                true,
                "sub2/RootAccessibleWrap_1.gcno",
                true,
            ),
            (ItemFormat::Gcno, true, "nsMaiInterfaceValue_1.gcno", true),
            (ItemFormat::Gcno, true, "sub/prova2_1.gcno", true),
            (
                ItemFormat::Gcno,
                true,
                "nsMaiInterfaceDocument_1.gcno",
                true,
            ),
            (ItemFormat::Gcno, true, "nsGnomeModule_1.gcno", true),
            (ItemFormat::Gcno, true, "nsMaiInterfaceValue_2.gcno", true),
            (
                ItemFormat::Gcno,
                true,
                "nsMaiInterfaceDocument_2.gcno",
                true,
            ),
            (ItemFormat::Gcno, true, "nsGnomeModule_2.gcno", true),
            (ItemFormat::Gcno, true, "sub/prova2_2.gcno", true),
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
            (ItemFormat::Gcno, true, "Platform_1.gcno", true),
            (
                ItemFormat::Gcno,
                true,
                "sub2/RootAccessibleWrap_1.gcno",
                true,
            ),
            (ItemFormat::Gcno, true, "nsMaiInterfaceValue_1.gcno", true),
            (ItemFormat::Gcno, true, "sub/prova2_1.gcno", true),
            (
                ItemFormat::Gcno,
                true,
                "nsMaiInterfaceDocument_1.gcno",
                true,
            ),
            (ItemFormat::Gcno, true, "nsGnomeModule_1.gcno", true),
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
            (ItemFormat::Gcno, true, "Platform_1.gcno", true),
            (
                ItemFormat::Gcno,
                true,
                "sub2/RootAccessibleWrap_1.gcno",
                true,
            ),
            (ItemFormat::Gcno, true, "nsMaiInterfaceValue_1.gcno", true),
            (ItemFormat::Gcno, true, "sub/prova2_1.gcno", true),
            (
                ItemFormat::Gcno,
                true,
                "nsMaiInterfaceDocument_1.gcno",
                true,
            ),
            (ItemFormat::Gcno, true, "nsGnomeModule_1.gcno", true),
            (ItemFormat::Gcno, true, "nsMaiInterfaceValue_2.gcno", true),
            (
                ItemFormat::Gcno,
                true,
                "nsMaiInterfaceDocument_2.gcno",
                true,
            ),
            (ItemFormat::Gcno, true, "nsGnomeModule_2.gcno", true),
            (ItemFormat::Gcno, true, "sub/prova2_2.gcno", true),
        ];

        check_produced(tmp_path, &receiver, expected);
    }

    // Test extracting profraw files.
    #[test]
    fn test_zip_producer_profraw_files() {
        let (sender, receiver) = unbounded();

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        producer(
            &tmp_path,
            &[
                "test/profraw1.zip".to_string(),
                "test/profraw2.zip".to_string(),
            ],
            &sender,
            false,
            false,
        );

        let expected = vec![
            (ItemFormat::Profraw, true, "default_1.profraw", false),
            (ItemFormat::Profraw, true, "default_2.profraw", false),
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
            (ItemFormat::Info, false, "1494603967-2977-2_0.info", true),
            (ItemFormat::Info, false, "1494603967-2977-3_0.info", true),
            (ItemFormat::Info, false, "1494603967-2977-4_0.info", true),
            (ItemFormat::Info, false, "1494603968-2977-5_0.info", true),
            (ItemFormat::Info, false, "1494603972-2977-6_0.info", true),
            (ItemFormat::Info, false, "1494603973-2977-7_0.info", true),
            (ItemFormat::Info, false, "1494603967-2977-2_1.info", true),
            (ItemFormat::Info, false, "1494603967-2977-3_1.info", true),
            (ItemFormat::Info, false, "1494603967-2977-4_1.info", true),
            (ItemFormat::Info, false, "1494603968-2977-5_1.info", true),
            (ItemFormat::Info, false, "1494603972-2977-6_1.info", true),
            (ItemFormat::Info, false, "1494603973-2977-7_1.info", true),
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
                ItemFormat::JacocoXml,
                false,
                "jacoco/basic-jacoco.xml",
                true,
            ),
            (ItemFormat::JacocoXml, false, "inner-classes.xml", true),
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
                ItemFormat::JacocoXml,
                false,
                "jacoco/basic-jacoco.xml",
                true,
            ),
            (ItemFormat::JacocoXml, false, "inner-classes.xml", true),
            (ItemFormat::Info, false, "1494603967-2977-2_0.info", true),
            (ItemFormat::Info, false, "1494603967-2977-3_0.info", true),
            (ItemFormat::Info, false, "1494603967-2977-4_0.info", true),
            (ItemFormat::Info, false, "1494603968-2977-5_0.info", true),
            (ItemFormat::Info, false, "1494603972-2977-6_0.info", true),
            (ItemFormat::Info, false, "1494603973-2977-7_0.info", true),
            (ItemFormat::Info, false, "1494603967-2977-2_1.info", true),
            (ItemFormat::Info, false, "1494603967-2977-3_1.info", true),
            (ItemFormat::Info, false, "1494603967-2977-4_1.info", true),
            (ItemFormat::Info, false, "1494603968-2977-5_1.info", true),
            (ItemFormat::Info, false, "1494603972-2977-6_1.info", true),
            (ItemFormat::Info, false, "1494603973-2977-7_1.info", true),
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
            (ItemFormat::Gcno, true, "Platform_1.gcno", true),
            (
                ItemFormat::Gcno,
                true,
                "sub2/RootAccessibleWrap_1.gcno",
                true,
            ),
            (ItemFormat::Gcno, true, "nsMaiInterfaceValue_1.gcno", true),
            (ItemFormat::Gcno, true, "sub/prova2_1.gcno", true),
            (
                ItemFormat::Gcno,
                true,
                "nsMaiInterfaceDocument_1.gcno",
                true,
            ),
            (ItemFormat::Gcno, true, "nsGnomeModule_1.gcno", true),
            (ItemFormat::Info, false, "1494603967-2977-2_0.info", true),
            (ItemFormat::Info, false, "1494603967-2977-3_0.info", true),
            (ItemFormat::Info, false, "1494603967-2977-4_0.info", true),
            (ItemFormat::Info, false, "1494603968-2977-5_0.info", true),
            (ItemFormat::Info, false, "1494603972-2977-6_0.info", true),
            (ItemFormat::Info, false, "1494603973-2977-7_0.info", true),
            (ItemFormat::Info, false, "1494603967-2977-2_1.info", true),
            (ItemFormat::Info, false, "1494603967-2977-3_1.info", true),
            (ItemFormat::Info, false, "1494603967-2977-4_1.info", true),
            (ItemFormat::Info, false, "1494603968-2977-5_1.info", true),
            (ItemFormat::Info, false, "1494603972-2977-6_1.info", true),
            (ItemFormat::Info, false, "1494603973-2977-7_1.info", true),
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

        let expected = vec![(ItemFormat::Gcno, true, "main_1.gcno", false)];

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

        let expected = vec![(ItemFormat::Gcno, true, "main_1.gcno", true)];

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
            (ItemFormat::Gcno, true, "Platform_1.gcno", false),
            (
                ItemFormat::Gcno,
                true,
                "sub2/RootAccessibleWrap_1.gcno",
                false,
            ),
            (ItemFormat::Gcno, true, "nsMaiInterfaceValue_1.gcno", true),
            (ItemFormat::Gcno, true, "sub/prova2_1.gcno", true),
            (
                ItemFormat::Gcno,
                true,
                "nsMaiInterfaceDocument_1.gcno",
                true,
            ),
            (ItemFormat::Gcno, true, "nsGnomeModule_1.gcno", true),
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
            (ItemFormat::Gcno, true, "Platform_1.gcno", false),
            (
                ItemFormat::Gcno,
                true,
                "sub2/RootAccessibleWrap_1.gcno",
                false,
            ),
            (ItemFormat::Gcno, true, "nsMaiInterfaceValue_1.gcno", true),
            (ItemFormat::Gcno, true, "nsMaiInterfaceValue_2.gcno", true),
            (ItemFormat::Gcno, true, "sub/prova2_1.gcno", true),
            (ItemFormat::Gcno, true, "sub/prova2_2.gcno", true),
            (
                ItemFormat::Gcno,
                true,
                "nsMaiInterfaceDocument_1.gcno",
                true,
            ),
            (
                ItemFormat::Gcno,
                true,
                "nsMaiInterfaceDocument_2.gcno",
                true,
            ),
            (ItemFormat::Gcno, true, "nsGnomeModule_1.gcno", true),
            (ItemFormat::Gcno, true, "nsGnomeModule_2.gcno", true),
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
            (ItemFormat::Gcno, true, "nsMaiInterfaceValue_1.gcno", true),
            (ItemFormat::Gcno, true, "sub/prova2_1.gcno", true),
            (
                ItemFormat::Gcno,
                true,
                "nsMaiInterfaceDocument_1.gcno",
                true,
            ),
            (ItemFormat::Gcno, true, "nsGnomeModule_1.gcno", true),
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
            (ItemFormat::Gcno, true, "nsMaiInterfaceValue_1.gcno", true),
            (ItemFormat::Gcno, true, "nsMaiInterfaceValue_2.gcno", true),
            (ItemFormat::Gcno, true, "sub/prova2_1.gcno", true),
            (ItemFormat::Gcno, true, "sub/prova2_2.gcno", true),
            (
                ItemFormat::Gcno,
                true,
                "nsMaiInterfaceDocument_1.gcno",
                true,
            ),
            (
                ItemFormat::Gcno,
                true,
                "nsMaiInterfaceDocument_2.gcno",
                true,
            ),
            (ItemFormat::Gcno, true, "nsGnomeModule_1.gcno", true),
            (ItemFormat::Gcno, true, "nsGnomeModule_2.gcno", true),
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

                assert!(stem == "file", "Unexpected file: {:?}", stem);
                assert_eq!(buffers.gcno_buf, gcno_buf);
                assert_eq!(buffers.gcda_buf, vec![gcda1_buf.clone(), gcda2_buf.clone()]);
            } else {
                panic!("Buffers expected");
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

        let expected = vec![(ItemFormat::Info, false, "prova_1.info", true)];

        if let Ok(mut reader) = File::open(json_path) {
            let mut json = Vec::new();
            reader.read_to_end(&mut json).unwrap();
            assert_eq!(json, mapping);
        } else {
            panic!("Failed to read the file: {}", json_path);
        }

        check_produced(tmp_path, &receiver, expected);
    }

    #[test]
    fn test_plain_profraw_producer() {
        let (sender, receiver) = unbounded();

        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let tmp_path = tmp_dir.path().to_owned();
        producer(
            &tmp_path,
            &["test/default.profraw".to_string()],
            &sender,
            true,
            false,
        );

        let expected = vec![(ItemFormat::Profraw, true, "default.profraw", false)];

        check_produced(PathBuf::from("test"), &receiver, expected);
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
        let mut file = File::open("./test/jacoco/basic-report.xml").ok();
        assert!(
            Archive::check_file(file.as_mut(), &Archive::is_jacoco),
            "A Jacoco XML file expected"
        );
        let mut file =
            File::open("./test/jacoco/full-junit4-report-multiple-top-level-classes.xml").ok();
        assert!(
            Archive::check_file(file.as_mut(), &Archive::is_jacoco),
            "A Jacoco XML file expected"
        );
        let mut file = File::open("./test/jacoco/inner-classes.xml").ok();
        assert!(
            Archive::check_file(file.as_mut(), &Archive::is_jacoco),
            "A Jacoco XML file expected"
        );
        let mut file = File::open("./test/jacoco/multiple-top-level-classes.xml").ok();
        assert!(
            Archive::check_file(file.as_mut(), &Archive::is_jacoco),
            "A Jacoco XML file expected"
        );
        let mut file = File::open("./test/jacoco/not_jacoco_file.xml").ok();
        assert!(
            !Archive::check_file(file.as_mut(), &Archive::is_jacoco),
            "Not a Jacoco XML file expected"
        );
    }

    #[test]
    fn test_info_files() {
        let mut file = File::open("./test/1494603973-2977-7.info").ok();
        assert!(
            Archive::check_file(file.as_mut(), &Archive::is_info),
            "An info file expected"
        );
        let mut file = File::open("./test/empty_line.info").ok();
        assert!(
            Archive::check_file(file.as_mut(), &Archive::is_info),
            "An info file expected"
        );
        let mut file = File::open("./test/relative_path/relative_path.info").ok();
        assert!(
            Archive::check_file(file.as_mut(), &Archive::is_info),
            "An info file expected"
        );
        let mut file = File::open("./test/not_info_file.info").ok();
        assert!(
            !Archive::check_file(file.as_mut(), &Archive::is_info),
            "Not an info file expected"
        );
    }
}
