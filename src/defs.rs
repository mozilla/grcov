use crossbeam::channel::{Receiver, Sender};
use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::collections::BTreeMap;
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
    pub functions: FxHashMap<String, Function>,
}

#[derive(Debug, PartialEq, Copy, Clone)]
#[allow(non_camel_case_types)]
pub enum ItemFormat {
    GCNO,
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
    Path(PathBuf),
    Content(Vec<u8>),
    Buffers(GcnoBuffers),
}

#[derive(Debug)]
pub struct WorkItem {
    pub format: ItemFormat,
    pub item: ItemType,
    pub name: String,
}

pub type JobReceiver = Receiver<Option<WorkItem>>;
pub type JobSender = Sender<Option<WorkItem>>;

pub type CovResultMap = FxHashMap<String, CovResult>;
pub type SyncCovResultMap = Mutex<CovResultMap>;
pub type CovResultIter = Box<Iterator<Item = (PathBuf, PathBuf, CovResult)>>;

#[derive(Debug, Default)]
pub struct FMStats {
    pub total: usize,
    pub covered: usize,
    pub missed: usize,
    pub percent: f64,
}

#[derive(Debug)]
pub struct FMFileStats {
    pub name: String,
    pub stats: FMStats,
    pub coverage: Vec<i64>,
}

#[derive(Debug)]
pub struct FMDirStats {
    pub name: String,
    pub files: Vec<FMFileStats>,
    pub dirs: Vec<Rc<RefCell<FMDirStats>>>,
    pub stats: FMStats,
}
