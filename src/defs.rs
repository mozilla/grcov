use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;
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
pub enum ItemType {
    Path(PathBuf),
    Content(Vec<u8>),
}

#[derive(Debug)]
pub struct WorkItem {
    pub format: ItemFormat,
    pub item: ItemType,
}

impl WorkItem {
    pub fn path(&self) -> &PathBuf {
        if let ItemType::Path(ref p) = self.item {
            p
        } else {
            panic!("Path expected");
        }
    }
}

pub type WorkQueue = MsQueue<Option<WorkItem>>;

pub type CovResultMap = HashMap<String,CovResult>;
pub type SyncCovResultMap = Mutex<CovResultMap>;
pub type CovResultIter = Box<Iterator<Item=(PathBuf,PathBuf,CovResult)>>;
