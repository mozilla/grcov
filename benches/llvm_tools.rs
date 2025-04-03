#![feature(test)]
#![allow(clippy::unit_arg)]
extern crate test;

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use test::{black_box, Bencher};

#[bench]
fn bench_find_binaries(b: &mut Bencher) {
    b.iter(|| {
        black_box(grcov::find_binaries(Path::new("target")));
    });
}
