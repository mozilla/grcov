#[cfg(all(unix, feature = "tc"))]
#[global_allocator]
static GLOBAL: tcmalloc::TCMalloc = tcmalloc::TCMalloc;

use crossbeam::channel::bounded;
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
use std::borrow::Borrow;
use structopt::{clap::ArgGroup, StructOpt};

use grcov::*;

enum OutputType {
    Ade,
    Lcov,
    Coveralls,
    CoverallsPlus,
    Files,
    Covdir,
    Html,
    Cobertura,
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
            _ => return Err(format!("{} is not a supported output type", s)),
        })
    }
}

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

#[derive(StructOpt)]
#[structopt(
    author,
    about = "Parse, collect and aggregate code coverage data for multiple source files"
)]
struct Opt {
    /// Sets the input paths to use.
    #[structopt(required = true)]
    paths: Vec<String>,
    /// Sets the path to the compiled binary to be used.
    #[structopt(short, long, value_name = "PATH")]
    binary_path: Option<PathBuf>,
    /// Sets a custom output type.
    #[structopt(
        short = "t",
        long,
        long_help = "\
            Sets a custom output type:\n\
            - *html* for a HTML coverage report;\n\
            - *coveralls* for the Coveralls specific format;\n\
            - *lcov* for the lcov INFO format;\n\
            - *covdir* for the covdir recursive JSON format;\n\
            - *coveralls+* for the Coveralls specific format with function information;\n\
            - *ade* for the ActiveData-ETL specific format;\n\
            - *files* to only return a list of files.\n\
        ",
        value_name = "OUTPUT TYPE",
        default_value = "lcov",
        requires_ifs = &[
            ("coveralls", "coveralls-auth"),
            ("coveralls+", "coveralls-auth"),
        ],
        possible_values = &[
            "ade",
            "lcov",
            "coveralls",
            "coveralls+",
            "files",
            "covdir",
            "html",
            "cobertura",
        ],
        multiple=true
    )]
    output_type: Vec<OutputType>,
    /// Specifies the output path.
    #[structopt(short, long, value_name = "PATH", alias = "output-file")]
    output_path: Option<PathBuf>,
    /// Specifies the root directory of the source files.
    #[structopt(short, long, value_name = "DIRECTORY", parse(from_os_str))]
    source_dir: Option<PathBuf>,
    /// Specifies a prefix to remove from the paths (e.g. if grcov is run on a different machine
    /// than the one that generated the code coverage information).
    #[structopt(short, long, value_name = "PATH")]
    prefix_dir: Option<PathBuf>,
    /// Ignore source files that can't be found on the disk.
    #[structopt(long)]
    ignore_not_existing: bool,
    /// Ignore files/directories specified as globs.
    #[structopt(long = "ignore", value_name = "PATH", number_of_values = 1)]
    ignore_dir: Vec<String>,
    /// Keep only files/directories specified as globs.
    #[structopt(long = "keep-only", value_name = "PATH", number_of_values = 1)]
    keep_dir: Vec<String>,
    #[structopt(long, value_name = "PATH")]
    path_mapping: Option<PathBuf>,
    /// Enables parsing branch coverage information.
    #[structopt(long)]
    branch: bool,
    /// Filters out covered/uncovered files. Use 'covered' to only return covered files, 'uncovered'
    /// to only return uncovered files.
    #[structopt(long, possible_values = &["covered", "uncovered"])]
    filter: Option<Filter>,
    /// Speeds-up parsing, when the code coverage information is exclusively coming from a llvm
    /// build.
    #[structopt(long)]
    llvm: bool,
    /// Sets the repository token from Coveralls, required for the 'coveralls' and 'coveralls+'
    /// formats.
    #[structopt(long, value_name = "TOKEN")]
    token: Option<String>,
    /// Sets the hash of the commit used to generate the code coverage data.
    #[structopt(long, value_name = "COMMIT HASH")]
    commit_sha: Option<String>,
    /// Sets the service name.
    #[structopt(long, value_name = "SERVICE NAME")]
    service_name: Option<String>,
    /// Sets the service number.
    #[structopt(long, value_name = "SERVICE NUMBER")]
    service_number: Option<String>,
    /// Sets the service job id.
    #[structopt(
        long,
        value_name = "SERVICE JOB ID",
        visible_alias = "service-job-number",
        requires = "service-name"
    )]
    service_job_id: Option<String>,
    /// Sets the service pull request number.
    #[structopt(long, value_name = "SERVICE PULL REQUEST")]
    service_pull_request: Option<String>,
    /// Sets the build type to be parallel for 'coveralls' and 'coveralls+' formats.
    #[structopt(long)]
    parallel: bool,
    #[structopt(long, value_name = "NUMBER")]
    threads: Option<usize>,
    #[structopt(long = "guess-directory-when-missing")]
    guess_directory: bool,
    /// Set the branch for coveralls report. Defaults to 'master'.
    #[structopt(long, value_name = "VCS BRANCH", default_value = "master")]
    vcs_branch: String,
    /// Set the file where to log (or stderr or stdout). Defaults to 'stderr'.
    #[structopt(long, value_name = "LOG", default_value = "stderr")]
    log: PathBuf,
    /// Set the log level.
    #[structopt(
        long,
        value_name = "LEVEL",
        default_value = "ERROR",
        possible_values = &["OFF", "ERROR","WARN", "INFO", "DEBUG", "TRACE"],
    )]
    log_level: LevelFilter,
    /// Lines in covered files containing this marker will be excluded.
    #[structopt(long, value_name = "regex")]
    excl_line: Option<Regex>,
    /// Marks the beginning of an excluded section. The current line is part of this section.
    #[structopt(long, value_name = "regex")]
    excl_start: Option<Regex>,
    /// Marks the end of an excluded section. The current line is part of this section.
    #[structopt(long, value_name = "regex")]
    excl_stop: Option<Regex>,
    /// Lines in covered files containing this marker will be excluded from branch coverage.
    #[structopt(long, value_name = "regex")]
    excl_br_line: Option<Regex>,
    /// Marks the beginning of a section excluded from branch coverage. The current line is part of
    /// this section.
    #[structopt(long, value_name = "regex")]
    excl_br_start: Option<Regex>,
    /// Marks the end of a section excluded from branch coverage. The current line is part of this
    /// section.
    #[structopt(long, value_name = "regex")]
    excl_br_stop: Option<Regex>,
    /// No symbol demangling.
    #[structopt(long)]
    no_demangle: bool,
}

fn main() {
    let opt = Opt::from_clap(
        &Opt::clap()
            // This group requires that at least one of --token and --service-job-id
            // be present. --service-job-id requires --service-name, so this
            // effectively means we accept the following combinations:
            // - --token
            // - --token --service-job-id --service-name
            // - --service-job-id --service-name
            .group(
                ArgGroup::with_name("coveralls-auth")
                    .args(&["token", "service-job-id"])
                    .multiple(true),
            )
            .get_matches(),
    );

    let filter_option = opt.filter.map(|filter| match filter {
        Filter::Covered => true,
        Filter::Uncovered => false,
    });
    let stdout = Path::new("stdout");
    let stderr = Path::new("stderr");

    if opt.log == stdout {
        let _ = TermLogger::init(
            opt.log_level,
            Config::default(),
            TerminalMode::Stdout,
            ColorChoice::Auto,
        );
    } else if opt.log == stderr {
        let _ = TermLogger::init(
            opt.log_level,
            Config::default(),
            TerminalMode::Stderr,
            ColorChoice::Auto,
        );
    } else if let Ok(file) = File::create(&opt.log) {
        let _ = WriteLogger::init(opt.log_level, Config::default(), file);
    } else {
        let _ = TermLogger::init(
            opt.log_level,
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
        .map(|source_dir| {
            canonicalize_path(&source_dir).expect("Source directory does not exist.")
        });

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


    let Opt {
        service_number,
        service_pull_request,
        commit_sha,
        output_path,
        branch,
        parallel,
        ignore_not_existing,
        ..
    } = opt;
    let service_number = service_number.unwrap_or_default();
    let service_pull_request = service_pull_request.unwrap_or_default();
    let commit_sha = commit_sha.unwrap_or_default();
    let output_path = output_path.as_deref();
    let ignore_dir = &opt.ignore_dir;
    let keep_dir = &opt.keep_dir;
    let token = opt.token.as_deref();
    let service_name = opt.service_name.as_deref();
    let service_job = opt.service_job_id.as_deref();
    let vcs_branch = &opt.vcs_branch;
    opt.output_type
        .iter()
        .for_each(|f| {
            let result_map = Arc::clone(&result_map);
            let path_mapping = Arc::clone(&path_mapping);
            let result_map_mutex = Arc::try_unwrap(result_map).unwrap();
            let result_map = result_map_mutex.into_inner().unwrap();

            let path_mapping_mutex = Arc::try_unwrap(path_mapping).unwrap();
            let path_mapping = path_mapping_mutex.into_inner().unwrap();
            let path = rewrite_paths(
                result_map,
                path_mapping,
                source_root.as_deref(),
                prefix_dir.as_deref(),
                ignore_not_existing,
                ignore_dir,
                keep_dir,
                filter_option,
                file_filter.clone(),
            );
            let iterator = Box::new(path.into_iter());
            match f {
                OutputType::Ade => output_activedata_etl(iterator, output_path, demangle),
                OutputType::Lcov => output_lcov(iterator, output_path, demangle),
                OutputType::Coveralls => output_coveralls(
                    iterator,
                    token,
                    service_name,
                    &service_number,
                    service_job,
                    &service_pull_request,
                    &commit_sha,
                    false,
                    output_path,
                    vcs_branch,
                    parallel,
                    demangle,
                ),
                OutputType::CoverallsPlus => output_coveralls(
                    iterator,
                    token,
                    service_name,
                    &service_number,
                    service_job,
                    &service_pull_request,
                    &commit_sha,
                    true,
                    output_path,
                    vcs_branch,
                    parallel,
                    demangle,
                ),
                OutputType::Files => output_files(iterator, output_path),
                OutputType::Covdir => output_covdir(iterator, output_path),
                OutputType::Html => output_html(
                    iterator,
                    output_path,
                    num_threads,
                    branch,
                ),
                OutputType::Cobertura => output_cobertura(
                    source_root.as_deref(),
                    iterator,
                    output_path,
                    demangle,
                ),
            };
        });
}
