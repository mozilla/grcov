#![feature(test)]
extern crate grcov;
extern crate test;

use grcov::{GcovReaderBuf, GCNO};
use std::path::PathBuf;
use test::{black_box, Bencher};

#[bench]
fn bench_reader_gcno(b: &mut Bencher) {
    let mut gcno = GCNO::new();
    b.iter(|| {
        let file = GcovReaderBuf::from("test/llvm/reader.gcno");
        black_box(gcno.read(file).unwrap());
    });
}

#[bench]
fn bench_reader_gcno_gcda(b: &mut Bencher) {
    let mut gcno = GCNO::new();
    gcno.read(GcovReaderBuf::from("test/llvm/reader.gcno"))
        .unwrap();

    b.iter(|| {
        let file = GcovReaderBuf::from("test/llvm/reader.gcda");
        black_box(gcno.read_gcda(file).unwrap());
    });
}

#[bench]
fn bench_reader_gcno_counter(b: &mut Bencher) {
    let mut gcno = GCNO::new();
    gcno.read(GcovReaderBuf::from("test/llvm/reader.gcno"))
        .unwrap();
    b.iter(|| {
        let mut output = Vec::new();
        black_box(
            gcno.dump(
                &PathBuf::from("test/llvm/reader.c"),
                "reader.c",
                &mut output,
            )
            .unwrap(),
        );
    });
}

#[bench]
fn bench_reader_gcno_gcda_counter(b: &mut Bencher) {
    let mut gcno = GCNO::new();
    gcno.read(GcovReaderBuf::from("test/llvm/reader.gcno"))
        .unwrap();
    gcno.read_gcda(GcovReaderBuf::from("test/llvm/reader.gcda"))
        .unwrap();
    b.iter(|| {
        let mut output = Vec::new();
        black_box(
            gcno.dump(
                &PathBuf::from("test/llvm/reader.c"),
                "reader.c",
                &mut output,
            )
            .unwrap(),
        );
    });
}

#[bench]
fn bench_reader_finalize_file(b: &mut Bencher) {
    let mut gcno = GCNO::new();
    gcno.read(GcovReaderBuf::from("test/llvm/file.gcno"))
        .unwrap();
    gcno.read_gcda(GcovReaderBuf::from("test/llvm/file.gcda"))
        .unwrap();
    b.iter(|| black_box(gcno.finalize(true)));
}

#[bench]
fn bench_reader_finalize_file_branch(b: &mut Bencher) {
    let mut gcno = GCNO::new();
    gcno.read(GcovReaderBuf::from("test/llvm/file_branch.gcno"))
        .unwrap();
    gcno.read_gcda(GcovReaderBuf::from("test/llvm/file_branch.gcda"))
        .unwrap();
    b.iter(|| black_box(gcno.finalize(true)));
}
