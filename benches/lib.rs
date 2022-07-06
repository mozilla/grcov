#![feature(test)]
#![allow(clippy::unit_arg)]
extern crate test;

use crossbeam_channel::unbounded;
use grcov::{CovResult, Function, FunctionMap};
use rustc_hash::FxHashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use test::{black_box, Bencher};

use grcov::*;

#[bench]
fn bench_lib_merge_results(b: &mut Bencher) {
    let mut functions1: FunctionMap = FxHashMap::default();
    functions1.insert(
        "f1".to_string(),
        Function {
            start: 1,
            executed: false,
        },
    );
    functions1.insert(
        "f2".to_string(),
        Function {
            start: 2,
            executed: false,
        },
    );
    let mut result = CovResult {
        lines: [(1, 21), (2, 7), (7, 0)].iter().cloned().collect(),
        branches: [
            (1, vec![false, false]),
            (2, vec![false, true]),
            (4, vec![true]),
        ]
        .iter()
        .cloned()
        .collect(),
        functions: functions1,
    };

    let mut functions2: FunctionMap = FxHashMap::default();
    functions2.insert(
        "f1".to_string(),
        Function {
            start: 1,
            executed: false,
        },
    );
    functions2.insert(
        "f2".to_string(),
        Function {
            start: 2,
            executed: true,
        },
    );
    let result2 = CovResult {
        lines: [(1, 21), (3, 42), (4, 7), (2, 0), (8, 0)]
            .iter()
            .cloned()
            .collect(),
        branches: [
            (1, vec![false, false]),
            (2, vec![false, true]),
            (3, vec![true]),
        ]
        .iter()
        .cloned()
        .collect(),
        functions: functions2,
    };

    b.iter(|| black_box(grcov::merge_results(&mut result, result2.clone())));
}

#[bench]
fn bench_lib_consumer(b: &mut Bencher) {
    let num_threads = 2;
    let result_map: Arc<SyncCovResultMap> = Arc::new(Mutex::new(
        FxHashMap::with_capacity_and_hasher(20_000, Default::default()),
    ));
    let (sender, receiver) = unbounded();
    let working_dir = PathBuf::from("");
    let gcno_buf: Vec<u8> = vec![
        111, 110, 99, 103, 42, 50, 48, 52, 74, 200, 254, 66, 0, 0, 0, 1, 9, 0, 0, 0, 0, 0, 0, 0,
        236, 217, 93, 255, 2, 0, 0, 0, 109, 97, 105, 110, 0, 0, 0, 0, 2, 0, 0, 0, 102, 105, 108,
        101, 46, 99, 0, 0, 1, 0, 0, 0, 0, 0, 65, 1, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 67, 1, 3, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 67, 1, 3, 0, 0, 0, 1, 0,
        0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 69, 1, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 69, 1, 8, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 102, 105, 108, 101, 46, 99, 0,
        0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];

    b.iter(|| {
        let mut parsers = Vec::new();

        for i in 0..num_threads {
            let receiver = receiver.clone();
            let result_map = Arc::clone(&result_map);
            let working_dir = working_dir.clone();

            let t = thread::Builder::new()
                .name(format!("Consumer {}", i))
                .spawn(move || {
                    consumer(
                        &working_dir,
                        None,
                        &result_map,
                        receiver,
                        false,
                        false,
                        None,
                    );
                })
                .unwrap();

            parsers.push(t);
        }

        for _ in 0..10_000 {
            sender
                .send(Some(WorkItem {
                    format: ItemFormat::Gcno,
                    item: ItemType::Buffers(GcnoBuffers {
                        stem: "".to_string(),
                        gcno_buf: gcno_buf.clone(),
                        gcda_buf: Vec::new(),
                    }),
                    name: "".to_string(),
                }))
                .unwrap();
        }

        for _ in 0..num_threads {
            sender.send(None).unwrap();
        }

        for parser in parsers {
            parser.join().unwrap();
        }
    });
}
