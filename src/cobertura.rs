use crate::defs::*;
use quick_xml::{
    events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event},
    Writer,
};
use rustc_hash::FxHashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{
    fmt::Display,
    io::{BufWriter, Cursor, Write},
};
use std::{fmt::Formatter, path::Path};
use symbolic_common::Name;
use symbolic_demangle::{Demangle, DemangleOptions};

use crate::output::get_target_output_writable;

macro_rules! demangle {
    ($name: expr, $demangle: expr, $options: expr) => {{
        if $demangle {
            Name::from($name)
                .demangle($options)
                .unwrap_or_else(|| $name.clone())
        } else {
            $name.clone()
        }
    }};
}

// http://cobertura.sourceforge.net/xml/coverage-04.dtd

struct Coverage {
    sources: Vec<String>,
    packages: Vec<Package>,
}

#[derive(Default)]
struct CoverageStats {
    lines_covered: f64,
    lines_valid: f64,
    branches_covered: f64,
    branches_valid: f64,
    complexity: f64,
}

impl std::ops::Add for CoverageStats {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self {
            lines_covered: self.lines_covered + rhs.lines_covered,
            lines_valid: self.lines_valid + rhs.lines_valid,
            branches_covered: self.branches_covered + rhs.branches_covered,
            branches_valid: self.branches_valid + rhs.branches_valid,
            complexity: self.complexity + rhs.complexity,
        }
    }
}

impl CoverageStats {
    fn from_lines(lines: FxHashMap<u32, Line>) -> Self {
        let lines_covered = lines
            .iter()
            .fold(0.0, |c, (_, l)| if l.covered() { c + 1.0 } else { c });
        let lines_valid = lines.len() as f64;

        let branches: Vec<Vec<Condition>> = lines
            .into_iter()
            .filter_map(|(_, l)| match l {
                Line::Branch { conditions, .. } => Some(conditions),
                Line::Plain { .. } => None,
            })
            .collect();
        let (branches_covered, branches_valid) =
            branches
                .iter()
                .fold((0.0, 0.0), |(covered, valid), conditions| {
                    (
                        covered + conditions.iter().fold(0.0, |hits, c| c.coverage + hits),
                        valid + conditions.len() as f64,
                    )
                });

        Self {
            lines_valid,
            lines_covered,
            branches_valid,
            branches_covered,
            // for now always 0
            complexity: 0.0,
        }
    }

    fn line_rate(&self) -> f64 {
        if self.lines_valid > 0.0 {
            self.lines_covered / self.lines_valid
        } else {
            0.0
        }
    }
    fn branch_rate(&self) -> f64 {
        if self.branches_valid > 0.0 {
            self.branches_covered / self.branches_valid
        } else {
            0.0
        }
    }
}

trait Stats {
    fn get_lines(&self) -> FxHashMap<u32, Line>;

    fn get_stats(&self) -> CoverageStats {
        CoverageStats::from_lines(self.get_lines())
    }
}

impl Stats for Coverage {
    fn get_lines(&self) -> FxHashMap<u32, Line> {
        unimplemented!("does not make sense to ask Coverage for lines")
    }

    fn get_stats(&self) -> CoverageStats {
        self.packages
            .iter()
            .map(|p| p.get_stats())
            .fold(CoverageStats::default(), |acc, stats| acc + stats)
    }
}

struct Package {
    name: String,
    classes: Vec<Class>,
}

impl Stats for Package {
    fn get_lines(&self) -> FxHashMap<u32, Line> {
        self.classes.get_lines()
    }
}

struct Class {
    name: String,
    file_name: String,
    lines: Vec<Line>,
    methods: Vec<Method>,
}

impl Stats for Class {
    fn get_lines(&self) -> FxHashMap<u32, Line> {
        let mut lines = self.lines.get_lines();
        lines.extend(self.methods.get_lines());
        lines
    }
}

struct Method {
    name: String,
    signature: String,
    lines: Vec<Line>,
}

impl Stats for Method {
    fn get_lines(&self) -> FxHashMap<u32, Line> {
        self.lines.get_lines()
    }
}

impl<T: Stats> Stats for Vec<T> {
    fn get_lines(&self) -> FxHashMap<u32, Line> {
        let mut lines = FxHashMap::default();
        for item in self {
            lines.extend(item.get_lines());
        }
        lines
    }
}

#[derive(Debug, Clone)]
enum Line {
    Plain {
        number: u32,
        hits: u64,
    },

    Branch {
        number: u32,
        hits: u64,
        conditions: Vec<Condition>,
    },
}

impl Line {
    fn number(&self) -> u32 {
        match self {
            Line::Plain { number, .. } | Line::Branch { number, .. } => *number,
        }
    }

    fn covered(&self) -> bool {
        matches!(self, Line::Plain { hits, .. } | Line::Branch { hits, .. } if *hits > 0)
    }
}

impl Stats for Line {
    fn get_lines(&self) -> FxHashMap<u32, Line> {
        let mut lines = FxHashMap::default();
        lines.insert(self.number(), self.clone());
        lines
    }
}

#[derive(Debug, Clone)]
struct Condition {
    number: usize,
    cond_type: ConditionType,
    coverage: f64,
}

// Condition types
#[derive(Debug, Clone)]
enum ConditionType {
    Jump,
}

impl Display for ConditionType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Jump => write!(f, "jump"),
        }
    }
}

fn get_coverage(
    results: &[ResultTuple],
    sources: Vec<String>,
    demangle: bool,
    demangle_options: DemangleOptions,
) -> Coverage {
    let packages: Vec<Package> = results
        .iter()
        .map(|(_, rel_path, result)| {
            let all_lines: Vec<u32> = result.lines.keys().cloned().collect();

            let end: u32 = result.lines.keys().last().unwrap_or(&0) + 1;

            let mut start_indexes: Vec<u32> = Vec::new();
            for function in result.functions.values() {
                start_indexes.push(function.start);
            }
            start_indexes.sort_unstable();

            let line_from_number = |number| {
                let hits = result.lines.get(&number).cloned().unwrap_or_default();
                if let Some(branches) = result.branches.get(&number) {
                    let conditions = branches
                        .iter()
                        .enumerate()
                        .map(|(i, b)| Condition {
                            cond_type: ConditionType::Jump,
                            coverage: if *b { 1.0 } else { 0.0 },
                            number: i,
                        })
                        .collect::<Vec<_>>();
                    Line::Branch {
                        number,
                        hits,
                        conditions,
                    }
                } else {
                    Line::Plain { number, hits }
                }
            };

            let methods: Vec<Method> = result
                .functions
                .iter()
                .map(|(name, function)| {
                    let mut func_end = end;

                    for start in &start_indexes {
                        if *start > function.start {
                            func_end = *start;
                            break;
                        }
                    }

                    let mut lines_in_function: Vec<u32> = Vec::new();
                    for line in all_lines
                        .iter()
                        .filter(|&&x| x >= function.start && x < func_end)
                    {
                        lines_in_function.push(*line);
                    }

                    let lines: Vec<Line> = lines_in_function
                        .into_iter()
                        .map(line_from_number)
                        .collect();

                    Method {
                        name: demangle!(name, demangle, demangle_options),
                        signature: String::new(),
                        lines,
                    }
                })
                .collect();

            let lines: Vec<Line> = all_lines.into_iter().map(line_from_number).collect();
            let class = Class {
                name: rel_path
                    .file_stem()
                    .map(|x| x.to_str().unwrap())
                    .unwrap_or_default()
                    .to_string(),
                file_name: rel_path.to_str().unwrap_or_default().to_string(),
                lines,
                methods,
            };

            Package {
                name: rel_path.to_str().unwrap_or_default().to_string(),
                classes: vec![class],
            }
        })
        .collect();

    Coverage { sources, packages }
}

pub fn output_cobertura(
    source_dir: Option<&Path>,
    results: &[ResultTuple],
    output_file: Option<&Path>,
    demangle: bool,
    pretty: bool,
) {
    let demangle_options = DemangleOptions::name_only();
    let sources = vec![source_dir
        .unwrap_or_else(|| Path::new("."))
        .display()
        .to_string()];
    let coverage = get_coverage(results, sources, demangle, demangle_options);

    let mut writer = if pretty {
        Writer::new_with_indent(Cursor::new(vec![]), b' ', 4)
    } else {
        Writer::new(Cursor::new(vec![]))
    };
    writer
        .write_event(Event::Decl(BytesDecl::new("1.0", None, None)))
        .unwrap();
    writer
        .write_event(Event::DocType(BytesText::from_escaped(
            " coverage SYSTEM 'http://cobertura.sourceforge.net/xml/coverage-04.dtd'",
        )))
        .unwrap();

    let cov_tag = "coverage";
    let mut cov = BytesStart::from_content(cov_tag, cov_tag.len());
    let stats = coverage.get_stats();
    cov.push_attribute(("lines-covered", stats.lines_covered.to_string().as_ref()));
    cov.push_attribute(("lines-valid", stats.lines_valid.to_string().as_ref()));
    cov.push_attribute(("line-rate", stats.line_rate().to_string().as_ref()));
    cov.push_attribute((
        "branches-covered",
        stats.branches_covered.to_string().as_ref(),
    ));
    cov.push_attribute(("branches-valid", stats.branches_valid.to_string().as_ref()));
    cov.push_attribute(("branch-rate", stats.branch_rate().to_string().as_ref()));
    cov.push_attribute(("complexity", "0"));
    cov.push_attribute(("version", "1.9"));

    let secs = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(s) => s.as_secs().to_string(),
        Err(_) => String::from("0"),
    };
    cov.push_attribute(("timestamp", secs.as_ref()));

    writer.write_event(Event::Start(cov)).unwrap();

    // export header
    let sources_tag = "sources";
    let source_tag = "source";
    writer
        .write_event(Event::Start(BytesStart::from_content(
            sources_tag,
            sources_tag.len(),
        )))
        .unwrap();
    for path in &coverage.sources {
        writer
            .write_event(Event::Start(BytesStart::from_content(
                source_tag,
                source_tag.len(),
            )))
            .unwrap();
        writer
            .write_event(Event::Text(BytesText::new(path)))
            .unwrap();
        writer
            .write_event(Event::End(BytesEnd::new(source_tag)))
            .unwrap();
    }
    writer
        .write_event(Event::End(BytesEnd::new(sources_tag)))
        .unwrap();

    // export packages
    let packages_tag = "packages";
    let pack_tag = "package";

    writer
        .write_event(Event::Start(BytesStart::from_content(
            packages_tag,
            packages_tag.len(),
        )))
        .unwrap();
    // Export the package
    for package in &coverage.packages {
        let mut pack = BytesStart::from_content(pack_tag, pack_tag.len());
        pack.push_attribute(("name", package.name.as_ref()));
        let stats = package.get_stats();
        pack.push_attribute(("line-rate", stats.line_rate().to_string().as_ref()));
        pack.push_attribute(("branch-rate", stats.branch_rate().to_string().as_ref()));
        pack.push_attribute(("complexity", stats.complexity.to_string().as_ref()));

        writer.write_event(Event::Start(pack)).unwrap();

        // export_classes
        let classes_tag = "classes";
        let class_tag = "class";
        let methods_tag = "methods";
        let method_tag = "method";

        writer
            .write_event(Event::Start(BytesStart::from_content(
                classes_tag,
                classes_tag.len(),
            )))
            .unwrap();

        for class in &package.classes {
            let mut c = BytesStart::from_content(class_tag, class_tag.len());
            c.push_attribute(("name", class.name.as_ref()));
            c.push_attribute(("filename", class.file_name.as_ref()));
            let stats = class.get_stats();
            c.push_attribute(("line-rate", stats.line_rate().to_string().as_ref()));
            c.push_attribute(("branch-rate", stats.branch_rate().to_string().as_ref()));
            c.push_attribute(("complexity", stats.complexity.to_string().as_ref()));

            writer.write_event(Event::Start(c)).unwrap();
            writer
                .write_event(Event::Start(BytesStart::from_content(
                    methods_tag,
                    methods_tag.len(),
                )))
                .unwrap();

            for method in &class.methods {
                let mut m = BytesStart::from_content(method_tag, method_tag.len());
                m.push_attribute(("name", method.name.as_ref()));
                m.push_attribute(("signature", method.signature.as_ref()));
                let stats = method.get_stats();
                m.push_attribute(("line-rate", stats.line_rate().to_string().as_ref()));
                m.push_attribute(("branch-rate", stats.branch_rate().to_string().as_ref()));
                m.push_attribute(("complexity", stats.complexity.to_string().as_ref()));
                writer.write_event(Event::Start(m)).unwrap();

                write_lines(&mut writer, &method.lines);
                writer
                    .write_event(Event::End(BytesEnd::new(method_tag)))
                    .unwrap();
            }
            writer
                .write_event(Event::End(BytesEnd::new(methods_tag)))
                .unwrap();
            write_lines(&mut writer, &class.lines);
        }
        writer
            .write_event(Event::End(BytesEnd::new(class_tag)))
            .unwrap();
        writer
            .write_event(Event::End(BytesEnd::new(classes_tag)))
            .unwrap();
        writer
            .write_event(Event::End(BytesEnd::new(pack_tag)))
            .unwrap();
    }

    writer
        .write_event(Event::End(BytesEnd::new(packages_tag)))
        .unwrap();

    writer
        .write_event(Event::End(BytesEnd::new(cov_tag)))
        .unwrap();

    let result = writer.into_inner().into_inner();
    let mut file = BufWriter::new(get_target_output_writable(output_file));
    file.write_all(&result).unwrap();
}

fn write_lines(writer: &mut Writer<Cursor<Vec<u8>>>, lines: &[Line]) {
    let lines_tag = "lines";
    let line_tag = "line";

    writer
        .write_event(Event::Start(BytesStart::from_content(
            lines_tag,
            lines_tag.len(),
        )))
        .unwrap();
    for line in lines {
        let mut l = BytesStart::from_content(line_tag, line_tag.len());
        match line {
            Line::Plain {
                ref number,
                ref hits,
            } => {
                l.push_attribute(("number", number.to_string().as_ref()));
                l.push_attribute(("hits", hits.to_string().as_ref()));
                writer.write_event(Event::Empty(l)).unwrap();
            }
            Line::Branch {
                ref number,
                ref hits,
                conditions,
            } => {
                l.push_attribute(("number", number.to_string().as_ref()));
                l.push_attribute(("hits", hits.to_string().as_ref()));
                l.push_attribute(("branch", "true"));
                writer.write_event(Event::Start(l)).unwrap();

                let conditions_tag = "conditions";
                let condition_tag = "condition";

                writer
                    .write_event(Event::Start(BytesStart::from_content(
                        conditions_tag,
                        conditions_tag.len(),
                    )))
                    .unwrap();
                for condition in conditions {
                    let mut c = BytesStart::from_content(condition_tag, condition_tag.len());
                    c.push_attribute(("number", condition.number.to_string().as_ref()));
                    c.push_attribute(("type", condition.cond_type.to_string().as_ref()));
                    c.push_attribute(("coverage", condition.coverage.to_string().as_ref()));
                    writer.write_event(Event::Empty(c)).unwrap();
                }
                writer
                    .write_event(Event::End(BytesEnd::new(conditions_tag)))
                    .unwrap();
                writer
                    .write_event(Event::End(BytesEnd::new(line_tag)))
                    .unwrap();
            }
        }
    }
    writer
        .write_event(Event::End(BytesEnd::new(lines_tag)))
        .unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CovResult, Function};
    use std::io::Read;
    use std::{collections::BTreeMap, path::PathBuf};
    use std::{fs::File, path::Path};

    enum Result {
        Main,
        Test,
    }

    fn coverage_result(which: Result) -> CovResult {
        match which {
            Result::Main => CovResult {
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
            Result::Test => CovResult {
                /* main.rs
                   fn main() {
                   }

                   #[test]
                   fn test_fn() {
                       let s = "s";
                       if s == "s" {
                           println!("test");
                       }
                       println!("test");
                   }
                */
                lines: [
                    (1, 2),
                    (3, 0),
                    (6, 2),
                    (7, 1),
                    (8, 2),
                    (9, 1),
                    (11, 1),
                    (12, 2),
                ]
                .iter()
                .cloned()
                .collect(),
                branches: {
                    let mut map = BTreeMap::new();
                    map.insert(8, vec![true, false]);
                    map
                },
                functions: {
                    let mut map = FxHashMap::default();
                    map.insert(
                        "_ZN8cov_test7test_fn17hbf19ec7bfabe8524E".to_string(),
                        Function {
                            start: 6,
                            executed: true,
                        },
                    );

                    map.insert(
                        "_ZN8cov_test4main17h7eb435a3fb3e6f20E".to_string(),
                        Function {
                            start: 1,
                            executed: false,
                        },
                    );

                    map.insert(
                        "_ZN8cov_test4main17h29b45b3d7d8851d2E".to_string(),
                        Function {
                            start: 1,
                            executed: true,
                        },
                    );

                    map.insert(
                        "_ZN8cov_test7test_fn28_$u7b$$u7b$closure$u7d$$u7d$17hab7a162ac9b573fcE"
                            .to_string(),
                        Function {
                            start: 6,
                            executed: true,
                        },
                    );

                    map.insert(
                        "_ZN8cov_test4main17h679717cd8503f8adE".to_string(),
                        Function {
                            start: 1,
                            executed: false,
                        },
                    );
                    map
                },
            },
        }
    }

    fn read_file(path: &Path) -> String {
        let mut f =
            File::open(path).unwrap_or_else(|_| panic!("{:?} file not found", path.file_name()));
        let mut s = String::new();
        f.read_to_string(&mut s).unwrap();
        s
    }

    #[test]
    fn test_cobertura() {
        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let file_name = "test_cobertura.xml";
        let file_path = tmp_dir.path().join(file_name);

        let results = vec![(
            PathBuf::from("src/main.rs"),
            PathBuf::from("src/main.rs"),
            coverage_result(Result::Main),
        )];

        for pretty in [false, true] {
            output_cobertura(None, &results, Some(&file_path), true, pretty);

            let results = read_file(&file_path);

            assert!(results.contains(r#"<source>.</source>"#));

            assert!(results.contains(r#"package name="src/main.rs""#));
            assert!(results.contains(r#"class name="main" filename="src/main.rs""#));
            assert!(results.contains(r#"method name="cov_test::main""#));
            assert!(results.contains(r#"line number="1" hits="1"/>"#));
            assert!(results.contains(r#"line number="3" hits="2" branch="true""#));
            assert!(results.contains(r#"<condition number="0" type="jump" coverage="1"/>"#));

            assert!(results.contains(r#"lines-covered="6""#));
            assert!(results.contains(r#"lines-valid="8""#));
            assert!(results.contains(r#"line-rate="0.75""#));

            assert!(results.contains(r#"branches-covered="1""#));
            assert!(results.contains(r#"branches-valid="4""#));
            assert!(results.contains(r#"branch-rate="0.25""#));
        }
    }

    #[test]
    fn test_cobertura_double_lines() {
        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let file_name = "test_cobertura.xml";
        let file_path = tmp_dir.path().join(file_name);

        let results = vec![(
            PathBuf::from("src/main.rs"),
            PathBuf::from("src/main.rs"),
            coverage_result(Result::Test),
        )];

        output_cobertura(None, &results, Some(file_path.as_ref()), true, true);

        let results = read_file(&file_path);

        assert!(results.contains(r#"<source>.</source>"#));

        assert!(results.contains(r#"package name="src/main.rs""#));
        assert!(results.contains(r#"class name="main" filename="src/main.rs""#));
        assert!(results.contains(r#"method name="cov_test::main""#));
        assert!(results.contains(r#"method name="cov_test::test_fn""#));

        assert!(results.contains(r#"lines-covered="7""#));
        assert!(results.contains(r#"lines-valid="8""#));
        assert!(results.contains(r#"line-rate="0.875""#));

        assert!(results.contains(r#"branches-covered="1""#));
        assert!(results.contains(r#"branches-valid="2""#));
        assert!(results.contains(r#"branch-rate="0.5""#));
    }

    #[test]
    fn test_cobertura_multiple_files() {
        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let file_name = "test_cobertura.xml";
        let file_path = tmp_dir.path().join(file_name);

        let results = vec![
            (
                PathBuf::from("src/main.rs"),
                PathBuf::from("src/main.rs"),
                coverage_result(Result::Main),
            ),
            (
                PathBuf::from("src/test.rs"),
                PathBuf::from("src/test.rs"),
                coverage_result(Result::Test),
            ),
        ];

        output_cobertura(None, &results, Some(file_path.as_ref()), true, true);

        let results = read_file(&file_path);

        assert!(results.contains(r#"<source>.</source>"#));

        assert!(results.contains(r#"package name="src/main.rs""#));
        assert!(results.contains(r#"class name="main" filename="src/main.rs""#));
        assert!(results.contains(r#"package name="src/test.rs""#));
        assert!(results.contains(r#"class name="test" filename="src/test.rs""#));

        assert!(results.contains(r#"lines-covered="13""#));
        assert!(results.contains(r#"lines-valid="16""#));
        assert!(results.contains(r#"line-rate="0.8125""#));

        assert!(results.contains(r#"branches-covered="2""#));
        assert!(results.contains(r#"branches-valid="6""#));
        assert!(results.contains(r#"branch-rate="0.3333333333333333""#));
    }

    #[test]
    fn test_cobertura_source_root_none() {
        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let file_name = "test_cobertura.xml";
        let file_path = tmp_dir.path().join(file_name);

        let results = vec![(
            PathBuf::from("src/main.rs"),
            PathBuf::from("src/main.rs"),
            CovResult::default(),
        )];

        output_cobertura(None, &results, Some(&file_path), true, true);

        let results = read_file(&file_path);

        assert!(results.contains(r#"<source>.</source>"#));
        assert!(results.contains(r#"package name="src/main.rs""#));
    }

    #[test]
    fn test_cobertura_source_root_some() {
        let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
        let file_name = "test_cobertura.xml";
        let file_path = tmp_dir.path().join(file_name);

        let results = vec![(
            PathBuf::from("main.rs"),
            PathBuf::from("main.rs"),
            CovResult::default(),
        )];

        output_cobertura(
            Some(Path::new("src")),
            &results,
            Some(&file_path),
            true,
            true,
        );

        let results = read_file(&file_path);

        assert!(results.contains(r#"<source>src</source>"#));
        assert!(results.contains(r#"package name="main.rs""#));
    }
}
