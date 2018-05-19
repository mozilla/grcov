use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use crossbeam::sync::MsQueue;

#[derive(Debug,Clone,PartialEq)]
pub struct Function {
    pub start: u32,
    pub executed: bool,
}

#[derive(Debug,Clone,PartialEq)]
pub struct CovResult {
    pub lines: BTreeMap<u32,u64>,
    pub branches: BTreeMap<(u32,u32),bool>,
    pub functions: HashMap<String,Function>,
}

#[derive(Debug,PartialEq)]
pub enum ItemFormat {
    GCNO,
    INFO,
}

#[derive(Debug)]
pub struct GcnoBuffers {
    pub stem: String,
    pub gcno_buf: Arc<Vec<u8>>,
    pub gcda_buf: Vec<u8>,
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

pub type WorkQueue = MsQueue<Option<WorkItem>>;

pub type CovResultMap = HashMap<String,CovResult>;
pub type SyncCovResultMap = Mutex<CovResultMap>;
pub type CovResultIter = Box<Iterator<Item=(PathBuf,PathBuf,CovResult)>>;
