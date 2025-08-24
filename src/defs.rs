use crossbeam_channel::{Receiver, Sender};
use rustc_hash::FxHashMap;
use serde::ser::{Serialize, Serializer};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    pub start: u32,
    pub executed: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CovResult {
    pub lines: BTreeMap<u32, u64>,
    pub branches: BTreeMap<u32, Vec<bool>>,
    pub functions: FunctionMap,
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum ItemFormat {
    Gcno,
    Profraw,
    Profdata,
    Info,
    JacocoXml,
    Gocov,
}

#[derive(Debug)]
pub struct GcnoBuffers {
    pub stem: String,
    pub gcno_buf: Vec<u8>,
    pub gcda_buf: Vec<Vec<u8>>,
}

#[derive(Debug)]
pub enum ItemType {
    Path((String, PathBuf)),
    Paths(Vec<PathBuf>),
    Content(Vec<u8>),
    Buffers(GcnoBuffers),
}

#[derive(Debug)]
pub struct WorkItem {
    pub format: ItemFormat,
    pub item: ItemType,
    pub name: String,
}

pub type FunctionMap = FxHashMap<String, Function>;

pub type JobReceiver = Receiver<Option<WorkItem>>;
pub type JobSender = Sender<Option<WorkItem>>;

pub type CovResultMap = FxHashMap<String, CovResult>;
pub type SyncCovResultMap = Mutex<CovResultMap>;
pub type ResultTuple = (PathBuf, PathBuf, CovResult);

#[derive(Debug, Default)]
pub struct CDStats {
    pub total: usize,
    pub covered: usize,
    pub missed: usize,
    pub percent: f64,
}

#[derive(Debug)]
pub struct CDFileStats {
    pub name: String,
    pub stats: CDStats,
    pub coverage: Vec<i64>,
}

#[derive(Debug, Default)]
pub struct CDDirStats {
    pub name: String,
    pub files: Vec<CDFileStats>,
    pub dirs: Vec<Rc<RefCell<CDDirStats>>>,
    pub stats: CDStats,
}

#[derive(Debug)]
pub struct HtmlItem {
    pub abs_path: PathBuf,
    pub rel_path: PathBuf,
    pub result: CovResult,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct HtmlStats {
    pub total_lines: usize,
    pub covered_lines: usize,
    pub total_funs: usize,
    pub covered_funs: usize,
    pub total_branches: usize,
    pub covered_branches: usize,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct HtmlFileStats {
    pub stats: HtmlStats,
    pub abs_prefix: Option<PathBuf>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct HtmlDirStats {
    pub files: BTreeMap<String, HtmlFileStats>,
    pub stats: HtmlStats,
    pub abs_prefix: Option<PathBuf>,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct HtmlGlobalStats {
    pub dirs: BTreeMap<String, HtmlDirStats>,
    pub stats: HtmlStats,
    pub abs_prefix: Option<PathBuf>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum HtmlItemStats {
    Directory(HtmlDirStats),
    File(HtmlFileStats),
}

impl HtmlGlobalStats {
    pub fn list(&self, dir: String) -> BTreeMap<String, HtmlItemStats> {
        let mut result = BTreeMap::new();

        // Add files from the specified directory
        if let Some(dir_stats) = self.dirs.get(&dir) {
            for (file_name, file_stats) in &dir_stats.files {
                result.insert(file_name.clone(), HtmlItemStats::File(file_stats.clone()));
            }
        }

        // Add subdirectories as entries
        if dir.is_empty() {
            // For root directory, add top-level directories
            for (dir_path, dir_stats) in &self.dirs {
                if !dir_path.is_empty() && !dir_path.contains('/') {
                    result.insert(
                        dir_path.clone(),
                        HtmlItemStats::Directory(dir_stats.clone()),
                    );
                }
            }
        } else {
            // For specific directory, add immediate subdirectories
            let prefix = if dir.ends_with('/') {
                dir
            } else {
                format!("{}/", dir)
            };

            for (dir_path, dir_stats) in &self.dirs {
                if dir_path.starts_with(&prefix) {
                    let suffix = &dir_path[prefix.len()..];
                    if !suffix.is_empty() && !suffix.contains('/') {
                        result.insert(
                            suffix.to_string(),
                            HtmlItemStats::Directory(dir_stats.clone()),
                        );
                    }
                }
            }
        }

        result
    }
}

pub type HtmlJobReceiver = Receiver<Option<HtmlItem>>;
pub type HtmlJobSender = Sender<Option<HtmlItem>>;

pub enum StringOrRef<'a> {
    S(String),
    R(&'a String),
}

impl Display for StringOrRef<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            StringOrRef::S(s) => write!(f, "{s}"),
            StringOrRef::R(s) => write!(f, "{s}"),
        }
    }
}

impl Serialize for StringOrRef<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            StringOrRef::S(s) => serializer.serialize_str(s),
            StringOrRef::R(s) => serializer.serialize_str(s),
        }
    }
}

pub struct JacocoReport {
    pub lines: BTreeMap<u32, u64>,
    pub branches: BTreeMap<u32, Vec<bool>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_html_global_stats_list() {
        let global_json = r#"
        {
            "dirs": {
                "": {
                    "files": {
                        "build.rs": {
                            "stats": {"total_lines": 30, "covered_lines": 25, "total_funs": 3, "covered_funs": 2, "total_branches": 6, "covered_branches": 5},
                            "abs_prefix": null
                        }
                    },
                    "stats": {"total_lines": 30, "covered_lines": 25, "total_funs": 3, "covered_funs": 2, "total_branches": 6, "covered_branches": 5},
                    "abs_prefix": null
                },
                "src": {
                    "files": {
                        "lib.rs": {
                            "stats": {"total_lines": 50, "covered_lines": 40, "total_funs": 5, "covered_funs": 4, "total_branches": 10, "covered_branches": 8},
                            "abs_prefix": null
                        }
                    },
                    "stats": {"total_lines": 100, "covered_lines": 80, "total_funs": 10, "covered_funs": 8, "total_branches": 20, "covered_branches": 16},
                    "abs_prefix": null
                },
                "src/utils": {
                    "files": {
                        "mod.rs": {
                            "stats": {"total_lines": 50, "covered_lines": 40, "total_funs": 5, "covered_funs": 4, "total_branches": 10, "covered_branches": 8},
                            "abs_prefix": null
                        }
                    },
                    "stats": {"total_lines": 50, "covered_lines": 40, "total_funs": 5, "covered_funs": 4, "total_branches": 10, "covered_branches": 8},
                    "abs_prefix": null
                }
            },
            "stats": {"total_lines": 130, "covered_lines": 105, "total_funs": 13, "covered_funs": 10, "total_branches": 26, "covered_branches": 21},
            "abs_prefix": null
        }
        "#;

        let global: HtmlGlobalStats = serde_json::from_str(global_json).unwrap();

        let root_items = global.list("".to_string());
        assert_eq!(root_items.len(), 2);
        assert!(root_items.contains_key("build.rs"));
        assert!(root_items.contains_key("src"));

        // Check that build.rs is a file and src is a directory
        match root_items.get("build.rs").unwrap() {
            HtmlItemStats::File(_) => {}
            HtmlItemStats::Directory(_) => panic!("build.rs should be a file"),
        }
        match root_items.get("src").unwrap() {
            HtmlItemStats::Directory(_) => {}
            HtmlItemStats::File(_) => panic!("src should be a directory"),
        }

        let src_items = global.list("src".to_string());
        assert_eq!(src_items.len(), 2);
        assert!(src_items.contains_key("lib.rs"));
        assert!(src_items.contains_key("utils"));

        // Check that utils is a directory
        match src_items.get("utils").unwrap() {
            HtmlItemStats::Directory(_) => {}
            HtmlItemStats::File(_) => panic!("utils should be a directory"),
        }

        let utils_items = global.list("src/utils".to_string());
        assert_eq!(utils_items.len(), 1);
        assert!(utils_items.contains_key("mod.rs"));
    }
}
