use regex::Regex;
use std::path::PathBuf;

pub enum FilterType {
    Line(u32),
    Branch(u32),
    Both(u32),
}

pub struct FileFilter {
    excl_line: Option<Regex>,
    excl_start: Option<Regex>,
    excl_stop: Option<Regex>,
    excl_br_line: Option<Regex>,
    excl_br_start: Option<Regex>,
    excl_br_stop: Option<Regex>,
}

impl Default for FileFilter {
    fn default() -> Self {
        Self {
            excl_line: None,
            excl_start: None,
            excl_stop: None,
            excl_br_line: None,
            excl_br_start: None,
            excl_br_stop: None,
        }
    }
}

fn matches(regex: &Option<Regex>, line: &str) -> bool {
    if let Some(regex) = regex {
        regex.is_match(line)
    } else {
        false
    }
}

impl FileFilter {
    pub fn new(
        excl_line: Option<Regex>,
        excl_start: Option<Regex>,
        excl_stop: Option<Regex>,
        excl_br_line: Option<Regex>,
        excl_br_start: Option<Regex>,
        excl_br_stop: Option<Regex>,
    ) -> Self {
        Self {
            excl_line,
            excl_start,
            excl_stop,
            excl_br_line,
            excl_br_start,
            excl_br_stop,
        }
    }

    pub fn create<'a>(&'a self, file: &PathBuf) -> Vec<FilterType> {
        if self.excl_line.is_none()
            && self.excl_start.is_none()
            && self.excl_br_line.is_none()
            && self.excl_br_start.is_none()
        {
            return Vec::new();
        }

        let file = std::fs::read_to_string(file);
        let file = if let Ok(file) = file {
            file
        } else {
            return Vec::new();
        };

        let mut ignore_br = false;
        let mut ignore = false;

        file.split("\n")
            .enumerate()
            .into_iter()
            .filter_map(move |(number, line)| {
                let number = (number + 1) as u32;
                let line = if line.ends_with("\r") {
                    &line[..(line.len() - 1)]
                } else {
                    line
                };

                if ignore_br {
                    if matches(&self.excl_br_stop, line) {
                        ignore_br = false
                    }
                }

                if ignore {
                    if matches(&self.excl_stop, line) {
                        ignore = false
                    }
                }

                if matches(&self.excl_br_start, line) {
                    ignore_br = true;
                }

                if matches(&self.excl_start, line) {
                    ignore = true;
                }

                if ignore_br {
                    if ignore {
                        Some(FilterType::Both(number))
                    } else {
                        Some(FilterType::Branch(number))
                    }
                } else if ignore {
                    Some(FilterType::Line(number))
                } else if matches(&self.excl_br_line, line) {
                    if matches(&self.excl_line, line) {
                        Some(FilterType::Both(number))
                    } else {
                        Some(FilterType::Branch(number))
                    }
                } else if matches(&self.excl_line, line) {
                    Some(FilterType::Line(number))
                } else {
                    None
                }
            })
            .collect()
    }
}
