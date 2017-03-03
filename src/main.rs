#![feature(proc_macro)]

#[macro_use]
extern crate serde_json;
extern crate crossbeam;
extern crate walkdir;
extern crate num_cpus;

use std::cmp;
use std::collections::{HashSet,HashMap};
use std::collections::hash_map::Entry;
use std::env;
use std::path::PathBuf;
use std::ffi::OsStr;
use std::fs;
use std::fs::File;
use std::io::BufReader;
use std::io::BufRead;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use crossbeam::sync::MsQueue;
use walkdir::WalkDir;
use serde_json::{Value, Map};

/*
use libc::size_t;
use std::ffi::CString;
use std::os::raw::c_char;

#[link(name = "gcov")]
extern {
    fn __gcov_read_unsigned() -> u32;
    fn __gcov_open(name: *const c_char) -> i32;
    fn __gcov_close();
}

fn gcov_open(file: String) -> i32 {
    let c_to_print = CString::new(file).unwrap();
    return unsafe { __gcov_open(c_to_print.as_ptr()) };
}

fn prova() {
  if gcov_open("~/Documenti/FD/mozilla-central/build-cov-gcc/toolkit/components/telemetry/Telemetry.gcda".to_string()) == 1 {
    panic!();
  }
}*/

fn rmdir(directory: &str) {
    if PathBuf::from(directory).exists() {
        fs::remove_dir_all("workingDir").expect("Failed to remove working directory");
    }
}

fn producer(directory: &String, queue: Arc<MsQueue<PathBuf>>) {
    for entry in WalkDir::new(directory) {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_file() && path.extension() == Some(OsStr::new("gcda")) {
            queue.push(fs::canonicalize(&path).unwrap());
        }
    }
}

#[test]
fn test_producer() {
    let queue: Arc<MsQueue<PathBuf>> = Arc::new(MsQueue::new());
    let queue_consumer = queue.clone();

    producer(&"test".to_string(), queue);

    let endswith_strings: Vec<String> = vec![
        "grcov/test/Platform.gcda".to_string(),
        "grcov/test/RootAccessibleWrap.gcda".to_string(),
        "grcov/test/nsMaiInterfaceValue.gcda".to_string(),
        "grcov/test/sub/prova2.gcda".to_string(),
        "grcov/test/nsMaiInterfaceDocument.gcda".to_string(),
        "grcov/test/Unified_cpp_netwerk_base0.gcda".to_string(),
        "grcov/test/prova.gcda".to_string(),
        "grcov/test/nsGnomeModule.gcda".to_string(),
        "grcov/test/negative_counts.gcda".to_string(),
        "grcov/test/64bit_count.gcda".to_string(),
    ];

    let mut vec: Vec<PathBuf> = Vec::new();
    for _ in 0..endswith_strings.len() {
        vec.push(queue_consumer.pop());
    }

    assert_eq!(vec.len(), 10);

    for endswith_string in endswith_strings.iter() {
        assert!(vec.iter().any(|&ref x| x.ends_with(endswith_string)), "Missing {}", endswith_string);
    }

    assert_eq!(queue_consumer.try_pop(), None);
}

fn run_gcov(gcda_path: &PathBuf, working_dir: &PathBuf) {
    let status = Command::new("gcov")
                         .arg(gcda_path)
                         .arg("-i") // Generate intermediate gcov format, faster to parse.
                         .current_dir(working_dir)
                         .stdout(Stdio::null())
                         .stderr(Stdio::null())
                         .status()
                         .expect("Failed to execute process");

    assert!(status.success(), "gcov wasn't successfully executed");
}

struct Function {
    start: u32,
    executed: bool,
}

struct Result {
    name: String,
    covered: Vec<u32>,
    uncovered: Vec<u32>,
    functions: HashMap<String,Function>,
}

fn parse_gcov(gcov_path: PathBuf) -> Vec<Result> {
    let mut cur_file = String::new();
    let mut cur_lines_covered: Vec<u32> = Vec::new();
    let mut cur_lines_uncovered: Vec<u32> = Vec::new();
    let mut cur_functions: HashMap<String,Function> = HashMap::new();

    let mut results = Vec::new();

    let f = File::open(&gcov_path).unwrap();
    let file = BufReader::new(&f);
    for line in file.lines() {
        let l = line.unwrap();
        let key_value: Vec<&str> = l.splitn(2, ':').collect();
        let key = key_value[0];
        let value = key_value[1];
        match key {
            "file" => {
                if !cur_file.is_empty() && (cur_lines_covered.len() > 0 || cur_lines_uncovered.len() > 0) {
                    // println!("{} {} {:?} {:?}", gcov_path.display(), cur_file, cur_lines_covered, cur_lines_uncovered);
                    results.push(Result {
                        name: cur_file,
                        covered: cur_lines_covered,
                        uncovered: cur_lines_uncovered,
                        functions: cur_functions,
                    });
                }

                cur_file = value.to_string();
                cur_lines_covered = Vec::new();
                cur_lines_uncovered = Vec::new();
                cur_functions = HashMap::new();
            },
            "function" => {
                let f_splits: Vec<&str> = value.splitn(3, ',').collect();
                cur_functions.insert(f_splits[2].to_string(), Function {
                  start: f_splits[0].parse().unwrap(),
                  executed: f_splits[1] != "0",
                });
            },
            "lcount" => {
                let values: Vec<&str> = value.splitn(2, ',').collect();
                if values[1] == "0" || values[1].starts_with("-") {
                    cur_lines_uncovered.push(values[0].parse().unwrap());
                } else {
                    cur_lines_covered.push(values[0].parse().unwrap());
                }
            },
            _ => {}
        }
    }

    if cur_lines_covered.len() > 0 || cur_lines_uncovered.len() > 0 {
        results.push(Result {
            name: cur_file,
            covered: cur_lines_covered,
            uncovered: cur_lines_uncovered,
            functions: cur_functions,
        });
    }

    results
}

#[test]
fn test_parser() {
    let results = parse_gcov(PathBuf::from("./test/prova.gcov"));

    assert_eq!(results.len(), 10);

    let ref result1 = results[0];
    assert_eq!(result1.name, "/home/marco/Documenti/FD/mozilla-central/build-cov-gcc/dist/include/nsExpirationTracker.h");
    assert!(result1.covered.is_empty());
    assert_eq!(result1.uncovered, vec![393,397,399,401,402,403,405]);
    assert!(result1.functions.contains_key("_ZN19nsExpirationTrackerIN11nsIDocument16SelectorCacheKeyELj4EE25ExpirationTrackerObserver7ReleaseEv"));
    let mut func = result1.functions.get("_ZN19nsExpirationTrackerIN11nsIDocument16SelectorCacheKeyELj4EE25ExpirationTrackerObserver7ReleaseEv").unwrap();
    assert_eq!(func.start, 393);
    assert_eq!(func.executed, false);

    let ref result5 = results[5];
    assert_eq!(result5.name, "/home/marco/Documenti/FD/mozilla-central/accessible/atk/Platform.cpp");
    assert_eq!(result5.covered, vec![136, 138, 216, 218, 226, 253, 261, 265, 268, 274, 277, 278, 281, 288, 289, 293, 294, 295, 298, 303, 306, 307, 309, 311, 312, 316, 317, 321, 322, 323, 324, 327, 328, 329, 330, 331, 332, 333, 338, 339, 340, 352, 353, 354, 355, 361, 362, 364, 365]);
    assert_eq!(result5.uncovered, vec![81, 83, 85, 87, 88, 90, 94, 96, 97, 98, 99, 100, 101, 103, 104, 108, 110, 111, 112, 115, 117, 118, 122, 123, 124, 128, 129, 130, 141, 142, 146, 147, 148, 151, 152, 153, 154, 155, 156, 157, 161, 162, 165, 166, 167, 168, 169, 170, 171, 172, 184, 187, 189, 190, 194, 195, 196, 200, 201, 202, 203, 207, 208, 219, 220, 221, 222, 223, 232, 233, 234, 313, 318, 343, 344, 345, 346, 347, 370, 372, 373, 374, 376]);
    assert!(result5.functions.contains_key("_ZL13LoadGtkModuleR24GnomeAccessibilityModule"));
    func = result5.functions.get("_ZL13LoadGtkModuleR24GnomeAccessibilityModule").unwrap();
    assert_eq!(func.start, 81);
    assert_eq!(func.executed, false);
    assert!(result5.functions.contains_key("_ZN7mozilla4a11y12PlatformInitEv"));
    func = result5.functions.get("_ZN7mozilla4a11y12PlatformInitEv").unwrap();
    assert_eq!(func.start, 136);
    assert_eq!(func.executed, true);
    assert!(result5.functions.contains_key("_ZN7mozilla4a11y16PlatformShutdownEv"));
    func = result5.functions.get("_ZN7mozilla4a11y16PlatformShutdownEv").unwrap();
    assert_eq!(func.start, 216);
    assert_eq!(func.executed, true);
    assert!(result5.functions.contains_key("_ZN7mozilla4a11y7PreInitEv"));
    func = result5.functions.get("_ZN7mozilla4a11y7PreInitEv").unwrap();
    assert_eq!(func.start, 261);
    assert_eq!(func.executed, true);
    assert!(result5.functions.contains_key("_ZN7mozilla4a11y19ShouldA11yBeEnabledEv"));
    func = result5.functions.get("_ZN7mozilla4a11y19ShouldA11yBeEnabledEv").unwrap();
    assert_eq!(func.start, 303);
    assert_eq!(func.executed, true);

    let results = parse_gcov(PathBuf::from("./test/negative_counts.gcov"));
    assert_eq!(results.len(), 118);
    let ref negative_count_result = results[14];
    assert_eq!(negative_count_result.name, "/home/marco/Documenti/FD/mozilla-central/build-cov-gcc/dist/include/mozilla/Assertions.h");
    assert!(negative_count_result.covered.is_empty());
    assert_eq!(negative_count_result.uncovered, vec![40]);

    let results = parse_gcov(PathBuf::from("./test/64bit_count.gcov"));
    assert_eq!(results.len(), 46);
    let ref a64bit_count_result = results[8];
    assert_eq!(a64bit_count_result.name, "/home/marco/Documenti/FD/mozilla-central/build-cov-gcc/dist/include/js/HashTable.h");
    assert_eq!(a64bit_count_result.covered, vec![324, 343, 344, 345, 357, 361, 399, 402, 403, 420, 709, 715, 801, 834, 835, 838, 840, 841, 842, 843, 845, 846, 847, 853, 854, 886, 887, 904, 908, 913, 916, 917, 940, 945, 960, 989, 990, 1019, 1029, 1038, 1065, 1075, 1076, 1090, 1112, 1113, 1118, 1119, 1120, 1197, 1202, 1207, 1210, 1211, 1212, 1222, 1223, 1225, 1237, 1238, 1240, 1244, 1250, 1257, 1264, 1278, 1279, 1283, 1284, 1285, 1286, 1289, 1293, 1294, 1297, 1299, 1309, 1310, 1316, 1327, 1329, 1330, 1331, 1337, 1344, 1345, 1353, 1354, 1364, 1372, 1381, 1382, 1385, 1391, 1397, 1400, 1403, 1404, 1405, 1407, 1408, 1412, 1414, 1415, 1417, 1420, 1433, 1442, 1443, 1446, 1452, 1456, 1459, 1461, 1462, 1471, 1474, 1475, 1476, 1477, 1478, 1484, 1485, 1489, 1490, 1491, 1492, 1495, 1496, 1497, 1498, 1499, 1500, 1506, 1507, 1513, 1516, 1518, 1522, 1527, 1530, 1547, 1548, 1549, 1552, 1554, 1571, 1573, 1574, 1575, 1576, 1577, 1580, 1581, 1582, 1693, 1711, 1730, 1732, 1733, 1735, 1736, 1739, 1741, 1743, 1744, 1747, 1749, 1750, 1752, 1753, 1754, 1755, 1759, 1761, 1767, 1772, 1773, 1776, 1777, 1780, 1781, 1785, 1786, 1789, 1790, 1796]);
    assert_eq!(a64bit_count_result.uncovered, vec![822, 825, 826, 828, 829, 831, 844, 1114, 1115, 1280, 1534, 1536, 1537, 1538, 1540, 1589, 1592, 1593,1594,1596,1597,1599,1600, 1601, 1604, 1605, 1606, 1607, 1609, 1610, 1611, 1615, 1616, 1625]);

    // Assert more stuff.
}

// Merge results, without caring about duplicate lines (they will be removed at the end).
fn merge_results(result: &mut Result, result2: &mut Result) {
    result.covered.append(&mut result2.covered);
    result.uncovered.append(&mut result2.uncovered);
    for (name, function) in result2.functions.drain() {
        match result.functions.entry(name) {
            Entry::Occupied(f) => f.into_mut().executed |= function.executed,
            Entry::Vacant(v) => { v.insert(function); }
        };
    }
}

#[test]
fn test_merge_results() {
    let mut functions1: HashMap<String,Function> = HashMap::new();
    functions1.insert("f1".to_string(), Function {
        start: 1,
        executed: false,
    });
    functions1.insert("f2".to_string(), Function {
        start: 2,
        executed: false,
    });
    let mut result = Result {
        name: "name".to_string(),
        covered: vec![1, 2],
        uncovered: vec![1, 7],
        functions: functions1,
    };
    let mut functions2: HashMap<String,Function> = HashMap::new();
    functions2.insert("f1".to_string(), Function {
        start: 1,
        executed: false,
    });
    functions2.insert("f2".to_string(), Function {
        start: 2,
        executed: true,
    });
    let mut result2 = Result {
        name: "name".to_string(),
        covered: vec![3, 4],
        uncovered: vec![1, 2, 8],
        functions: functions2,
    };

    merge_results(&mut result, &mut result2);
    assert_eq!(result.name, "name");
    assert_eq!(result.covered, vec![1, 2, 3, 4]);
    assert_eq!(result.uncovered, vec![1, 7, 1, 2, 8]);
    assert!(result.functions.contains_key("f1"));
    assert!(result.functions.contains_key("f2"));
    let mut func = result.functions.get("f1").unwrap();
    assert_eq!(func.start, 1);
    assert_eq!(func.executed, false);
    func = result.functions.get("f2").unwrap();
    assert_eq!(func.start, 2);
    assert_eq!(func.executed, true);
}

fn add_result(mut result: Result, map: &mut HashMap<String,Result>) {
    match map.entry(result.name.clone()) { // XXX: Can we avoid copying the string here?
        Entry::Occupied(obj) => {
            merge_results(obj.into_mut(), &mut result);
        },
        Entry::Vacant(v) => {
            v.insert(result);
        }
    };
}

fn clean_covered_lines(results: &mut HashMap<String,Result>) {
    for result in results.values_mut() {
        let ref mut result = *result;
        result.covered.sort();
        result.covered.dedup();

        result.uncovered.sort();
        result.uncovered.dedup();

        let set: HashSet<_> = result.covered.iter().collect();
        result.uncovered.retain(|x| !set.contains(x));
    }
}

fn output_activedata_etl(results: &mut HashMap<String,Result>) {
    for (key, result) in results {
        let ref mut result = *result;

        let end: u32 = cmp::max(
            match result.covered.last() { Some(v) => *v, None => 0 },
            match result.uncovered.last() { Some(v) => *v, None => 0 },
        ) + 1;

        let mut methods = Map::new();

        if result.covered.len() > 0 {
            let mut start_indexes: Vec<u32> = Vec::new();
            for function in result.functions.values() {
                start_indexes.push(function.start);
            }
            start_indexes.sort();

            for (name, function) in result.functions.drain() {
                // println!("{} {} {}", name, function.executed, function.start);
                if !function.executed {
                    continue;
                }

                let mut func_end = end;

                for start in start_indexes.iter() {
                    if *start > function.start {
                        func_end = *start;
                        break;
                    }
                }

                let lines_covered: Vec<u32> = result.covered.iter().filter(|&&x| x >= function.start && x < func_end).cloned().collect();

                methods.insert(name, Value::from(lines_covered));
            }
        }

        let asd = json!({
            "sourceFile": key,
            "testUrl": key,
            "covered": result.covered,
            "uncovered": result.uncovered,
            "methods": methods,
            
        });

        println!("{}", asd.to_string());
    }
}

fn output_lcov(results: &mut HashMap<String,Result>) {
    println!("TN:");

    for (key, result) in results {
        let ref mut result = *result;

        // println!("{} {:?} {:?}", key, result.covered, result.uncovered);

        println!("SF:{}", key);
        for (name, function) in result.functions.iter() {
            println!("FN:{},{}", function.start, name);
        }
        for (name, function) in result.functions.iter() {
            println!("FNDA:{},{}", if function.executed { 1 } else { 0 }, name);
        }
        if result.functions.len() > 0 {
            println!("FNF:{}", result.functions.len());
            println!("FNH:{}", result.functions.values().filter(|x| x.executed).count());
        }

        let mut lines_map: HashMap<u32,u8> = HashMap::new();
        for line in result.covered.iter() {
            lines_map.insert(*line, 1);
        }
        for line in result.uncovered.iter() {
            lines_map.insert(*line, 0);
        }
        let mut all_lines: Vec<u32> = result.covered.clone();
        all_lines.append(&mut result.uncovered.clone());
        all_lines.sort();
        for line in all_lines.iter() {
            println!("DA:{},{}", line, lines_map[line]);
        }
        println!("LF:{}",result.covered.len());
        println!("LH:{}",all_lines.len());
        println!("end_of_record");
    }
}

fn print_usage(program: &String) {
    println!("Usage: {} DIRECTORY [OUTPUT_TYPE]", program);
    println!("OUTPUT_TYPE can be one of:");
    println!(" - (DEFAULT) ade for the ActiveData-ETL specific format;");
    println!(" - lcov for the lcov INFO format.");
}

fn main() {
    rmdir("workingDir");
    fs::create_dir("workingDir").expect("Failed to create initial directory");

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        print_usage(&args[0]);
        return;
    }
    let ref directory = args[1];
    let output_type = if args.len() == 2 { "ade".to_string() } else { args[2].clone() };
    if output_type != "ade" && output_type != "lcov" {
        println!("'{}' output format is not supported.", output_type);
        print_usage(&args[0]);
        return;
    }

    let results: Arc<Mutex<HashMap<String,Result>>> = Arc::new(Mutex::new(HashMap::new()));
    let queue: Arc<MsQueue<PathBuf>> = Arc::new(MsQueue::new());
    let finished_producing = Arc::new(AtomicBool::new(false));

    let mut parsers = Vec::new();

    let num_threads = num_cpus::get() * 2;

    for i in 0..num_threads {
        let queue_consumer = queue.clone();
        let results_consumer = results.clone();
        let finished_producing_consumer = finished_producing.clone();

        let t = thread::spawn(move || {
            let working_dir = PathBuf::from(&format!("workingDir/{}/", i));
            fs::create_dir(&working_dir).expect("Failed to create working directory");

            loop {
                if let Some(gcda_path) = queue_consumer.try_pop() {
                    run_gcov(&gcda_path, &working_dir);

                    let mut results = parse_gcov(working_dir.join(gcda_path.file_name().unwrap().to_str().unwrap().to_string() + ".gcov"));

                    let mut map = results_consumer.lock().unwrap();
                    for result in results.drain(..) {
                        add_result(result, &mut map);
                    }
                } else {
                    if finished_producing_consumer.load(Ordering::Acquire) {
                        break;
                    }

                    thread::yield_now();
                }
            }
        });

        parsers.push(t);
    }

    producer(directory, queue);
    finished_producing.store(true, Ordering::Release);

    for parser in parsers {
        let _ = parser.join();
    }

    rmdir("workingDir");

    let ref mut results_obj = *results.lock().unwrap();

    clean_covered_lines(results_obj);

    if output_type == "ade" {
        output_activedata_etl(results_obj);
    } else if output_type == "lcov" {
        output_lcov(results_obj);
    }
}
