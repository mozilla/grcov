#![feature(test)]
extern crate test;
extern crate grcov;

use test::{black_box, Bencher};
use std::path::Path;

#[bench]
fn bench_parser_gcov(b: &mut Bencher) {
    let path = Path::new("./test/negative_counts.gcov");
    b.iter(|| black_box(grcov::parse_gcov(path)));
}
