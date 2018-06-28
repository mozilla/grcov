#![feature(test)]
extern crate grcov;
extern crate test;

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use test::{black_box, Bencher};

#[bench]
fn bench_parser_lcov(b: &mut Bencher) {
    let f = File::open("./test/prova.info").expect("Failed to open lcov file");
    b.iter(|| {
        let file = BufReader::new(&f);
        black_box(grcov::parse_lcov(file, true));
    });
}

#[bench]
fn bench_parser_gcov(b: &mut Bencher) {
    let path = Path::new("./test/negative_counts.gcov");
    b.iter(|| black_box(grcov::parse_gcov(path)));
}
