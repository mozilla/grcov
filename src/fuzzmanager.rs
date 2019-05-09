use serde_json::map::Map;

pub use crate::defs::*;

impl FMStats {

    pub fn new(coverage: &Vec<i64>) -> Self {
        let mut covered = 0;
        let mut total = 0;

        for x in coverage.iter() {
            let x = *x;
            total += (x >= 0) as usize;
            covered += (x > 0) as usize;
        }
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

impl FMFileStats {

    pub fn new(name: String, coverage: Vec<i64>) -> Self {
        Self {
            name,
            stats: FMStats::new(&coverage),
            coverage,
        }
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

impl FMDirStats {

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

    pub fn to_json(&mut self) -> serde_json::Value {
        let mut children = Map::new();
        for file in self.files.drain(..) {
            children.insert(file.name.clone(), file.to_json());
        }
        for dir in self.dirs.drain(..) {
            let mut dir = dir.borrow_mut();
            children.insert(dir.name.clone(), dir.to_json());
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
