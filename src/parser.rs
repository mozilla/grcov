use flate2::read::GzDecoder;
use serde::Deserialize;
use std::cmp::Ordering;
use std::collections::{btree_map, hash_map, BTreeMap};
use std::fmt;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::num::ParseIntError;
use std::path::Path;
use std::str;

use log::error;

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

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
            quick_xml::Error::Io(e) => ParserError::Io(e),
            _ => ParserError::Parse(format!("{:?}", err)),
        }
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
            ParserError::Io(ref err) => write!(f, "IO error: {}", err),
            ParserError::Parse(ref s) => write!(f, "Record containing invalid integer: '{}'", s),
            ParserError::InvalidRecord(ref s) => write!(f, "Invalid record: '{}'", s),
            ParserError::InvalidData(ref s) => write!(f, "Invalid data: '{}'", s),
        }
    }
}

macro_rules! try_parse {
    ($v:expr, $l:expr) => {
        match $v.parse() {
            Ok(val) => val,
            Err(_err) => return Err(ParserError::Parse($l.to_string())),
        }
    };
}

macro_rules! try_next {
    ($v:expr, $l:expr) => {
        if let Some(val) = $v.next() {
            val
        } else {
            return Err(ParserError::InvalidRecord($l.to_string()));
        }
    };
}

macro_rules! try_parse_next {
    ($v:expr, $l:expr) => {
        try_parse!(try_next!($v, $l), $l)
    };
}

fn remove_newline(l: &mut Vec<u8>) {
    loop {
        let last = {
            let last = l.last();
            if last.is_none() {
                break;
            }
            *last.unwrap()
        };

        if last != b'\n' && last != b'\r' {
            break;
        }

        l.pop();
    }
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

    while let Some(c) = iter.next() {
        line += 1;
        match *c {
            b'e' => {
                // we've a end_of_record
                results.push((
                    cur_file.unwrap(),
                    CovResult {
                        lines: cur_lines,
                        branches: cur_branches,
                        functions: cur_functions,
                    },
                ));

                cur_file = None;
                cur_lines = BTreeMap::new();
                cur_branches = BTreeMap::new();
                cur_functions = FxHashMap::default();
                iter.take_while(|&&c| c != b'\n').last();
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
                    .take_while(|&&c| c != b':')
                    .fold(*c as u32, |r, &x| r * (1 << 8) + u32::from(x));
                match key {
                    SF => {
                        // SF:string
                        cur_file = Some(
                            iter.take_while(|&&c| c != b'\n' && c != b'\r')
                                .map(|&c| c as char)
                                .collect(),
                        );
                    }
                    DA => {
                        // DA:uint,int
                        let line_no = iter
                            .take_while(|&&c| c != b',')
                            .fold(0, |r, &x| r * 10 + u32::from(x - b'0'));
                        if iter.peek().is_none() {
                            return Err(ParserError::InvalidRecord(format!("DA at line {}", line)));
                        }
                        let execution_count = if let Some(c) = iter.next() {
                            if *c == b'-' {
                                iter.take_while(|&&c| c != b'\n').last();
                                0
                            } else {
                                iter.take_while(|&&c| c != b'\n' && c != b'\r')
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
                        // FN:int,string
                        let start = iter
                            .take_while(|&&c| c != b',')
                            .fold(0, |r, &x| r * 10 + u32::from(x - b'0'));
                        if iter.peek().is_none() {
                            return Err(ParserError::InvalidRecord(format!("FN at line {}", line)));
                        }
                        let f_name: String = iter
                            .take_while(|&&c| c != b'\n' && c != b'\r')
                            .map(|&c| c as char)
                            .collect();
                        if !duplicated_error_logged && cur_functions.contains_key(&f_name) {
                            error!(
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
                        // FNDA:int,string
                        let executed = iter
                            .take_while(|&&c| c != b',')
                            .fold(0, |r, &x| r * 10 + u64::from(x - b'0'));
                        if iter.peek().is_none() {
                            return Err(ParserError::InvalidRecord(format!(
                                "FNDA at line {}",
                                line
                            )));
                        }
                        let f_name: String = iter
                            .take_while(|&&c| c != b'\n' && c != b'\r')
                            .map(|&c| c as char)
                            .collect();
                        if let Some(f) = cur_functions.get_mut(&f_name) {
                            f.executed |= executed != 0;
                        } else {
                            return Err(ParserError::Parse(format!(
                                "FN record missing for function {}",
                                f_name
                            )));
                        }
                    }
                    BRDA => {
                        // BRDA:int,int,int,int or -
                        if branch_enabled {
                            let line_no = iter
                                .take_while(|&&c| c != b',')
                                .fold(0, |r, &x| r * 10 + u32::from(x - b'0'));
                            if iter.peek().is_none() {
                                return Err(ParserError::InvalidRecord(format!(
                                    "BRDA at line {}",
                                    line
                                )));
                            }
                            let _block_number = iter
                                .take_while(|&&c| c != b',')
                                .fold(0, |r, &x| r * 10 + u64::from(x - b'0'));
                            if iter.peek().is_none() {
                                return Err(ParserError::InvalidRecord(format!(
                                    "BRDA at line {}",
                                    line
                                )));
                            }
                            let branch_number = iter
                                .take_while(|&&c| c != b',')
                                .fold(0, |r, &x| r * 10 + u32::from(x - b'0'));
                            if iter.peek().is_none() {
                                return Err(ParserError::InvalidRecord(format!(
                                    "BRDA at line {}",
                                    line
                                )));
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
struct GcovLine {
    line_number: u32,
    function_name: Option<String>,
    count: u64,
    unexecuted_block: bool,
    branches: Vec<GcovBr>,
}

#[derive(Debug, Deserialize)]
struct GcovBr {
    count: u64,
    throw: bool,
    fallthrough: bool,
}

#[derive(Debug, Deserialize)]
struct GcovFunction {
    name: String,
    demangled_name: String,
    start_line: u32,
    start_column: u32,
    end_line: u32,
    end_column: u32,
    blocks: u32,
    blocks_executed: u32,
    execution_count: u64,
}

pub fn parse_gcov_gz(gcov_path: &Path) -> Result<Vec<(String, CovResult)>, ParserError> {
    let f = File::open(&gcov_path)
        .unwrap_or_else(|_| panic!("Failed to open gcov file {}", gcov_path.display()));

    let file = BufReader::new(&f);
    let gz = GzDecoder::new(file);
    let mut gcov: GcovJson = serde_json::from_reader(gz).unwrap();
    let mut results = Vec::new();

    if gcov.format_version != "1" {
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

    Ok(results)
}

pub fn parse_gcov(gcov_path: &Path) -> Result<Vec<(String, CovResult)>, ParserError> {
    let mut cur_file = None;
    let mut cur_lines = BTreeMap::new();
    let mut cur_branches = BTreeMap::new();
    let mut cur_functions = FxHashMap::default();
    let mut results = Vec::new();

    let f = File::open(&gcov_path)
        .unwrap_or_else(|_| panic!("Failed to open gcov file {}", gcov_path.display()));

    let mut file = BufReader::new(&f);
    let mut l = vec![];

    loop {
        l.clear();

        let num_bytes = file.read_until(b'\n', &mut l)?;
        if num_bytes == 0 {
            break;
        }
        remove_newline(&mut l);

        let l = unsafe { str::from_utf8_unchecked(&l) };

        let mut key_value = l.splitn(2, ':');
        let key = try_next!(key_value, l);
        let value = try_next!(key_value, l);

        match key {
            "file" => {
                if let Some(cur_file) = cur_file.filter(|_: &String| !cur_lines.is_empty()) {
                    // println!("{} {} {:?}", gcov_path.display(), cur_file, cur_lines);
                    results.push((
                        cur_file,
                        CovResult {
                            lines: cur_lines,
                            branches: cur_branches,
                            functions: cur_functions,
                        },
                    ));
                }

                cur_file = Some(value.to_owned());
                cur_lines = BTreeMap::new();
                cur_branches = BTreeMap::new();
                cur_functions = FxHashMap::default();
            }
            "function" => {
                let mut f_splits = value.splitn(3, ',');
                let start = try_parse_next!(f_splits, l);
                let executed = try_next!(f_splits, l) != "0";
                let f_name = try_next!(f_splits, l);
                cur_functions.insert(f_name.to_owned(), Function { start, executed });
            }
            "lcount" => {
                let mut values = value.splitn(2, ',');
                let line_no = try_parse_next!(values, l);
                let execution_count = try_next!(values, l);
                if execution_count == "0" || execution_count.starts_with('-') {
                    cur_lines.insert(line_no, 0);
                } else {
                    cur_lines.insert(line_no, try_parse!(execution_count, l));
                }
            }
            "branch" => {
                let mut values = value.splitn(2, ',');
                let line_no = try_parse_next!(values, l);
                let taken = try_next!(values, l) == "taken";
                match cur_branches.entry(line_no) {
                    btree_map::Entry::Occupied(c) => {
                        let v = c.into_mut();
                        v.push(taken);
                    }
                    btree_map::Entry::Vacant(p) => {
                        p.insert(vec![taken; 1]);
                    }
                }
            }
            _ => {}
        }
    }

    if !cur_lines.is_empty() {
        results.push((
            cur_file.unwrap(),
            CovResult {
                lines: cur_lines,
                branches: cur_branches,
                functions: cur_functions,
            },
        ));
    }

    Ok(results)
}

fn get_xml_attribute<R: BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    name: &str,
) -> Result<String, ParserError> {
    for a in event.attributes() {
        let a = a?;
        if a.key == name.as_bytes() {
            return Ok(a.unescape_and_decode_value(reader)?);
        }
    }
    Err(ParserError::InvalidRecord(format!(
        "Attribute {} not found",
        name
    )))
}

fn parse_jacoco_report_sourcefile<T: BufRead>(
    parser: &mut Reader<T>,
    buf: &mut Vec<u8>,
) -> Result<JacocoReport, ParserError> {
    let mut lines: BTreeMap<u32, u64> = BTreeMap::new();
    let mut branches: BTreeMap<u32, Vec<bool>> = BTreeMap::new();

    loop {
        match parser.read_event(buf) {
            Ok(Event::Start(ref e)) if e.local_name() == b"line" => {
                let (mut ci, mut cb, mut mb, mut nr) = (None, None, None, None);
                for a in e.attributes() {
                    let a = a?;
                    match a.key {
                        b"ci" => ci = Some(parser.decode(&a.value)?.parse::<u64>()?),
                        b"cb" => cb = Some(parser.decode(&a.value)?.parse::<u64>()?),
                        b"mb" => mb = Some(parser.decode(&a.value)?.parse::<u64>()?),
                        b"nr" => nr = Some(parser.decode(&a.value)?.parse::<u32>()?),
                        _ => (),
                    }
                }

                fn try_att<T>(opt: Option<T>, name: &str) -> Result<T, ParserError> {
                    opt.ok_or_else(|| {
                        ParserError::InvalidRecord(format!("Attribute {} not found", name))
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
                    let hit = if ci > 0 { 1 } else { 0 };
                    lines.insert(nr, hit);
                }
            }
            Ok(Event::End(ref e)) if e.local_name() == b"sourcefile" => {
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
        match parser.read_event(buf) {
            Ok(Event::Start(ref e)) if e.local_name() == b"counter" => {
                if get_xml_attribute(parser, e, "type")? == "METHOD" {
                    executed = get_xml_attribute(parser, e, "covered")?.parse::<u32>()? > 0;
                }
            }
            Ok(Event::End(ref e)) if e.local_name() == b"method" => break,
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
        match parser.read_event(buf) {
            Ok(Event::Start(ref e)) if e.local_name() == b"method" => {
                let name = get_xml_attribute(parser, e, "name")?;
                let full_name = format!("{}#{}", class_name, name);

                let start_line = get_xml_attribute(parser, e, "line")?.parse::<u32>()?;
                let function = parse_jacoco_report_method(parser, buf, start_line)?;
                functions.insert(full_name, function);
            }
            Ok(Event::End(ref e)) if e.local_name() == b"class" => break,
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
        match parser.read_event(buf) {
            Ok(Event::Start(ref e)) => {
                match e.local_name() {
                    b"class" => {
                        // Fully qualified class name: "org/example/Person$Age"
                        let fq_class = get_xml_attribute(parser, e, "name")?;
                        // Class name: "Person$Age"
                        let class = fq_class
                            .split('/')
                            .last()
                            .expect("Failed to parse class name");
                        // Class name "Person"
                        let top_class = class
                            .split('$')
                            .next()
                            .expect("Failed to parse top class name");

                        // Process all <method /> and <counter /> for this class
                        let functions = parse_jacoco_report_class(parser, buf, class)?;

                        match results_map.entry(top_class.to_string()) {
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
                    b"sourcefile" => {
                        let sourcefile = get_xml_attribute(parser, e, "name")?;
                        let class = sourcefile.trim_end_matches(".java");
                        let JacocoReport { lines, branches } =
                            parse_jacoco_report_sourcefile(parser, buf)?;

                        match results_map.entry(class.to_string()) {
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
            Ok(Event::End(ref e)) if e.local_name() == b"package" => break,
            Err(e) => return Err(ParserError::Parse(e.to_string())),
            _ => {}
        }
    }

    for (class, result) in &results_map {
        if result.lines.is_empty() && result.branches.is_empty() {
            return Err(ParserError::InvalidData(format!(
                "Class {}/{} is not the top class in its file.",
                package, class
            )));
        }
    }

    // Change all keys from the class name to the file name and turn the result into a Vec.
    // If package is the empty string, we have to trim the leading '/' in order to obtain a
    // relative path.
    Ok(results_map
        .into_iter()
        .map(|(class, result)| {
            (
                format!("{}/{}.java", package, class)
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
    parser.expand_empty_elements(true).trim_text(false);

    let mut results = Vec::new();
    let mut buf = Vec::new();

    loop {
        match parser.read_event(&mut buf) {
            Ok(Event::Start(ref e)) if e.local_name() == b"package" => {
                let package = get_xml_attribute(&parser, e, "name")?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remove_newline() {
        let mut l = "Marco".as_bytes().to_vec();
        remove_newline(&mut l);
        assert_eq!(l, "Marco".as_bytes().to_vec());

        let mut l = "Marco\n".as_bytes().to_vec();
        remove_newline(&mut l);
        assert_eq!(l, "Marco".as_bytes().to_vec());

        let mut l = "Marco\r".as_bytes().to_vec();
        remove_newline(&mut l);
        assert_eq!(l, "Marco".as_bytes().to_vec());

        let mut l = "Marco\r\n".as_bytes().to_vec();
        remove_newline(&mut l);
        assert_eq!(l, "Marco".as_bytes().to_vec());

        let mut l = "\r\n".as_bytes().to_vec();
        remove_newline(&mut l);
        assert_eq!(l, "".as_bytes().to_vec());
    }

    #[test]
    fn test_lcov_parser() {
        let mut f = File::open("./test/prova.info").expect("Failed to open lcov file");
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        let results = parse_lcov(buf, false).unwrap();

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
        let results = parse_lcov(buf, true).unwrap();

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
        let results = parse_lcov(buf, true).unwrap();

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
        let results = parse_lcov(buf, true).unwrap();

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
    fn test_lcov_parser_invalid_DA_record() {
        let mut f = File::open("./test/invalid_DA_record.info").expect("Failed to open lcov file");
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        let result = parse_lcov(buf, true);
        assert!(result.is_err());
    }

    #[test]
    fn test_parser() {
        let results = parse_gcov(Path::new("./test/prova.gcov")).unwrap();

        assert_eq!(results.len(), 10);

        let (ref source_name, ref result) = results[0];
        assert_eq!(source_name, "/home/marco/Documenti/FD/mozilla-central/build-cov-gcc/dist/include/nsExpirationTracker.h");
        assert_eq!(
            result.lines,
            [
                (393, 0),
                (397, 0),
                (399, 0),
                (401, 0),
                (402, 0),
                (403, 0),
                (405, 0)
            ]
            .iter()
            .cloned()
            .collect()
        );
        assert!(result.functions.contains_key("_ZN19nsExpirationTrackerIN11nsIDocument16SelectorCacheKeyELj4EE25ExpirationTrackerObserver7ReleaseEv"));
        let mut func = result.functions.get("_ZN19nsExpirationTrackerIN11nsIDocument16SelectorCacheKeyELj4EE25ExpirationTrackerObserver7ReleaseEv").unwrap();
        assert_eq!(func.start, 393);
        assert!(!func.executed);

        let (ref source_name, ref result) = results[5];
        assert_eq!(
            source_name,
            "/home/marco/Documenti/FD/mozilla-central/accessible/atk/Platform.cpp"
        );
        assert_eq!(
            result.lines,
            [
                (81, 0),
                (83, 0),
                (85, 0),
                (87, 0),
                (88, 0),
                (90, 0),
                (94, 0),
                (96, 0),
                (97, 0),
                (98, 0),
                (99, 0),
                (100, 0),
                (101, 0),
                (103, 0),
                (104, 0),
                (108, 0),
                (110, 0),
                (111, 0),
                (112, 0),
                (115, 0),
                (117, 0),
                (118, 0),
                (122, 0),
                (123, 0),
                (124, 0),
                (128, 0),
                (129, 0),
                (130, 0),
                (136, 17),
                (138, 17),
                (141, 0),
                (142, 0),
                (146, 0),
                (147, 0),
                (148, 0),
                (151, 0),
                (152, 0),
                (153, 0),
                (154, 0),
                (155, 0),
                (156, 0),
                (157, 0),
                (161, 0),
                (162, 0),
                (165, 0),
                (166, 0),
                (167, 0),
                (168, 0),
                (169, 0),
                (170, 0),
                (171, 0),
                (172, 0),
                (184, 0),
                (187, 0),
                (189, 0),
                (190, 0),
                (194, 0),
                (195, 0),
                (196, 0),
                (200, 0),
                (201, 0),
                (202, 0),
                (203, 0),
                (207, 0),
                (208, 0),
                (216, 17),
                (218, 17),
                (219, 0),
                (220, 0),
                (221, 0),
                (222, 0),
                (223, 0),
                (226, 17),
                (232, 0),
                (233, 0),
                (234, 0),
                (253, 17),
                (261, 11390),
                (265, 11390),
                (268, 373),
                (274, 373),
                (277, 373),
                (278, 373),
                (281, 373),
                (288, 373),
                (289, 373),
                (293, 373),
                (294, 373),
                (295, 373),
                (298, 373),
                (303, 5794),
                (306, 5794),
                (307, 5558),
                (309, 236),
                (311, 236),
                (312, 236),
                (313, 0),
                (316, 236),
                (317, 236),
                (318, 0),
                (321, 236),
                (322, 236),
                (323, 236),
                (324, 236),
                (327, 236),
                (328, 236),
                (329, 236),
                (330, 236),
                (331, 472),
                (332, 472),
                (333, 236),
                (338, 236),
                (339, 236),
                (340, 236),
                (343, 0),
                (344, 0),
                (345, 0),
                (346, 0),
                (347, 0),
                (352, 236),
                (353, 236),
                (354, 236),
                (355, 236),
                (361, 236),
                (362, 236),
                (364, 236),
                (365, 236),
                (370, 0),
                (372, 0),
                (373, 0),
                (374, 0),
                (376, 0)
            ]
            .iter()
            .cloned()
            .collect()
        );
        assert!(result
            .functions
            .contains_key("_ZL13LoadGtkModuleR24GnomeAccessibilityModule"));
        func = result
            .functions
            .get("_ZL13LoadGtkModuleR24GnomeAccessibilityModule")
            .unwrap();
        assert_eq!(func.start, 81);
        assert!(!func.executed);
        assert!(result
            .functions
            .contains_key("_ZN7mozilla4a11y12PlatformInitEv"));
        func = result
            .functions
            .get("_ZN7mozilla4a11y12PlatformInitEv")
            .unwrap();
        assert_eq!(func.start, 136);
        assert!(func.executed);
        assert!(result
            .functions
            .contains_key("_ZN7mozilla4a11y16PlatformShutdownEv"));
        func = result
            .functions
            .get("_ZN7mozilla4a11y16PlatformShutdownEv")
            .unwrap();
        assert_eq!(func.start, 216);
        assert!(func.executed);
        assert!(result.functions.contains_key("_ZN7mozilla4a11y7PreInitEv"));
        func = result.functions.get("_ZN7mozilla4a11y7PreInitEv").unwrap();
        assert_eq!(func.start, 261);
        assert!(func.executed);
        assert!(result
            .functions
            .contains_key("_ZN7mozilla4a11y19ShouldA11yBeEnabledEv"));
        func = result
            .functions
            .get("_ZN7mozilla4a11y19ShouldA11yBeEnabledEv")
            .unwrap();
        assert_eq!(func.start, 303);
        assert!(func.executed);
    }

    #[test]
    fn test_parser_gcov_with_negative_counts() {
        let results = parse_gcov(Path::new("./test/negative_counts.gcov")).unwrap();
        assert_eq!(results.len(), 118);
        let (ref source_name, ref result) = results[14];
        assert_eq!(source_name, "/home/marco/Documenti/FD/mozilla-central/build-cov-gcc/dist/include/mozilla/Assertions.h");
        assert_eq!(result.lines, [(40, 0)].iter().cloned().collect());
    }

    #[test]
    fn test_parser_gcov_with_64bit_counts() {
        let results = parse_gcov(Path::new("./test/64bit_count.gcov")).unwrap();
        assert_eq!(results.len(), 46);
        let (ref source_name, ref result) = results[8];
        assert_eq!(
            source_name,
            "/home/marco/Documenti/FD/mozilla-central/build-cov-gcc/dist/include/js/HashTable.h"
        );
        assert_eq!(
            result.lines,
            [
                (324, 8096),
                (343, 12174),
                (344, 6085),
                (345, 23331),
                (357, 10720),
                (361, 313_165_934),
                (399, 272_539_208),
                (402, 31_491_125),
                (403, 35_509_735),
                (420, 434_104),
                (709, 313_172_766),
                (715, 272_542_535),
                (801, 584_943_263),
                (822, 0),
                (825, 0),
                (826, 0),
                (828, 0),
                (829, 0),
                (831, 0),
                (834, 2_210_404_897),
                (835, 196_249_666),
                (838, 3_764_974),
                (840, 516_370_744),
                (841, 1_541_684),
                (842, 2_253_988_941),
                (843, 197_245_483),
                (844, 0),
                (845, 5_306_658),
                (846, 821_426_720),
                (847, 47_096_565),
                (853, 82_598_134),
                (854, 247_796_865),
                (886, 272_542_256),
                (887, 272_542_256),
                (904, 599_154_437),
                (908, 584_933_028),
                (913, 584_943_263),
                (916, 543_534_922),
                (917, 584_933_028),
                (940, 508_959_481),
                (945, 1_084_660_344),
                (960, 545_084_512),
                (989, 534_593),
                (990, 128_435),
                (1019, 427_973_453),
                (1029, 504_065_334),
                (1038, 1_910_289_238),
                (1065, 425_402),
                (1075, 10_613_316),
                (1076, 5_306_658),
                (1090, 392_499_332),
                (1112, 48_208),
                (1113, 48_208),
                (1114, 0),
                (1115, 0),
                (1118, 48211),
                (1119, 8009),
                (1120, 48211),
                (1197, 40347),
                (1202, 585_715_301),
                (1207, 1_171_430_602),
                (1210, 585_715_301),
                (1211, 910_968),
                (1212, 585_715_301),
                (1222, 30_644),
                (1223, 70_165),
                (1225, 1647),
                (1237, 4048),
                (1238, 4048),
                (1240, 8096),
                (1244, 6087),
                (1250, 6087),
                (1257, 6085),
                (1264, 6085),
                (1278, 6085),
                (1279, 6085),
                (1280, 0),
                (1283, 6085),
                (1284, 66935),
                (1285, 30425),
                (1286, 30425),
                (1289, 6085),
                (1293, 12171),
                (1294, 6086),
                (1297, 6087),
                (1299, 6087),
                (1309, 4048),
                (1310, 4048),
                (1316, 632_104_110),
                (1327, 251_893_735),
                (1329, 251_893_735),
                (1330, 251_893_735),
                (1331, 503_787_470),
                (1337, 528_619_265),
                (1344, 35_325_952),
                (1345, 35_325_952),
                (1353, 26236),
                (1354, 13118),
                (1364, 305_520_839),
                (1372, 585_099_705),
                (1381, 585_099_705),
                (1382, 585_099_705),
                (1385, 585_099_705),
                (1391, 1_135_737_600),
                (1397, 242_807_686),
                (1400, 242_807_686),
                (1403, 1_032_741_488),
                (1404, 1_290_630),
                (1405, 1_042_115),
                (1407, 515_080_114),
                (1408, 184_996_962),
                (1412, 516_370_744),
                (1414, 516_370_744),
                (1415, 516_370_744),
                (1417, 154_330_912),
                (1420, 812_664_176),
                (1433, 47_004_405),
                (1442, 47_004_405),
                (1443, 47_004_405),
                (1446, 94_008_810),
                (1452, 9_086_049),
                (1456, 24_497_042),
                (1459, 12_248_521),
                (1461, 12_248_521),
                (1462, 24_497_042),
                (1471, 30642),
                (1474, 30642),
                (1475, 30642),
                (1476, 30642),
                (1477, 30642),
                (1478, 30642),
                (1484, 64904),
                (1485, 34260),
                (1489, 34260),
                (1490, 34260),
                (1491, 34260),
                (1492, 34260),
                (1495, 34260),
                (1496, 69_792_911),
                (1497, 139_524_496),
                (1498, 94_193_130),
                (1499, 47_096_565),
                (1500, 47_096_565),
                (1506, 61326),
                (1507, 30663),
                (1513, 58000),
                (1516, 35_325_952),
                (1518, 35_325_952),
                (1522, 29000),
                (1527, 29000),
                (1530, 29000),
                (1534, 0),
                (1536, 0),
                (1537, 0),
                (1538, 0),
                (1540, 0),
                (1547, 10_613_316),
                (1548, 1_541_684),
                (1549, 1_541_684),
                (1552, 3_764_974),
                (1554, 5_306_658),
                (1571, 8009),
                (1573, 8009),
                (1574, 8009),
                (1575, 31345),
                (1576, 5109),
                (1577, 5109),
                (1580, 8009),
                (1581, 1647),
                (1582, 8009),
                (1589, 0),
                (1592, 0),
                (1593, 0),
                (1594, 0),
                (1596, 0),
                (1597, 0),
                (1599, 0),
                (1600, 0),
                (1601, 0),
                (1604, 0),
                (1605, 0),
                (1606, 0),
                (1607, 0),
                (1609, 0),
                (1610, 0),
                (1611, 0),
                (1615, 0),
                (1616, 0),
                (1625, 0),
                (1693, 655_507),
                (1711, 35_615_006),
                (1730, 10720),
                (1732, 10720),
                (1733, 10720),
                (1735, 10720),
                (1736, 10720),
                (1739, 313_162_046),
                (1741, 313_162_046),
                (1743, 313_162_046),
                (1744, 313_162_046),
                (1747, 272_542_535),
                (1749, 272_542_535),
                (1750, 272_542_535),
                (1752, 272_542_535),
                (1753, 272_542_535),
                (1754, 272_542_256),
                (1755, 272_542_256),
                (1759, 35_509_724),
                (1761, 35_509_724),
                (1767, 71_019_448),
                (1772, 35_505_028),
                (1773, 179_105),
                (1776, 179_105),
                (1777, 179_105),
                (1780, 35_325_923),
                (1781, 35_326_057),
                (1785, 35_326_058),
                (1786, 29011),
                (1789, 71_010_332),
                (1790, 35_505_166),
                (1796, 35_505_166)
            ]
            .iter()
            .cloned()
            .collect()
        );

        // Assert more stuff.
    }

    #[test]
    fn test_parser_gcov_with_branches() {
        let results = parse_gcov(Path::new("./test/intermediate_with_branches.gcov")).unwrap();
        assert_eq!(results.len(), 1);
        let (ref source_name, ref result) = results[0];

        assert_eq!(source_name, "/home/marco/Documenti/FD/mozilla-central/build-cov-gcc/dist/include/nsExpirationTracker.h");

        assert_eq!(
            result.lines,
            [
                (393, 0),
                (397, 0),
                (399, 0),
                (401, 1),
                (402, 0),
                (403, 0),
                (405, 0)
            ]
            .iter()
            .cloned()
            .collect()
        );

        assert_eq!(
            result.branches,
            [(399, vec![false, false]), (401, vec![true, false])]
                .iter()
                .cloned()
                .collect()
        );

        assert!(result.functions.contains_key("_ZN19nsExpirationTrackerIN11nsIDocument16SelectorCacheKeyELj4EE25ExpirationTrackerObserver7ReleaseEv"));
        let func = result.functions.get("_ZN19nsExpirationTrackerIN11nsIDocument16SelectorCacheKeyELj4EE25ExpirationTrackerObserver7ReleaseEv").unwrap();
        assert_eq!(func.start, 393);
        assert!(!func.executed);
    }

    #[test]
    fn test_parser_gcov_rust_generics_with_two_parameters() {
        let results = parse_gcov(Path::new(
            "./test/rust/generics_with_two_parameters_intermediate.gcov",
        ))
        .unwrap();
        assert_eq!(results.len(), 1);
        let (ref source_name, ref result) = results[0];

        assert_eq!(source_name, "src/main.rs");

        assert_eq!(
            result.lines,
            [(4, 3), (5, 3), (6, 1), (9, 2), (10, 1), (11, 1), (12, 2)]
                .iter()
                .cloned()
                .collect()
        );

        assert_eq!(result.branches, [].iter().cloned().collect());

        assert!(result
            .functions
            .contains_key("_ZN27rust_code_coverage_sample_24mainE"));
        let func = result
            .functions
            .get("_ZN27rust_code_coverage_sample_24mainE")
            .unwrap();
        assert_eq!(func.start, 8);
        assert!(func.executed);

        assert!(result.functions.contains_key(
            "_ZN27rust_code_coverage_sample_244compare_types<[i32; 3],alloc::vec::Vec<i32>>E"
        ));
        let func = result
            .functions
            .get("_ZN27rust_code_coverage_sample_244compare_types<[i32; 3],alloc::vec::Vec<i32>>E")
            .unwrap();
        assert_eq!(func.start, 3);
        assert!(func.executed);
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
    #[should_panic]
    fn test_parser_jacoco_xml_non_top_level_classes_panics() {
        let f = File::open("./test/jacoco/multiple-top-level-classes.xml")
            .expect("Failed to open xml file");
        let file = BufReader::new(&f);
        let _results = parse_jacoco_xml_report(file).unwrap();
    }

    #[test]
    #[should_panic]
    fn test_parser_jacoco_xml_full_report_with_non_top_level_classes_panics() {
        let f = File::open("./test/jacoco/full-junit4-report-multiple-top-level-classes.xml")
            .expect("Failed to open xml file");
        let file = BufReader::new(&f);
        let _results = parse_jacoco_xml_report(file).unwrap();
    }
}
