#![feature(test)]
#![allow(clippy::unit_arg)]
extern crate test;

use std::path::Path;
use test::{black_box, Bencher};

#[bench]
fn bench_find_binaries(b: &mut Bencher) {
    let files = grcov::find_binaries(Path::new("target"));
    println!("files: {}", files.len());
    b.iter(|| {
        black_box(grcov::find_binaries(Path::new("target")));
    });
}
