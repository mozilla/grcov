#![feature(test)]
#![allow(clippy::unit_arg)]
extern crate test;

use grcov::{Gcno, GcovReaderBuf, LittleEndian};
use std::path::PathBuf;
use test::{black_box, Bencher};

const LLVM_READER_GCNO: &[u8] = include_bytes!("../test/llvm/reader.gcno");
const LLVM_READER_GCDA: &[u8] = include_bytes!("../test/llvm/reader.gcda");

#[bench]
fn bench_reader_gcno(b: &mut Bencher) {
    let mut gcno = Gcno::new();
    b.iter(|| {
        let file = GcovReaderBuf::<LittleEndian>::new("reader", LLVM_READER_GCNO.to_vec());
        black_box(gcno.read_gcno(file).unwrap());
    });
}

#[bench]
fn bench_reader_gcda(b: &mut Bencher) {
    let mut gcno = Gcno::new();
    gcno.read_gcno(GcovReaderBuf::<LittleEndian>::new(
        "reader",
        LLVM_READER_GCNO.to_vec(),
    ))
    .unwrap();

    b.iter(|| {
        let file = GcovReaderBuf::<LittleEndian>::new("reader", LLVM_READER_GCDA.to_vec());
        black_box(gcno.read_gcda(file).unwrap());
    });
}

#[bench]
fn bench_reader_gcno_dump(b: &mut Bencher) {
    let mut gcno = Gcno::new();
    gcno.read_gcno(GcovReaderBuf::<LittleEndian>::new(
        "reader",
        LLVM_READER_GCNO.to_vec(),
    ))
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
fn bench_reader_gcno_gcda_dump(b: &mut Bencher) {
    let mut gcno = Gcno::new();
    gcno.read_gcno(GcovReaderBuf::<LittleEndian>::new(
        "reader",
        LLVM_READER_GCNO.to_vec(),
    ))
    .unwrap();
    gcno.read_gcda(GcovReaderBuf::<LittleEndian>::new(
        "reader",
        LLVM_READER_GCDA.to_vec(),
    ))
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
    let mut gcno = Gcno::new();
    gcno.read_gcno(GcovReaderBuf::<LittleEndian>::new(
        "reader",
        LLVM_READER_GCNO.to_vec(),
    ))
    .unwrap();
    gcno.read_gcda(GcovReaderBuf::<LittleEndian>::new(
        "reader",
        LLVM_READER_GCDA.to_vec(),
    ))
    .unwrap();

    b.iter(|| black_box(gcno.finalize(true)));
}

#[bench]
fn bench_reader_finalize_file_branch(b: &mut Bencher) {
    let mut gcno = Gcno::new();
    gcno.read_gcno(GcovReaderBuf::<LittleEndian>::new(
        "reader",
        LLVM_READER_GCNO.to_vec(),
    ))
    .unwrap();
    gcno.read_gcda(GcovReaderBuf::<LittleEndian>::new(
        "reader",
        LLVM_READER_GCDA.to_vec(),
    ))
    .unwrap();

    b.iter(|| black_box(gcno.finalize(true)));
}
