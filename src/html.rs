use chrono::{DateTime, Utc};
use std::cmp::Ordering;
use std::collections::{btree_map, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::defs::*;

#[derive(Debug)]
enum Line<'a> {
    Str(String),
    Slice(&'a str),
}

struct Lines<'a> {
    start: usize,
    data: &'a str,
}

impl HtmlStats {
    #[inline(always)]
    pub fn add(&mut self, stats: &Self) {
        self.total_lines += stats.total_lines;
        self.covered_lines += stats.covered_lines;
        self.total_funs += stats.total_funs;
        self.covered_funs += stats.covered_funs;
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    hi_limit: f64,
    med_limit: f64,
    fn_hi_limit: f64,
    fn_med_limit: f64,
    date: DateTime<Utc>,
}

pub fn get_config() -> Config {
    Config {
        hi_limit: 90.,
        med_limit: 75.,
        fn_hi_limit: 90.,
        fn_med_limit: 75.,
        date: Utc::now(),
    }
}

impl Ord for HtmlFileStats {
    fn cmp(&self, other: &Self) -> Ordering {
        self.file_name.cmp(&other.file_name)
    }
}

impl PartialOrd for HtmlFileStats {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for HtmlFileStats {
    fn eq(&self, other: &Self) -> bool {
        self.file_name == other.file_name
    }
}

impl Eq for HtmlFileStats {}

fn get_fn_severity(conf: &Config, rate: f64) -> &str {
    if conf.fn_hi_limit <= rate && rate <= 100. {
        "funHi"
    } else if conf.fn_med_limit <= rate && rate < conf.fn_hi_limit {
        "funMed"
    } else {
        "funLow"
    }
}

fn get_severity(conf: &Config, rate: f64) -> &str {
    if conf.hi_limit <= rate && rate <= 100. {
        "lineHi"
    } else if conf.med_limit <= rate && rate < conf.hi_limit {
        "lineMed"
    } else {
        "lineLow"
    }
}

macro_rules! lines_iter {
    ($data: expr) => {{
        Lines {
            start: 0,
            data: $data,
        }
    }};
}

impl<'a> Iterator for Lines<'a> {
    type Item = Line<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.start >= self.data.len() {
            return None;
        }
        let mut buf = String::with_capacity(512);
        let mut n = self.start;
        for c in self.data[self.start..].bytes() {
            match c {
                b'>' => {
                    buf.push_str(&self.data[self.start..n]);
                    buf.push_str("&gt;");
                    self.start = n + 1;
                }
                b'<' => {
                    buf.push_str(&self.data[self.start..n]);
                    buf.push_str("&lt;");
                    self.start = n + 1;
                }
                b'&' => {
                    buf.push_str(&self.data[self.start..n]);
                    buf.push_str("&amp;");
                    self.start = n + 1;
                }
                b'\'' => {
                    buf.push_str(&self.data[self.start..n]);
                    buf.push_str("&#39;");
                    self.start = n + 1;
                }
                b'"' => {
                    buf.push_str(&self.data[self.start..n]);
                    buf.push_str("&quot;");
                    self.start = n + 1;
                }
                b'\n' => {
                    let s = &self.data[self.start..n];
                    let res = if buf.is_empty() {
                        Some(Line::Slice(s))
                    } else {
                        buf.push_str(s);
                        Some(Line::Str(buf))
                    };
                    self.start = n + 1;
                    return res;
                }
                _ => {}
            };
            n += 1;
        }

        if self.start < self.data.len() {
            let s = &self.data[self.start..];
            let res = if buf.is_empty() {
                Some(Line::Slice(s))
            } else {
                buf.push_str(s);
                Some(Line::Str(buf))
            };
            self.start = self.data.len();
            res
        } else {
            None
        }
    }
}

fn create_parent(path: &PathBuf) {
    let dest_parent = path.parent().unwrap();
    if !dest_parent.exists() && fs::create_dir_all(dest_parent).is_err() {
        panic!("Cannot create parent directory: {:?}", dest_parent);
    }
}

fn add_html_ext(path: &PathBuf) -> PathBuf {
    if let Some(ext) = path.extension() {
        let mut ext = ext.to_str().unwrap().to_owned();
        ext.push_str(".html");
        path.with_extension(ext)
    } else {
        path.with_extension(".html")
    }
}

fn get_stats(result: &CovResult) -> HtmlStats {
    let total_lines = result.lines.len();
    let covered_lines = result.lines.values().filter(|x| **x > 0).count();
    let total_funs = result.functions.len();
    let covered_funs = result.functions.values().filter(|f| f.executed).count();

    HtmlStats {
        total_lines,
        covered_lines,
        total_funs,
        covered_funs,
    }
}

#[inline(always)]
fn get_percentage(x: usize, y: usize) -> f64 {
    if y != 0 {
        (x as f64) / (y as f64) * 100.
    } else {
        0.0
    }
}

fn get_base(rel_path: &PathBuf) -> String {
    let count = rel_path.components().count() - 1 /* -1 for the file itself */;
    "../".repeat(count).to_string()
}

fn get_dirs_result(global: Arc<Mutex<HtmlGlobalStats>>, rel_path: &PathBuf, stats: &HtmlStats) {
    let parent = rel_path.parent().unwrap().to_str().unwrap().to_string();
    let file_name = rel_path.file_name().unwrap().to_str().unwrap().to_string();
    let fs = HtmlFileStats {
        file_name,
        stats: stats.clone(),
    };
    let mut global = global.lock().unwrap();
    global.stats.add(stats);
    match global.dirs.entry(parent) {
        btree_map::Entry::Occupied(ds) => {
            let ds = ds.into_mut();
            ds.stats.add(stats);
            ds.files.insert(fs);
        }
        btree_map::Entry::Vacant(v) => {
            let mut files = BTreeSet::new();
            files.insert(fs);
            v.insert(HtmlDirStats {
                files,
                stats: stats.clone(),
            });
        }
    };
}

pub fn gen_index(global: HtmlGlobalStats, conf: Config, output: &PathBuf) {
    let output_file = output.join("index.html");
    create_parent(&output_file);
    let mut output_stream = match File::create(&output_file) {
        Err(_) => {
            eprintln!("Cannot create file {:?}", output_file);
            return;
        }
        Ok(f) => f,
    };

    let covered_lines_per = get_percentage(global.stats.covered_lines, global.stats.total_lines);
    let covered_funs_per = get_percentage(global.stats.covered_funs, global.stats.total_funs);
    let funs_sev = get_fn_severity(&conf, covered_funs_per);
    let lines_sev = get_severity(&conf, covered_lines_per);
    let out = fomat!(
            r#"<!DOCTYPE html>"# "\n"
            r#"<html lang="en-us">"# "\n"
            r#"<head>"# "\n"
            r#"<title>Grcov report</title>"# "\n"
            r#"<link rel="stylesheet" href="grcov.css">"# "\n"
            r#"<meta http-equiv="Content-Type" content="text/html; charset=UTF-8">"# "\n"
            r#"</head>"# "\n"
            r#"<body>"# "\n"
            r#"<div class="header">"# "\n"
            r#"<div class="view">"# "\n"
            r#"<div class="viewRow"><span class="viewItem">Current view:</span>"#
            r#"<span class="viewValue">top level</span></div>"# "\n"
            r#"<div class="viewRow"><span class="viewItem">Date:</span><span class="viewValue">"# (conf.date.format("%F %T")) r#"</span></div>"# "\n"
            r#"</div>"# "\n"
            r#"<div class="stats">"# "\n"
            r#"<div class="statsRow">"#
            r#"<span></span><span class="statsHit">Hit</span><span class="statsTotal">Total</span><span class="statsCoverage">Coverage</span></div>"# "\n"
            r#"<div class="statsRow">"#
            r#"<span class="statsLine">Lines</span><span class="linesHit">"#
            (global.stats.covered_lines)
            r#"</span>"#
            r#"<span class="linesTotal">"#
            (global.stats.total_lines)
            r#"</span>"#
            r#"<span class="linesPercentage "# (lines_sev) r#"">"#
            {(covered_lines_per):.1}
            r#" %</span></div>"# "\n"
            r#"<div class="statsRow">"#
            r#"<span class="statsFun">Functions</span><span class="funsHit">"#
            (global.stats.covered_funs)
            r#"</span>"#
            r#"<span class="funsTotal">"#
            (global.stats.total_funs)
            r#"</span>"#
            r#"<span class="funsPercentage "# (funs_sev) r#"">"#
            {(covered_funs_per):.1}
            r#" %</span></div>"# "\n"
            r#"</div>"# "\n"
            r#"</div>"# "\n"
            r#"<div class="dirStatsHeader"><div class="dirStatsLineHeader">"#
            r#"<span class="dirNameHeader">Directory</span>"#
            r#"<span class="dirLineCovHeader">Line Coverage</span>"#
            r#"<span class="dirFunsHeader">Functions</span></div></div>"# "\n"
            r#"<div class="dirStats">"# "\n"
            for (dir, stats, lines_percent, lines_sev, funs_percent, funs_sev) in global.dirs.iter().map(|(d, s)| {
                let lp = get_percentage(s.stats.covered_lines, s.stats.total_lines);
                let fp = get_percentage(s.stats.covered_funs, s.stats.total_funs);
                (d, s, lp, get_severity(&conf, lp), fp, get_fn_severity(&conf, fp))
            }) {
                r#"<div class="lineDir">"#
                    r#"<span class="dirName"><a href=""# (dir) r#"/index.html">"# (dir) r#"</a></span>"#
                    r#"<span class="dirBarPer"><span class="dirBar"><span class="percentBar "# (lines_sev) r#"" style="width:"# (lines_percent) r#"%;"></span></span></span>"#
                    r#"<span class="dirLinesPer "# (lines_sev) r#"">"# {(lines_percent):.1} r#"%</span>"#
                    r#"<span class="dirLinesRatio "# (lines_sev) r#"">"# (stats.stats.covered_lines) " / " (stats.stats.total_lines) r#"</span>"#
                    r#"<span class="dirFunsPer "# (funs_sev) r#"">"# {(funs_percent):.1} r#"%</span>"#
                    r#"<span class="dirFunsRatio "# (funs_sev) r#"">"# (stats.stats.covered_funs) " / " (stats.stats.total_funs) r#"</span>"#
                r#"</div>"# "\n"
            }
            r#"</div>"# "\n"
            r#"</body>"# "\n"
            r#"</html>"# "\n"
    );

    if output_stream.write_all(out.as_bytes()).is_err() {
        eprintln!("Cannot write the file {:?}", output_file);
        return;
    }

    for (dir_name, dir_stats) in global.dirs.iter() {
        gen_dir_index(dir_name, dir_stats, &conf, output);
    }
}

pub fn gen_dir_index(dir_name: &str, dir_stats: &HtmlDirStats, conf: &Config, output: &PathBuf) {
    let index = PathBuf::from(dir_name).join("index.html");
    let output_file = output.join(&index);
    create_parent(&output_file);
    let mut output = match File::create(&output_file) {
        Err(_) => {
            eprintln!("Cannot create file {:?}", output_file);
            return;
        }
        Ok(f) => f,
    };

    let base_url = get_base(&index);
    let mut css_url = base_url.clone();
    css_url.push_str("grcov.css");
    let mut index_url = base_url.clone();
    index_url.push_str("index.html");
    let covered_lines_per =
        get_percentage(dir_stats.stats.covered_lines, dir_stats.stats.total_lines);
    let covered_funs_per = get_percentage(dir_stats.stats.covered_funs, dir_stats.stats.total_funs);
    let funs_sev = get_fn_severity(&conf, covered_funs_per);
    let lines_sev = get_severity(&conf, covered_lines_per);
    let out = fomat!(
            r#"<!DOCTYPE html>"# "\n"
            r#"<html lang="en-us">"# "\n"
            r#"<head>"# "\n"
            r#"<title>Grcov report &mdash;"# (dir_name) r#"</title>"# "\n"
            r#"<link rel="stylesheet" href=""# (css_url) r#"">"# "\n"
            r#"<meta http-equiv="Content-Type" content="text/html; charset=UTF-8">"# "\n"
            r#"</head>"# "\n"
            r#"<body>"# "\n"
            r#"<div class="header">"# "\n"
            r#"<div class="view">"# "\n"
            r#"<div class="viewRow"><span class="viewItem">Current view:</span>"#
            r#"<span class="viewValue"><a href=""# (index_url) r#"">top level</a> - "# (dir_name) r#"</span></div>"# "\n"
            r#"<div class="viewRow"><span class="viewItem">Date:</span><span class="viewValue">"# (conf.date.format("%F %T")) r#"</span></div>"# "\n"
            r#"</div>"# "\n"
            r#"<div class="stats">"# "\n"
            r#"<div class="statsRow">"#
            r#"<span></span><span class="statsHit">Hit</span><span class="statsTotal">Total</span><span class="statsCoverage">Coverage</span></div>"# "\n"
            r#"<div class="statsRow">"#
            r#"<span class="statsLine">Lines</span><span class="linesHit">"#
            (dir_stats.stats.covered_lines)
            r#"</span>"#
            r#"<span class="linesTotal">"#
            (dir_stats.stats.total_lines)
            r#"</span>"#
            r#"<span class="linesPercentage "# (lines_sev) r#"">"#
            {(covered_lines_per):.1}
            r#" %</span></div>"# "\n"
            r#"<div class="statsRow">"#
            r#"<span class="statsFun">Functions</span><span class="funsHit">"#
            (dir_stats.stats.covered_funs)
            r#"</span>"#
            r#"<span class="funsTotal">"#
            (dir_stats.stats.total_funs)
            r#"</span>"#
            r#"<span class="funsPercentage "# (funs_sev) r#"">"#
            {(covered_funs_per):.1}
            r#" %</span></div>"# "\n"
            r#"</div>"# "\n"
            r#"</div>"# "\n"
            r#"<div class="dirStatsHeader"><div class="dirStatsLineHeader">"#
            r#"<span class="dirNameHeader">Filename</span>"#
            r#"<span class="dirLineCovHeader">Line Coverage</span>"#
            r#"<span class="dirFunsHeader">Functions</span></div></div>"# "\n"
            r#"<div class="dirStats">"# "\n"
            for (file_name, stats, lines_percent, lines_sev, funs_percent, funs_sev) in dir_stats.files.iter().map(|fs| {
                let lp = get_percentage(fs.stats.covered_lines, fs.stats.total_lines);
                let fp = get_percentage(fs.stats.covered_funs, fs.stats.total_funs);
                (&fs.file_name, &fs.stats, lp, get_severity(conf, lp), fp, get_fn_severity(conf, fp))
            }) {
                r#"<div class="lineDir">"#
                    r#"<span class="dirName"><a href=""# (file_name) r#".html">"# (file_name) r#"</a></span>"#
                    r#"<span class="dirBarPer"><span class="dirBar"><span class="percentBar "# (lines_sev) r#"" style="width:"# (lines_percent) r#"%;"></span></span></span>"#
                    r#"<span class="dirLinesPer "# (lines_sev) r#"">"# {(lines_percent):.1} r#"%</span>"#
                    r#"<span class="dirLinesRatio "# (lines_sev) r#"">"# (stats.covered_lines) " / " (stats.total_lines) r#"</span>"#
                    r#"<span class="dirFunsPer "# (funs_sev) r#"">"# {(funs_percent):.1} r#"%</span>"#
                    r#"<span class="dirFunsRatio "# (funs_sev) r#"">"# (stats.covered_funs) " / " (stats.total_funs) r#"</span>"#
                r#"</div>"# "\n"
            }
            r#"</div>"# "\n"
            r#"</body>"# "\n"
            r#"</html>"# "\n"
    );

    if output.write_all(out.as_bytes()).is_err() {
        eprintln!("Cannot write the file {:?}", output_file);
    }
}

fn gen_html(
    path: PathBuf,
    result: &CovResult,
    conf: &Config,
    output: &PathBuf,
    rel_path: &PathBuf,
    global: Arc<Mutex<HtmlGlobalStats>>,
) {
    if !rel_path.is_relative() {
        return;
    }

    let mut f = match File::open(&path) {
        Err(_) => {
            //eprintln!("Warning: cannot open file {:?}", path);
            return;
        }
        Ok(f) => f,
    };

    let stats = get_stats(&result);
    get_dirs_result(global, &rel_path, &stats);

    let output_file = output.join(add_html_ext(&rel_path));
    create_parent(&output_file);
    let mut output = match File::create(&output_file) {
        Err(_) => {
            eprintln!("Cannot create file {:?}", output_file);
            return;
        }
        Ok(f) => f,
    };
    let base_url = get_base(&rel_path);
    let filename = rel_path.file_name().unwrap().to_str().unwrap();
    let rel_path_str = rel_path.parent().unwrap().to_str().unwrap().to_string();

    // Read the source file
    let mut input = String::new();
    f.read_to_string(&mut input).unwrap();

    let mut index_url = base_url.clone();
    index_url.push_str("index.html");
    let mut css_url = base_url.clone();
    css_url.push_str("grcov.css");

    let covered_lines_per = get_percentage(stats.covered_lines, stats.total_lines);
    let covered_funs_per = get_percentage(stats.covered_funs, stats.total_funs);

    let funs_sev = get_fn_severity(&conf, covered_funs_per);
    let lines_sev = get_severity(&conf, covered_lines_per);

    let out = fomat!(
            r#"<!DOCTYPE html>"# "\n"
            r#"<html lang="en-us">"# "\n"
            r#"<head>"# "\n"
            r#"<title>Grcov report &mdash;"# (filename) r#"</title>"# "\n"
            r#"<link rel="stylesheet" href=""# (css_url) r#"">"# "\n"
            r#"<meta http-equiv="Content-Type" content="text/html; charset=UTF-8">"# "\n"
            r#"</head>"# "\n"
            r#"<body>"# "\n"
            r#"<div class="header">"# "\n"
            r#"<div class="view">"# "\n"
            r#"<div class="viewRow"><span class="viewItem">Current view:</span>"#
            r#"<span class="viewValue"><a href=""# (index_url) r#"">top level</a> - <a href="index.html">"# (rel_path_str) r#"</a> - "# (filename) r#"</span></div>"# "\n"
            r#"<div class="viewRow"><span class="viewItem">Date:</span><span class="viewValue">"# (conf.date.format("%F %T")) r#"</span></div>"# "\n"
            r#"</div>"# "\n"
            r#"<div class="stats">"# "\n"
            r#"<div class="statsRow">"#
            r#"<span></span><span class="statsHit">Hit</span><span class="statsTotal">Total</span><span class="statsCoverage">Coverage</span></div>"# "\n"
            r#"<div class="statsRow">"#
            r#"<span class="statsLine">Lines</span><span class="linesHit">"#
            (stats.covered_lines)
            r#"</span>"#
            r#"<span class="linesTotal">"#
            (stats.total_lines)
            r#"</span>"#
            r#"<span class="linesPercentage "# (lines_sev) r#"">"#
            {(covered_lines_per):.1}
            r#" %</span></div>"# "\n"
            r#"<div class="statsRow">"#
            r#"<span class="statsFun">Functions</span><span class="funsHit">"#
            (stats.covered_funs)
            r#"</span>"#
            r#"<span class="funsTotal">"#
            (stats.total_funs)
            r#"</span>"#
            r#"<span class="funsPercentage "# (funs_sev) r#"">"#
            {(covered_funs_per):.1}
            r#" %</span></div>"# "\n"
            r#"</div>"# "\n"
            r#"</div>"# "\n"
            r#"<div class="sourceCode">"# "\n"
            for (index, line) in lines_iter!(&input).enumerate().map(move |(i, l)| (i + 1, l)) {
                r#"<div class="line">"#
                r#"<span id=""# (index) r##"" class="lineNum"><a href="#"## (index) r#"">"# (index) r#"</a></span>"#
                if let Some(counter) = result.lines.get(&(index as u32)) {
                    if *counter > 0 {
                        r#"<span class="counterCov">"# (*counter) r#"</span><span class="lineCov">"#
                    } else {
                        r#"<span class="counterNoCov">0</span><span class="lineNoCov">"#
                    }
                } else {
                    r#"<span class="counterUnCov"></span><span class="lineUnCov">"#
                }
                match line {
                    Line::Str(s) => { (s) }
                    Line::Slice(s) => { (s) }
                }
                r#"</span></div>"# "\n"
            }
            r#"</div>"# "\n"
            r#"</body>"# "\n"
            r#"</html>"# "\n"
    );

    if output.write_all(out.as_bytes()).is_err() {
        eprintln!("Cannot write the file {:?}", output_file);
    }
}

pub fn consumer_html(
    receiver: HtmlJobReceiver,
    global: Arc<Mutex<HtmlGlobalStats>>,
    output: PathBuf,
    conf: Config,
) {
    while let Ok(job) = receiver.recv() {
        if job.is_none() {
            break;
        }
        let job = job.unwrap();
        gen_html(
            job.abs_path,
            &job.result,
            &conf,
            &output,
            &job.rel_path,
            global.clone(),
        );
    }
}

macro_rules! write_resource {
    ($name: expr, $output: expr) => {{
        let data = include_bytes!(concat!("../resources/", $name));
        let output = $output.join($name);
        let mut output = match File::create(&output) {
            Err(_) => {
                eprintln!("Cannot create file {:?}", output);
                return;
            }
            Ok(f) => f,
        };
        if output.write_all(data).is_err() {
            eprintln!("Cannot write the file {:?}", output);
            return;
        }
    }};
}

pub fn write_static_files(output: PathBuf) {
    write_resource!("grcov.css", output);
}
