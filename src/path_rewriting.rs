use globset::{Glob, GlobSet, GlobSetBuilder};
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use serde_json::Value;
use std::collections::hash_map;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use walkdir::{DirEntry, WalkDir};

use crate::defs::*;
use crate::filter::*;

fn to_lowercase_first(s: &str) -> String {
    let mut c = s.chars();
    c.next().unwrap().to_lowercase().collect::<String>() + c.as_str()
}

fn to_uppercase_first(s: &str) -> String {
    let mut c = s.chars();
    c.next().unwrap().to_uppercase().collect::<String>() + c.as_str()
}

pub fn canonicalize_path<P: AsRef<Path>>(path: P) -> io::Result<PathBuf> {
    let path = fs::canonicalize(path)?;

    #[cfg(windows)]
    let path = path
        .to_str()
        .unwrap()
        .strip_prefix(r"\\?\")
        .map(PathBuf::from)
        .unwrap_or(path);

    Ok(path)
}

pub fn has_no_parent(path: &str) -> bool {
    PathBuf::from(path).parent() == Some(&PathBuf::from(""))
}

pub fn normalize_path<P: AsRef<Path>>(path: P) -> Option<PathBuf> {
    // Copied from Cargo sources: https://github.com/rust-lang/cargo/blob/911f0b94e5c10f514b13affbeccd5fd2661a32d9/src/cargo/util/paths.rs#L60
    let mut components = path.as_ref().components().peekable();
    let mut ret = if let Some(c @ Component::Prefix(..)) = components.peek().cloned() {
        components.next();
        PathBuf::from(c.as_os_str())
    } else {
        PathBuf::new()
    };

    for component in components {
        match component {
            Component::Prefix(..) => unreachable!(),
            Component::RootDir => {
                ret.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !ret.pop() {
                    eprintln!(
                        "Warning: {:?} cannot be normalized because of \"..\", so skip it.",
                        path.as_ref()
                    );
                    return None;
                }
            }
            Component::Normal(c) => {
                ret.push(c);
            }
        }
    }
    Some(ret)
}

// Search the source file's path in the mapping.
fn apply_mapping(mapping: &Option<Value>, path: &str) -> PathBuf {
    if let Some(mapping) = mapping {
        if let Some(p) = mapping.get(to_lowercase_first(path)) {
            return PathBuf::from(p.as_str().unwrap());
        } else if let Some(p) = mapping.get(to_uppercase_first(path)) {
            return PathBuf::from(p.as_str().unwrap());
        }
    }

    PathBuf::from(path)
}

// If the join of the source and the relative path is a file, return it.
// Otherwise, remove common part between the source's end and the relative
// path's start.
fn guess_abs_path(prefix_dir: &Path, path: &Path) -> PathBuf {
    let full_path = prefix_dir.join(path);
    if full_path.is_file() {
        return full_path;
    }
    for ancestor in path.ancestors() {
        if prefix_dir.ends_with(ancestor) && !ancestor.as_os_str().is_empty() {
            return prefix_dir.join(path.strip_prefix(ancestor).unwrap());
        }
    }
    full_path
}

// Remove prefix from the source file's path.
fn remove_prefix(prefix_dir: Option<&Path>, path: PathBuf) -> PathBuf {
    if let Some(prefix_dir) = prefix_dir {
        if path.starts_with(prefix_dir) {
            return path.strip_prefix(prefix_dir).unwrap().to_path_buf();
        }
    }

    path
}

fn fixup_rel_path(source_dir: Option<&Path>, abs_path: &Path, rel_path: PathBuf) -> PathBuf {
    if let Some(ref source_dir) = source_dir {
        if abs_path.starts_with(source_dir) {
            return abs_path.strip_prefix(source_dir).unwrap().to_path_buf();
        } else if !rel_path.is_relative() {
            return abs_path.to_owned();
        }
    }

    rel_path
}

// Get the absolute path for the source file's path, resolving symlinks.
fn get_abs_path(source_dir: Option<&Path>, rel_path: PathBuf) -> Option<(PathBuf, PathBuf)> {
    let mut abs_path = if !rel_path.is_relative() {
        rel_path.to_owned()
    } else if let Some(source_dir) = source_dir {
        if !cfg!(windows) {
            guess_abs_path(source_dir, &rel_path)
        } else {
            guess_abs_path(
                source_dir,
                &PathBuf::from(&rel_path.to_str().unwrap().replace('/', "\\")),
            )
        }
    } else {
        rel_path.to_owned()
    };

    // Canonicalize, if possible.
    if let Ok(p) = canonicalize_path(&abs_path) {
        abs_path = p;
    }

    // Fixup the relative path, in case the absolute path was a symlink.
    let rel_path = fixup_rel_path(source_dir, &abs_path, rel_path);

    // Normalize the path in removing './' or '//' or '..'
    let rel_path = normalize_path(rel_path);
    let abs_path = normalize_path(abs_path);

    abs_path.zip(rel_path)
}

fn check_extension(path: &Path, e: &str) -> bool {
    if let Some(ext) = &path.extension() {
        if let Some(ext) = ext.to_str() {
            ext == e
        } else {
            false
        }
    } else {
        false
    }
}

fn map_partial_path(file_to_paths: &FxHashMap<String, Vec<PathBuf>>, path: PathBuf) -> PathBuf {
    let options = file_to_paths.get(path.file_name().unwrap().to_str().unwrap());

    if options.is_none() {
        return path;
    }

    let options = options.unwrap();

    if options.len() == 1 {
        return options[0].clone();
    }

    let mut result: Option<&PathBuf> = None;
    for option in options {
        if option.ends_with(&path) {
            assert!(
                result.is_none(),
                "Only one file in the repository should end with {} ({} and {} both end with that)",
                path.display(),
                result.unwrap().display(),
                option.display()
            );
            result = Some(option)
        }
    }

    if let Some(result) = result {
        result.clone()
    } else {
        path
    }
}

fn is_hidden(entry: &DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map(|s| s.starts_with('.'))
        .unwrap_or(false)
}

fn is_symbolic_link(entry: &DirEntry) -> bool {
    entry.path_is_symlink()
}

fn to_globset(dirs: &[impl AsRef<str>]) -> GlobSet {
    let mut glob_builder = GlobSetBuilder::new();

    for dir in dirs {
        glob_builder.add(Glob::new(dir.as_ref()).unwrap());
    }

    glob_builder.build().unwrap()
}

pub fn rewrite_paths(
    result_map: CovResultMap,
    path_mapping: Option<Value>,
    source_dir: Option<&Path>,
    prefix_dir: Option<&Path>,
    ignore_not_existing: bool,
    to_ignore_dirs: &[impl AsRef<str>],
    to_keep_dirs: &[impl AsRef<str>],
    filter_option: Option<bool>,
    file_filter: crate::FileFilter,
) -> Vec<ResultTuple> {
    let to_ignore_globset = to_globset(to_ignore_dirs);
    let to_keep_globset = to_globset(to_keep_dirs);

    if let Some(p) = &source_dir {
        assert!(p.is_absolute());
    }

    // Traverse source dir and store all paths, reversed.
    let mut file_to_paths: FxHashMap<String, Vec<PathBuf>> = FxHashMap::default();
    if let Some(ref source_dir) = source_dir {
        for entry in WalkDir::new(source_dir)
            .into_iter()
            .filter_entry(|e| !is_hidden(e) && !is_symbolic_link(e))
        {
            let entry = entry
                .unwrap_or_else(|_| panic!("Failed to open directory '{}'.", source_dir.display()));

            let full_path = entry.path();
            if !full_path.is_file() {
                continue;
            }

            let path = full_path.strip_prefix(source_dir).unwrap().to_path_buf();
            if to_ignore_globset.is_match(&path) {
                continue;
            }

            let name = entry.file_name().to_str().unwrap().to_string();
            match file_to_paths.entry(name) {
                hash_map::Entry::Occupied(f) => f.into_mut().push(path),
                hash_map::Entry::Vacant(v) => {
                    v.insert(vec![path]);
                }
            };
        }
    }

    let results = result_map
        .into_par_iter()
        .filter_map(move |(path, mut result)| {
            let path = path.replace('\\', "/");

            // Get path from the mapping.
            let rel_path = apply_mapping(&path_mapping, &path);

            // Remove prefix from the path.
            let rel_path = remove_prefix(prefix_dir, rel_path);

            // Try mapping a partial path to a full path.
            let rel_path = if check_extension(&rel_path, "java") {
                map_partial_path(&file_to_paths, rel_path)
            } else {
                rel_path
            };

            // Get absolute path to the source file.
            let (abs_path, rel_path) = get_abs_path(source_dir, rel_path)?;

            if to_ignore_globset.is_match(&rel_path) {
                return None;
            }

            if !to_keep_globset.is_empty() && !to_keep_globset.is_match(&rel_path) {
                return None;
            }

            if ignore_not_existing && !abs_path.exists() {
                return None;
            }

            // Always return results with '/'.
            let rel_path = PathBuf::from(rel_path.to_str().unwrap().replace('\\', "/"));

            for filter in file_filter.create(&abs_path) {
                match filter {
                    crate::FilterType::Both(number) => {
                        result.branches.remove(&number);
                        result.lines.remove(&number);
                    }
                    crate::FilterType::Line(number) => {
                        result.lines.remove(&number);
                    }
                    crate::FilterType::Branch(number) => {
                        result.branches.remove(&number);
                    }
                }
            }

            match filter_option {
                Some(true) => {
                    if !is_covered(&result) {
                        return None;
                    }
                }
                Some(false) => {
                    if is_covered(&result) {
                        return None;
                    }
                }
                None => (),
            };

            Some((abs_path, rel_path, result))
        });

    results.collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn test_to_lowercase_first() {
        assert_eq!(to_lowercase_first("marco"), "marco");
        assert_eq!(to_lowercase_first("Marco"), "marco");
    }

    #[test]
    #[should_panic]
    fn test_to_lowercase_first_empty() {
        to_lowercase_first("");
    }

    #[test]
    fn test_to_uppercase_first() {
        assert_eq!(to_uppercase_first("marco"), "Marco");
        assert_eq!(to_uppercase_first("Marco"), "Marco");
    }

    #[test]
    #[should_panic]
    fn test_to_uppercase_first_empty() {
        to_uppercase_first("");
    }

    macro_rules! empty_result {
        () => {{
            CovResult {
                lines: BTreeMap::new(),
                branches: BTreeMap::new(),
                functions: FxHashMap::default(),
            }
        }};
    }

    macro_rules! covered_result {
        () => {{
            CovResult {
                lines: [(42, 1)].iter().cloned().collect(),
                branches: BTreeMap::new(),
                functions: FxHashMap::default(),
            }
        }};
    }

    macro_rules! uncovered_result {
        () => {{
            CovResult {
                lines: [(42, 0)].iter().cloned().collect(),
                branches: BTreeMap::new(),
                functions: FxHashMap::default(),
            }
        }};
    }

    macro_rules! skipping_result {
        () => {{
            let mut result = empty_result!();
            for i in 1..20 {
                result.lines.insert(i, 1);
                result.branches.insert(i, vec![true]);
            }
            result
        }};
    }

    #[test]
    fn test_rewrite_paths_basic() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert("main.cpp".to_string(), empty_result!());
        let results = rewrite_paths(
            result_map,
            None,
            None,
            None,
            false,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert_eq!(abs_path, PathBuf::from("main.cpp"));
            assert_eq!(rel_path, PathBuf::from("main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[cfg(unix)]
    #[test]
    fn test_rewrite_paths_remove_prefix() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert(
            "/home/worker/src/workspace/main.cpp".to_string(),
            empty_result!(),
        );
        let results = rewrite_paths(
            result_map,
            None,
            None,
            Some(Path::new("/home/worker/src/workspace/")),
            false,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert_eq!(abs_path, PathBuf::from("main.cpp"));
            assert_eq!(rel_path, PathBuf::from("main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[cfg(windows)]
    #[test]
    fn test_rewrite_paths_remove_prefix() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert(
            "C:\\Users\\worker\\src\\workspace\\main.cpp".to_string(),
            empty_result!(),
        );
        let results = rewrite_paths(
            result_map,
            None,
            None,
            Some(Path::new("C:\\Users\\worker\\src\\workspace\\")),
            false,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert_eq!(abs_path, PathBuf::from("main.cpp"));
            assert_eq!(rel_path, PathBuf::from("main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[cfg(windows)]
    #[test]
    fn test_rewrite_paths_remove_prefix_with_slash() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert(
            "C:/Users/worker/src/workspace/main.cpp".to_string(),
            empty_result!(),
        );
        let results = rewrite_paths(
            result_map,
            None,
            None,
            Some(Path::new("C:/Users/worker/src/workspace/")),
            false,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert_eq!(abs_path, PathBuf::from("main.cpp"));
            assert_eq!(rel_path, PathBuf::from("main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[cfg(windows)]
    #[test]
    fn test_rewrite_paths_remove_prefix_with_slash_longer_path() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert(
            "C:/Users/worker/src/workspace/main.cpp".to_string(),
            empty_result!(),
        );
        let results = rewrite_paths(
            result_map,
            None,
            None,
            Some(Path::new("C:/Users/worker/src/")),
            false,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert_eq!(abs_path, PathBuf::from("workspace/main.cpp"));
            assert_eq!(rel_path.to_str().unwrap(), "workspace/main.cpp");
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[cfg(unix)]
    #[test]
    fn test_rewrite_paths_ignore_non_existing_files() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert("tests/class/main.cpp".to_string(), empty_result!());
        result_map.insert("tests/class/doesntexist.cpp".to_string(), empty_result!());
        let results = rewrite_paths(
            result_map,
            None,
            None,
            None,
            true,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(
                abs_path.is_absolute(),
                "{} is not absolute",
                abs_path.display()
            );
            assert!(abs_path.ends_with("tests/class/main.cpp"));
            assert!(rel_path.ends_with("tests/class/main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[cfg(windows)]
    #[test]
    fn test_rewrite_paths_ignore_non_existing_files() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert("tests\\class\\main.cpp".to_string(), empty_result!());
        result_map.insert("tests\\class\\doesntexist.cpp".to_string(), empty_result!());
        let results = rewrite_paths(
            result_map,
            None,
            None,
            None,
            true,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests\\class\\main.cpp"));
            assert!(rel_path.ends_with("tests\\class\\main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[cfg(unix)]
    #[test]
    fn test_rewrite_paths_ignore_a_directory() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert("main.cpp".to_string(), empty_result!());
        result_map.insert("mydir/prova.h".to_string(), empty_result!());
        let results = rewrite_paths(
            result_map,
            None,
            None,
            None,
            false,
            &["mydir/*"],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert_eq!(abs_path, PathBuf::from("main.cpp"));
            assert_eq!(rel_path, PathBuf::from("main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[cfg(windows)]
    #[test]
    fn test_rewrite_paths_ignore_a_directory() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert("main.cpp".to_string(), empty_result!());
        result_map.insert("mydir\\prova.h".to_string(), empty_result!());
        let results = rewrite_paths(
            result_map,
            None,
            None,
            None,
            false,
            &["mydir/*"],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert_eq!(abs_path, PathBuf::from("main.cpp"));
            assert_eq!(rel_path, PathBuf::from("main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[cfg(unix)]
    #[test]
    fn test_rewrite_paths_ignore_multiple_directories() {
        let mut ignore_dirs = vec!["mydir/*", "mydir2/*"];
        for _ in 0..2 {
            // we run the test twice, one with ignore_dirs and the other with ignore_dirs.reverse()
            let mut result_map: CovResultMap = FxHashMap::default();
            result_map.insert("main.cpp".to_string(), empty_result!());
            result_map.insert("mydir/prova.h".to_string(), empty_result!());
            result_map.insert("mydir2/prova.h".to_string(), empty_result!());
            let results = rewrite_paths(
                result_map,
                None,
                None,
                None,
                false,
                &ignore_dirs,
                &[""; 0],
                None,
                Default::default(),
            );
            let mut count = 0;
            for (abs_path, rel_path, result) in results {
                count += 1;
                assert_eq!(abs_path, PathBuf::from("main.cpp"));
                assert_eq!(rel_path, PathBuf::from("main.cpp"));
                assert_eq!(result, empty_result!());
            }
            assert_eq!(count, 1);
            ignore_dirs.reverse();
        }
    }

    #[cfg(windows)]
    #[test]
    fn test_rewrite_paths_ignore_multiple_directories() {
        let mut ignore_dirs = vec!["mydir/*", "mydir2/*"];
        for _ in 0..2 {
            // we run the test twice, one with ignore_dirs and the other with ignore_dirs.reverse()
            let mut result_map: CovResultMap = FxHashMap::default();
            result_map.insert("main.cpp".to_string(), empty_result!());
            result_map.insert("mydir\\prova.h".to_string(), empty_result!());
            result_map.insert("mydir2\\prova.h".to_string(), empty_result!());
            let results = rewrite_paths(
                result_map,
                None,
                None,
                None,
                false,
                &ignore_dirs,
                &[""; 0],
                None,
                Default::default(),
            );
            let mut count = 0;
            for (abs_path, rel_path, result) in results {
                count += 1;
                assert_eq!(abs_path, PathBuf::from("main.cpp"));
                assert_eq!(rel_path, PathBuf::from("main.cpp"));
                assert_eq!(result, empty_result!());
            }
            assert_eq!(count, 1);
            ignore_dirs.reverse();
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_rewrite_paths_keep_only_a_directory() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert("main.cpp".to_string(), empty_result!());
        result_map.insert("mydir/prova.h".to_string(), empty_result!());
        let results = rewrite_paths(
            result_map,
            None,
            None,
            None,
            false,
            &[""; 0],
            &["mydir/*"],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert_eq!(abs_path, PathBuf::from("mydir/prova.h"));
            assert_eq!(rel_path, PathBuf::from("mydir/prova.h"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[cfg(windows)]
    #[test]
    fn test_rewrite_paths_keep_only_a_directory() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert("main.cpp".to_string(), empty_result!());
        result_map.insert("mydir\\prova.h".to_string(), empty_result!());
        let results = rewrite_paths(
            result_map,
            None,
            None,
            None,
            false,
            &[""; 0],
            &["mydir/*"],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert_eq!(abs_path, PathBuf::from("mydir\\prova.h"));
            assert_eq!(rel_path, PathBuf::from("mydir\\prova.h"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[cfg(unix)]
    #[test]
    fn test_rewrite_paths_keep_only_multiple_directories() {
        let mut keep_only_dirs = vec!["mydir/*", "mydir2/*"];
        for _ in 0..2 {
            // we run the test twice, one with keep_only_dirs and the other with keep_only_dirs.reverse()
            let mut result_map: CovResultMap = FxHashMap::default();
            result_map.insert("main.cpp".to_string(), empty_result!());
            result_map.insert("mydir/prova.h".to_string(), empty_result!());
            result_map.insert("mydir2/prova.h".to_string(), empty_result!());
            let results = rewrite_paths(
                result_map,
                None,
                None,
                None,
                false,
                &[""; 0],
                &keep_only_dirs,
                None,
                Default::default(),
            );
            let mut count = 0;
            for (abs_path, rel_path, result) in results {
                count += 1;
                assert_ne!(abs_path, PathBuf::from("main.cpp"));
                assert_ne!(rel_path, PathBuf::from("main.cpp"));
                assert_eq!(result, empty_result!());
            }
            assert_eq!(count, 2);
            keep_only_dirs.reverse();
        }
    }

    #[cfg(windows)]
    #[test]
    fn test_rewrite_paths_keep_only_multiple_directories() {
        let mut keep_only_dirs = vec!["mydir/*", "mydir2/*"];
        for _ in 0..2 {
            // we run the test twice, one with keep_only_dirs and the other with keep_only_dirs.reverse()
            let mut result_map: CovResultMap = FxHashMap::default();
            result_map.insert("main.cpp".to_string(), empty_result!());
            result_map.insert("mydir\\prova.h".to_string(), empty_result!());
            result_map.insert("mydir2\\prova.h".to_string(), empty_result!());
            let results = rewrite_paths(
                result_map,
                None,
                None,
                None,
                false,
                &[""; 0],
                &keep_only_dirs,
                None,
                Default::default(),
            );
            let mut count = 0;
            for (abs_path, rel_path, result) in results {
                count += 1;
                assert_ne!(abs_path, PathBuf::from("main.cpp"));
                assert_ne!(rel_path, PathBuf::from("main.cpp"));
                assert_eq!(result, empty_result!());
            }
            assert_eq!(count, 2);
            keep_only_dirs.reverse();
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_rewrite_paths_keep_only_and_ignore() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert("main.rs".to_string(), empty_result!());
        result_map.insert("foo/keep.rs".to_string(), empty_result!());
        result_map.insert("foo/not_keep.cpp".to_string(), empty_result!());
        result_map.insert("foo/bar_ignore.rs".to_string(), empty_result!());
        let results = rewrite_paths(
            result_map,
            None,
            None,
            None,
            false,
            &["foo/bar_*.rs"],
            &["foo/*.rs"],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert_eq!(abs_path, PathBuf::from("foo/keep.rs"));
            assert_eq!(rel_path, PathBuf::from("foo/keep.rs"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[cfg(windows)]
    #[test]
    fn test_rewrite_paths_keep_only_and_ignore() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert("main.rs".to_string(), empty_result!());
        result_map.insert("foo\\keep.rs".to_string(), empty_result!());
        result_map.insert("foo\\not_keep.cpp".to_string(), empty_result!());
        result_map.insert("foo\\bar_ignore.rs".to_string(), empty_result!());
        let results = rewrite_paths(
            result_map,
            None,
            None,
            None,
            false,
            &["foo/bar_*.rs"],
            &["foo/*.rs"],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert_eq!(abs_path, PathBuf::from("foo\\keep.rs"));
            assert_eq!(rel_path, PathBuf::from("foo\\keep.rs"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[test]
    #[should_panic]
    fn test_rewrite_paths_rewrite_path_using_relative_source_directory() {
        let result_map: CovResultMap = FxHashMap::default();
        rewrite_paths(
            result_map,
            None,
            Some(Path::new("tests")),
            None,
            true,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        )
        .iter()
        .any(|_| false);
    }

    #[cfg(unix)]
    #[test]
    fn test_rewrite_paths_rewrite_path_using_absolute_source_directory() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert("java/main.java".to_string(), empty_result!());
        result_map.insert("test/java/main.java".to_string(), empty_result!());
        let results = rewrite_paths(
            result_map,
            None,
            Some(&canonicalize_path("test").unwrap()),
            None,
            true,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("test/java/main.java"));
            assert_eq!(rel_path, PathBuf::from("java/main.java"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 2);
    }

    #[cfg(windows)]
    #[test]
    fn test_rewrite_paths_rewrite_path_using_absolute_source_directory() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert("java\\main.java".to_string(), empty_result!());
        result_map.insert("test\\java\\main.java".to_string(), empty_result!());
        let results = rewrite_paths(
            result_map,
            None,
            Some(&canonicalize_path("test").unwrap()),
            None,
            true,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("test\\java\\main.java"));
            assert_eq!(rel_path, PathBuf::from("java\\main.java"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 2);
    }

    #[cfg(unix)]
    #[test]
    fn test_rewrite_paths_subfolder_same_as_root() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert("test/main.rs".to_string(), empty_result!());
        let results = rewrite_paths(
            result_map,
            None,
            Some(&canonicalize_path("test").unwrap()),
            None,
            false,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("test/test/main.rs"));
            assert_eq!(rel_path, PathBuf::from("test/main.rs"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[cfg(windows)]
    #[test]
    fn test_rewrite_paths_subfolder_same_as_root() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert("test\\main.rs".to_string(), empty_result!());
        let results = rewrite_paths(
            result_map,
            None,
            Some(&canonicalize_path("test").unwrap()),
            None,
            false,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("test\\test\\main.rs"));
            assert_eq!(rel_path, PathBuf::from("test\\main.rs"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[cfg(unix)]
    #[test]
    fn test_rewrite_paths_rewrite_path_for_java_and_rust() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert("java/main.java".to_string(), empty_result!());
        result_map.insert("main.rs".to_string(), empty_result!());
        let mut results = rewrite_paths(
            result_map,
            None,
            Some(&canonicalize_path(".").unwrap()),
            None,
            true,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        assert!(results.len() == 1);

        let (abs_path, rel_path, result) = results.remove(0);
        assert!(abs_path.is_absolute());
        assert!(abs_path.ends_with("test/java/main.java"));
        assert_eq!(rel_path, PathBuf::from("test/java/main.java"));
        assert_eq!(result, empty_result!());
    }

    #[cfg(windows)]
    #[test]
    fn test_rewrite_paths_rewrite_path_for_java_and_rust() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert("java\\main.java".to_string(), empty_result!());
        result_map.insert("main.rs".to_string(), empty_result!());
        let mut results = rewrite_paths(
            result_map,
            None,
            Some(&canonicalize_path(".").unwrap()),
            None,
            true,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        assert!(results.len() == 1);

        let (abs_path, rel_path, result) = results.remove(0);
        assert!(abs_path.is_absolute());
        assert!(abs_path.ends_with("test\\java\\main.java"));
        assert_eq!(rel_path, PathBuf::from("test\\java\\main.java"));
        assert_eq!(result, empty_result!());
    }

    #[cfg(unix)]
    #[test]
    fn test_rewrite_paths_rewrite_path_using_absolute_source_directory_and_partial_path() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert("java/main.java".to_string(), empty_result!());
        let results = rewrite_paths(
            result_map,
            None,
            Some(&canonicalize_path(".").unwrap()),
            None,
            true,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("test/java/main.java"));
            assert_eq!(rel_path, PathBuf::from("test/java/main.java"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[cfg(windows)]
    #[test]
    fn test_rewrite_paths_rewrite_path_using_absolute_source_directory_and_partial_path() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert("java\\main.java".to_string(), empty_result!());
        let results = rewrite_paths(
            result_map,
            None,
            Some(&canonicalize_path(".").unwrap()),
            None,
            true,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("test\\java\\main.java"));
            assert_eq!(rel_path, PathBuf::from("test\\java\\main.java"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[cfg(unix)]
    #[test]
    fn test_rewrite_paths_rewrite_path_and_remove_prefix() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert(
            "/home/worker/src/workspace/class/main.cpp".to_string(),
            empty_result!(),
        );
        let results = rewrite_paths(
            result_map,
            None,
            Some(&canonicalize_path("tests").unwrap()),
            Some(Path::new("/home/worker/src/workspace")),
            true,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests/class/main.cpp"));
            eprintln!("{:?}", rel_path);
            assert_eq!(rel_path, PathBuf::from("class/main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[cfg(windows)]
    #[test]
    fn test_rewrite_paths_rewrite_path_and_remove_prefix() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert(
            "C:\\Users\\worker\\src\\workspace\\class\\main.cpp".to_string(),
            empty_result!(),
        );
        let results = rewrite_paths(
            result_map,
            None,
            Some(&canonicalize_path("tests").unwrap()),
            Some(Path::new("C:\\Users\\worker\\src\\workspace")),
            true,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests\\class\\main.cpp"));
            assert_eq!(rel_path, PathBuf::from("class\\main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[cfg(unix)]
    #[test]
    fn test_rewrite_paths_rewrite_path_using_mapping() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert("class/main.cpp".to_string(), empty_result!());
        let results = rewrite_paths(
            result_map,
            Some(json!({"class/main.cpp": "rewritten/main.cpp"})),
            None,
            None,
            false,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert_eq!(abs_path, PathBuf::from("rewritten/main.cpp"));
            assert_eq!(rel_path, PathBuf::from("rewritten/main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[cfg(windows)]
    #[test]
    fn test_rewrite_paths_rewrite_path_using_mapping() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert("class\\main.cpp".to_string(), empty_result!());
        let results = rewrite_paths(
            result_map,
            Some(json!({"class/main.cpp": "rewritten/main.cpp"})),
            None,
            None,
            false,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert_eq!(abs_path, PathBuf::from("rewritten\\main.cpp"));
            assert_eq!(rel_path, PathBuf::from("rewritten\\main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[cfg(unix)]
    #[test]
    fn test_rewrite_paths_rewrite_path_using_mapping_and_ignore_non_existing() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert("rewritten/main.cpp".to_string(), empty_result!());
        result_map.insert("tests/class/main.cpp".to_string(), empty_result!());
        let results = rewrite_paths(
            result_map,
            Some(
                json!({"rewritten/main.cpp": "tests/class/main.cpp", "tests/class/main.cpp": "rewritten/main.cpp"}),
            ),
            None,
            None,
            true,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests/class/main.cpp"));
            assert_eq!(rel_path, PathBuf::from("tests/class/main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[cfg(windows)]
    #[test]
    fn test_rewrite_paths_rewrite_path_using_mapping_and_ignore_non_existing() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert("rewritten\\main.cpp".to_string(), empty_result!());
        result_map.insert("tests\\class\\main.cpp".to_string(), empty_result!());
        let results = rewrite_paths(
            result_map,
            Some(
                json!({"rewritten/main.cpp": "tests/class/main.cpp", "tests/class/main.cpp": "rewritten/main.cpp"}),
            ),
            None,
            None,
            true,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests\\class\\main.cpp"));
            assert_eq!(rel_path, PathBuf::from("tests\\class\\main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[cfg(unix)]
    #[test]
    fn test_rewrite_paths_rewrite_path_using_mapping_and_remove_prefix() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert(
            "/home/worker/src/workspace/rewritten/main.cpp".to_string(),
            empty_result!(),
        );
        let results = rewrite_paths(
            result_map,
            Some(json!({"/home/worker/src/workspace/rewritten/main.cpp": "tests/class/main.cpp"})),
            None,
            Some(Path::new("/home/worker/src/workspace")),
            true,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests/class/main.cpp"));
            assert_eq!(rel_path, PathBuf::from("tests/class/main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[cfg(windows)]
    #[test]
    fn test_rewrite_paths_rewrite_path_using_mapping_and_remove_prefix() {
        // Mapping with uppercase disk and prefix with uppercase disk.
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert(
            "C:\\Users\\worker\\src\\workspace\\rewritten\\main.cpp".to_string(),
            empty_result!(),
        );
        let results = rewrite_paths(
            result_map,
            Some(
                json!({"C:/Users/worker/src/workspace/rewritten/main.cpp": "tests/class/main.cpp"}),
            ),
            None,
            Some(Path::new("C:\\Users\\worker\\src\\workspace")),
            true,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests\\class\\main.cpp"));
            assert_eq!(rel_path, PathBuf::from("tests\\class\\main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);

        // Mapping with lowercase disk and prefix with uppercase disk.
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert(
            "C:\\Users\\worker\\src\\workspace\\rewritten\\main.cpp".to_string(),
            empty_result!(),
        );
        let results = rewrite_paths(
            result_map,
            Some(
                json!({"c:/Users/worker/src/workspace/rewritten/main.cpp": "tests/class/main.cpp"}),
            ),
            None,
            Some(Path::new("C:\\Users\\worker\\src\\workspace")),
            true,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests\\class\\main.cpp"));
            assert_eq!(rel_path, PathBuf::from("tests\\class\\main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);

        // Mapping with uppercase disk and prefix with lowercase disk.
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert(
            "C:\\Users\\worker\\src\\workspace\\rewritten\\main.cpp".to_string(),
            empty_result!(),
        );
        let results = rewrite_paths(
            result_map,
            Some(
                json!({"C:/Users/worker/src/workspace/rewritten/main.cpp": "tests/class/main.cpp"}),
            ),
            None,
            Some(Path::new("c:\\Users\\worker\\src\\workspace")),
            true,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests\\class\\main.cpp"));
            assert_eq!(rel_path, PathBuf::from("tests\\class\\main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);

        // Mapping with lowercase disk and prefix with lowercase disk.
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert(
            "C:\\Users\\worker\\src\\workspace\\rewritten\\main.cpp".to_string(),
            empty_result!(),
        );
        let results = rewrite_paths(
            result_map,
            Some(
                json!({"c:/Users/worker/src/workspace/rewritten/main.cpp": "tests/class/main.cpp"}),
            ),
            None,
            Some(Path::new("c:\\Users\\worker\\src\\workspace")),
            true,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests\\class\\main.cpp"));
            assert_eq!(rel_path, PathBuf::from("tests\\class\\main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[cfg(unix)]
    #[test]
    fn test_rewrite_paths_rewrite_path_using_mapping_and_source_directory_and_remove_prefix() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert(
            "/home/worker/src/workspace/rewritten/main.cpp".to_string(),
            empty_result!(),
        );
        let results = rewrite_paths(
            result_map,
            Some(json!({"/home/worker/src/workspace/rewritten/main.cpp": "class/main.cpp"})),
            Some(&canonicalize_path("tests").unwrap()),
            Some(Path::new("/home/worker/src/workspace")),
            true,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests/class/main.cpp"));
            assert_eq!(rel_path, PathBuf::from("class/main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[cfg(windows)]
    #[test]
    fn test_rewrite_paths_rewrite_path_using_mapping_and_source_directory_and_remove_prefix() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert(
            "C:\\Users\\worker\\src\\workspace\\rewritten\\main.cpp".to_string(),
            empty_result!(),
        );
        let results = rewrite_paths(
            result_map,
            Some(json!({"C:/Users/worker/src/workspace/rewritten/main.cpp": "class/main.cpp"})),
            Some(&canonicalize_path("tests").unwrap()),
            Some(Path::new("C:\\Users\\worker\\src\\workspace")),
            true,
            &[""; 0],
            &[""; 0],
            None,
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert!(abs_path.is_absolute());
            assert!(abs_path.ends_with("tests\\class\\main.cpp"));
            assert_eq!(rel_path, PathBuf::from("class\\main.cpp"));
            assert_eq!(result, empty_result!());
        }
        assert_eq!(count, 1);
    }

    #[test]
    fn test_rewrite_paths_only_covered() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert("covered.cpp".to_string(), covered_result!());
        result_map.insert("uncovered.cpp".to_string(), uncovered_result!());
        let results = rewrite_paths(
            result_map,
            None,
            None,
            None,
            false,
            &[""; 0],
            &[""; 0],
            Some(true),
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert_eq!(abs_path, PathBuf::from("covered.cpp"));
            assert_eq!(rel_path, PathBuf::from("covered.cpp"));
            assert_eq!(result, covered_result!());
        }
        assert_eq!(count, 1);
    }

    #[test]
    fn test_rewrite_paths_only_uncovered() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert("covered.cpp".to_string(), covered_result!());
        result_map.insert("uncovered.cpp".to_string(), uncovered_result!());
        let results = rewrite_paths(
            result_map,
            None,
            None,
            None,
            false,
            &[""; 0],
            &[""; 0],
            Some(false),
            Default::default(),
        );
        let mut count = 0;
        for (abs_path, rel_path, result) in results {
            count += 1;
            assert_eq!(abs_path, PathBuf::from("uncovered.cpp"));
            assert_eq!(rel_path, PathBuf::from("uncovered.cpp"));
            assert_eq!(result, uncovered_result!());
        }
        assert_eq!(count, 1);
    }

    #[test]
    fn test_normalize_path() {
        assert_eq!(
            normalize_path("./foo/bar").unwrap(),
            PathBuf::from("foo/bar")
        );
        assert_eq!(
            normalize_path("./foo//bar").unwrap(),
            PathBuf::from("foo/bar")
        );
        assert_eq!(
            normalize_path("./foo/./bar/./oof/").unwrap(),
            PathBuf::from("foo/bar/oof")
        );
        assert_eq!(
            normalize_path("./foo/../bar/./oof/").unwrap(),
            PathBuf::from("bar/oof")
        );
        assert!(normalize_path("../bar/oof/").is_none());
        assert!(normalize_path("bar/foo/../../../oof/").is_none());
    }

    #[test]
    fn test_has_no_parent() {
        assert!(has_no_parent("foo.bar"));
        assert!(has_no_parent("foo"));
        assert!(!has_no_parent("/foo.bar"));
        assert!(!has_no_parent("./foo.bar"));
        assert!(!has_no_parent("../foo.bar"));
        assert!(!has_no_parent("foo/foo.bar"));
        assert!(!has_no_parent("bar/foo/foo.bar"));
        assert!(!has_no_parent("/"));
        assert!(!has_no_parent("/foo/bar.oof"));
    }

    #[cfg(unix)]
    #[test]
    fn test_rewrite_paths_filter_lines_and_branches() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert("test/java/skip.java".to_string(), skipping_result!());
        let results = rewrite_paths(
            result_map,
            None,
            Some(&canonicalize_path("test").unwrap()),
            None,
            true,
            &[""; 0],
            &[""; 0],
            None,
            crate::FileFilter::new(
                Some(regex::Regex::new("excluded line").unwrap()),
                Some(regex::Regex::new("skip line start").unwrap()),
                Some(regex::Regex::new("skip line end").unwrap()),
                Some(regex::Regex::new("excluded branch").unwrap()),
                Some(regex::Regex::new("skip branch start").unwrap()),
                Some(regex::Regex::new("skip branch end").unwrap()),
            ),
        );
        let mut count = 0;
        for (_, _, result) in results {
            count += 1;
            for inc in [1, 2, 3, 5, 8, 9, 10, 11, 12, 13, 14, 15, 16].iter() {
                assert!(result.lines.contains_key(inc));
            }
            for inc in [4, 6, 7, 17, 18, 19, 20].iter() {
                assert!(!result.lines.contains_key(inc));
            }

            for inc in [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 12, 15, 16, 17].iter() {
                assert!(result.branches.contains_key(inc));
            }
            for inc in [11, 13, 14, 18, 19, 20].iter() {
                assert!(!result.branches.contains_key(inc));
            }
        }
        assert_eq!(count, 1);
    }

    #[cfg(windows)]
    #[test]
    fn test_rewrite_paths_filter_lines_and_branches() {
        let mut result_map: CovResultMap = FxHashMap::default();
        result_map.insert("test\\java\\skip.java".to_string(), skipping_result!());
        let results = rewrite_paths(
            result_map,
            None,
            Some(&canonicalize_path("test").unwrap()),
            None,
            true,
            &[""; 0],
            &[""; 0],
            None,
            crate::FileFilter::new(
                Some(regex::Regex::new("excluded line").unwrap()),
                Some(regex::Regex::new("skip line start").unwrap()),
                Some(regex::Regex::new("skip line end").unwrap()),
                Some(regex::Regex::new("excluded branch").unwrap()),
                Some(regex::Regex::new("skip branch start").unwrap()),
                Some(regex::Regex::new("skip branch end").unwrap()),
            ),
        );
        let mut count = 0;
        for (_, _, result) in results {
            count += 1;
            for inc in [1, 2, 3, 5, 8, 9, 10, 11, 12, 13, 14, 15, 16].iter() {
                assert!(result.lines.contains_key(inc));
            }
            for inc in [4, 6, 7, 17, 18, 19, 20].iter() {
                assert!(!result.lines.contains_key(inc));
            }

            for inc in [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 12, 15, 16, 17].iter() {
                assert!(result.branches.contains_key(inc));
            }
            for inc in [11, 13, 14, 18, 19, 20].iter() {
                assert!(!result.branches.contains_key(inc));
            }
        }
        assert_eq!(count, 1);
    }
}
