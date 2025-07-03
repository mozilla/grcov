#![feature(test)]
#![allow(clippy::unit_arg)]
extern crate test;

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use test::{black_box, Bencher};

#[bench]
fn bench_find_binaries(b: &mut Bencher) {
    let files = grcov::find_binaries(Path::new("target"));
    println!("files: {}", files.len());
    b.iter(|| {
        black_box(grcov::find_binaries(Path::new("target")));
    });
}

#[cfg(feature = "ignore")]
mod ignore_bench {
    use super::*;
    use crossbeam_channel::unbounded;
    use ignore::WalkBuilder;
    use ignore::WalkState::Continue;
    use std::io::Read;

    pub fn find_binaries_ignore(binary_path: &Path) -> Vec<PathBuf> {
        let mut paths = vec![];

        let (sender, receiver) = unbounded();
        let walker = WalkBuilder::new(binary_path)
            .threads(num_cpus::get() - 1)
            .standard_filters(false)
            .build_parallel();
        walker.run(|| {
            let sender = sender.clone();
            let mut bytes = [0u8; 128];

            Box::new(move |result| {
                let entry = result.unwrap();

                if !entry.file_type().unwrap().is_file() {
                    return Continue;
                }

                let file = File::open(entry.path()).unwrap();
                let read = file.take(128).read(&mut bytes).unwrap();
                if read == 0 {
                    return Continue;
                }

                if infer::is_app(&bytes) {
                    sender.send(entry.into_path()).unwrap();
                }

                Continue
            })
        });

        while let Ok(path) = receiver.try_recv() {
            paths.push(path);
        }

        paths
    }

    #[bench]
    fn bench_find_binaries(b: &mut Bencher) {
        let files = find_binaries_ignore(Path::new("target"));
        println!("files: {}", files.len());
        b.iter(|| {
            black_box(find_binaries_ignore(Path::new("target")));
        });
    }
}
