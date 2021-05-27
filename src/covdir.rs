use serde_json::{json, map::Map};
use std::collections::BTreeMap;

pub use crate::defs::*;

impl CDStats {
    pub fn new(total: usize, covered: usize) -> Self {
        let missed = total - covered;
        Self {
            total,
            covered,
            missed,
            percent: Self::get_percent(covered, total),
        }
    }

    pub fn add(&mut self, other: &Self) {
        // Add stats to self without recomputing the percentage because it's time consuming.
        // So once all the stats are merged into one for a directory
        // then need to call set_percent()
        self.total += other.total;
        self.covered += other.covered;
        self.missed += other.missed;
    }

    pub fn set_percent(&mut self) {
        self.percent = Self::get_percent(self.covered, self.total);
    }

    pub fn get_percent(x: usize, y: usize) -> f64 {
        if y != 0 {
            f64::round(x as f64 / (y as f64) * 10_000.) / 100.
        } else {
            0.0
        }
    }
}

impl CDFileStats {
    pub fn new(name: String, coverage: BTreeMap<u32, u64>) -> Self {
        let (total, covered, lines) = Self::get_coverage(coverage);
        Self {
            name,
            stats: CDStats::new(total, covered),
            coverage: lines,
        }
    }

    fn get_coverage(coverage: BTreeMap<u32, u64>) -> (usize, usize, Vec<i64>) {
        let mut covered = 0;
        let last_line = *coverage.keys().last().unwrap_or(&0) as usize;
        let total = coverage.len();
        let mut lines: Vec<i64> = vec![-1; last_line];
        for (line_num, line_count) in coverage.iter() {
            let line_count = *line_count;
            unsafe {
                *lines.get_unchecked_mut((*line_num - 1) as usize) = line_count as i64;
            }
            covered += (line_count > 0) as usize;
        }
        (total, covered, lines)
    }

    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "name": self.name,
            "linesTotal": self.stats.total,
            "linesCovered": self.stats.covered,
            "linesMissed": self.stats.missed,
            "coveragePercent": self.stats.percent,
            "coverage": self.coverage,
        })
    }
}

impl CDDirStats {
    pub fn new(name: String) -> Self {
        Self {
            name,
            files: Vec::new(),
            dirs: Vec::new(),
            stats: Default::default(),
        }
    }

    pub fn set_stats(&mut self) {
        for file in self.files.iter() {
            self.stats.add(&file.stats);
        }
        for dir in self.dirs.iter() {
            let mut dir = dir.borrow_mut();
            dir.set_stats();
            self.stats.add(&dir.stats);
        }
        self.stats.set_percent();
    }

    pub fn into_json(self) -> serde_json::Value {
        let mut children = Map::new();
        for file in self.files {
            children.insert(file.name.clone(), file.to_json());
        }
        for dir in self.dirs {
            let dir = dir.take();
            children.insert(dir.name.clone(), dir.into_json());
        }
        json!({
            "name": self.name,
            "linesTotal": self.stats.total,
            "linesCovered": self.stats.covered,
            "linesMissed": self.stats.missed,
            "coveragePercent": self.stats.percent,
            "children": children,
        })
    }
}
