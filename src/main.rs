#[cfg(all(unix, feature = "tc"))]
#[global_allocator]
static GLOBAL: tcmalloc::TCMalloc = tcmalloc::TCMalloc;

use clap::{builder::PossibleValue, ArgGroup, Parser, ValueEnum};
use crossbeam_channel::bounded;
use log::error;
use regex::Regex;
use rustc_hash::FxHashMap;
use serde_json::Value;
use simplelog::{ColorChoice, Config, LevelFilter, TermLogger, TerminalMode, WriteLogger};
use std::fs::{self, File};
use std::ops::Deref;
use std::panic;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::{process, thread};

use grcov::*;

#[derive(Clone, PartialEq)]
enum OutputType {
    Ade,
    Lcov,
    Coveralls,
    CoverallsPlus,
    Files,
    Covdir,
    Html,
    Cobertura,
    CoberturaPretty,
    Markdown,
}

impl FromStr for OutputType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "ade" => Self::Ade,
            "lcov" => Self::Lcov,
            "coveralls" => Self::Coveralls,
            "coveralls+" => Self::CoverallsPlus,
            "files" => Self::Files,
            "covdir" => Self::Covdir,
            "html" => Self::Html,
            "cobertura" => Self::Cobertura,
            "cobertura-pretty" => Self::CoberturaPretty,
            "markdown" => Self::Markdown,
            _ => return Err(format!("{} is not a supported output type", s)),
        })
    }
}

impl OutputType {
    fn to_file_name(&self, output_path: Option<&Path>) -> Option<PathBuf> {
        output_path.map(|path| {
            if path.is_dir() {
                match self {
                    OutputType::Ade => path.join("activedata"),
                    OutputType::Lcov => path.join("lcov"),
                    OutputType::Coveralls => path.join("coveralls"),
                    OutputType::CoverallsPlus => path.join("coveralls+"),
                    OutputType::Files => path.join("files"),
                    OutputType::Covdir => path.join("covdir"),
                    OutputType::Html => path.join("html"),
                    OutputType::Cobertura | OutputType::CoberturaPretty => {
                        path.join("cobertura.xml")
                    }
                    OutputType::Markdown => path.join("markdown.md"),
                }
            } else {
                path.to_path_buf()
            }
        })
    }
}

#[derive(clap::ValueEnum, Clone)]
enum Filter {
    Covered,
    Uncovered,
}

impl FromStr for Filter {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "covered" => Self::Covered,
            "uncovered" => Self::Uncovered,
            _ => return Err(format!("{} is not a supported filter", s)),
        })
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct LevelFilterArg(LevelFilter);

impl ValueEnum for LevelFilterArg {
    fn value_variants<'a>() -> &'a [Self] {
        &[
            Self(LevelFilter::Off),
            Self(LevelFilter::Error),
            Self(LevelFilter::Warn),
            Self(LevelFilter::Info),
            Self(LevelFilter::Debug),
            Self(LevelFilter::Trace),
        ]
    }

    fn to_possible_value(&self) -> Option<PossibleValue> {
        match self.0 {
            LevelFilter::Off => Some(PossibleValue::new("OFF")),
            LevelFilter::Error => Some(PossibleValue::new("ERROR")),
            LevelFilter::Warn => Some(PossibleValue::new("WARN")),
            LevelFilter::Info => Some(PossibleValue::new("INFO")),
            LevelFilter::Debug => Some(PossibleValue::new("DEBUG")),
            LevelFilter::Trace => Some(PossibleValue::new("TRACE")),
        }
    }
}

#[derive(Parser)]
#[command(
    author,
    version,
    max_term_width = 100,
    about = "Parse, collect and aggregate code coverage data for multiple source files",
    help_template = "\
{before-help}{name}
{author-with-newline}{about-with-newline}
{usage-heading} {usage}

{all-args}{after-help}
",
    // This group requires that at least one of --token and --service-job-id
    // be present. --service-job-id requires --service-name, so this
    // effectively means we accept the following combinations:
    // - --token
    // - --token --service-job-id --service-name
    // - --service-job-id --service-name
    group = ArgGroup::new("coveralls-auth")
        .args(&["token", "service_job_id"])
        .multiple(true),
)]
struct Opt {
    /// Sets the input paths to use.
    #[arg(required = true)]
    paths: Vec<String>,
    /// Sets the path to the compiled binary to be used.
    #[arg(short, long, value_name = "PATH")]
    binary_path: Option<PathBuf>,
    /// Sets the path to the LLVM bin directory.
    #[arg(long, value_name = "PATH")]
    llvm_path: Option<PathBuf>,
    /// Sets a custom output type.
    #[arg(
        short = 't',
        long,
        long_help = "\
            Comma separated list of custom output types:\n\
            - *html* for a HTML coverage report;\n\
            - *coveralls* for the Coveralls specific format;\n\
            - *lcov* for the lcov INFO format;\n\
            - *covdir* for the covdir recursive JSON format;\n\
            - *coveralls+* for the Coveralls specific format with function information;\n\
            - *ade* for the ActiveData-ETL specific format;\n\
            - *files* to only return a list of files.\n\
            - *markdown* for human easy read.\n\
            - *cobertura* for output in cobertura format.\n\
            - *cobertura-pretty* to pretty-print in cobertura format.\n\
        ",
        value_name = "OUTPUT TYPE",
        requires_ifs = [
            ("coveralls", "coveralls-auth"),
            ("coveralls+", "coveralls-auth"),
        ],

        value_delimiter = ',',
        alias = "output-type",
        default_value = "lcov",
    )]
    output_types: Vec<OutputType>,
    /// Specifies the output path. This is a file for a single output type and must be a folder
    /// for multiple output types.
    #[arg(short, long, value_name = "PATH", alias = "output-file")]
    output_path: Option<PathBuf>,
    /// Specifies the output config file.
    #[arg(long, value_name = "PATH", alias = "output-config-file")]
    output_config_file: Option<PathBuf>,
    /// Specifies the root directory of the source files.
    #[arg(short, long, value_name = "DIRECTORY")]
    source_dir: Option<PathBuf>,
    /// Specifies a prefix to remove from the paths (e.g. if grcov is run on a different machine
    /// than the one that generated the code coverage information).
    #[arg(short, long, value_name = "PATH")]
    prefix_dir: Option<PathBuf>,
    /// Ignore source files that can't be found on the disk.
    #[arg(long)]
    ignore_not_existing: bool,
    /// Ignore files/directories specified as globs.
    #[arg(long = "ignore", value_name = "PATH", num_args = 1)]
    ignore_dir: Vec<String>,
    /// Keep only files/directories specified as globs.
    #[arg(long = "keep-only", value_name = "PATH", num_args = 1)]
    keep_dir: Vec<String>,
    #[arg(long, value_name = "PATH")]
    path_mapping: Option<PathBuf>,
    /// Enables parsing branch coverage information.
    #[arg(long)]
    branch: bool,
    /// Filters out covered/uncovered files. Use 'covered' to only return covered files, 'uncovered'
    /// to only return uncovered files.
    #[arg(long, value_enum)]
    filter: Option<Filter>,
    /// Comma separated list of output types to sort files lexicographically for.
    #[arg(
        long,
        value_name = "OUTPUT TYPES",
        value_delimiter = ',',
        alias = "sort-output-types",
        default_value = "markdown"
    )]
    sort_output_types: Vec<OutputType>,
    /// Speeds-up parsing, when the code coverage information is exclusively coming from a llvm
    /// build.
    #[arg(long)]
    llvm: bool,
    /// Sets the repository token from Coveralls, required for the 'coveralls' and 'coveralls+'
    /// formats.
    #[arg(long, value_name = "TOKEN")]
    token: Option<String>,
    /// Sets the hash of the commit used to generate the code coverage data.
    #[arg(long, value_name = "COMMIT HASH")]
    commit_sha: Option<String>,
    /// Sets the service name.
    #[arg(long, value_name = "SERVICE NAME")]
    service_name: Option<String>,
    /// Sets the service number.
    #[arg(long, value_name = "SERVICE NUMBER")]
    service_number: Option<String>,
    /// Sets the service job id.
    #[arg(
        long,
        value_name = "SERVICE JOB ID",
        visible_alias = "service-job-number",
        requires = "service_name"
    )]
    service_job_id: Option<String>,
    /// Sets the service pull request number.
    #[arg(long, value_name = "SERVICE PULL REQUEST")]
    service_pull_request: Option<String>,
    /// Sets the service flag name for coveralls parallel/carryover mode
    #[arg(long, value_name = "SERVICE FLAG NAME")]
    service_flag_name: Option<String>,
    /// Sets the build type to be parallel for 'coveralls' and 'coveralls+' formats.
    #[arg(long)]
    parallel: bool,
    #[arg(long, value_name = "NUMBER")]
    threads: Option<usize>,
    /// Sets coverage decimal point precision on output reports.
    #[arg(long, value_name = "NUMBER", default_value = "2")]
    precision: usize,
    #[arg(long = "guess-directory-when-missing")]
    guess_directory: bool,
    /// Set the branch for coveralls report. Defaults to 'master'.
    #[arg(long, value_name = "VCS BRANCH", default_value = "master")]
    vcs_branch: String,
    /// Set the file where to log (or stderr or stdout). Defaults to 'stderr'.
    #[arg(long, value_name = "LOG", default_value = "stderr")]
    log: PathBuf,
    /// Set the log level.
    #[arg(long, value_name = "LEVEL", default_value = "ERROR", value_enum)]
    log_level: LevelFilterArg,
    /// Lines in covered files containing this marker will be excluded.
    #[arg(long, value_name = "regex")]
    excl_line: Option<Regex>,
    /// Marks the beginning of an excluded section. The current line is part of this section.
    #[arg(long, value_name = "regex")]
    excl_start: Option<Regex>,
    /// Marks the end of an excluded section. The current line is part of this section.
    #[arg(long, value_name = "regex")]
    excl_stop: Option<Regex>,
    /// Lines in covered files containing this marker will be excluded from branch coverage.
    #[arg(long, value_name = "regex")]
    excl_br_line: Option<Regex>,
    /// Marks the beginning of a section excluded from branch coverage. The current line is part of
    /// this section.
    #[arg(long, value_name = "regex")]
    excl_br_start: Option<Regex>,
    /// Marks the end of a section excluded from branch coverage. The current line is part of this
    /// section.
    #[arg(long, value_name = "regex")]
    excl_br_stop: Option<Regex>,
    /// No symbol demangling.
    #[arg(long)]
    no_demangle: bool,
}

fn main() {
    let opt = Opt::parse();

    if let Some(path) = opt.llvm_path {
        LLVM_PATH.set(path).unwrap();
    }

    let filter_option = opt.filter.map(|filter| match filter {
        Filter::Covered => true,
        Filter::Uncovered => false,
    });
    let stdout = Path::new("stdout");
    let stderr = Path::new("stderr");

    if opt.log == stdout {
        let _ = TermLogger::init(
            opt.log_level.0,
            Config::default(),
            TerminalMode::Stdout,
            ColorChoice::Auto,
        );
    } else if opt.log == stderr {
        let _ = TermLogger::init(
            opt.log_level.0,
            Config::default(),
            TerminalMode::Stderr,
            ColorChoice::Auto,
        );
    } else if let Ok(file) = File::create(&opt.log) {
        let _ = WriteLogger::init(opt.log_level.0, Config::default(), file);
    } else {
        let _ = TermLogger::init(
            opt.log_level.0,
            Config::default(),
            TerminalMode::Stderr,
            ColorChoice::Auto,
        );
        error!(
            "Unable to create log file: {}. Switch to stderr",
            opt.log.display()
        );
    }

    let file_filter = FileFilter::new(
        opt.excl_line,
        opt.excl_start,
        opt.excl_stop,
        opt.excl_br_line,
        opt.excl_br_start,
        opt.excl_br_stop,
    );
    let demangle = !opt.no_demangle;

    panic::set_hook(Box::new(|panic_info| {
        let (filename, line) = panic_info
            .location()
            .map(|loc| (loc.file(), loc.line()))
            .unwrap_or(("<unknown>", 0));
        let cause = panic_info
            .payload()
            .downcast_ref::<String>()
            .map(String::deref);
        let cause = cause.unwrap_or_else(|| {
            panic_info
                .payload()
                .downcast_ref::<&str>()
                .copied()
                .unwrap_or("<cause unknown>")
        });
        error!("A panic occurred at {}:{}: {}", filename, line, cause);
    }));

    let num_threads: usize = opt.threads.unwrap_or_else(|| 1.max(num_cpus::get() - 1));
    let source_root = opt
        .source_dir
        .filter(|source_dir| source_dir != Path::new(""))
        .map(|source_dir| canonicalize_path(source_dir).expect("Source directory does not exist."));

    let prefix_dir = opt.prefix_dir.or_else(|| source_root.clone());

    let tmp_dir = tempfile::tempdir().expect("Failed to create temporary directory");
    let tmp_path = tmp_dir.path().to_owned();
    assert!(tmp_path.exists());

    let result_map: Arc<SyncCovResultMap> = Arc::new(Mutex::new(
        FxHashMap::with_capacity_and_hasher(20_000, Default::default()),
    ));
    let (sender, receiver) = bounded(2 * num_threads);
    let path_mapping: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));

    let producer = {
        let sender: JobSender = sender.clone();
        let tmp_path = tmp_path.clone();
        let path_mapping_file = opt.path_mapping;
        let path_mapping = Arc::clone(&path_mapping);
        let paths = opt.paths;
        let is_llvm = opt.llvm;

        thread::Builder::new()
            .name(String::from("Producer"))
            .spawn(move || {
                let producer_path_mapping_buf = producer(
                    &tmp_path,
                    &paths,
                    &sender,
                    filter_option.is_some() && filter_option.unwrap(),
                    is_llvm,
                );

                let mut path_mapping = path_mapping.lock().unwrap();
                *path_mapping = if let Some(path) = path_mapping_file {
                    let file = File::open(path).unwrap();
                    Some(serde_json::from_reader(file).unwrap())
                } else {
                    producer_path_mapping_buf.map(|producer_path_mapping_buf| {
                        serde_json::from_slice(&producer_path_mapping_buf).unwrap()
                    })
                };
            })
            .unwrap()
    };

    let mut parsers = Vec::new();

    for i in 0..num_threads {
        let receiver = receiver.clone();
        let result_map = Arc::clone(&result_map);
        let working_dir = tmp_path.join(format!("{}", i));
        let source_root = source_root.clone();
        let binary_path = opt.binary_path.clone();
        let branch_enabled = opt.branch;
        let guess_directory = opt.guess_directory;

        let t = thread::Builder::new()
            .name(format!("Consumer {}", i))
            .spawn(move || {
                fs::create_dir(&working_dir).expect("Failed to create working directory");
                consumer(
                    &working_dir,
                    source_root.as_deref(),
                    &result_map,
                    receiver,
                    branch_enabled,
                    guess_directory,
                    binary_path.as_deref(),
                );
            })
            .unwrap();

        parsers.push(t);
    }

    if producer.join().is_err() {
        process::exit(1);
    }

    // Poison the receiver, now that the producer is finished.
    for _ in 0..num_threads {
        sender.send(None).unwrap();
    }

    for parser in parsers {
        if parser.join().is_err() {
            process::exit(1);
        }
    }

    let result_map_mutex = Arc::try_unwrap(result_map).unwrap();
    let result_map = result_map_mutex.into_inner().unwrap();

    let path_mapping_mutex = Arc::try_unwrap(path_mapping).unwrap();
    let path_mapping = path_mapping_mutex.into_inner().unwrap();

    let iterator = rewrite_paths(
        result_map,
        path_mapping,
        source_root.as_deref(),
        prefix_dir.as_deref(),
        opt.ignore_not_existing,
        &opt.ignore_dir,
        &opt.keep_dir,
        filter_option,
        file_filter,
    );
    let mut sorted_iterator: Option<Vec<ResultTuple>> = None;

    let service_number = opt.service_number.unwrap_or_default();
    let service_pull_request = opt.service_pull_request.unwrap_or_default();
    let commit_sha = opt.commit_sha.unwrap_or_default();

    let output_types = opt.output_types;

    let output_path = match output_types.len() {
        0 => unreachable!("Output types has a default value"),
        1 => opt.output_path.as_deref(),
        _ => match opt.output_path.as_deref() {
            Some(output_path) => {
                if output_path.is_dir() {
                    Some(output_path)
                } else {
                    panic!("output_path must be a directory when using multiple outputs");
                }
            }
            _ => None,
        },
    };

    for output_type in &output_types {
        let output_path = output_type.to_file_name(output_path);
        let results = if opt.sort_output_types.contains(output_type) {
            // compute and cache the sorted results if not already used
            sorted_iterator = sorted_iterator.or_else(|| {
                let mut results = iterator.clone();
                results.sort_by_key(|result| result.0.display().to_string());
                Some(results)
            });
            sorted_iterator.as_ref().unwrap()
        } else {
            &iterator
        };

        match output_type {
            OutputType::Ade => output_activedata_etl(results, output_path.as_deref(), demangle),
            OutputType::Lcov => output_lcov(results, output_path.as_deref(), demangle),
            OutputType::Coveralls => output_coveralls(
                results,
                opt.token.as_deref(),
                opt.service_name.as_deref(),
                &service_number,
                opt.service_job_id.as_deref(),
                &service_pull_request,
                opt.service_flag_name.as_deref(),
                &commit_sha,
                false,
                output_path.as_deref(),
                &opt.vcs_branch,
                opt.parallel,
                demangle,
            ),
            OutputType::CoverallsPlus => output_coveralls(
                results,
                opt.token.as_deref(),
                opt.service_name.as_deref(),
                &service_number,
                opt.service_job_id.as_deref(),
                &service_pull_request,
                opt.service_flag_name.as_deref(),
                &commit_sha,
                true,
                output_path.as_deref(),
                &opt.vcs_branch,
                opt.parallel,
                demangle,
            ),
            OutputType::Files => output_files(results, output_path.as_deref()),
            OutputType::Covdir => output_covdir(results, output_path.as_deref(), opt.precision),
            OutputType::Html => output_html(
                results,
                output_path.as_deref(),
                num_threads,
                opt.branch,
                opt.output_config_file.as_deref(),
                opt.precision,
            ),
            OutputType::Cobertura => output_cobertura(
                source_root.as_deref(),
                results,
                output_path.as_deref(),
                demangle,
                false,
            ),
            OutputType::CoberturaPretty => output_cobertura(
                source_root.as_deref(),
                results,
                output_path.as_deref(),
                demangle,
                true,
            ),
            OutputType::Markdown => output_markdown(results, output_path.as_deref(), opt.precision),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use clap::CommandFactory;

    #[test]
    fn clap_debug_assert() {
        Opt::command().debug_assert();
    }
}
