use chrono::{DateTime, Utc};
use serde_json::value::{from_value, to_value, Value};
use std::collections::HashMap;
use std::collections::{btree_map, BTreeMap};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tera::try_get_value;

use crate::defs::*;

impl HtmlStats {
    #[inline(always)]
    pub fn add(&mut self, stats: &Self) {
        self.total_lines += stats.total_lines;
        self.covered_lines += stats.covered_lines;
        self.total_funs += stats.total_funs;
        self.covered_funs += stats.covered_funs;
        self.total_branches += stats.total_branches;
        self.covered_branches += stats.covered_branches;
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    hi_limit: f64,
    med_limit: f64,
    fn_hi_limit: f64,
    fn_med_limit: f64,
    branch_hi_limit: f64,
    branch_med_limit: f64,
    date: DateTime<Utc>,
}

pub fn get_config() -> (Tera, Config) {
    let conf = Config {
        hi_limit: 90.,
        med_limit: 75.,
        fn_hi_limit: 90.,
        fn_med_limit: 75.,
        branch_hi_limit: 90.,
        branch_med_limit: 75.,
        date: Utc::now(),
    };

    let mut tera = Tera::default();

    tera.register_filter("severity", conf.clone());
    tera.register_function("percent", &percent);

    tera.add_raw_templates(vec![
        ("macros.html", include_str!("templates/macros.html")),
        ("base.html", include_str!("templates/base.html")),
        ("index.html", include_str!("templates/index.html")),
        ("file.html", include_str!("templates/file.html")),
    ])
    .unwrap();

    (tera, conf)
}

impl tera::Filter for Config {
    fn filter(&self, value: &Value, args: &HashMap<String, Value>) -> tera::Result<Value> {
        let rate = try_get_value!("severity", "value", f64, value);

        let kind = match args.get("kind") {
            Some(val) => try_get_value!("severity", "kind", String, val),
            None => "lines".to_string(),
        };

        fn severity(hi: f64, medium: f64, rate: f64) -> Value {
            to_value(if hi <= rate && rate <= 100. {
                "success"
            } else if medium <= rate && rate < hi {
                "warning"
            } else {
                "danger"
            })
            .unwrap()
        }

        match kind.as_ref() {
            "lines" => Ok(severity(self.hi_limit, self.med_limit, rate)),
            "branches" => Ok(severity(self.branch_hi_limit, self.branch_med_limit, rate)),
            "functions" => Ok(severity(self.fn_hi_limit, self.fn_med_limit, rate)),
            _ => Err(tera::Error::msg("Unsupported kind")),
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
    let total_branches = result.branches.values().map(|v| v.len()).sum();
    let covered_branches = result
        .branches
        .values()
        .map(|v| v.iter().filter(|x| **x).count())
        .sum();

    HtmlStats {
        total_lines,
        covered_lines,
        total_funs,
        covered_funs,
        total_branches,
        covered_branches,
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

fn percent(args: &HashMap<String, Value>) -> tera::Result<Value> {
    if let (Some(n), Some(d)) = (args.get("num"), args.get("den")) {
        if let (Ok(num), Ok(den)) = (
            from_value::<usize>(n.clone()),
            from_value::<usize>(d.clone()),
        ) {
            Ok(to_value(get_percentage(num, den)).unwrap())
        } else {
            Err(tera::Error::msg("Invalid arguments"))
        }
    } else {
        Err(tera::Error::msg("Not enough arguments"))
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
        stats: stats.clone(),
    };
    let mut global = global.lock().unwrap();
    global.stats.add(stats);
    match global.dirs.entry(parent) {
        btree_map::Entry::Occupied(ds) => {
            let ds = ds.into_mut();
            ds.stats.add(stats);
            ds.files.insert(file_name, fs);
        }
        btree_map::Entry::Vacant(v) => {
            let mut files = BTreeMap::new();
            files.insert(file_name, fs);
            v.insert(HtmlDirStats {
                files,
                stats: stats.clone(),
            });
        }
    };
}

use tera::{Context, Tera};

pub fn gen_index(
    tera: &Tera,
    global: HtmlGlobalStats,
    conf: Config,
    output: &PathBuf,
    branch_enabled: bool,
) {
    let output_file = output.join("index.html");
    create_parent(&output_file);
    let mut output_stream = match File::create(&output_file) {
        Err(_) => {
            eprintln!("Cannot create file {:?}", output_file);
            return;
        }
        Ok(f) => f,
    };

    let mut ctx = Context::new();
    let empty: &[&str] = &[];
    ctx.insert("date", &conf.date);
    ctx.insert("current", "top_level");
    ctx.insert("parents", empty);
    ctx.insert("stats", &global.stats);
    ctx.insert("items", &global.dirs);
    ctx.insert("kind", "Directory");
    ctx.insert("branch_enabled", &branch_enabled);

    let out = tera.render("index.html", &ctx).unwrap();

    if output_stream.write_all(out.as_bytes()).is_err() {
        eprintln!("Cannot write the file {:?}", output_file);
        return;
    }

    for (dir_name, dir_stats) in global.dirs.iter() {
        gen_dir_index(&tera, dir_name, dir_stats, &conf, output, branch_enabled);
    }
}

pub fn gen_dir_index(
    tera: &Tera,
    dir_name: &str,
    dir_stats: &HtmlDirStats,
    conf: &Config,
    output: &PathBuf,
    branch_enabled: bool,
) {
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

    let mut ctx = Context::new();
    ctx.insert("date", &conf.date);
    ctx.insert("current", dir_name);
    ctx.insert("parents", &[("../index.html", "top_level")]);
    ctx.insert("stats", &dir_stats.stats);
    ctx.insert("items", &dir_stats.files);
    ctx.insert("kind", "File");
    ctx.insert("branch_enabled", &branch_enabled);

    let out = tera.render("index.html", &ctx).unwrap();

    if output.write_all(out.as_bytes()).is_err() {
        eprintln!("Cannot write the file {:?}", output_file);
    }
}

fn gen_html(
    tera: &Tera,
    path: PathBuf,
    result: &CovResult,
    conf: &Config,
    output: &PathBuf,
    rel_path: &PathBuf,
    global: Arc<Mutex<HtmlGlobalStats>>,
    branch_enabled: bool,
) {
    if !rel_path.is_relative() {
        return;
    }

    let f = match File::open(&path) {
        Err(_) => {
            //eprintln!("Warning: cannot open file {:?}", path);
            return;
        }
        Ok(f) => f,
    };
    let f = BufReader::new(f);

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
    let parent = rel_path.parent().unwrap().to_str().unwrap().to_string();
    let mut index_url = base_url.clone();
    index_url.push_str("index.html");

    let mut ctx = Context::new();
    ctx.insert("date", &conf.date);
    ctx.insert("current", filename);
    ctx.insert(
        "parents",
        &[
            (index_url.as_str(), "top_level"),
            ("./index.html", parent.as_str()),
        ],
    );
    ctx.insert("stats", &stats);
    ctx.insert("branch_enabled", &branch_enabled);

    let items = f
        .lines()
        .enumerate()
        .map(move |(i, l)| {
            let index = i + 1;
            let count = result
                .lines
                .get(&(index as u32))
                .map(|&v| v as i64)
                .unwrap_or(-1);

            (index, count, l.unwrap())
        })
        .collect::<Vec<_>>();

    ctx.insert("items", &items);

    let out = tera.render("file.html", &ctx).unwrap();

    if output.write_all(out.as_bytes()).is_err() {
        eprintln!("Cannot write the file {:?}", output_file);
    }
}

pub fn consumer_html(
    tera: &Tera,
    receiver: HtmlJobReceiver,
    global: Arc<Mutex<HtmlGlobalStats>>,
    output: PathBuf,
    conf: Config,
    branch_enabled: bool,
) {
    while let Ok(job) = receiver.recv() {
        if job.is_none() {
            break;
        }
        let job = job.unwrap();
        gen_html(
            tera,
            job.abs_path,
            &job.result,
            &conf,
            &output,
            &job.rel_path,
            global.clone(),
            branch_enabled,
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
