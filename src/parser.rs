use std::collections::{BTreeMap, btree_map, HashMap};
use std::path::{Path};
use std::fs::File;
use std::io::{Read, BufRead, BufReader};
use std::ffi::CString;
use std::str;
use libc;

use defs::*;

#[link(name = "llvmgcov", kind="static")]
extern {
    fn parse_llvm_gcno(working_dir: *const libc::c_char, file_stem: *const libc::c_char);
}

pub fn call_parse_llvm_gcno(working_dir: &str, file_stem: &str) {
    let working_dir_c = CString::new(working_dir).unwrap();
    let file_stem_c = CString::new(file_stem).unwrap();
    unsafe {
        parse_llvm_gcno(working_dir_c.as_ptr(), file_stem_c.as_ptr());
    };
}

pub fn parse_lcov<T: Read>(lcov_reader: BufReader<T>, branch_enabled: bool) -> Vec<(String,CovResult)> {
    let mut cur_file = String::new();
    let mut cur_lines = BTreeMap::new();
    let mut cur_branches = BTreeMap::new();
    let mut cur_functions = HashMap::new();

    let mut results = Vec::new();

    for line in lcov_reader.lines() {
        let l = line.unwrap();

        if l == "end_of_record" {
            results.push((cur_file, CovResult {
                lines: cur_lines,
                branches: cur_branches,
                functions: cur_functions,
            }));

            cur_file = String::new();
            cur_lines = BTreeMap::new();
            cur_branches = BTreeMap::new();
            cur_functions = HashMap::new();
        } else {
            let mut key_value = l.splitn(2, ':');
            let key = key_value.next().unwrap();
            let value = key_value.next();
            if value.is_none() {
                // Ignore lines without a ':' character.
                continue;
            }
            let value = value.unwrap();
            match key {
                "SF" => {
                    cur_file = value.to_string();
                },
                "DA" => {
                    let mut values = value.splitn(3, ',');
                    let line_no = values.next().unwrap().parse().unwrap();
                    let execution_count = values.next().unwrap();
                    if execution_count == "0" || execution_count.starts_with('-') {
                        match cur_lines.entry(line_no) {
                            btree_map::Entry::Occupied(_) => {},
                            btree_map::Entry::Vacant(v) => {
                                v.insert(0);
                            }
                        };
                    } else {
                        let execution_count = execution_count.parse().unwrap();
                        match cur_lines.entry(line_no) {
                            btree_map::Entry::Occupied(c) => {
                                *c.into_mut() += execution_count;
                            },
                            btree_map::Entry::Vacant(v) => {
                                v.insert(execution_count);
                            }
                        };
                    }
                },
                "FN" => {
                    let mut f_splits = value.splitn(2, ',');
                    let start = f_splits.next().unwrap().parse().unwrap();
                    let f_name = f_splits.next().unwrap();
                    cur_functions.insert(f_name.to_string(), Function {
                      start: start,
                      executed: false,
                    });
                },
                "FNDA" => {
                    let mut f_splits = value.splitn(2, ',');
                    let executed = f_splits.next().unwrap() != "0";
                    let f_name = f_splits.next().unwrap();
                    let f = cur_functions.get_mut(f_name).expect(format!("FN record missing for function {}", f_name).as_str());
                    f.executed |= executed;
                },
                "BRDA" => {
                    if branch_enabled {
                        let mut values = value.splitn(4, ',');
                        let line_no = values.next().unwrap().parse().unwrap();
                        values.next();
                        let branch_number = values.next().unwrap().parse().unwrap();
                        let taken = values.next().unwrap() != "-";
                        match cur_branches.entry((line_no, branch_number)) {
                            btree_map::Entry::Occupied(c) => {
                                *c.into_mut() |= taken;
                            },
                            btree_map::Entry::Vacant(v) => {
                                v.insert(taken);
                            }
                        };
                    }
                },
                _ => {}
            }
        }
    }

    results
}

fn remove_newline(l: &mut Vec<u8>) {
    loop {
        let last = *l.last().unwrap();
        if last != b'\n' && last != b'\r' {
            break;
        }

        l.pop();
    }
}

pub fn parse_old_gcov(gcov_path: &Path, branch_enabled: bool) -> (String,CovResult) {
    let mut lines = BTreeMap::new();
    let mut branches = BTreeMap::new();
    let mut functions = HashMap::new();

    let f = File::open(gcov_path).expect(&format!("Failed to open old gcov file {}", gcov_path.display()));
    let mut file = BufReader::new(&f);
    let mut line_no: u32 = 0;

    let mut l = vec![];
    let source_name = {
        file.read_until(b'\n', &mut l).unwrap();
        remove_newline(&mut l);
        let l = unsafe {
            str::from_utf8_unchecked(&l)
        };
        let mut splits = l.splitn(4, ':');
        splits.nth(3).unwrap().to_owned()
    };

    loop {
        l.clear();

        let num_bytes = file.read_until(b'\n', &mut l).unwrap();
        if num_bytes == 0 {
            break;
        }
        remove_newline(&mut l);

        let l = unsafe {
            str::from_utf8_unchecked(&l)
        };

        if l.starts_with("function") {
            let mut f_splits = l.splitn(5, ' ');
            let function_name = f_splits.nth(1).unwrap();
            let execution_count: u64 = f_splits.nth(1).unwrap().parse().expect(&format!("Failed parsing execution count: {}", l));
            functions.insert(function_name.to_owned(), Function {
              start: line_no + 1,
              executed: execution_count > 0,
            });
        } else if branch_enabled && l.starts_with("branch ") {
            let mut b_splits = l.splitn(5, ' ');
            let branch_number = b_splits.nth(2).unwrap().parse().unwrap();
            let taken = b_splits.nth(1).unwrap() != "0";
            branches.insert((line_no, branch_number), taken);
        } else {
            let mut splits = l.splitn(3, ':');
            let first_elem = splits.next();
            let second_elem = splits.next();
            if second_elem.is_none() {
                continue;
            }
            if splits.count() != 1 {
                panic!("GCOV lines should be in the format STRING:STRING:STRING, {}", l);
            }

            line_no = second_elem.unwrap().trim().parse().unwrap();

            let cover = first_elem.unwrap().trim();
            if cover == "-" {
                continue;
            }

            if cover == "#####" || cover.starts_with('-') {
                lines.insert(line_no, 0);
            } else {
                lines.insert(line_no, cover.parse().unwrap());
            }
        }
    }

    (source_name, CovResult {
      lines: lines,
      branches: branches,
      functions: functions,
    })
}

pub fn parse_gcov(gcov_path: &Path) -> Vec<(String,CovResult)> {
    let mut cur_file = String::new();
    let mut cur_lines = BTreeMap::new();
    let mut cur_branches = BTreeMap::new();
    let mut cur_functions = HashMap::new();
    let mut branch_number = 0;

    let mut results = Vec::new();

    let f = File::open(&gcov_path).expect(&format!("Failed to open gcov file {}", gcov_path.display()));
    let mut file = BufReader::new(&f);
    let mut l = vec![];

    loop {
        l.clear();

        let num_bytes = file.read_until(b'\n', &mut l).unwrap();
        if num_bytes == 0 {
            break;
        }
        remove_newline(&mut l);

        let l = unsafe {
            str::from_utf8_unchecked(&l)
        };

        let mut key_value = l.splitn(2, ':');
        let key = key_value.next().unwrap();
        let value = key_value.next().unwrap();

        match key {
            "file" => {
                if !cur_file.is_empty() && !cur_lines.is_empty() {
                    // println!("{} {} {:?}", gcov_path.display(), cur_file, cur_lines);
                    results.push((cur_file, CovResult {
                        lines: cur_lines,
                        branches: cur_branches,
                        functions: cur_functions,
                    }));
                }

                cur_file = value.to_owned();
                cur_lines = BTreeMap::new();
                cur_branches = BTreeMap::new();
                cur_functions = HashMap::new();
            },
            "function" => {
                let mut f_splits = value.splitn(3, ',');
                let start = f_splits.next().unwrap().parse().unwrap();
                let executed = f_splits.next().unwrap() != "0";
                let f_name = f_splits.next().unwrap();
                cur_functions.insert(f_name.to_owned(), Function {
                  start: start,
                  executed: executed,
                });
            },
            "lcount" => {
                branch_number = 0;

                let mut values = value.splitn(2, ',');
                let line_no = values.next().unwrap().parse().unwrap();
                let execution_count = values.next().unwrap();
                if execution_count == "0" || execution_count.starts_with('-') {
                    cur_lines.insert(line_no, 0);
                } else {
                    cur_lines.insert(line_no, execution_count.parse().unwrap());
                }
            },
            "branch" => {
                let mut values = value.splitn(2, ',');
                let line_no = values.next().unwrap().parse().unwrap();
                let taken = values.next().unwrap() == "taken";
                cur_branches.insert((line_no, branch_number), taken);
                branch_number += 1;
            },
            _ => {}
        }
    }

    if !cur_lines.is_empty() {
        results.push((cur_file, CovResult {
            lines: cur_lines,
            branches: cur_branches,
            functions: cur_functions,
        }));
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lcov_parser() {
        let f = File::open("./test/prova.info").expect("Failed to open lcov file");
        let file = BufReader::new(&f);
        let results = parse_lcov(file, false);

        assert_eq!(results.len(), 603);

        let ref result = results[0];
        assert_eq!(result.0, "resource://gre/components/MainProcessSingleton.js");
        assert_eq!(result.1.lines, [(7,1),(9,1),(10,1),(12,2),(13,1),(16,1),(17,1),(18,2),(19,1),(21,1),(22,0),(23,0),(24,0),(28,1),(29,0),(30,0),(32,0),(33,0),(34,0),(35,0),(37,0),(39,0),(41,0),(42,0),(44,0),(45,0),(46,0),(47,0),(49,0),(50,0),(51,0),(52,0),(53,0),(54,0),(55,0),(56,0),(59,0),(60,0),(61,0),(63,0),(65,0),(67,1),(68,2),(70,1),(74,1),(75,1),(76,1),(77,1),(78,1),(83,1),(84,1),(90,1)].iter().cloned().collect());
        assert_eq!(result.1.branches, [].iter().cloned().collect());
        assert!(result.1.functions.contains_key("MainProcessSingleton"));
        let func = result.1.functions.get("MainProcessSingleton").unwrap();
        assert_eq!(func.start, 15);
        assert_eq!(func.executed, true);
        assert!(result.1.functions.contains_key("logConsoleMessage"));
        let func = result.1.functions.get("logConsoleMessage").unwrap();
        assert_eq!(func.start, 21);
        assert_eq!(func.executed, false);
    }

    #[test]
    fn test_lcov_parser_with_branch_parsing() {
        // Parse the same file, but with branch parsing enabled.
        let f = File::open("./test/prova.info").expect("Failed to open lcov file");
        let file = BufReader::new(&f);
        let results = parse_lcov(file, true);

        assert_eq!(results.len(), 603);

        let ref result = results[0];
        assert_eq!(result.0, "resource://gre/components/MainProcessSingleton.js");
        assert_eq!(result.1.lines, [(7,1),(9,1),(10,1),(12,2),(13,1),(16,1),(17,1),(18,2),(19,1),(21,1),(22,0),(23,0),(24,0),(28,1),(29,0),(30,0),(32,0),(33,0),(34,0),(35,0),(37,0),(39,0),(41,0),(42,0),(44,0),(45,0),(46,0),(47,0),(49,0),(50,0),(51,0),(52,0),(53,0),(54,0),(55,0),(56,0),(59,0),(60,0),(61,0),(63,0),(65,0),(67,1),(68,2),(70,1),(74,1),(75,1),(76,1),(77,1),(78,1),(83,1),(84,1),(90,1)].iter().cloned().collect());
        assert_eq!(result.1.branches, [((34, 0), false), ((34, 1), false), ((41, 0), false), ((41, 1), false), ((44, 0), false), ((44, 1), false), ((60, 0), false), ((60, 1), false), ((63, 0), false), ((63, 1), false), ((68, 0), true), ((68, 1), true)].iter().cloned().collect());
        assert!(result.1.functions.contains_key("MainProcessSingleton"));
        let func = result.1.functions.get("MainProcessSingleton").unwrap();
        assert_eq!(func.start, 15);
        assert_eq!(func.executed, true);
        assert!(result.1.functions.contains_key("logConsoleMessage"));
        let func = result.1.functions.get("logConsoleMessage").unwrap();
        assert_eq!(func.start, 21);
        assert_eq!(func.executed, false);
    }

    #[test]
    fn test_lcov_parser_fn_with_commas() {
        let f = File::open("./test/prova_fn_with_commas.info").expect("Failed to open lcov file");
        let file = BufReader::new(&f);
        let results = parse_lcov(file, true);

        assert_eq!(results.len(), 1);

        let ref result = results[0];
        assert_eq!(result.0, "aFile.js");
        assert_eq!(result.1.lines, [(7,1),(9,1),(10,1),(12,2),(13,1),(16,1),(17,1),(18,2),(19,1),(21,1),(22,0),(23,0),(24,0),(28,1),(29,0),(30,0),(32,0),(33,0),(34,0),(35,0),(37,0),(39,0),(41,0),(42,0),(44,0),(45,0),(46,0),(47,0),(49,0),(50,0),(51,0),(52,0),(53,0),(54,0),(55,0),(56,0),(59,0),(60,0),(61,0),(63,0),(65,0),(67,1),(68,2),(70,1),(74,1),(75,1),(76,1),(77,1),(78,1),(83,1),(84,1),(90,1),(95,1),(96,1),(97,1),(98,1),(99,1)].iter().cloned().collect());
        assert!(result.1.functions.contains_key("MainProcessSingleton"));
        let func = result.1.functions.get("MainProcessSingleton").unwrap();
        assert_eq!(func.start, 15);
        assert_eq!(func.executed, true);
        assert!(result.1.functions.contains_key("cubic-bezier(0.0, 0.0, 1.0, 1.0)"));
        let func = result.1.functions.get("cubic-bezier(0.0, 0.0, 1.0, 1.0)").unwrap();
        assert_eq!(func.start, 95);
        assert_eq!(func.executed, true);
    }

    #[test]
    fn test_parser_old_gcov_with_encoding_different_from_utf8() {
        let (source_name, result) = parse_old_gcov(Path::new("./test/non-utf-8.gcov"), false);

        assert_eq!(source_name, "main.c");

        assert_eq!(result.lines, [(5, 2), (6, 1), (9, 0), (10, 0), (13, 1), (14, 1)].iter().cloned().collect());

        assert_eq!(result.branches, [].iter().cloned().collect());

        assert!(result.functions.contains_key("func1"));
        let func = result.functions.get("func1").unwrap();
        assert_eq!(func.start, 4);
        assert_eq!(func.executed, true);
        assert!(result.functions.contains_key("func2"));
        let func = result.functions.get("func2").unwrap();
        assert_eq!(func.start, 8);
        assert_eq!(func.executed, false);
    }

    #[test]
    fn test_parser_old_gcov_with_branches() {
        let (source_name, result) = parse_old_gcov(Path::new("./test/old_branches.gcov"), true);

        assert_eq!(source_name, "main.c");

        assert_eq!(result.lines, [(5, 20), (6, 9), (7, 3), (8, 3), (10, 9), (11, 0), (12, 0), (13, 9), (15, 1)].iter().cloned().collect());

        assert_eq!(result.branches, [((5, 0), true), ((5, 1), true), ((6, 0), true), ((6, 1), true), ((10, 0), false), ((10, 1), true)].iter().cloned().collect());

        assert!(result.functions.contains_key("main"));
        let func = result.functions.get("main").unwrap();
        assert_eq!(func.start, 3);
        assert_eq!(func.executed, true);
    }

    #[test]
    fn test_parser() {
        let results = parse_gcov(Path::new("./test/prova.gcov"));

        assert_eq!(results.len(), 10);

        let ref result = results[0];
        assert_eq!(result.0, "/home/marco/Documenti/FD/mozilla-central/build-cov-gcc/dist/include/nsExpirationTracker.h");
        assert_eq!(result.1.lines, [(393,0),(397,0),(399,0),(401,0),(402,0),(403,0),(405,0)].iter().cloned().collect());
        assert!(result.1.functions.contains_key("_ZN19nsExpirationTrackerIN11nsIDocument16SelectorCacheKeyELj4EE25ExpirationTrackerObserver7ReleaseEv"));
        let mut func = result.1.functions.get("_ZN19nsExpirationTrackerIN11nsIDocument16SelectorCacheKeyELj4EE25ExpirationTrackerObserver7ReleaseEv").unwrap();
        assert_eq!(func.start, 393);
        assert_eq!(func.executed, false);

        let ref result = results[5];
        assert_eq!(result.0, "/home/marco/Documenti/FD/mozilla-central/accessible/atk/Platform.cpp");
        assert_eq!(result.1.lines, [(81,0),(83,0),(85,0),(87,0),(88,0),(90,0),(94,0),(96,0),(97,0),(98,0),(99,0),(100,0),(101,0),(103,0),(104,0),(108,0),(110,0),(111,0),(112,0),(115,0),(117,0),(118,0),(122,0),(123,0),(124,0),(128,0),(129,0),(130,0),(136,17),(138,17),(141,0),(142,0),(146,0),(147,0),(148,0),(151,0),(152,0),(153,0),(154,0),(155,0),(156,0),(157,0),(161,0),(162,0),(165,0),(166,0),(167,0),(168,0),(169,0),(170,0),(171,0),(172,0),(184,0),(187,0),(189,0),(190,0),(194,0),(195,0),(196,0),(200,0),(201,0),(202,0),(203,0),(207,0),(208,0),(216,17),(218,17),(219,0),(220,0),(221,0),(222,0),(223,0),(226,17),(232,0),(233,0),(234,0),(253,17),(261,11390),(265,11390),(268,373),(274,373),(277,373),(278,373),(281,373),(288,373),(289,373),(293,373),(294,373),(295,373),(298,373),(303,5794),(306,5794),(307,5558),(309,236),(311,236),(312,236),(313,0),(316,236),(317,236),(318,0),(321,236),(322,236),(323,236),(324,236),(327,236),(328,236),(329,236),(330,236),(331,472),(332,472),(333,236),(338,236),(339,236),(340,236),(343,0),(344,0),(345,0),(346,0),(347,0),(352,236),(353,236),(354,236),(355,236),(361,236),(362,236),(364,236),(365,236),(370,0),(372,0),(373,0),(374,0),(376,0)].iter().cloned().collect());
        assert!(result.1.functions.contains_key("_ZL13LoadGtkModuleR24GnomeAccessibilityModule"));
        func = result.1.functions.get("_ZL13LoadGtkModuleR24GnomeAccessibilityModule").unwrap();
        assert_eq!(func.start, 81);
        assert_eq!(func.executed, false);
        assert!(result.1.functions.contains_key("_ZN7mozilla4a11y12PlatformInitEv"));
        func = result.1.functions.get("_ZN7mozilla4a11y12PlatformInitEv").unwrap();
        assert_eq!(func.start, 136);
        assert_eq!(func.executed, true);
        assert!(result.1.functions.contains_key("_ZN7mozilla4a11y16PlatformShutdownEv"));
        func = result.1.functions.get("_ZN7mozilla4a11y16PlatformShutdownEv").unwrap();
        assert_eq!(func.start, 216);
        assert_eq!(func.executed, true);
        assert!(result.1.functions.contains_key("_ZN7mozilla4a11y7PreInitEv"));
        func = result.1.functions.get("_ZN7mozilla4a11y7PreInitEv").unwrap();
        assert_eq!(func.start, 261);
        assert_eq!(func.executed, true);
        assert!(result.1.functions.contains_key("_ZN7mozilla4a11y19ShouldA11yBeEnabledEv"));
        func = result.1.functions.get("_ZN7mozilla4a11y19ShouldA11yBeEnabledEv").unwrap();
        assert_eq!(func.start, 303);
        assert_eq!(func.executed, true);
    }

    #[test]
    fn test_parser_gcov_with_negative_counts() {
        let results = parse_gcov(Path::new("./test/negative_counts.gcov"));
        assert_eq!(results.len(), 118);
        let ref result = results[14];
        assert_eq!(result.0, "/home/marco/Documenti/FD/mozilla-central/build-cov-gcc/dist/include/mozilla/Assertions.h");
        assert_eq!(result.1.lines, [(40,0)].iter().cloned().collect());
    }

    #[test]
    fn test_parser_gcov_with_64bit_counts() {
        let results = parse_gcov(Path::new("./test/64bit_count.gcov"));
        assert_eq!(results.len(), 46);
        let ref result = results[8];
        assert_eq!(result.0, "/home/marco/Documenti/FD/mozilla-central/build-cov-gcc/dist/include/js/HashTable.h");
        assert_eq!(result.1.lines, [(324,8096),(343,12174),(344,6085),(345,23331),(357,10720),(361,313165934),(399,272539208),(402,31491125),(403,35509735),(420,434104),(709,313172766),(715,272542535),(801,584943263),(822,0),(825,0),(826,0),(828,0),(829,0),(831,0),(834,2210404897),(835,196249666),(838,3764974),(840,516370744),(841,1541684),(842,2253988941),(843,197245483),(844,0),(845,5306658),(846,821426720),(847,47096565),(853,82598134),(854,247796865),(886,272542256),(887,272542256),(904,599154437),(908,584933028),(913,584943263),(916,543534922),(917,584933028),(940,508959481),(945,1084660344),(960,545084512),(989,534593),(990,128435),(1019,427973453),(1029,504065334),(1038,1910289238),(1065,425402),(1075,10613316),(1076,5306658),(1090,392499332),(1112,48208),(1113,48208),(1114,0),(1115,0),(1118,48211),(1119,8009),(1120,48211),(1197,40347),(1202,585715301),(1207,1171430602),(1210,585715301),(1211,910968),(1212,585715301),(1222,30644),(1223,70165),(1225,1647),(1237,4048),(1238,4048),(1240,8096),(1244,6087),(1250,6087),(1257,6085),(1264,6085),(1278,6085),(1279,6085),(1280,0),(1283,6085),(1284,66935),(1285,30425),(1286,30425),(1289,6085),(1293,12171),(1294,6086),(1297,6087),(1299,6087),(1309,4048),(1310,4048),(1316,632104110),(1327,251893735),(1329,251893735),(1330,251893735),(1331,503787470),(1337,528619265),(1344,35325952),(1345,35325952),(1353,26236),(1354,13118),(1364,305520839),(1372,585099705),(1381,585099705),(1382,585099705),(1385,585099705),(1391,1135737600),(1397,242807686),(1400,242807686),(1403,1032741488),(1404,1290630),(1405,1042115),(1407,515080114),(1408,184996962),(1412,516370744),(1414,516370744),(1415,516370744),(1417,154330912),(1420,812664176),(1433,47004405),(1442,47004405),(1443,47004405),(1446,94008810),(1452,9086049),(1456,24497042),(1459,12248521),(1461,12248521),(1462,24497042),(1471,30642),(1474,30642),(1475,30642),(1476,30642),(1477,30642),(1478,30642),(1484,64904),(1485,34260),(1489,34260),(1490,34260),(1491,34260),(1492,34260),(1495,34260),(1496,69792911),(1497,139524496),(1498,94193130),(1499,47096565),(1500,47096565),(1506,61326),(1507,30663),(1513,58000),(1516,35325952),(1518,35325952),(1522,29000),(1527,29000),(1530,29000),(1534,0),(1536,0),(1537,0),(1538,0),(1540,0),(1547,10613316),(1548,1541684),(1549,1541684),(1552,3764974),(1554,5306658),(1571,8009),(1573,8009),(1574,8009),(1575,31345),(1576,5109),(1577,5109),(1580,8009),(1581,1647),(1582,8009),(1589,0),(1592,0),(1593,0),(1594,0),(1596,0),(1597,0),(1599,0),(1600,0),(1601,0),(1604,0),(1605,0),(1606,0),(1607,0),(1609,0),(1610,0),(1611,0),(1615,0),(1616,0),(1625,0),(1693,655507),(1711,35615006),(1730,10720),(1732,10720),(1733,10720),(1735,10720),(1736,10720),(1739,313162046),(1741,313162046),(1743,313162046),(1744,313162046),(1747,272542535),(1749,272542535),(1750,272542535),(1752,272542535),(1753,272542535),(1754,272542256),(1755,272542256),(1759,35509724),(1761,35509724),(1767,71019448),(1772,35505028),(1773,179105),(1776,179105),(1777,179105),(1780,35325923),(1781,35326057),(1785,35326058),(1786,29011),(1789,71010332),(1790,35505166),(1796,35505166)].iter().cloned().collect());

        // Assert more stuff.
    }
}
