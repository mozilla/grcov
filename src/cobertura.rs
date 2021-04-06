use quick_xml::{
    events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event},
    Writer,
};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{
    collections::BTreeSet,
    io::{BufWriter, Cursor, Write},
};
use symbolic_common::Name;
use symbolic_demangle::{Demangle, DemangleOptions};

use crate::defs::CovResultIter;
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

trait LineRate {
    fn line_rate(&self) -> f64 {
        let valid = self.lines_valid();
        if valid > 0.0 {
            self.lines_covered() / valid
        } else {
            0.0
        }
    }
    fn branch_rate(&self) -> f64 {
        let valid = self.branches_valid();
        if valid > 0.0 {
            self.branches_covered() / valid
        } else {
            0.0
        }
    }
    fn lines_covered(&self) -> f64;
    fn lines_valid(&self) -> f64;
    fn branches_covered(&self) -> f64;
    fn branches_valid(&self) -> f64;

    fn complexity(&self) -> f64 {
        // for now always 0
        0.0
    }
}

impl LineRate for Coverage {
    fn lines_covered(&self) -> f64 {
        self.packages.lines_covered()
    }

    fn lines_valid(&self) -> f64 {
        self.packages.lines_valid()
    }

    fn branches_covered(&self) -> f64 {
        self.packages.branches_covered()
    }

    fn branches_valid(&self) -> f64 {
        self.packages.branches_valid()
    }
}

struct Package {
    name: String,
    classes: Vec<Class>,
}

impl LineRate for Package {
    fn lines_covered(&self) -> f64 {
        self.classes.lines_covered()
    }

    fn lines_valid(&self) -> f64 {
        self.classes.lines_valid()
    }

    fn branches_covered(&self) -> f64 {
        self.classes.branches_covered()
    }

    fn branches_valid(&self) -> f64 {
        self.classes.branches_valid()
    }
}

struct Class {
    name: String,
    file_name: String,
    lines: Vec<Line>,
    methods: Vec<Method>,
}

impl LineRate for Class {
    fn lines_covered(&self) -> f64 {
        self.lines.lines_covered() + self.methods.lines_covered()
    }

    fn lines_valid(&self) -> f64 {
        self.lines.lines_valid() + self.methods.lines_valid()
    }

    fn line_rate(&self) -> f64 {
        self.lines.line_rate() + self.methods.line_rate()
    }

    fn branches_covered(&self) -> f64 {
        self.lines.branches_covered() + self.methods.branches_covered()
    }

    fn branches_valid(&self) -> f64 {
        self.lines.branches_valid() + self.methods.branches_valid()
    }

    fn branch_rate(&self) -> f64 {
        self.lines.branch_rate() + self.methods.branch_rate()
    }
}

struct Method {
    name: String,
    signature: String,
    lines: Vec<Line>,
}

impl LineRate for Method {
    fn lines_covered(&self) -> f64 {
        self.lines.lines_covered()
    }

    fn lines_valid(&self) -> f64 {
        self.lines.lines_valid()
    }

    fn branches_covered(&self) -> f64 {
        self.lines.branches_covered()
    }

    fn branches_valid(&self) -> f64 {
        self.lines.branches_valid()
    }
}

impl<T: LineRate> LineRate for Vec<T> {
    fn line_rate(&self) -> f64 {
        if self.is_empty() {
            return 0.0;
        }
        self.iter().map(|i| i.line_rate()).sum::<f64>() / self.len() as f64
    }

    fn lines_covered(&self) -> f64 {
        self.iter().map(|i| i.lines_covered()).sum::<f64>()
    }

    fn lines_valid(&self) -> f64 {
        self.iter().map(|i| i.lines_valid()).sum::<f64>()
    }

    fn branch_rate(&self) -> f64 {
        if self.is_empty() {
            return 0.0;
        }
        self.iter().map(|i| i.branch_rate()).sum::<f64>() / self.len() as f64
    }

    fn branches_covered(&self) -> f64 {
        self.iter().map(|i| i.branches_covered()).sum::<f64>()
    }

    fn branches_valid(&self) -> f64 {
        self.iter().map(|i| i.branches_valid()).sum::<f64>()
    }
}

#[derive(Debug)]
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

impl LineRate for Line {
    fn lines_covered(&self) -> f64 {
        match self {
            Line::Plain { hits, .. } => {
                if *hits > 0 {
                    1.0
                } else {
                    0.0
                }
            }
            _ => 0.0,
        }
    }

    fn lines_valid(&self) -> f64 {
        match self {
            Line::Plain { .. } => 1.0,
            _ => 0.0,
        }
    }

    fn branches_covered(&self) -> f64 {
        match self {
            Line::Branch { conditions, .. } => {
                conditions.iter().fold(0.0, |hits, c| c.coverage + hits)
            }
            _ => 0.0,
        }
    }

    fn branches_valid(&self) -> f64 {
        match self {
            Line::Branch { conditions, .. } => conditions.len() as f64,
            _ => 0.0,
        }
    }
}

#[derive(Debug)]
struct Condition {
    number: usize,
    cond_type: ConditionType,
    coverage: f64,
}

// Condition types
#[derive(Debug)]
enum ConditionType {
    Jump,
}

impl ToString for ConditionType {
    fn to_string(&self) -> String {
        match *self {
            Self::Jump => String::from("jump"),
        }
    }
}

fn get_coverage(
    results: CovResultIter,
    demangle: bool,
    demangle_options: DemangleOptions,
) -> Coverage {
    let sources = vec![".".to_owned()];
    let packages: Vec<Package> = results
        .map(|(_, rel_path, result)| {
            let all_lines: Vec<u32> = result.lines.iter().map(|(k, _)| k).cloned().collect();

            let mut orphan_lines: BTreeSet<u32> = all_lines.iter().cloned().collect();

            let end: u32 = result.lines.keys().last().unwrap_or(&0) + 1;

            let mut start_indexes: Vec<u32> = Vec::new();
            for function in result.functions.values() {
                start_indexes.push(function.start);
            }
            start_indexes.sort_unstable();

            let functions = result.functions;
            let result_lines = result.lines;
            let result_branches = result.branches;

            let line_from_number = |number| {
                let hits = result_lines.get(&number).cloned().unwrap_or_default();
                if let Some(branches) = result_branches.get(&number) {
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

            let methods: Vec<Method> = functions
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
                        orphan_lines.remove(line);
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

            let lines: Vec<Line> = orphan_lines.into_iter().map(line_from_number).collect();
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

pub fn output_cobertura(results: CovResultIter, output_file: Option<&str>, demangle: bool) {
    let demangle_options = DemangleOptions::name_only();

    let coverage = get_coverage(results, demangle, demangle_options);

    let mut writer = Writer::new(Cursor::new(vec![]));
    writer
        .write_event(Event::Decl(BytesDecl::new(b"1.0", None, None)))
        .unwrap();
    writer
        .write_event(Event::DocType(BytesText::from_escaped_str(
            " coverage SYSTEM 'http://cobertura.sourceforge.net/xml/coverage-04.dtd'",
        )))
        .unwrap();

    let cov_tag = b"coverage";
    let mut cov = BytesStart::borrowed(cov_tag, cov_tag.len());
    cov.push_attribute((
        "lines-covered",
        coverage.lines_covered().to_string().as_ref(),
    ));
    cov.push_attribute(("lines-valid", coverage.lines_valid().to_string().as_ref()));
    cov.push_attribute(("line-rate", coverage.line_rate().to_string().as_ref()));
    cov.push_attribute((
        "branches-covered",
        coverage.branches_covered().to_string().as_ref(),
    ));
    cov.push_attribute((
        "branches-valid",
        coverage.branches_valid().to_string().as_ref(),
    ));
    cov.push_attribute(("branch-rate", coverage.branch_rate().to_string().as_ref()));
    cov.push_attribute(("complexity", "0"));
    cov.push_attribute(("version", "1.9"));

    let secs = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(s) => s.as_secs().to_string(),
        Err(_) => String::from("0"),
    };
    cov.push_attribute(("timestamp", secs.as_ref()));

    writer.write_event(Event::Start(cov)).unwrap();

    // export header
    let sources_tag = b"sources";
    let source_tag = b"source";
    writer
        .write_event(Event::Start(BytesStart::borrowed(
            sources_tag,
            sources_tag.len(),
        )))
        .unwrap();
    for path in &coverage.sources {
        writer
            .write_event(Event::Start(BytesStart::borrowed(
                source_tag,
                source_tag.len(),
            )))
            .unwrap();
        writer.write(path.as_bytes()).unwrap();
        writer
            .write_event(Event::End(BytesEnd::borrowed(source_tag)))
            .unwrap();
    }
    writer
        .write_event(Event::End(BytesEnd::borrowed(sources_tag)))
        .unwrap();

    // export packages
    let packages_tag = b"packages";
    let pack_tag = b"package";

    writer
        .write_event(Event::Start(BytesStart::borrowed(
            packages_tag,
            packages_tag.len(),
        )))
        .unwrap();
    // Export the package
    for package in &coverage.packages {
        let mut pack = BytesStart::borrowed(pack_tag, pack_tag.len());
        pack.push_attribute(("name", package.name.as_ref()));
        pack.push_attribute(("line-rate", package.line_rate().to_string().as_ref()));
        pack.push_attribute(("branch-rate", package.branch_rate().to_string().as_ref()));
        pack.push_attribute(("complexity", package.complexity().to_string().as_ref()));

        writer.write_event(Event::Start(pack)).unwrap();

        // export_classes
        let classes_tag = b"classes";
        let class_tag = b"class";
        let methods_tag = b"methods";
        let method_tag = b"method";

        writer
            .write_event(Event::Start(BytesStart::borrowed(
                classes_tag,
                classes_tag.len(),
            )))
            .unwrap();

        for class in &package.classes {
            let mut c = BytesStart::borrowed(class_tag, class_tag.len());
            c.push_attribute(("name", class.name.as_ref()));
            c.push_attribute(("filename", class.file_name.as_ref()));
            c.push_attribute(("line-rate", class.line_rate().to_string().as_ref()));
            c.push_attribute(("branch-rate", class.branch_rate().to_string().as_ref()));
            c.push_attribute(("complexity", class.complexity().to_string().as_ref()));

            writer.write_event(Event::Start(c)).unwrap();
            writer
                .write_event(Event::Start(BytesStart::borrowed(
                    methods_tag,
                    methods_tag.len(),
                )))
                .unwrap();

            for method in &class.methods {
                let mut m = BytesStart::borrowed(method_tag, method_tag.len());
                m.push_attribute(("name", method.name.as_ref()));
                m.push_attribute(("signature", method.signature.as_ref()));
                m.push_attribute(("line-rate", method.line_rate().to_string().as_ref()));
                m.push_attribute(("branch-rate", method.branch_rate().to_string().as_ref()));
                m.push_attribute(("complexity", method.complexity().to_string().as_ref()));
                writer.write_event(Event::Start(m)).unwrap();

                write_lines(&mut writer, &method.lines);
                writer
                    .write_event(Event::End(BytesEnd::borrowed(method_tag)))
                    .unwrap();
            }
            writer
                .write_event(Event::End(BytesEnd::borrowed(methods_tag)))
                .unwrap();
            write_lines(&mut writer, &class.lines);
        }
        writer
            .write_event(Event::End(BytesEnd::borrowed(class_tag)))
            .unwrap();
        writer
            .write_event(Event::End(BytesEnd::borrowed(classes_tag)))
            .unwrap();
        writer
            .write_event(Event::End(BytesEnd::borrowed(pack_tag)))
            .unwrap();
    }

    writer
        .write_event(Event::End(BytesEnd::borrowed(packages_tag)))
        .unwrap();

    writer
        .write_event(Event::End(BytesEnd::borrowed(cov_tag)))
        .unwrap();

    let result = writer.into_inner().into_inner();
    let mut file = BufWriter::new(get_target_output_writable(output_file));
    file.write_all(&result).unwrap();
}

fn write_lines(writer: &mut Writer<Cursor<Vec<u8>>>, lines: &[Line]) {
    let lines_tag = b"lines";
    let line_tag = b"line";

    writer
        .write_event(Event::Start(BytesStart::borrowed(
            lines_tag,
            lines_tag.len(),
        )))
        .unwrap();
    for line in lines {
        let mut l = BytesStart::borrowed(line_tag, line_tag.len());
        match line {
            Line::Plain {
                ref number,
                ref hits,
            } => {
                l.push_attribute(("number", number.to_string().as_ref()));
                l.push_attribute(("hits", hits.to_string().as_ref()));
                writer.write_event(Event::Start(l)).unwrap();
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

                let conditions_tag = b"conditions";
                let condition_tag = b"condition";

                writer
                    .write_event(Event::Start(BytesStart::borrowed(
                        conditions_tag,
                        conditions_tag.len(),
                    )))
                    .unwrap();
                for condition in conditions {
                    let mut c = BytesStart::borrowed(condition_tag, condition_tag.len());
                    c.push_attribute(("number", condition.number.to_string().as_ref()));
                    c.push_attribute(("type", condition.cond_type.to_string().as_ref()));
                    c.push_attribute(("coverage", condition.coverage.to_string().as_ref()));
                    writer.write_event(Event::Empty(c)).unwrap();
                }
                writer
                    .write_event(Event::End(BytesEnd::borrowed(conditions_tag)))
                    .unwrap();
            }
        }
        writer
            .write_event(Event::End(BytesEnd::borrowed(line_tag)))
            .unwrap();
    }
    writer
        .write_event(Event::End(BytesEnd::borrowed(lines_tag)))
        .unwrap();
}
