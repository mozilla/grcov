use std::path::PathBuf;

use crate::CovResult;

#[derive(Debug, Default)]
struct CoverageStats {
    lines_covered: i64,
    lines_valid: i64,
    branches_covered: i64,
    branches_valid: i64,
}

fn get_coverage_stats(results: &[(PathBuf, PathBuf, CovResult)]) -> CoverageStats {
    results
        .iter()
        .fold(CoverageStats::default(), |stats, (_, _, result)| {
            let (lines_covered, lines_valid) = result.lines.values().fold(
                (stats.lines_covered, stats.lines_valid),
                |(covered, valid), l| {
                    if *l == 0 {
                        (covered, valid + 1)
                    } else {
                        (covered + 1, valid + 1)
                    }
                },
            );
            let (branches_covered, branches_valid) = result.branches.values().fold(
                (stats.branches_covered, stats.branches_valid),
                |(covered, valid), branches| {
                    branches
                        .iter()
                        .fold((covered, valid), |(covered, valid), b| {
                            if *b {
                                (covered + 1, valid + 1)
                            } else {
                                (covered, valid + 1)
                            }
                        })
                },
            );
            CoverageStats {
                lines_covered,
                lines_valid,
                branches_covered,
                branches_valid,
            }
        })
}

pub fn print_summary(results: &[(PathBuf, PathBuf, CovResult)]) {
    let stats = get_coverage_stats(results);
    let lines_percentage = if stats.lines_valid == 0 {
        0.0
    } else {
        (stats.lines_covered as f64 / stats.lines_valid as f64) * 100.0
    };
    let branches_percentage = if stats.branches_valid == 0 {
        0.0
    } else {
        (stats.branches_covered as f64 / stats.branches_valid as f64) * 100.0
    };
    println!(
        "lines: {:.1}% ({} out of {})",
        lines_percentage, stats.lines_covered, stats.lines_valid
    );
    println!(
        "branches: {:.1}% ({} out of {})",
        branches_percentage, stats.branches_covered, stats.branches_valid
    );
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf};

    use rustc_hash::FxHashMap;

    use crate::{CovResult, Function};

    use super::get_coverage_stats;

    #[test]
    fn test_summary() {
        let results = vec![(
            PathBuf::from("src/main.rs"),
            PathBuf::from("src/main.rs"),
            CovResult {
                /* main.rs
                  fn main() {
                      let inp = "a";
                      if "a" == inp {
                          println!("a");
                      } else if "b" == inp {
                          println!("b");
                      }
                      println!("what?");
                  }
                */
                lines: [
                    (1, 1),
                    (2, 1),
                    (3, 2),
                    (4, 1),
                    (5, 0),
                    (6, 0),
                    (8, 1),
                    (9, 1),
                ]
                .iter()
                .cloned()
                .collect(),
                branches: {
                    let mut map = BTreeMap::new();
                    map.insert(3, vec![true, false]);
                    map.insert(5, vec![false, false]);
                    map
                },
                functions: {
                    let mut map = FxHashMap::default();
                    map.insert(
                        "_ZN8cov_test4main17h7eb435a3fb3e6f20E".to_string(),
                        Function {
                            start: 1,
                            executed: true,
                        },
                    );
                    map
                },
            },
        )];

        let stats = get_coverage_stats(&results);
        assert_eq!(stats.lines_covered, 6);
        assert_eq!(stats.lines_valid, 8);
        assert_eq!(stats.branches_covered, 1);
        assert_eq!(stats.branches_valid, 4);
    }
}
