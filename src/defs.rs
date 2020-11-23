use crossbeam::channel::{Receiver, Sender};
use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub start: u32,
    pub executed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CovResult {
    pub lines: BTreeMap<u32, u64>,
    pub branches: BTreeMap<u32, Vec<bool>>,
    pub functions: FunctionMap,
}

#[derive(Debug, PartialEq, Copy, Clone)]
#[allow(non_camel_case_types)]
pub enum ItemFormat {
    GCNO,
    PROFRAW,
    INFO,
    JACOCO_XML,
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
pub type CovResultIter = Box<dyn Iterator<Item = (PathBuf, PathBuf, CovResult)>>;

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

#[derive(Debug)]
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

#[derive(Clone, Debug, Default)]
pub struct HtmlStats {
    pub total_lines: usize,
    pub covered_lines: usize,
    pub total_funs: usize,
    pub covered_funs: usize,
    pub total_branches: usize,
    pub covered_branches: usize,
}

#[derive(Clone, Debug)]
pub struct HtmlFileStats {
    pub file_name: String,
    pub stats: HtmlStats,
}

#[derive(Clone, Debug)]
pub struct HtmlDirStats {
    pub files: BTreeSet<HtmlFileStats>,
    pub stats: HtmlStats,
}

#[derive(Debug, Default)]
pub struct HtmlGlobalStats {
    pub dirs: BTreeMap<String, HtmlDirStats>,
    pub stats: HtmlStats,
}

pub type HtmlJobReceiver = Receiver<Option<HtmlItem>>;
pub type HtmlJobSender = Sender<Option<HtmlItem>>;
