use crate::defs::*;

pub fn is_covered(result: &CovResult) -> bool {
    // For C/C++ source files, we can consider a file as being uncovered
    // when all its source lines are uncovered.
    let any_line_covered = result
        .lines
        .values()
        .any(|&execution_count| execution_count != 0);
    if !any_line_covered {
        return false;
    }
    // For JavaScript files, we can't do the same, as the top-level is always
    // executed, even if it just contains declarations. So, we need to check if
    // all its functions, except the top-level, are uncovered.
    let any_function_covered = result
        .functions
        .iter()
        .any(|(name, function)| function.executed && name != "top-level");
    result.functions.len() <= 1 || any_function_covered
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustc_hash::FxHashMap;

    #[test]
    fn test_covered() {
        let mut functions: FunctionMap = FxHashMap::default();
        functions.insert(
            "f1".to_string(),
            Function {
                start: 1,
                executed: true,
            },
        );
        functions.insert(
            "f2".to_string(),
            Function {
                start: 2,
                executed: false,
            },
        );
        let result = CovResult {
            lines: [(1, 21), (2, 7), (7, 0)].iter().cloned().collect(),
            branches: [].iter().cloned().collect(),
            functions,
        };

        assert!(is_covered(&result));
    }

    #[test]
    fn test_covered_no_functions() {
        let result = CovResult {
            lines: [(1, 21), (2, 7), (7, 0)].iter().cloned().collect(),
            branches: [].iter().cloned().collect(),
            functions: FxHashMap::default(),
        };

        assert!(is_covered(&result));
    }

    #[test]
    fn test_uncovered_no_lines_executed() {
        let mut functions: FunctionMap = FxHashMap::default();
        functions.insert(
            "f1".to_string(),
            Function {
                start: 1,
                executed: true,
            },
        );
        functions.insert(
            "f2".to_string(),
            Function {
                start: 2,
                executed: false,
            },
        );
        let result = CovResult {
            lines: [(1, 0), (2, 0), (7, 0)].iter().cloned().collect(),
            branches: [].iter().cloned().collect(),
            functions: FxHashMap::default(),
        };

        assert!(!is_covered(&result));
    }

    #[test]
    fn test_covered_functions_executed() {
        let mut functions: FunctionMap = FxHashMap::default();
        functions.insert(
            "top-level".to_string(),
            Function {
                start: 1,
                executed: true,
            },
        );
        functions.insert(
            "f".to_string(),
            Function {
                start: 2,
                executed: true,
            },
        );
        let result = CovResult {
            lines: [(1, 21), (2, 7), (7, 0)].iter().cloned().collect(),
            branches: [].iter().cloned().collect(),
            functions,
        };

        assert!(is_covered(&result));
    }

    #[test]
    fn test_covered_toplevel_executed() {
        let mut functions: FunctionMap = FxHashMap::default();
        functions.insert(
            "top-level".to_string(),
            Function {
                start: 1,
                executed: true,
            },
        );
        let result = CovResult {
            lines: [(1, 21), (2, 7), (7, 0)].iter().cloned().collect(),
            branches: [].iter().cloned().collect(),
            functions,
        };

        assert!(is_covered(&result));
    }

    #[test]
    fn test_uncovered_functions_not_executed() {
        let mut functions: FunctionMap = FxHashMap::default();
        functions.insert(
            "top-level".to_string(),
            Function {
                start: 1,
                executed: true,
            },
        );
        functions.insert(
            "f".to_string(),
            Function {
                start: 7,
                executed: false,
            },
        );
        let result = CovResult {
            lines: [(1, 21), (2, 7), (7, 0)].iter().cloned().collect(),
            branches: [].iter().cloned().collect(),
            functions,
        };

        assert!(!is_covered(&result));
    }
}
