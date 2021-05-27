use regex::Regex;
use std::path::Path;

pub enum FilterType {
    Line(u32),
    Branch(u32),
    Both(u32),
}

#[derive(Default)]
pub struct FileFilter {
    excl_line: Option<Regex>,
    excl_start: Option<Regex>,
    excl_stop: Option<Regex>,
    excl_br_line: Option<Regex>,
    excl_br_start: Option<Regex>,
    excl_br_stop: Option<Regex>,
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

    pub fn create(&self, file: &Path) -> Vec<FilterType> {
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

        file.split('\n')
            .enumerate()
            .filter_map(move |(number, line)| {
                // Line numbers are 1-based.
                let number = (number + 1) as u32;

                // The file is split on \n, which may result in a trailing \r
                // on Windows. Remove it.
                let line = line.strip_suffix('\r').unwrap_or(line);

                // End a branch ignore region. Region endings are exclusive.
                if ignore_br
                    && self
                        .excl_br_stop
                        .as_ref()
                        .map_or(false, |f| f.is_match(line))
                {
                    ignore_br = false
                }

                // End a line ignore region. Region endings are exclusive.
                if ignore && self.excl_stop.as_ref().map_or(false, |f| f.is_match(line)) {
                    ignore = false
                }

                // Start a branch ignore region. Region starts are inclusive.
                if !ignore_br
                    && self
                        .excl_br_start
                        .as_ref()
                        .map_or(false, |f| f.is_match(line))
                {
                    ignore_br = true;
                }

                // Start a line ignore region. Region starts are inclusive.
                if !ignore && self.excl_start.as_ref().map_or(false, |f| f.is_match(line)) {
                    ignore = true;
                }

                if ignore_br {
                    // Consuming code has to eliminate each of these
                    // individually, so it has to know when both are ignored vs.
                    // either.
                    if ignore {
                        Some(FilterType::Both(number))
                    } else {
                        Some(FilterType::Branch(number))
                    }
                } else if ignore {
                    Some(FilterType::Line(number))
                } else if self
                    .excl_br_line
                    .as_ref()
                    .map_or(false, |f| f.is_match(line))
                {
                    // Single line exclusion. If single line exclusions occur
                    // inside a region they are meaningless (would be applied
                    // anway), so they are lower priority.
                    if self.excl_line.as_ref().map_or(false, |f| f.is_match(line)) {
                        Some(FilterType::Both(number))
                    } else {
                        Some(FilterType::Branch(number))
                    }
                } else if self.excl_line.as_ref().map_or(false, |f| f.is_match(line)) {
                    Some(FilterType::Line(number))
                } else {
                    None
                }
            })
            .collect()
    }
}
