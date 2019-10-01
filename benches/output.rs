#![feature(test)]
extern crate grcov;
extern crate test;

use grcov::{
    output_activedata_etl, output_covdir, output_lcov, CovResult, CovResultIter, Function,
    FunctionMap,
};
use std::collections::HashMap;
use std::path::PathBuf;
use tempfile::tempdir;
use test::{black_box, Bencher};

fn generate_cov_result_iter() -> CovResultIter {
    let s: HashMap<PathBuf, CovResult> = HashMap::default();
    Box::new(s.into_iter().map(|(_, _): (PathBuf, CovResult)| {
        (
            PathBuf::from(""),
            PathBuf::from(""),
            CovResult {
                branches: [].iter().cloned().collect(),
                functions: {
                    let mut functions: FunctionMap = HashMap::default();
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
    }))
}
#[bench]
fn bench_output_activedata_etl(b: &mut Bencher) {
    let dir = tempdir().unwrap();
    b.iter(|| {
        black_box(output_activedata_etl(
            generate_cov_result_iter(),
            Some(dir.path().join("temp").to_str().unwrap()),
        ))
    });
}

#[bench]
fn bench_output_covdir(b: &mut Bencher) {
    let dir = tempdir().unwrap();
    b.iter(|| {
        black_box(output_covdir(
            generate_cov_result_iter(),
            Some(dir.path().join("temp").to_str().unwrap()),
        ));
    });
}

#[bench]
fn bench_output_lcov(b: &mut Bencher) {
    let dir = tempdir().unwrap();
    b.iter(|| {
        black_box(output_lcov(
            generate_cov_result_iter(),
            Some(dir.path().join("temp").to_str().unwrap()),
        ));
    });
}
