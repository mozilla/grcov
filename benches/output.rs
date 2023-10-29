#![feature(test)]
#![allow(clippy::unit_arg)]
extern crate test;

use grcov::{
    output_activedata_etl, output_covdir, output_lcov, CovResult, Function, FunctionMap,
    ResultTuple,
};
use rustc_hash::FxHashMap;
use std::path::PathBuf;
use tempfile::tempdir;
use test::{black_box, Bencher};

fn generate_cov_result_iter() -> Vec<ResultTuple> {
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
        })
        .collect::<Vec<_>>()
}
#[bench]
fn bench_output_activedata_etl(b: &mut Bencher) {
    let dir = tempdir().unwrap();
    b.iter(|| {
        black_box(output_activedata_etl(
            &generate_cov_result_iter(),
            Some(&dir.path().join("temp")),
            false,
        ))
    });
}

#[bench]
fn bench_output_covdir(b: &mut Bencher) {
    let dir = tempdir().unwrap();
    b.iter(|| {
        black_box(output_covdir(
            &generate_cov_result_iter(),
            Some(&dir.path().join("temp")),
            2,
        ));
    });
}

#[bench]
fn bench_output_lcov(b: &mut Bencher) {
    let dir = tempdir().unwrap();
    b.iter(|| {
        black_box(output_lcov(
            &generate_cov_result_iter(),
            Some(&dir.path().join("temp")),
            false,
        ));
    });
}
