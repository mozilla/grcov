#![feature(test)]
extern crate grcov;
extern crate rustc_hash;
extern crate test;

use grcov::{output_activedata_etl, CovResult, Function, FunctionMap};
use rustc_hash::FxHashMap;
use std::path::PathBuf;
use test::{black_box, Bencher};

#[bench]
fn bench_output_activedata_etl(b: &mut Bencher) {
    b.iter(|| {
        let j = Box::new(
            FxHashMap::default()
                .into_iter()
                .map(|(_, _): (PathBuf, CovResult)| {
                    (
                        PathBuf::from(""),
                        PathBuf::from(""),
                        CovResult {
                            branches: [].iter().cloned().collect(),
                            functions: {
                                let mut functions: FunctionMap = FxHashMap::default();
                                functions.insert(
                                    "f1".to_string(),
                                    Function {
                                        start: 1,
                                        executed: true,
                                    },
                                );
                                functions.insert(
                                    "f2".to_string(),
                                    Function {
                                        start: 2,
                                        executed: false,
                                    },
                                );
                                functions
                            },
                            lines: [(1, 21), (2, 7), (7, 0)].iter().cloned().collect(),
                        },
                    )
                }),
        );
        black_box(output_activedata_etl(j, None))
    });
}
