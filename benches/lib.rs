#![feature(test)]
extern crate grcov;
extern crate rustc_hash;
extern crate test;

use grcov::{CovResult, Function};
use rustc_hash::FxHashMap;
use test::{black_box, Bencher};

#[bench]
fn bench_lib_merge_results(b: &mut Bencher) {
    let mut functions1: FxHashMap<String, Function> = FxHashMap::default();
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

    b.iter(|| {
        let mut functions2: FxHashMap<String, Function> = FxHashMap::default();
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
        black_box(grcov::merge_results(&mut result, result2));
    });
}