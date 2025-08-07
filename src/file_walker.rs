use log::debug;
use rayon::prelude::*;
use std::fs::{self, DirEntry, FileType};
use std::path::{Path, PathBuf};

/// Returns true if the canonicalized `symlink_target` matches any canonicalized ancestor in `ancestor_stack`.
#[inline]
fn is_symlink_loop(symlink_target: &Path, ancestor_stack: &[PathBuf]) -> bool {
    if let Ok(target_canon) = symlink_target.canonicalize() {
        ancestor_stack
            .iter()
            .any(|ancestor| ancestor.canonicalize().is_ok_and(|a| a == target_canon))
    } else {
        false
    }
}
/// A Rayon based parallel file walker
pub struct ParallelWalker {
    paths: Vec<PathBuf>,
    follow_links: bool,
    max_depth: Option<usize>,
}

impl ParallelWalker {
    /// Create a new ParallelWalker for the given path
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        Self {
            paths: vec![path.as_ref().to_path_buf()],
            follow_links: false,
            max_depth: None,
        }
    }

    /// Enable or disable following symbolic links
    pub fn follow_links(mut self, enable: bool) -> Self {
        self.follow_links = enable;
        self
    }

    /// Set maximum directory depth to traverse
    pub fn max_depth(mut self, depth: Option<usize>) -> Self {
        self.max_depth = depth;
        self
    }

    /// Add additional paths to traverse
    pub fn add_path<P: AsRef<Path>>(mut self, path: P) -> Self {
        self.paths.push(path.as_ref().to_path_buf());
        self
    }

    /// Collect all paths matching a filter predicate
    pub fn collect<F>(self, filter: F) -> Vec<PathBuf>
    where
        F: Fn(&FileType, &DirEntry) -> bool + Sync + Send,
    {
        let max_depth = self.max_depth;

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(num_cpus::get() - 1)
            .build()
            .unwrap();

        pool.install(|| {
            if self.paths.len() == 1 {
                Self::walk_dir_parallel_collect(
                    &self.paths[0],
                    0,
                    &filter,
                    max_depth,
                    self.follow_links.then_some(self.paths[0..1].to_vec()),
                )
            } else {
                let symlink_ancestor_stack = self.follow_links.then_some(self.paths.clone());
                self.paths
                    .into_par_iter()
                    .flat_map(|path| {
                        Self::walk_dir_parallel_collect(
                            &path,
                            0,
                            &filter,
                            max_depth,
                            symlink_ancestor_stack.clone(),
                        )
                    })
                    .collect()
            }
        })
    }

    /// Static version of walk_dir_parallel_collect to collect paths matching a filter
    fn walk_dir_parallel_collect<F>(
        path: &Path,
        depth: usize,
        filter: &F,
        max_depth: Option<usize>,
        symlink_ancestor_stack: Option<Vec<PathBuf>>,
    ) -> Vec<PathBuf>
    where
        F: Fn(&FileType, &DirEntry) -> bool + Sync + Send,
    {
        // Check depth limit
        if max_depth.is_some_and(|max_depth| depth > max_depth) {
            return Vec::new();
        }

        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(e) => {
                debug!("Error reading directory {path:?}: {e}");
                // If we can't read the directory, return an empty vector
                return Vec::new();
            }
        };

        entries
            .par_bridge()
            .filter_map(|entry_result| {
                match entry_result {
                    Ok(entry) => {
                        // Get file type first before moving entry
                        let file_type = entry.file_type();
                        let entry_path = entry.path();
                        // Check if this entry matches the filter
                        let (is_dir, matched) = if let Ok(file_type) = &file_type {
                            let matched = filter(file_type, &entry);

                            // Only do symlink logic if ancestor_stack is Some
                            if file_type.is_dir() {
                                (true, matched)
                            } else if file_type.is_symlink() && symlink_ancestor_stack.is_some() {
                                (
                                    entry_path
                                        .metadata()
                                        .is_ok_and(|metadata| metadata.is_dir())
                                        && symlink_ancestor_stack.as_ref().is_none_or(|stack| {
                                            !is_symlink_loop(&entry_path, stack)
                                        }),
                                    matched,
                                )
                            } else {
                                (false, matched)
                            }
                        } else {
                            (false, false)
                        };
                        // recurse the search if it is a dir
                        if is_dir {
                            let mut paths = Self::walk_dir_parallel_collect(
                                &entry_path,
                                depth + 1,
                                filter,
                                max_depth,
                                symlink_ancestor_stack.as_ref().map(|stack| {
                                    let mut new_stack = stack.clone();
                                    new_stack.push(entry_path.clone());
                                    new_stack
                                }),
                            );
                            if matched {
                                paths.push(entry_path);
                            }
                            Some(paths)
                        } else if matched {
                            Some(vec![entry_path])
                        } else {
                            None
                        }
                    }
                    Err(e) => {
                        debug!("Error reading entry {path:?}: {e}");
                        None // Skip entries with errors
                    }
                }
            })
            .flatten()
            .collect()
    }
}

/// Detect known binaries for different platforms
fn is_known_binary(bytes: &[u8]) -> bool {
    let is_elf = bytes.starts_with(&[0x7F, b'E', b'L', b'F']);
    let is_pe = bytes.starts_with(&[0x4D, 0x5A]); // 'MZ'
    let is_macho = bytes.starts_with(&[0xFE, 0xED, 0xFA, 0xCE])
        || bytes.starts_with(&[0xFE, 0xED, 0xFA, 0xCF])
        || bytes.starts_with(&[0xCA, 0xFE, 0xBA, 0xBE])
        || bytes.starts_with(&[0xCE, 0xFA, 0xED, 0xFE])
        || bytes.starts_with(&[0xCF, 0xFA, 0xED, 0xFE]);

    let is_coff = bytes.len() >= 2 && matches!(
        u16::from_le_bytes([bytes[0], bytes[1]]),
        0x014C | 0x8664 | 0x01C0 | 0xAA64
    );

    is_elf || is_pe || is_macho || is_coff
}

/// Helper function to find binary files using the ParallelWalker
pub fn find_binaries<P: AsRef<Path>>(path: P) -> Vec<PathBuf> {
    let walker = ParallelWalker::new(path);

    walker.collect(|file_type, entry| {
        // Only process files
        if !file_type.is_file() {
            return false;
        }

        // Try to read the first 128 bytes to check if it's a binary
        if let Ok(file) = std::fs::File::open(entry.path()) {
            use std::io::Read;
            let mut bytes = [0u8; 128];
            if let Ok(read) = file.take(128).read(&mut bytes) {
                if read > 0 && is_known_binary(&bytes[..read]) {
                    return true;
                }
            }
        }

        false
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Write;
    use tempfile::tempdir;

    #[cfg(unix)]
    use std::os::unix::fs as unix_fs;

    fn create_test_file(dir: &Path, name: &str, contents: &[u8]) -> PathBuf {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut file = File::create(&path).unwrap();
        file.write_all(contents).unwrap();
        path
    }

    fn collect_paths<F>(walker: ParallelWalker, filter: F) -> Vec<PathBuf>
    where
        F: Fn(&FileType, &DirEntry) -> bool + Send + Sync,
    {
        walker.collect(filter)
    }

    fn collect_all_entries(walker: ParallelWalker) -> Vec<PathBuf> {
        collect_paths(walker, |_, _| true)
    }

    fn collect_files(walker: ParallelWalker) -> Vec<PathBuf> {
        collect_paths(walker, |ft, _entry| ft.is_file())
    }

    fn collect_dirs(walker: ParallelWalker) -> Vec<PathBuf> {
        collect_paths(walker, |ft, _entry| ft.is_dir())
    }

    #[test]
    fn test_basic_traversal() {
        let dir = tempdir().unwrap();

        // Create some test files
        create_test_file(dir.path(), "file1.txt", b"test content");
        create_test_file(dir.path(), "file2.txt", b"more content");

        let subdir = dir.path().join("subdir");
        fs::create_dir_all(&subdir).unwrap();
        create_test_file(&subdir, "file3.txt", b"sub content");

        let files = collect_files(ParallelWalker::new(dir.path()));

        // Should find all files
        assert!(
            files.len() >= 3,
            "Expected at least 3 files, found {}",
            files.len()
        );
        assert!(files.iter().any(|p| p.ends_with("file1.txt")));
        assert!(files.iter().any(|p| p.ends_with("file2.txt")));
        assert!(files.iter().any(|p| p.ends_with("file3.txt")));
    }

    #[test]
    fn test_complex_directory_structure() {
        let dir = tempdir().unwrap();

        // Create complex nested structure
        let paths = [
            "a/b/c/file1.txt",
            "a/b/file2.txt",
            "a/file3.txt",
            "x/y/z/file4.txt",
            "x/y/file5.txt",
            "x/file6.txt",
            "root_file.txt",
        ];

        for path in &paths {
            create_test_file(dir.path(), path, b"content");
        }

        let files = collect_files(ParallelWalker::new(dir.path()));

        // Should find all files
        assert_eq!(files.len(), paths.len());
        for expected_path in &paths {
            assert!(
                files.iter().any(|p| p.ends_with(expected_path)),
                "Missing file: {}",
                expected_path
            );
        }
    }

    #[test]
    fn test_max_depth() {
        let dir = tempdir().unwrap();

        // Create nested structure
        let level1 = dir.path().join("level1");
        let level2 = level1.join("level2");
        let level3 = level2.join("level3");

        fs::create_dir_all(&level3).unwrap();

        create_test_file(&level1, "file1.txt", b"level1");
        create_test_file(&level2, "file2.txt", b"level2");
        create_test_file(&level3, "file3.txt", b"level3");

        // Test depth 0 - only root level
        let files = collect_files(ParallelWalker::new(dir.path()).max_depth(Some(0)));
        assert_eq!(files.len(), 0); // No files at root level

        // Test depth 1 - include first level
        let files = collect_files(ParallelWalker::new(dir.path()).max_depth(Some(1)));
        assert!(files.iter().any(|p| p.ends_with("file1.txt")));
        assert!(!files.iter().any(|p| p.ends_with("file2.txt")));
        assert!(!files.iter().any(|p| p.ends_with("file3.txt")));

        // Test depth 2 - include up to second level
        let files = collect_files(ParallelWalker::new(dir.path()).max_depth(Some(2)));
        assert!(files.iter().any(|p| p.ends_with("file1.txt")));
        assert!(files.iter().any(|p| p.ends_with("file2.txt")));
        assert!(!files.iter().any(|p| p.ends_with("file3.txt")));

        // Test no depth limit
        let files = collect_files(ParallelWalker::new(dir.path()));
        assert!(files.iter().any(|p| p.ends_with("file1.txt")));
        assert!(files.iter().any(|p| p.ends_with("file2.txt")));
        assert!(files.iter().any(|p| p.ends_with("file3.txt")));
    }

    #[test]
    fn test_multiple_paths() {
        let dir1 = tempdir().unwrap();
        let dir2 = tempdir().unwrap();

        create_test_file(dir1.path(), "file1.txt", b"dir1");
        create_test_file(dir2.path(), "file2.txt", b"dir2");

        let files = collect_files(ParallelWalker::new(dir1.path()).add_path(dir2.path()));

        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|p| p.ends_with("file1.txt")));
        assert!(files.iter().any(|p| p.ends_with("file2.txt")));
    }

    #[test]
    fn test_directory_filtering() {
        let dir = tempdir().unwrap();

        // Create directory structure
        let skip_dir = dir.path().join("skip_me");
        let keep_dir = dir.path().join("keep_me");

        fs::create_dir_all(&skip_dir).unwrap();
        fs::create_dir_all(&keep_dir).unwrap();

        create_test_file(&skip_dir, "should_not_find.txt", b"skip");
        create_test_file(&keep_dir, "should_find.txt", b"keep");

        // Use collect with a filter that excludes files in skip_me directory
        let files = collect_paths(ParallelWalker::new(dir.path()), |ft, entry| {
            let path = entry.path();
            let is_file = ft.is_file();

            // Only include files that are not in the skip_me directory
            is_file && !path.to_string_lossy().contains("skip_me")
        });

        assert!(files.iter().any(|p| p.ends_with("should_find.txt")));
        assert!(!files.iter().any(|p| p.ends_with("should_not_find.txt")));
    }

    #[test]
    fn test_collect_directories() {
        let dir = tempdir().unwrap();

        // Create directory structure
        let subdir1 = dir.path().join("subdir1");
        let subdir2 = dir.path().join("subdir2");
        let nested_dir = subdir1.join("nested");

        fs::create_dir_all(&subdir1).unwrap();
        fs::create_dir_all(&subdir2).unwrap();
        fs::create_dir_all(&nested_dir).unwrap();

        // Create some files too
        create_test_file(dir.path(), "file.txt", b"content");
        create_test_file(&subdir1, "file1.txt", b"content1");

        let dirs = collect_dirs(ParallelWalker::new(dir.path()));

        // Should find all directories
        assert!(
            dirs.len() >= 3,
            "Expected at least 3 directories, found {}",
            dirs.len()
        );
        assert!(dirs.iter().any(|p| p.ends_with("subdir1")));
        assert!(dirs.iter().any(|p| p.ends_with("subdir2")));
        assert!(dirs.iter().any(|p| p.ends_with("nested")));
    }

    #[test]
    fn test_collect_directories_with_depth() {
        let dir = tempdir().unwrap();

        // Create nested directory structure
        let level1 = dir.path().join("level1");
        let level2 = level1.join("level2");
        let level3 = level2.join("level3");

        fs::create_dir_all(&level3).unwrap();

        // Test with max depth 0 - should only find level1
        let dirs = collect_dirs(ParallelWalker::new(dir.path()).max_depth(Some(0)));
        assert!(dirs.iter().any(|p| p.ends_with("level1")));
        assert!(!dirs.iter().any(|p| p.ends_with("level2")));
        assert!(!dirs.iter().any(|p| p.ends_with("level3")));

        // Test with max depth 1 - should find level1 and level2
        let dirs = collect_dirs(ParallelWalker::new(dir.path()).max_depth(Some(1)));
        assert!(dirs.iter().any(|p| p.ends_with("level1")));
        assert!(dirs.iter().any(|p| p.ends_with("level2")));
        assert!(!dirs.iter().any(|p| p.ends_with("level3")));

        // Test without depth limit - should find all
        let dirs = collect_dirs(ParallelWalker::new(dir.path()));
        assert!(dirs.iter().any(|p| p.ends_with("level1")));
        assert!(dirs.iter().any(|p| p.ends_with("level2")));
        assert!(dirs.iter().any(|p| p.ends_with("level3")));
    }

    // Remove test_walk_state_quit as it's no longer applicable without the run method

    #[cfg(unix)]
    #[test]
    fn test_symlinks_no_follow() {
        let dir = tempdir().unwrap();

        // Create a file and a symlink to it
        let real_file = create_test_file(dir.path(), "real_file.txt", b"real content");
        let symlink_path = dir.path().join("symlink.txt");
        unix_fs::symlink(&real_file, &symlink_path).unwrap();

        let all_entries = collect_all_entries(ParallelWalker::new(dir.path()));
        let files = collect_files(ParallelWalker::new(dir.path()));
        assert!(all_entries.len() > files.len());
        // Should find the real file at minimum
        assert!(!files.is_empty());
        assert!(files.iter().any(|p| p.ends_with("real_file.txt")));
        // Symlink behavior may vary by platform/configuration
    }

    #[cfg(unix)]
    #[test]
    fn test_symlinks_follow() {
        let dir = tempdir().unwrap();

        // Create a directory with a file
        let real_dir = dir.path().join("real_dir");
        fs::create_dir_all(&real_dir).unwrap();
        create_test_file(&real_dir, "file_in_real_dir.txt", b"content");

        // Create a symlink to the directory
        let symlink_dir = dir.path().join("symlink_dir");
        unix_fs::symlink(&real_dir, &symlink_dir).unwrap();

        let files = collect_files(ParallelWalker::new(dir.path()).follow_links(true));

        // Should find the file twice - once through real path, once through symlink
        assert!(files.len() >= 2);
        let real_file_count = files
            .iter()
            .filter(|p| p.to_string_lossy().contains("file_in_real_dir.txt"))
            .count();
        assert!(real_file_count >= 1);
    }

    #[cfg(unix)]
    #[test]
    fn test_symlink_loop_detection() {
        let dir = tempdir().unwrap();

        // Create directories
        let dir_a = dir.path().join("a");
        let dir_b = dir_a.join("b");
        fs::create_dir_all(&dir_b).unwrap();

        // Create symlink loop: a/b/c -> a
        let symlink_c = dir_b.join("c");
        unix_fs::symlink(&dir_a, &symlink_c).unwrap();

        create_test_file(&dir_a, "file.txt", b"content");

        let files = collect_files(ParallelWalker::new(dir.path()).follow_links(true));

        // Should find the file at least once (through the real path)
        assert!(
            files.iter().any(|p| p.ends_with("file.txt")),
            "Should find file.txt through real path"
        );
    }

    #[test]
    fn test_empty_directory() {
        let dir = tempdir().unwrap();

        let entries = collect_all_entries(ParallelWalker::new(dir.path()));

        // Empty directory should have no entries
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_single_file() {
        let dir = tempdir().unwrap();
        create_test_file(dir.path(), "single.txt", b"content");

        // Test walking a directory containing a single file
        let files = collect_files(ParallelWalker::new(dir.path()));

        // Should find just the single file
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("single.txt"));
    }

    #[test]
    fn test_deeply_nested_structure() {
        let dir = tempdir().unwrap();

        // Create deeply nested structure (20 levels)
        let mut current_path = dir.path().to_path_buf();
        for i in 0..20 {
            current_path = current_path.join(format!("level{i}"));
            fs::create_dir_all(&current_path).unwrap();
            create_test_file(&current_path, &format!("file{i}.txt"), b"content");
        }

        let files = collect_files(ParallelWalker::new(dir.path()));

        // Should find all 20 files
        assert_eq!(files.len(), 20);

        // Test with max depth
        let files = collect_files(ParallelWalker::new(dir.path()).max_depth(Some(10)));
        assert_eq!(files.len(), 10);
    }

    #[test]
    fn test_large_directory() {
        let dir = tempdir().unwrap();

        // Create many files and directories
        for i in 0..100 {
            create_test_file(dir.path(), &format!("file{i}.txt"), b"content");

            let subdir = dir.path().join(format!("dir{i}"));
            fs::create_dir_all(&subdir).unwrap();
            create_test_file(&subdir, &format!("subfile{i}.txt"), b"subcontent");
        }

        let files = collect_files(ParallelWalker::new(dir.path()));

        // Should find all 200 files (100 in root + 100 in subdirs)
        assert_eq!(files.len(), 200);
    }

    #[test]
    fn test_special_filenames() {
        let dir = tempdir().unwrap();

        // Test files with special characters
        let special_names = [
            "file with spaces.txt",
            "file-with-dashes.txt",
            "file_with_underscores.txt",
            "file.with.dots.txt",
            "file,with,commas.txt",
            "file(with)parens.txt",
            "file[with]brackets.txt",
            "файл.txt", // Unicode
        ];

        for name in &special_names {
            create_test_file(dir.path(), name, b"content");
        }

        let files = collect_files(ParallelWalker::new(dir.path()));

        assert_eq!(files.len(), special_names.len());
        for expected_name in &special_names {
            assert!(
                files
                    .iter()
                    .any(|p| p.to_string_lossy().contains(expected_name)),
                "Missing file with special name: {}",
                expected_name
            );
        }
    }

    #[test]
    fn test_find_binaries() {
        let dir = tempdir().unwrap();

        // Create a text file
        create_test_file(dir.path(), "text.txt", b"This is a text file");

        // Create a proper ELF binary file that infer will recognize
        let mut elf_header = vec![0u8; 64]; // ELF header is 64 bytes for 64-bit
        elf_header[0..4].copy_from_slice(&[0x7F, 0x45, 0x4C, 0x46]); // ELF magic
        elf_header[4] = 0x02; // 64-bit
        elf_header[5] = 0x01; // little-endian
        elf_header[6] = 0x01; // current version
        elf_header[7] = 0x00; // System V ABI
        elf_header[16] = 0x02; // executable file
        elf_header[17] = 0x00;
        elf_header[18] = 0x3E; // x86-64
        elf_header[19] = 0x00;

        create_test_file(dir.path(), "binary", &elf_header);

        let binaries = find_binaries(dir.path());

        // Should find the binary but not the text file
        assert!(
            !binaries.is_empty(),
            "Expected to find at least 1 binary, found {}",
            binaries.len()
        );
        assert!(
            binaries.iter().any(|p| p.ends_with("binary")),
            "Expected to find 'binary' file"
        );
    }

    #[test]
    fn test_find_binaries_subdirectories() {
        let dir = tempdir().unwrap();

        // Create binary in subdirectory with proper ELF header
        let subdir = dir.path().join("bin");
        fs::create_dir_all(&subdir).unwrap();

        // Create a proper ELF binary file that infer will recognize
        let mut elf_header = vec![0u8; 64]; // ELF header is 64 bytes for 64-bit
        elf_header[0..4].copy_from_slice(&[0x7F, 0x45, 0x4C, 0x46]); // ELF magic
        elf_header[4] = 0x02; // 64-bit
        elf_header[5] = 0x01; // little-endian
        elf_header[6] = 0x01; // current version
        elf_header[7] = 0x00; // System V ABI
        elf_header[16] = 0x02; // executable file
        elf_header[17] = 0x00;
        elf_header[18] = 0x3E; // x86-64
        elf_header[19] = 0x00;

        create_test_file(&subdir, "mybinary", &elf_header);

        let binaries = find_binaries(dir.path());

        assert_eq!(binaries.len(), 1);
        assert!(binaries[0].ends_with("mybinary"));
    }

    #[test]
    fn test_parallel_complex_file_structure() {
        let dir = tempdir().unwrap();

        // Create a moderately complex structure to test parallel efficiency
        for i in 0..10 {
            let subdir = dir.path().join(format!("dir{i}"));
            fs::create_dir_all(&subdir).unwrap();

            for j in 0..10 {
                create_test_file(&subdir, &format!("file{i}_{j}.txt"), b"content");
            }
        }

        let files = collect_files(ParallelWalker::new(dir.path()));

        // Should find all 100 files
        assert_eq!(files.len(), 100);
    }

    #[cfg(unix)]
    #[test]
    fn test_symlink_ancestor_cycle_prevention() {
        use std::collections::HashSet;
        let dir = tempdir().unwrap();

        // Create nested directories: a/b/c
        let dir_a = dir.path().join("a");
        let dir_b = dir_a.join("b");
        let dir_c = dir_b.join("c");
        fs::create_dir_all(&dir_c).unwrap();

        // Create a file in c
        let _file_c = create_test_file(&dir_c, "file.txt", b"content");

        // Create a symlink in c that points back to a (ancestor)
        let symlink_to_a = dir_c.join("loop");
        unix_fs::symlink(&dir_a, &symlink_to_a).unwrap();

        // Walk with follow_links enabled
        let files = collect_files(ParallelWalker::new(dir.path()).follow_links(true));

        // Should find file.txt exactly once (no infinite loop, no duplicate)
        let file_txt_count = files.iter().filter(|p| p.ends_with("file.txt")).count();
        assert_eq!(
            file_txt_count, 1,
            "file.txt should be found exactly once, found {file_txt_count}"
        );

        // Should not visit more files than exist (no cycle)
        let unique_files: HashSet<_> = files.iter().collect();
        assert_eq!(
            files.len(),
            unique_files.len(),
            "No duplicate files should be found"
        );
    }
}
