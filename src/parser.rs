use regex::Regex;
use serde::{Deserialize, Deserializer};
use std::cmp::{max, Ordering};
use std::collections::{btree_map, hash_map, BTreeMap};
use std::fmt;
use std::io::{self, BufRead, BufReader, Read};
use std::num::ParseIntError;
use std::str;
use std::sync::Arc;

use log::{error, warn};

use quick_xml::encoding::EncodingError;
use quick_xml::events::attributes::AttrError;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use quick_xml::XmlVersion;

use rustc_hash::FxHashMap;

use crate::defs::*;

#[derive(Debug)]
pub enum ParserError {
    Io(io::Error),
    Parse(String),
    InvalidRecord(String),
    InvalidData(String),
}

impl From<io::Error> for ParserError {
    fn from(err: io::Error) -> ParserError {
        ParserError::Io(err)
    }
}

impl From<quick_xml::Error> for ParserError {
    fn from(err: quick_xml::Error) -> ParserError {
        match err {
            quick_xml::Error::Io(e) => ParserError::Io(Arc::try_unwrap(e).unwrap()),
            _ => ParserError::Parse(format!("{err:?}")),
        }
    }
}

impl From<EncodingError> for ParserError {
    fn from(err: EncodingError) -> ParserError {
        ParserError::Parse(format!("{err:?}"))
    }
}

impl From<AttrError> for ParserError {
    fn from(err: AttrError) -> ParserError {
        ParserError::Parse(format!("{err:?}"))
    }
}

impl From<ParseIntError> for ParserError {
    fn from(err: ParseIntError) -> ParserError {
        ParserError::Parse(err.to_string())
    }
}

impl fmt::Display for ParserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            ParserError::Io(ref err) => write!(f, "IO error: {err}"),
            ParserError::Parse(ref s) => write!(f, "Parsing error: '{s}'"),
            ParserError::InvalidRecord(ref s) => write!(f, "Invalid record: '{s}'"),
            ParserError::InvalidData(ref s) => write!(f, "Invalid data: '{s}'"),
        }
    }
}

macro_rules! manage_parsing_error {
    ($i:expr, $l:expr) => {
        if $i {
            warn!("{}, ignore this record", $l.to_string());
        } else {
            return Err(ParserError::InvalidRecord($l.to_string()));
        }
    };
}

pub fn add_branch(branches: &mut BTreeMap<u32, Vec<bool>>, line_no: u32, no: u32, taken: bool) {
    match branches.entry(line_no) {
        btree_map::Entry::Occupied(c) => {
            let v = c.into_mut();
            let l = v.len();
            let no = no as usize;

            match no.cmp(&l) {
                Ordering::Equal => v.push(taken),
                Ordering::Greater => {
                    v.extend(vec![false; no - l]);
                    v.push(taken);
                }
                Ordering::Less => v[no] |= taken,
            }
        }
        btree_map::Entry::Vacant(v) => {
            v.insert(vec![taken; 1]);
        }
    };
}

pub fn parse_lcov(
    buffer: Vec<u8>,
    branch_enabled: bool,
    ignore_parsing_error: bool,
) -> Result<Vec<(String, CovResult)>, ParserError> {
    let mut cur_file = None;
    let mut cur_lines = BTreeMap::new();
    let mut cur_branches = BTreeMap::new();
    let mut cur_functions = FxHashMap::default();

    // We only log the duplicated FN error once per parse_lcov call.
    let mut duplicated_error_logged = false;

    let mut results = Vec::new();
    let iter = &mut buffer.iter().peekable();

    const SF: u32 = (b'S' as u32) * (1 << 8) + (b'F' as u32);
    const DA: u32 = (b'D' as u32) * (1 << 8) + (b'A' as u32);
    const FN: u32 = (b'F' as u32) * (1 << 8) + (b'N' as u32);
    const FNDA: u32 = (b'F' as u32) * (1 << 24)
        + (b'N' as u32) * (1 << 16)
        + (b'D' as u32) * (1 << 8)
        + (b'A' as u32);
    const BRDA: u32 = (b'B' as u32) * (1 << 24)
        + (b'R' as u32) * (1 << 16)
        + (b'D' as u32) * (1 << 8)
        + (b'A' as u32);

    let mut line = 0;
    let mut parsing_error_occurs = false;

    while let Some(c) = iter.next() {
        line += 1;
        match *c {
            b'e' => {
                if ignore_parsing_error && parsing_error_occurs {
                    continue;
                }

                // Only `end_of_record` closes a record: any other line starting
                // with 'e' is an unknown record and has to be skipped like the
                // ones handled by the catch-all arm below.
                let start = buffer.len() - iter.len();
                let len = iter.take_while(|&&c| c != b'\n').count();
                let rest = &buffer[start..start + len];
                let rest = rest.strip_suffix(b"\r").unwrap_or(rest);
                if rest != b"nd_of_record" {
                    continue;
                }

                // we've a end_of_record
                match cur_file.take() {
                    Some(cur_file) => results.push((
                        cur_file,
                        CovResult {
                            lines: cur_lines,
                            branches: cur_branches,
                            functions: cur_functions,
                        },
                    )),
                    None => {
                        manage_parsing_error!(
                            ignore_parsing_error,
                            format!("end_of_record without SF at line {line}")
                        );
                        parsing_error_occurs = true;
                        continue;
                    }
                }

                cur_lines = BTreeMap::new();
                cur_branches = BTreeMap::new();
                cur_functions = FxHashMap::default();
            }
            b'\n' => {
                continue;
            }
            _ => {
                if *c != b'S' && *c != b'D' && *c != b'F' && *c != b'B' {
                    iter.take_while(|&&c| c != b'\n').last();
                    continue;
                }

                let key = iter
                    .take_while(|&&c| c.is_ascii_uppercase())
                    .try_fold(*c as u32, |r, &x| {
                        r.checked_mul(1 << 8)?.checked_add(u32::from(x))
                    });

                let key = match key {
                    Some(key) => key,
                    None => {
                        manage_parsing_error!(
                            ignore_parsing_error,
                            format!("Invalid key at line {line}")
                        );
                        parsing_error_occurs = true;
                        iter.take_while(|&&c| c != b'\n').last();
                        continue;
                    }
                };

                match key {
                    SF => {
                        // SF:string
                        cur_file = Some(
                            iter.take_while(|&&c| c != b'\n' && c != b'\r')
                                .map(|&c| c as char)
                                .collect(),
                        );
                        parsing_error_occurs = false;
                    }
                    DA => {
                        if ignore_parsing_error && parsing_error_occurs {
                            continue;
                        }

                        // DA:uint,int
                        if let Some(c) = iter.peek() {
                            if !c.is_ascii_digit() {
                                manage_parsing_error!(
                                    ignore_parsing_error,
                                    format!("DA at line {line}")
                                );
                                parsing_error_occurs = true;
                            }
                        }

                        let line_no = iter
                            .take_while(|&&c| c.is_ascii_digit())
                            .fold(0, |r, &x| r * 10 + u32::from(x - b'0'));

                        if iter.peek().is_none() {
                            manage_parsing_error!(
                                ignore_parsing_error,
                                format!("DA at line {line}")
                            );
                            parsing_error_occurs = true;
                        }
                        let execution_count = if let Some(c) = iter.next() {
                            if *c == b'-' {
                                iter.take_while(|&&c| c != b'\n').last();
                                0
                            } else {
                                iter.take_while(|&&c| c.is_ascii_digit())
                                    .fold(u64::from(*c - b'0'), |r, &x| {
                                        r * 10 + u64::from(x - b'0')
                                    })
                            }
                        } else {
                            0
                        };
                        *cur_lines.entry(line_no).or_insert(0) += execution_count;
                    }
                    FN => {
                        if ignore_parsing_error && parsing_error_occurs {
                            continue;
                        }

                        // FN:int,string
                        // A negative line number marks a synthetic function
                        // that has no location in the source file. Treat it
                        // like any other malformed record: fail by default,
                        // or with --ignore-parsing-error keep the record and
                        // report it at line 0 instead of discarding the whole
                        // coverage file.
                        let synthetic = iter.peek() == Some(&&b'-');
                        if synthetic {
                            // Without --ignore-parsing-error this returns
                            // Err and stops parsing here, same as any other
                            // malformed record. With the flag, it only logs
                            // a warning and falls through: the record is
                            // normalized (not dropped), so parsing_error_occurs
                            // must NOT be set here, or the end_of_record
                            // handler below would discard this otherwise-valid
                            // result.
                            manage_parsing_error!(
                                ignore_parsing_error,
                                format!(
                                    "FN at line {line}: negative line number marks a synthetic function"
                                )
                            );
                            iter.next();
                        }
                        if let Some(c) = iter.peek() {
                            if !c.is_ascii_digit() {
                                manage_parsing_error!(
                                    ignore_parsing_error,
                                    format!("FN at line {line}")
                                );
                                parsing_error_occurs = true;
                            }
                        }
                        let start = iter
                            .take_while(|&&c| c.is_ascii_digit())
                            .fold(0, |r, &x| r * 10 + u32::from(x - b'0'));
                        let start = if synthetic { 0 } else { start };
                        if iter.peek().is_none() {
                            manage_parsing_error!(
                                ignore_parsing_error,
                                format!("FN at line {line}")
                            );
                            parsing_error_occurs = true;
                        }
                        let f_name: String = iter
                            .take_while(|&&c| c != b'\n' && c != b'\r')
                            .map(|&c| c as char)
                            .collect();
                        if !duplicated_error_logged && cur_functions.contains_key(&f_name) {
                            warn!(
                                "FN '{}' duplicated for '{}' in a lcov file",
                                f_name,
                                cur_file.as_ref().unwrap()
                            );
                            duplicated_error_logged = true;
                        }
                        cur_functions.insert(
                            f_name,
                            Function {
                                start,
                                executed: false,
                            },
                        );
                    }
                    FNDA => {
                        if ignore_parsing_error && parsing_error_occurs {
                            continue;
                        }

                        // FNDA:int,string
                        if let Some(c) = iter.peek() {
                            if !c.is_ascii_digit() {
                                manage_parsing_error!(
                                    ignore_parsing_error,
                                    format!("FNDA at line {line}")
                                );
                                parsing_error_occurs = true;
                            }
                        }
                        let executed = iter
                            .take_while(|&&c| c.is_ascii_digit())
                            .fold(0, |r, &x| r * 10 + u64::from(x - b'0'));
                        if iter.peek().is_none() {
                            manage_parsing_error!(
                                ignore_parsing_error,
                                format!("FNDA at line {line}")
                            );
                            parsing_error_occurs = true;
                        }
                        let f_name: String = iter
                            .take_while(|&&c| c != b'\n' && c != b'\r')
                            .map(|&c| c as char)
                            .collect();
                        if let Some(f) = cur_functions.get_mut(&f_name) {
                            f.executed |= executed != 0;
                        } else {
                            manage_parsing_error!(
                                ignore_parsing_error,
                                format!("FN record missing for function {f_name}")
                            );
                            parsing_error_occurs = true;
                        }
                    }
                    BRDA => {
                        if ignore_parsing_error && parsing_error_occurs {
                            continue;
                        }

                        // BRDA:int,int,int,int or -
                        if branch_enabled {
                            if let Some(c) = iter.peek() {
                                if !c.is_ascii_digit() {
                                    manage_parsing_error!(
                                        ignore_parsing_error,
                                        format!("BRDA at line {line}")
                                    );
                                    parsing_error_occurs = true;
                                }
                            }
                            let line_no = iter
                                .take_while(|&&c| c.is_ascii_digit())
                                .fold(0, |r, &x| r * 10 + u32::from(x - b'0'));
                            if iter.peek().is_none() {
                                manage_parsing_error!(
                                    ignore_parsing_error,
                                    format!("BRDA at line {line}")
                                );
                                parsing_error_occurs = true;
                            }
                            let _block_number = iter
                                .take_while(|&&c| c.is_ascii_digit())
                                .fold(0, |r, &x| r * 10 + u64::from(x - b'0'));
                            if iter.peek().is_none() {
                                manage_parsing_error!(
                                    ignore_parsing_error,
                                    format!("BRDA at line {line}")
                                );
                                parsing_error_occurs = true;
                            }
                            let branch_number = iter
                                .take_while(|&&c| c.is_ascii_digit())
                                .fold(0, |r, &x| r * 10 + u32::from(x - b'0'));
                            if iter.peek().is_none() {
                                manage_parsing_error!(
                                    ignore_parsing_error,
                                    format!("BRDA at line {line}")
                                );
                                parsing_error_occurs = true;
                            }
                            let taken = iter
                                .take_while(|&&c| c != b'\n' && c != b'\r')
                                .any(|&x| x != b'-');
                            add_branch(&mut cur_branches, line_no, branch_number, taken);
                        } else {
                            iter.take_while(|&&c| c != b'\n').last();
                        }
                    }
                    _ => {
                        iter.take_while(|&&c| c != b'\n').last();
                    }
                }
            }
        }
    }

    Ok(results)
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GcovJson {
    format_version: String,
    gcc_version: String,
    // the cwd during gcno generation
    current_working_directory: Option<String>,
    // the file used to generated this json
    data_file: String,
    files: Vec<GcovFile>,
}

#[derive(Debug, Deserialize)]
struct GcovFile {
    file: String,
    functions: Vec<GcovFunction>,
    lines: Vec<GcovLine>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GcovLine {
    line_number: u32,
    function_name: Option<String>,
    #[serde(deserialize_with = "deserialize_counter")]
    count: u64,
    unexecuted_block: bool,
    branches: Vec<GcovBr>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GcovBr {
    #[serde(deserialize_with = "deserialize_counter")]
    count: u64,
    throw: bool,
    fallthrough: bool,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GcovFunction {
    name: String,
    demangled_name: String,
    start_line: u32,
    start_column: u32,
    end_line: u32,
    end_column: u32,
    blocks: u32,
    blocks_executed: u32,
    #[serde(deserialize_with = "deserialize_counter")]
    execution_count: u64,
}

// JSON sometimes surprises us with floats where we expected integers, use
// a custom deserializer to ensure all the counters are converted to u64.
pub fn deserialize_counter<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let n: serde_json::Number = Deserialize::deserialize(deserializer)?;
    if n.is_f64() {
        let value: f64 = n.as_f64().unwrap();
        if (value >= 0.0) && (value <= u64::MAX as f64) {
            return Ok(value as u64);
        }
    }

    match n.as_u64() {
        Some(value) => Ok(value),
        None => Err(serde::de::Error::custom(format!(
            "Unable to parse u64 from {n}"
        ))),
    }
}

/// Parses the JSON that `gcov -i -t` writes, which may hold several documents.
pub fn parse_gcov_json(buf: &[u8]) -> Result<Vec<(String, CovResult)>, ParserError> {
    let mut results = Vec::new();

    for gcov in serde_json::Deserializer::from_slice(buf).into_iter::<GcovJson>() {
        let mut gcov = gcov.map_err(|e| ParserError::Parse(e.to_string()))?;

        if gcov.format_version != "1" && gcov.format_version != "2" {
            error!(
                "Format version {} is not expected, please file a bug on https://github.com/mozilla/grcov",
                gcov.format_version
            );
        }

        for mut file in gcov.files.drain(..) {
            let mut lines = BTreeMap::new();
            let mut branches = BTreeMap::new();
            for mut line in file.lines.drain(..) {
                lines.insert(line.line_number, line.count);
                if !line.branches.is_empty() {
                    branches.insert(
                        line.line_number,
                        line.branches.drain(..).map(|b| b.count > 0).collect(),
                    );
                }
            }
            if lines.is_empty() {
                continue;
            }
            let mut functions = FxHashMap::default();
            for fun in file.functions.drain(..) {
                functions.insert(
                    fun.demangled_name,
                    Function {
                        start: fun.start_line,
                        executed: fun.execution_count > 0,
                    },
                );
            }
            results.push((
                file.file,
                CovResult {
                    lines,
                    branches,
                    functions,
                },
            ));
        }
    }

    Ok(results)
}

fn get_xml_attribute(event: &BytesStart<'_>, name: &str) -> Result<String, ParserError> {
    for a in event.attributes() {
        let a = a?;
        if a.key.into_inner() == name {
            return Ok(a.normalized_value(XmlVersion::Implicit1_0)?.into_owned());
        }
    }
    Err(ParserError::InvalidRecord(format!(
        "Attribute {name} not found"
    )))
}

fn parse_jacoco_report_sourcefile<T: BufRead>(
    parser: &mut Reader<T>,
    buf: &mut Vec<u8>,
) -> Result<JacocoReport, ParserError> {
    let mut lines: BTreeMap<u32, u64> = BTreeMap::new();
    let mut branches: BTreeMap<u32, Vec<bool>> = BTreeMap::new();

    loop {
        match parser.read_event_into(buf) {
            Ok(Event::Start(ref e)) if e.local_name().into_inner() == "line" => {
                let (mut ci, mut cb, mut mb, mut nr) = (None, None, None, None);
                for a in e.attributes() {
                    let a = a?;
                    match a.key.into_inner() {
                        "ci" => ci = Some(a.value.parse::<u64>()?),
                        "cb" => cb = Some(a.value.parse::<u64>()?),
                        "mb" => mb = Some(a.value.parse::<u64>()?),
                        "nr" => nr = Some(a.value.parse::<u32>()?),
                        _ => (),
                    }
                }

                fn try_att<T>(opt: Option<T>, name: &str) -> Result<T, ParserError> {
                    opt.ok_or_else(|| {
                        ParserError::InvalidRecord(format!("Attribute {name} not found"))
                    })
                }

                let ci = try_att(ci, "ci")?;
                let cb = try_att(cb, "cb")?;
                let mb = try_att(mb, "mb")?;
                let nr = try_att(nr, "nr")?;

                if mb > 0 || cb > 0 {
                    // This line is a branch.
                    let mut v = vec![true; cb as usize];
                    v.extend(vec![false; mb as usize]);
                    branches.insert(nr, v);
                } else {
                    // This line is a statement.
                    // JaCoCo does not feature execution counts, so we set the
                    // count to 0 or 1.
                    let hit = u64::from(ci > 0);
                    lines.insert(nr, hit);
                }
            }
            Ok(Event::End(ref e)) if e.local_name().into_inner() == "sourcefile" => {
                break;
            }
            Err(e) => return Err(ParserError::Parse(e.to_string())),
            _ => {}
        }
        buf.clear();
    }

    Ok(JacocoReport { lines, branches })
}

fn parse_jacoco_report_method<T: BufRead>(
    parser: &mut Reader<T>,
    buf: &mut Vec<u8>,
    start: u32,
) -> Result<Function, ParserError> {
    let mut executed = false;

    loop {
        match parser.read_event_into(buf) {
            Ok(Event::Start(ref e)) if e.local_name().into_inner() == "counter" => {
                if get_xml_attribute(e, "type")? == "METHOD" {
                    executed = get_xml_attribute(e, "covered")?.parse::<u32>()? > 0;
                }
            }
            Ok(Event::End(ref e)) if e.local_name().into_inner() == "method" => break,
            Err(e) => return Err(ParserError::Parse(e.to_string())),
            _ => {}
        }
        buf.clear();
    }

    Ok(Function { start, executed })
}

fn parse_jacoco_report_class<T: BufRead>(
    parser: &mut Reader<T>,
    buf: &mut Vec<u8>,
    class_name: &str,
) -> Result<FunctionMap, ParserError> {
    let mut functions: FunctionMap = FxHashMap::default();

    loop {
        match parser.read_event_into(buf) {
            Ok(Event::Start(ref e)) if e.local_name().into_inner() == "method" => {
                let name = get_xml_attribute(e, "name")?;
                let full_name = format!("{class_name}#{name}");

                let start_line = get_xml_attribute(e, "line")?.parse::<u32>()?;
                let function = parse_jacoco_report_method(parser, buf, start_line)?;
                functions.insert(full_name, function);
            }
            Ok(Event::End(ref e)) if e.local_name().into_inner() == "class" => break,
            Err(e) => return Err(ParserError::Parse(e.to_string())),
            _ => {}
        }
        buf.clear();
    }

    Ok(functions)
}

fn parse_jacoco_report_package<T: BufRead>(
    parser: &mut Reader<T>,
    buf: &mut Vec<u8>,
    package: &str,
) -> Result<Vec<(String, CovResult)>, ParserError> {
    let mut results_map: FxHashMap<String, CovResult> = FxHashMap::default();

    loop {
        match parser.read_event_into(buf) {
            Ok(Event::Start(ref e)) => {
                match e.local_name().into_inner() {
                    "class" => {
                        let fq_class = get_xml_attribute(e, "name")?;
                        // Class name: "Person$Age"
                        let class = fq_class
                            .split('/')
                            .next_back()
                            .expect("Failed to parse class name");
                        // Class name "Person"
                        let top_class = class
                            .split('$')
                            .next()
                            .expect("Failed to parse top class name");
                        // Fully qualified class name: "org/example/Person$Age"
                        // Generally, we will use the filename if its present,
                        // but if it isn't, fallback to the top level class name
                        let file = get_xml_attribute(e, "sourcefilename")
                            .unwrap_or(format!("{top_class}.java"));

                        // Process all <method /> and <counter /> for this class
                        let functions = parse_jacoco_report_class(parser, buf, class)?;

                        match results_map.entry(file.to_string()) {
                            hash_map::Entry::Occupied(obj) => {
                                obj.into_mut().functions.extend(functions);
                            }
                            hash_map::Entry::Vacant(v) => {
                                v.insert(CovResult {
                                    functions,
                                    lines: BTreeMap::new(),
                                    branches: BTreeMap::new(),
                                });
                            }
                        };
                    }
                    "sourcefile" => {
                        // Fully qualified class name: "org/example/Person$Age"
                        let file = get_xml_attribute(e, "name")?;

                        let JacocoReport { lines, branches } =
                            parse_jacoco_report_sourcefile(parser, buf)?;

                        match results_map.entry(file.to_string()) {
                            hash_map::Entry::Occupied(obj) => {
                                let obj = obj.into_mut();
                                obj.lines = lines;
                                obj.branches = branches;
                            }
                            hash_map::Entry::Vacant(v) => {
                                v.insert(CovResult {
                                    functions: FxHashMap::default(),
                                    lines,
                                    branches,
                                });
                            }
                        };
                    }
                    &_ => {}
                }
            }
            Ok(Event::End(ref e)) if e.local_name().into_inner() == "package" => break,
            Err(e) => return Err(ParserError::Parse(e.to_string())),
            _ => {}
        }
    }

    // Change all keys from the class name to the file name and turn the result into a Vec.
    // If package is the empty string, we have to trim the leading '/' in order to obtain a
    // relative path.
    Ok(results_map
        .into_iter()
        .map(|(class, result)| {
            (
                format!("{package}/{class}")
                    .trim_start_matches('/')
                    .to_string(),
                result,
            )
        })
        .collect())
}

pub fn parse_jacoco_xml_report<T: Read>(
    xml_reader: BufReader<T>,
) -> Result<Vec<(String, CovResult)>, ParserError> {
    let mut parser = Reader::from_reader(xml_reader);
    let config = parser.config_mut();
    config.expand_empty_elements = true;
    config.trim_text(false);

    let mut results = Vec::new();
    let mut buf = Vec::new();

    loop {
        match parser.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) if e.local_name().into_inner() == "package" => {
                let package = get_xml_attribute(e, "name")?;
                let mut package_results =
                    parse_jacoco_report_package(&mut parser, &mut buf, &package)?;
                results.append(&mut package_results);
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(ParserError::Parse(e.to_string())),
            _ => {}
        }
        buf.clear();
    }

    Ok(results)
}

pub fn parse_gocov<T: Read>(
    reader: &mut BufReader<T>,
) -> Result<Vec<(String, CovResult)>, ParserError> {
    let re = Regex::new(r"([^:]+):(\d+)\.\d+,(\d+)\.\d+ (\d+) (\d+)").unwrap();
    let mut results = FxHashMap::<String, BTreeMap<u32, u64>>::default();
    let mut lines = BTreeMap::new();
    let mut current_file = "".to_string();

    loop {
        let mut line = String::new();

        match reader.read_line(&mut line) {
            Ok(0) => {
                if !current_file.is_empty() {
                    results.insert(current_file.to_string(), lines.clone());
                }
                return Ok(results
                    .into_iter()
                    .map(|(file, lines)| {
                        (
                            file,
                            CovResult {
                                lines,
                                branches: BTreeMap::new(),
                                functions: FxHashMap::default(),
                            },
                        )
                    })
                    .collect());
            }
            Ok(_) => {
                if line.starts_with("mode:") {
                    continue;
                }

                if let Some(caps) = re.captures(&line) {
                    let file = caps
                        .get(1)
                        .map(|c| c.as_str())
                        .ok_or(ParserError::InvalidData(line.clone()))?;
                    if file != current_file {
                        if !current_file.is_empty() {
                            results.insert(current_file.to_string(), lines.clone());
                            lines.clear();
                        }
                        current_file = file.to_owned();
                    }
                    let start = caps
                        .get(2)
                        .and_then(|s| s.as_str().parse::<u32>().ok())
                        .ok_or(ParserError::InvalidData(line.clone()))?;
                    let end = caps
                        .get(3)
                        .and_then(|s| s.as_str().parse::<u32>().ok())
                        .ok_or(ParserError::InvalidData(line.clone()))?;
                    // unsure what this is actually for
                    let _number_of_statements = caps
                        .get(4)
                        .and_then(|s| s.as_str().parse::<u64>().ok())
                        .ok_or(ParserError::InvalidData(line.clone()))?;
                    let number_of_covered_statements = caps
                        .get(5)
                        .and_then(|s| s.as_str().parse::<u64>().ok())
                        .ok_or(ParserError::InvalidData(line.clone()))?;
                    let is_covered = if number_of_covered_statements > 0 {
                        1
                    } else {
                        0
                    };

                    for i in start..=end {
                        match lines.entry(i) {
                            btree_map::Entry::Occupied(mut e) => {
                                *e.get_mut() = max(*e.get(), is_covered);
                            }
                            btree_map::Entry::Vacant(e) => {
                                e.insert(is_covered);
                            }
                        };
                    }
                } else {
                    error!("`{line}` Line didn't match expected format, ignoring");
                }
            }
            Err(e) => return Err(ParserError::Io(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[test]
    fn test_lcov_parser() {
        let mut f = File::open("./test/prova.info").expect("Failed to open lcov file");
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        let results = parse_lcov(buf, false, false).unwrap();

        assert_eq!(results.len(), 603);

        let (ref source_name, ref result) = results[0];
        assert_eq!(
            source_name,
            "resource://gre/components/MainProcessSingleton.js"
        );
        assert_eq!(
            result.lines,
            [
                (7, 1),
                (9, 1),
                (10, 1),
                (12, 2),
                (13, 1),
                (16, 1),
                (17, 1),
                (18, 2),
                (19, 1),
                (21, 1),
                (22, 0),
                (23, 0),
                (24, 0),
                (28, 1),
                (29, 0),
                (30, 0),
                (32, 0),
                (33, 0),
                (34, 0),
                (35, 0),
                (37, 0),
                (39, 0),
                (41, 0),
                (42, 0),
                (44, 0),
                (45, 0),
                (46, 0),
                (47, 0),
                (49, 0),
                (50, 0),
                (51, 0),
                (52, 0),
                (53, 0),
                (54, 0),
                (55, 0),
                (56, 0),
                (59, 0),
                (60, 0),
                (61, 0),
                (63, 0),
                (65, 0),
                (67, 1),
                (68, 2),
                (70, 1),
                (74, 1),
                (75, 1),
                (76, 1),
                (77, 1),
                (78, 1),
                (83, 1),
                (84, 1),
                (90, 1)
            ]
            .iter()
            .cloned()
            .collect()
        );
        assert_eq!(result.branches, [].iter().cloned().collect());
        assert!(result.functions.contains_key("MainProcessSingleton"));
        let func = result.functions.get("MainProcessSingleton").unwrap();
        assert_eq!(func.start, 15);
        assert!(func.executed);
        assert!(result.functions.contains_key("logConsoleMessage"));
        let func = result.functions.get("logConsoleMessage").unwrap();
        assert_eq!(func.start, 21);
        assert!(!func.executed);
    }

    #[test]
    fn test_lcov_parser_with_branch_parsing() {
        // Parse the same file, but with branch parsing enabled.
        let mut f = File::open("./test/prova.info").expect("Failed to open lcov file");
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        let results = parse_lcov(buf, true, false).unwrap();

        assert_eq!(results.len(), 603);

        let (ref source_name, ref result) = results[0];
        assert_eq!(
            source_name,
            "resource://gre/components/MainProcessSingleton.js"
        );
        assert_eq!(
            result.lines,
            [
                (7, 1),
                (9, 1),
                (10, 1),
                (12, 2),
                (13, 1),
                (16, 1),
                (17, 1),
                (18, 2),
                (19, 1),
                (21, 1),
                (22, 0),
                (23, 0),
                (24, 0),
                (28, 1),
                (29, 0),
                (30, 0),
                (32, 0),
                (33, 0),
                (34, 0),
                (35, 0),
                (37, 0),
                (39, 0),
                (41, 0),
                (42, 0),
                (44, 0),
                (45, 0),
                (46, 0),
                (47, 0),
                (49, 0),
                (50, 0),
                (51, 0),
                (52, 0),
                (53, 0),
                (54, 0),
                (55, 0),
                (56, 0),
                (59, 0),
                (60, 0),
                (61, 0),
                (63, 0),
                (65, 0),
                (67, 1),
                (68, 2),
                (70, 1),
                (74, 1),
                (75, 1),
                (76, 1),
                (77, 1),
                (78, 1),
                (83, 1),
                (84, 1),
                (90, 1)
            ]
            .iter()
            .cloned()
            .collect()
        );
        assert_eq!(
            result.branches,
            [
                (34, vec![false, false]),
                (41, vec![false, false]),
                (44, vec![false, false]),
                (60, vec![false, false]),
                (63, vec![false, false]),
                (68, vec![true, true])
            ]
            .iter()
            .cloned()
            .collect()
        );
        assert!(result.functions.contains_key("MainProcessSingleton"));
        let func = result.functions.get("MainProcessSingleton").unwrap();
        assert_eq!(func.start, 15);
        assert!(func.executed);
        assert!(result.functions.contains_key("logConsoleMessage"));
        let func = result.functions.get("logConsoleMessage").unwrap();
        assert_eq!(func.start, 21);
        assert!(!func.executed);
    }

    #[test]
    fn test_lcov_parser_fn_with_commas() {
        let mut f =
            File::open("./test/prova_fn_with_commas.info").expect("Failed to open lcov file");
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        let results = parse_lcov(buf, true, false).unwrap();

        assert_eq!(results.len(), 1);

        let (ref source_name, ref result) = results[0];
        assert_eq!(source_name, "aFile.js");
        assert_eq!(
            result.lines,
            [
                (7, 1),
                (9, 1),
                (10, 1),
                (12, 2),
                (13, 1),
                (16, 1),
                (17, 1),
                (18, 2),
                (19, 1),
                (21, 1),
                (22, 0),
                (23, 0),
                (24, 0),
                (28, 1),
                (29, 0),
                (30, 0),
                (32, 0),
                (33, 0),
                (34, 0),
                (35, 0),
                (37, 0),
                (39, 0),
                (41, 0),
                (42, 0),
                (44, 0),
                (45, 0),
                (46, 0),
                (47, 0),
                (49, 0),
                (50, 0),
                (51, 0),
                (52, 0),
                (53, 0),
                (54, 0),
                (55, 0),
                (56, 0),
                (59, 0),
                (60, 0),
                (61, 0),
                (63, 0),
                (65, 0),
                (67, 1),
                (68, 2),
                (70, 1),
                (74, 1),
                (75, 1),
                (76, 1),
                (77, 1),
                (78, 1),
                (83, 1),
                (84, 1),
                (90, 1),
                (95, 1),
                (96, 1),
                (97, 1),
                (98, 1),
                (99, 1)
            ]
            .iter()
            .cloned()
            .collect()
        );
        assert!(result.functions.contains_key("MainProcessSingleton"));
        let func = result.functions.get("MainProcessSingleton").unwrap();
        assert_eq!(func.start, 15);
        assert!(func.executed);
        assert!(result
            .functions
            .contains_key("cubic-bezier(0.0, 0.0, 1.0, 1.0)"));
        let func = result
            .functions
            .get("cubic-bezier(0.0, 0.0, 1.0, 1.0)")
            .unwrap();
        assert_eq!(func.start, 95);
        assert!(func.executed);
    }

    #[test]
    fn test_lcov_parser_empty_line() {
        let mut f = File::open("./test/empty_line.info").expect("Failed to open lcov file");
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        let results = parse_lcov(buf, true, false).unwrap();

        assert_eq!(results.len(), 1);

        let (ref source_name, ref result) = results[0];
        assert_eq!(source_name, "aFile.js");
        assert_eq!(
            result.lines,
            [
                (7, 1),
                (9, 1),
                (10, 1),
                (12, 2),
                (13, 1),
                (16, 1),
                (17, 1),
                (18, 2),
                (19, 1),
                (21, 1),
                (22, 0),
                (23, 0),
                (24, 0),
                (28, 1),
                (29, 0),
                (30, 0),
                (32, 0),
                (33, 0),
                (34, 0),
                (35, 0),
                (37, 0),
                (39, 0),
                (41, 0),
                (42, 0),
                (44, 0),
                (45, 0),
                (46, 0),
                (47, 0),
                (49, 0),
                (50, 0),
                (51, 0),
                (52, 0),
                (53, 0),
                (54, 0),
                (55, 0),
                (56, 0),
                (59, 0),
                (60, 0),
                (61, 0),
                (63, 0),
                (65, 0),
                (67, 1),
                (68, 2),
                (70, 1),
                (74, 1),
                (75, 1),
                (76, 1),
                (77, 1),
                (78, 1),
                (83, 1),
                (84, 1),
                (90, 1),
                (95, 1),
                (96, 1),
                (97, 1),
                (98, 1),
                (99, 1)
            ]
            .iter()
            .cloned()
            .collect()
        );
        assert!(result.functions.contains_key("MainProcessSingleton"));
        let func = result.functions.get("MainProcessSingleton").unwrap();
        assert_eq!(func.start, 15);
        assert!(func.executed);
        assert!(result
            .functions
            .contains_key("cubic-bezier(0.0, 0.0, 1.0, 1.0)"));
        let func = result
            .functions
            .get("cubic-bezier(0.0, 0.0, 1.0, 1.0)")
            .unwrap();
        assert_eq!(func.start, 95);
        assert!(func.executed);
    }

    #[allow(non_snake_case)]
    #[test]
    fn test_lcov_parser_FN_record_with_negative_line_number_ignored() {
        // Some producers emit synthetic functions without a source line, e.g.
        // JaCoCo for Scala's `.curried` / `.tupled`. With --ignore-parsing-error,
        // the record is kept and reported at line 0 instead of discarding the
        // whole coverage file.
        let buf = "TN:\nSF:foo.scala\nFN:-1,curried\nFNDA:0,curried\nDA:1,0\nend_of_record\n"
            .as_bytes()
            .to_vec();
        let results = parse_lcov(buf, false, true).unwrap();
        assert_eq!(results.len(), 1);

        let (ref source_name, ref result) = results[0];
        assert_eq!(source_name, "foo.scala");
        assert_eq!(result.lines, [(1, 0)].iter().cloned().collect());
        let func = result.functions.get("curried").unwrap();
        assert_eq!(func.start, 0);
        assert!(!func.executed);
    }

    #[allow(non_snake_case)]
    #[test]
    fn test_lcov_parser_FN_record_with_negative_line_number_not_ignored() {
        // Without --ignore-parsing-error, a negative FN line number is a hard
        // parse error, same as any other malformed record.
        let buf = "TN:\nSF:foo.scala\nFN:-1,curried\nFNDA:0,curried\nDA:1,0\nend_of_record\n"
            .as_bytes()
            .to_vec();
        let result = parse_lcov(buf, false, false);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Invalid record: 'FN at line 3: negative line number marks a synthetic function'"
        );
    }

    #[allow(non_snake_case)]
    #[test]
    fn test_lcov_parser_FN_record_without_line_number() {
        // Control: a missing line number is still an invalid record.
        let buf = "SF:foo.scala\nFN:,curried\nend_of_record\n"
            .as_bytes()
            .to_vec();
        let result = parse_lcov(buf, false, false);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.to_string(), "Invalid record: 'FN at line 2'");
    }

    #[allow(non_snake_case)]
    #[test]
    fn test_lcov_parser_invalid_DA_record() {
        let mut f = File::open("./test/invalid_DA_record.info").expect("Failed to open lcov file");
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        let result = parse_lcov(buf, true, false);
        assert!(result.is_err());
    }

    #[allow(non_snake_case)]
    #[test]
    fn test_lcov_parser_ignoring_invalid_DA_record() {
        let mut f = File::open("./test/invalid_DA_record.info").expect("Failed to open lcov file");
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        let results = parse_lcov(buf, true, true).unwrap();
        assert_eq!(results.len(), 1);

        let (ref source_name, ref result) = results[0];
        assert!(source_name.ends_with("/socks_client_subprocess.js"));
        assert_eq!(
            result.lines,
            [
                (1, 1),
                (2, 1),
                (3, 1),
                (5, 2),
                (6, 1),
                (7, 1),
                (8, 2),
                (9, 1),
                (10, 2),
                (11, 1),
                (15, 6),
                (16, 6),
                (17, 2),
                (19, 6),
                (20, 12),
                (21, 6),
                (22, 6),
                (23, 6),
                (24, 6),
                (25, 6),
                (26, 6),
                (27, 6),
                (28, 6),
                (29, 6),
                (31, 0),
                (32, 0),
                (34, 6),
                (37, 8),
                (38, 6),
                (39, 6),
                (40, 12),
                (41, 6),
            ]
            .iter()
            .cloned()
            .collect()
        );
    }

    #[allow(non_snake_case)]
    #[test]
    fn test_lcov_parser_empty_DA_record() {
        let buf = "DA:152,4
DA:153,4
DA:154,8
DA:156,12
DA
TN:http_3a_2f_2fweb_2dplatform_2etest_3a8000_2freferrer_2dpolicy_2fgen_2fsrcdoc_2dinherit_2emeta_2funset_2fiframe_2dtag_2ehttp_2ehtml_2c_20about_3ablank"
        .as_bytes().to_vec();
        let result = parse_lcov(buf, true, false);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.to_string(), "Invalid record: 'DA at line 5'");
    }

    #[test]
    fn test_lcov_parser_end_of_record_without_source_file() {
        let buf = "end_of_record\n".as_bytes().to_vec();
        let result = parse_lcov(buf, true, false);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Invalid record: 'end_of_record without SF at line 1'"
        );
    }

    #[test]
    fn test_lcov_parser_ignoring_end_of_record_without_source_file() {
        let buf = "end_of_record\nSF:f.rs\nDA:1,1\nend_of_record\n"
            .as_bytes()
            .to_vec();
        let results = parse_lcov(buf, true, true).unwrap();
        assert_eq!(results.len(), 1);
        let (ref source_name, ref result) = results[0];
        assert_eq!(source_name, "f.rs");
        assert_eq!(result.lines, [(1, 1)].iter().cloned().collect());
    }

    #[test]
    fn test_lcov_parser_line_starting_with_e_is_not_end_of_record() {
        let buf = "SF:f.rs\nDA:1,1\nend_of_record\nexcluded_by_a_tool\n"
            .as_bytes()
            .to_vec();
        let results = parse_lcov(buf, true, false).unwrap();
        assert_eq!(results.len(), 1);
        let (ref source_name, ref result) = results[0];
        assert_eq!(source_name, "f.rs");
        assert_eq!(result.lines, [(1, 1)].iter().cloned().collect());
    }

    #[test]
    fn test_lcov_parser_end_of_record_line_endings() {
        for ending in ["\n", "\r\n", "", "\r"] {
            let buf = format!("SF:f.rs\nDA:1,1\nend_of_record{ending}").into_bytes();
            let results = parse_lcov(buf, true, false).unwrap();
            assert_eq!(results.len(), 1, "ending: {ending:?}");
            assert_eq!(results[0].0, "f.rs");
            assert_eq!(results[0].1.lines, [(1, 1)].iter().cloned().collect());
        }
    }

    #[test]
    fn test_lcov_parser_unknown_e_record_consumes_whole_line() {
        for record in [
            "e",
            "end_of_recor",
            "end_of_record_extra",
            "end_of_record\r\r",
            "excluded:SF:wrong.rs",
        ] {
            let buf = format!("SF:f.rs\n{record}\nDA:1,1\nend_of_record\n").into_bytes();
            let results = parse_lcov(buf, true, false).unwrap();
            assert_eq!(results.len(), 1, "record: {record:?}");
            assert_eq!(results[0].0, "f.rs");
            assert_eq!(results[0].1.lines, [(1, 1)].iter().cloned().collect());
        }
    }

    #[allow(non_snake_case)]
    #[test]
    fn test_lcov_parser_DA_record_with_checksum() {
        // `DA:<line>,<count>,<checksum>` is valid lcov: the checksum used to be
        // read back as the beginning of a new record.
        let buf = "SF:f.rs\nDA:5,3,e4f2\nDA:6,1\nend_of_record\n"
            .as_bytes()
            .to_vec();
        let results = parse_lcov(buf, true, false).unwrap();
        assert_eq!(results.len(), 1);
        let (ref source_name, ref result) = results[0];
        assert_eq!(source_name, "f.rs");
        assert_eq!(result.lines, [(5, 3), (6, 1)].iter().cloned().collect());
    }

    #[test]
    fn test_lcov_parser_invalid_key() {
        let buf = "SF:f.rs\nSFFFFF:x\nDA:1,1\nend_of_record\n"
            .as_bytes()
            .to_vec();
        let result = parse_lcov(buf, true, false);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.to_string(), "Invalid record: 'Invalid key at line 2'");
    }

    #[test]
    fn test_lcov_parser_ignoring_invalid_key() {
        // As for any other ignored parsing error the rest of the record is
        // dropped, but the parser must not panic.
        let buf = "SF:f.rs\nSFFFFF:x\nDA:1,1\nend_of_record\n"
            .as_bytes()
            .to_vec();
        let results = parse_lcov(buf, true, true).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_parser_gcov_json() {
        let results = parse_gcov_json(&read_gz(
            "./test/mozillavpn_serverconnection.gcno.gcov.json.gz",
        ))
        .unwrap();
        assert_eq!(results.len(), 37);
        let (ref source_name, ref result) = results[0];

        assert_eq!(source_name, "server/serverconnection.cpp");

        assert_eq!(
            result.lines,
            [
                (32, 0),
                (33, 0),
                (35, 0),
                (36, 0),
                (37, 0),
                (38, 0),
                (40, 0),
                (41, 0),
                (42, 0),
                (43, 0),
                (44, 0),
                (45, 0),
                (46, 0),
                (48, 0),
                (49, 0),
                (50, 0),
                (51, 0),
                (52, 0),
                (55, 0),
                (56, 0),
                (57, 0),
                (58, 0),
                (59, 0),
                (61, 0),
                (62, 0),
                (63, 0),
                (66, 0),
                (67, 0),
                (68, 0),
                (71, 0),
                (74, 0),
                (75, 0),
                (78, 0),
                (79, 0),
                (82, 0),
                (83, 0),
                (85, 0),
                (86, 0),
                (87, 0),
                (88, 0),
                (90, 0),
                (91, 0),
                (94, 0),
                (95, 0),
                (96, 0),
                (97, 0),
                (101, 0),
                (102, 0),
                (103, 0),
                (104, 0),
                (107, 0),
                (112, 0),
                (113, 0),
                (114, 0),
                (118, 0),
                (119, 0),
                (120, 0),
                (124, 0),
                (125, 0),
                (126, 0),
                (129, 0),
                (130, 0),
                (131, 0),
                (135, 0),
                (136, 0),
                (137, 0),
                (138, 0),
                (139, 0),
                (142, 0),
                (143, 0),
                (144, 0),
                (148, 0),
                (149, 0),
                (150, 0),
                (151, 0),
                (157, 0),
                (158, 0),
                (159, 0),
                (164, 0),
                (169, 0),
                (171, 0),
                (172, 0),
                (175, 0),
                (176, 0),
                (178, 0),
                (179, 0),
                (181, 0),
                (183, 0),
                (184, 0),
                (185, 0),
                (186, 0),
                (188, 0),
                (189, 0),
                (190, 0),
                (193, 0),
                (194, 0),
                (195, 0),
                (196, 0),
                (199, 0),
                (200, 0),
                (202, 0),
                (203, 0),
                (205, 0),
                (206, 0),
                (207, 0),
                (210, 0),
                (216, 0),
                (217, 0),
                (220, 0),
                (221, 0),
                (223, 0),
                (225, 0),
                (226, 0),
                (227, 0),
                (230, 0),
                (231, 0),
                (234, 0),
                (237, 0),
                (238, 0),
                (239, 0),
                (241, 0),
                (242, 0),
                (243, 0),
                (245, 0),
                (247, 0),
                (248, 0),
                (249, 0),
                (251, 0),
                (252, 0),
                (254, 0),
                (255, 0),
                (256, 0),
                (257, 0),
                (258, 0),
                (260, 0),
                (261, 0),
                (262, 0),
                (263, 0),
                (264, 0),
                (267, 0),
                (268, 0),
                (270, 0),
                (271, 0),
                (272, 0),
                (273, 0),
                (274, 0),
                (275, 0),
                (279, 0)
            ]
            .iter()
            .cloned()
            .collect()
        );

        assert_eq!(result.branches, [].iter().cloned().collect());

        assert!(result
            .functions
            .contains_key("ServerConnection::readData()"));
        let func = result
            .functions
            .get("ServerConnection::readData()")
            .unwrap();
        assert_eq!(func.start, 188);
        assert!(!func.executed);
    }

    fn read_gz(path: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        flate2::read::GzDecoder::new(File::open(path).unwrap())
            .read_to_end(&mut buf)
            .unwrap();
        buf
    }

    #[test]
    fn test_parser_gcov_json_several_documents() {
        let mut buf = read_gz("./test/mozillavpn_serverconnection.gcno.gcov.json.gz");
        buf.extend(buf.clone());
        assert_eq!(parse_gcov_json(&buf).unwrap().len(), 74);
    }

    #[test]
    fn test_parser_gcov_json_garbage() {
        assert!(parse_gcov_json(b"{\"format_version\": \"2\", \xcc\xa5").is_err());
    }

    #[test]
    fn test_parser_jacoco_xml_basic() {
        let mut lines: BTreeMap<u32, u64> = BTreeMap::new();
        lines.insert(1, 0);
        lines.insert(4, 1);
        lines.insert(6, 1);
        let mut functions: FunctionMap = FxHashMap::default();
        functions.insert(
            String::from("hello#<init>"),
            Function {
                executed: false,
                start: 1,
            },
        );
        functions.insert(
            String::from("hello#main"),
            Function {
                executed: true,
                start: 3,
            },
        );
        let mut branches: BTreeMap<u32, Vec<bool>> = BTreeMap::new();
        branches.insert(3, vec![true, true]);
        let expected = vec![(
            String::from("hello.java"),
            CovResult {
                lines,
                branches,
                functions,
            },
        )];

        let f = File::open("./test/jacoco/basic-report.xml").expect("Failed to open xml file");
        let file = BufReader::new(&f);
        let results = parse_jacoco_xml_report(file).unwrap();

        assert_eq!(results, expected);
    }

    #[test]
    fn test_parser_jacoco_xml_inner_classes() {
        let mut lines: BTreeMap<u32, u64> = BTreeMap::new();
        for i in &[5, 10, 14, 15, 18, 22, 23, 25, 27, 31, 34, 37, 44, 49] {
            lines.insert(*i, 0);
        }
        let mut functions: FunctionMap = FxHashMap::default();

        for (name, start, executed) in vec![
            ("Person$InnerClassForPerson#getSomethingElse", 31, false),
            ("Person#getSurname", 10, false),
            ("Person$InnerClassForPerson#<init>", 25, false),
            ("Person#setSurname", 14, false),
            ("Person#getAge", 18, false),
            (
                "Person$InnerClassForPerson$InnerInnerClass#<init>",
                34,
                false,
            ),
            ("Person$InnerClassForPerson#getSomething", 27, false),
            ("Person#<init>", 5, false),
            (
                "Person$InnerClassForPerson$InnerInnerClass#everything",
                37,
                false,
            ),
            ("Person#setAge", 22, false),
        ] {
            functions.insert(String::from(name), Function { start, executed });
        }
        let branches: BTreeMap<u32, Vec<bool>> = BTreeMap::new();
        let expected = vec![(
            String::from("org/gradle/Person.java"),
            CovResult {
                lines,
                branches,
                functions,
            },
        )];

        let f = File::open("./test/jacoco/inner-classes.xml").expect("Failed to open xml file");
        let file = BufReader::new(&f);
        let results = parse_jacoco_xml_report(file).unwrap();

        assert_eq!(results, expected);
    }

    #[test]
    fn test_parser_jacoco_kotlin() {
        let mut lines: BTreeMap<u32, u64> = BTreeMap::new();
        for i in &[
            (5, 0),
            (9, 0),
            (14, 0),
            (27, 0),
            (30, 0),
            (32, 0),
            (41, 0),
            (49, 0),
            (57, 0),
            (65, 0),
            (73, 0),
            (81, 0),
            (89, 0),
            (97, 0),
            (104, 0),
            (105, 0),
            (106, 0),
            (107, 0),
            (108, 0),
            (109, 0),
            (110, 0),
            (111, 0),
            (112, 0),
            (118, 0),
            (119, 0),
            (120, 0),
        ] {
            lines.insert(i.0, i.1);
        }

        let mut functions: FunctionMap = FxHashMap::default();
        for (name, start, executed) in vec![
            ("Breakpoint#getEntries", 112, false),
            ("BreakpointValue#xxsmall", 49, false),
            ("Breakpoint#<clinit>", 104, false),
            ("BreakpointValue#xxlarge", 97, false),
            ("BreakpointValue#none", 41, false),
            ("BreakpointValue#xlarge", 89, false),
            ("BreakpointValue#setValue", 26, false),
            ("BreakpointValue#<init>", 5, false),
            ("BreakpointValue#getBreakpointValueMap", 14, false),
            ("BreakpointDirection#getEntries", 120, false),
            ("BreakpointValue#small", 65, false),
            ("BreakpointDirection#<clinit>", 118, false),
            ("BreakpointValue#xsmall", 57, false),
            ("BreakpointValue#large", 81, false),
            ("BreakpointValue#medium", 73, false),
        ] {
            functions.insert(String::from(name), Function { start, executed });
        }

        let mut branches: BTreeMap<u32, Vec<bool>> = BTreeMap::new();
        branches.insert(26, vec![false, false, false, false]);

        let expected = vec![(
            String::from("BreakpointValue.kt"),
            CovResult {
                lines,
                branches,
                functions,
            },
        )];

        let f =
            File::open("./test/jacoco/kotlin-jacoco-report.xml").expect("Failed to open xml file");
        let file = BufReader::new(&f);
        let results = parse_jacoco_xml_report(file).unwrap();
        assert_eq!(results, expected);
    }

    #[test]
    fn parse_go() {
        let mut aggregator_lines: BTreeMap<u32, u64> = BTreeMap::new();
        let mut decoder_lines: BTreeMap<u32, u64> = BTreeMap::new();
        let no_branches = BTreeMap::<u32, Vec<bool>>::new();
        let no_functions: FxHashMap<String, Function> = FxHashMap::default();

        for i in &[
            (29, 1),
            (30, 1),
            (31, 1),
            (32, 1),
            (33, 1),
            (34, 1),
            (35, 0),
            (36, 0),
            (37, 0),
            (39, 1),
            (41, 1),
            (42, 1),
            (43, 1),
            (44, 1),
            (45, 1),
            (46, 1),
            (47, 1),
            (48, 1),
            (49, 1),
            (50, 1),
            (94, 1),
            (95, 1),
            (96, 1),
            (97, 1),
            (98, 1),
            (99, 1),
            (100, 1),
            (101, 1),
            (103, 0),
            (104, 0),
            (105, 0),
            (107, 0),
            (108, 0),
            (109, 0),
            (112, 1),
            (113, 1),
            (114, 1),
            (115, 1),
            (116, 1),
            (117, 1),
            (118, 1),
            (120, 1),
            (121, 1),
            (122, 1),
            (123, 1),
            (124, 1),
            (125, 1),
            (126, 1),
            (127, 1),
            (128, 1),
            (129, 1),
            (130, 1),
            (133, 1),
            (136, 1),
            (137, 0),
            (138, 0),
            (139, 0),
            (142, 1),
            (143, 1),
            (144, 1),
            (145, 1),
            (146, 1),
            (147, 1),
            (148, 1),
            (149, 1),
            (150, 1),
            (153, 1),
            (157, 1),
            (158, 1),
            (159, 1),
            (160, 1),
            (161, 1),
            (162, 1),
            (163, 1),
            (164, 1),
            (165, 1),
            (166, 0),
            (167, 0),
            (168, 0),
            (169, 0),
            (170, 0),
            (174, 1),
            (175, 1),
            (176, 1),
            (177, 1),
            (178, 1),
            (179, 1),
            (181, 1),
            (185, 1),
            (186, 1),
            (187, 1),
            (188, 1),
            (190, 1),
            (191, 1),
            (192, 1),
            (193, 0),
            (194, 0),
            (195, 0),
            (198, 1),
            (199, 1),
            (200, 1),
            (201, 1),
            (202, 1),
            (203, 1),
            (204, 1),
            (205, 1),
            (206, 1),
            (208, 1),
            (209, 1),
            (210, 1),
            (211, 1),
            (212, 1),
            (213, 1),
            (214, 1),
            (215, 1),
            (216, 1),
            (217, 1),
            (218, 1),
            (219, 1),
            (220, 1),
            (221, 1),
            (222, 1),
            (223, 1),
            (224, 1),
            (225, 1),
            (226, 1),
            (227, 1),
            (228, 1),
            (229, 1),
            (231, 1),
            (234, 1),
            (235, 1),
            (236, 1),
            (239, 1),
            (240, 1),
            (241, 1),
            (242, 1),
            (246, 1),
            (247, 1),
            (248, 1),
            (249, 1),
            (250, 1),
            (251, 1),
            (252, 1),
            (253, 1),
            (254, 1),
            (255, 1),
            (256, 1),
            (259, 1),
            (260, 1),
            (261, 1),
            (262, 1),
            (263, 1),
            (267, 1),
            (268, 1),
            (269, 1),
            (270, 1),
            (271, 1),
            (272, 1),
            (273, 1),
            (274, 1),
            (275, 1),
            (276, 1),
            (277, 1),
            (278, 1),
            (279, 1),
            (280, 1),
            (281, 1),
            (282, 1),
            (283, 1),
            (284, 1),
            (285, 1),
            (287, 1),
            (288, 1),
            (289, 1),
            (290, 1),
            (291, 1),
            (292, 1),
            (293, 1),
            (297, 1),
            (302, 1),
            (303, 1),
            (304, 1),
            (305, 1),
            (306, 0),
            (307, 0),
            (311, 1),
            (312, 1),
            (313, 1),
            (314, 1),
            (315, 1),
            (317, 1),
            (318, 1),
            (319, 1),
            (320, 1),
            (321, 1),
            (322, 1),
            (323, 1),
            (324, 1),
            (327, 1),
            (339, 1),
            (340, 1),
            (341, 1),
            (342, 1),
            (343, 1),
            (344, 1),
            (345, 1),
            (346, 1),
            (349, 1),
            (350, 1),
            (351, 1),
            (352, 1),
            (353, 1),
            (354, 1),
            (357, 1),
            (358, 1),
            (359, 1),
            (360, 1),
            (361, 1),
            (362, 1),
            (365, 1),
            (369, 1),
            (370, 1),
            (371, 1),
            (372, 1),
            (373, 1),
            (374, 1),
            (383, 1),
            (384, 1),
            (385, 1),
            (386, 1),
            (387, 1),
            (388, 1),
            (396, 1),
            (397, 1),
            (398, 1),
            (401, 1),
            (402, 1),
            (403, 1),
            (406, 1),
            (407, 1),
            (408, 1),
            (411, 1),
            (416, 1),
            (417, 1),
            (418, 1),
            (419, 1),
            (420, 1),
            (421, 1),
            (422, 1),
            (423, 1),
            (424, 1),
            (425, 1),
            (426, 1),
            (427, 0),
            (428, 0),
            (429, 0),
            (430, 0),
            (434, 1),
            (435, 1),
            (436, 1),
            (437, 1),
            (440, 1),
            (441, 1),
            (442, 1),
            (443, 1),
            (444, 1),
            (445, 1),
            (447, 1),
            (448, 1),
            (449, 1),
            (450, 1),
            (452, 1),
            (455, 1),
            (456, 1),
            (457, 1),
            (458, 1),
            (459, 1),
            (460, 1),
            (461, 1),
            (462, 1),
            (463, 1),
            (464, 1),
            (465, 1),
            (466, 1),
            (468, 1),
            (469, 1),
            (470, 1),
            (471, 1),
            (472, 1),
            (473, 1),
            (474, 1),
            (475, 1),
            (478, 1),
            (479, 1),
            (480, 1),
            (481, 1),
            (482, 1),
            (483, 1),
            (484, 1),
            (485, 1),
            (486, 1),
            (487, 1),
            (490, 1),
            (491, 1),
            (492, 1),
            (493, 1),
            (497, 1),
            (501, 1),
            (502, 1),
            (503, 1),
            (504, 1),
            (505, 1),
            (508, 1),
            (509, 1),
            (510, 1),
            (511, 0),
            (512, 0),
            (513, 0),
            (517, 1),
            (518, 1),
            (519, 1),
            (520, 1),
            (521, 1),
            (522, 1),
            (523, 1),
            (524, 1),
            (525, 1),
            (526, 1),
            (527, 1),
            (528, 1),
            (529, 1),
            (530, 1),
            (531, 1),
            (532, 1),
            (533, 1),
            (534, 1),
            (535, 1),
            (536, 1),
            (538, 1),
            (540, 1),
            (543, 1),
            (544, 1),
            (545, 1),
            (546, 1),
            (547, 1),
            (548, 1),
            (549, 1),
            (550, 1),
            (551, 1),
            (552, 1),
            (553, 1),
            (554, 1),
            (555, 1),
            (556, 1),
            (557, 1),
            (558, 1),
            (559, 1),
            (560, 0),
            (561, 0),
            (562, 0),
            (565, 1),
            (566, 1),
            (567, 1),
            (568, 1),
            (569, 1),
            (570, 1),
            (571, 1),
            (574, 1),
            (575, 1),
            (576, 1),
            (577, 1),
            (578, 1),
            (579, 1),
            (580, 1),
            (581, 1),
            (582, 1),
            (583, 1),
            (584, 1),
            (585, 1),
            (586, 1),
            (587, 1),
            (588, 1),
            (589, 1),
            (590, 1),
            (591, 1),
            (592, 1),
            (593, 1),
            (594, 1),
            (595, 1),
            (596, 1),
            (597, 1),
            (599, 1),
            (600, 1),
            (601, 1),
            (602, 1),
            (603, 1),
            (606, 1),
            (609, 1),
            (610, 1),
            (611, 1),
            (612, 1),
            (613, 1),
            (625, 1),
            (626, 1),
            (627, 1),
            (628, 1),
            (629, 1),
            (630, 1),
            (631, 1),
            (634, 1),
            (635, 1),
            (636, 1),
            (639, 1),
            (640, 1),
            (641, 1),
            (642, 1),
            (643, 1),
            (644, 1),
            (646, 1),
            (651, 1),
            (652, 1),
            (653, 1),
            (654, 1),
            (655, 1),
            (656, 1),
            (657, 1),
            (658, 1),
            (659, 1),
            (660, 1),
            (661, 1),
            (663, 1),
            (664, 1),
            (665, 1),
            (666, 1),
            (669, 1),
            (670, 1),
            (671, 1),
            (672, 1),
            (673, 1),
            (674, 1),
            (675, 1),
            (676, 1),
            (677, 1),
            (678, 1),
            (679, 1),
            (680, 1),
            (681, 1),
            (682, 1),
            (683, 1),
            (684, 1),
        ] {
            aggregator_lines.insert(i.0, i.1);
        }

        for i in &[
            (14, 1),
            (15, 1),
            (16, 1),
            (17, 1),
            (18, 1),
            (19, 1),
            (20, 1),
            (21, 1),
            (22, 1),
            (23, 1),
            (24, 1),
            (25, 1),
            (26, 1),
            (27, 1),
            (28, 0),
            (29, 0),
            (30, 0),
            (33, 1),
            (34, 1),
            (35, 0),
            (36, 0),
            (37, 0),
            (40, 1),
            (41, 1),
            (42, 0),
            (43, 0),
            (47, 1),
            (48, 0),
            (49, 0),
            (50, 0),
            (51, 0),
            (52, 0),
            (55, 0),
            (56, 0),
            (57, 0),
            (58, 0),
            (59, 0),
            (62, 0),
            (63, 0),
            (64, 0),
            (68, 1),
            (69, 1),
            (70, 1),
            (71, 1),
            (72, 1),
            (73, 1),
            (74, 1),
            (76, 1),
            (77, 0),
            (78, 0),
            (79, 0),
            (80, 0),
        ] {
            decoder_lines.insert(i.0, i.1);
        }

        let expected = vec![
            (
                String::from("package/folder/aggregator.go"),
                CovResult {
                    lines: aggregator_lines,
                    branches: no_branches.clone(),
                    functions: no_functions.clone(),
                },
            ),
            (
                String::from("package/folder/decoder.go"),
                CovResult {
                    lines: decoder_lines,
                    branches: no_branches.clone(),
                    functions: no_functions.clone(),
                },
            ),
        ];

        let f = File::open("./test/go/go.out").expect("Failed to open go coverage file");
        let mut file = BufReader::new(&f);
        let results = parse_gocov(&mut file).unwrap();
        assert_eq!(results, expected);
    }
}
