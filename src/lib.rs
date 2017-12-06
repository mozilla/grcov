#![cfg_attr(feature="alloc_system",feature(alloc_system))]
#[cfg(feature="alloc_system")]
extern crate alloc_system;
extern crate crypto;
#[macro_use]
extern crate serde_json;
extern crate crossbeam;
extern crate walkdir;
extern crate semver;
extern crate zip;
extern crate tempdir;
extern crate libc;
extern crate uuid;

mod defs;
pub use defs::*;

mod producer;
pub use producer::*;

mod gcov;
pub use gcov::*;

mod parser;
pub use parser::*;

mod path_rewriting;
pub use path_rewriting::*;

mod output;
pub use output::*;

use std::collections::{btree_map, HashMap, hash_map};

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
