use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use std::cmp;
use std::collections::{btree_map, hash_map, BTreeMap};
use std::convert::From;
use std::fmt::{Debug, Display, Formatter};
use std::fs::File;
use std::io::{BufReader, Error, Read, Write};
use std::path::PathBuf;
use std::result::Result;

use crate::defs::{CovResult, Function};

#[derive(Default)]
pub struct GCNO {
    version: u32,
    checksum: u32,
    runcounts: u32,
    functions: Vec<GcovFunction>,
}

#[derive(Debug)]
struct GcovFunction {
    identifier: u32,
    line_number: u32,
    line_checksum: u32,
    file_name: String,
    name: String,
    blocks: SmallVec<[GcovBlock; 16]>,
    edges: SmallVec<[GcovEdge; 16]>,
    lines: FxHashMap<u32, u64>,
    executed: bool,
}

#[derive(Debug)]
struct GcovBlock {
    no: usize,
    source: SmallVec<[usize; 4]>,
    destination: SmallVec<[usize; 4]>,
    lines: SmallVec<[u32; 16]>,
    line_max: u32,
    counter: u64,
}

#[derive(Debug)]
struct GcovEdge {
    source: usize,
    destination: usize,
    counter: u64,
    cycles: u64,
}

#[derive(Debug)]
pub enum GcovError {
    Io(std::io::Error),
    Str(String),
}

impl From<Error> for GcovError {
    fn from(err: Error) -> GcovError {
        GcovError::Str(format!("Reader error: {}", err))
    }
}

pub trait GcovReader {
    fn read_string(&mut self) -> Result<String, GcovError>;
    fn read_u32(&mut self) -> Result<u32, GcovError>;
    fn read_u64(&mut self) -> Result<u64, GcovError>;
    fn read_counter(&mut self) -> Result<u64, GcovError>;
    fn read_version(&mut self) -> Result<u32, GcovError>;
    fn check_type(&mut self, typ: [u8; 4]) -> Result<(), GcovError>;
    fn get_pos(&self) -> usize;
    fn get_stem(&self) -> &str;
    fn skip_u32(&mut self) -> Result<(), GcovError>;
    fn skip_u64(&mut self) -> Result<(), GcovError>;
}

pub struct GcovReaderBuf {
    stem: String,
    buffer: Vec<u8>,
    pos: usize,
}

impl From<&str> for GcovReaderBuf {
    fn from(path: &str) -> GcovReaderBuf {
        let path = PathBuf::from(path);
        let mut f = File::open(&path).unwrap();
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        GcovReaderBuf::new(path.to_str().unwrap(), buf)
    }
}

macro_rules! read_u_le {
    ($ty: ty, $buf: expr) => {{
        let size = std::mem::size_of::<$ty>();
        let start = $buf.pos;
        $buf.pos += size;
        if $buf.pos <= $buf.buffer.len() {
            Ok(unsafe {
                *std::mem::transmute::<*const u8, *const $ty>($buf.buffer[start..].as_ptr())
            }
            .to_le())
        } else {
            Err(GcovError::Str(format!(
                "Not enough data in buffer: cannot read integer in {}",
                $buf.get_stem()
            )))
        }
    }};
}

macro_rules! skip {
    ($ty: ty, $buf: expr) => {{
        let size = std::mem::size_of::<$ty>();
        $buf.pos += size;
        if $buf.pos < $buf.buffer.len() {
            Ok(())
        } else {
            Err(GcovError::Str(format!(
                "Not enough data in buffer: cannot skip {} bytes in {}",
                size,
                $buf.get_stem()
            )))
        }
    }};
}

impl GcovReaderBuf {
    pub fn new(stem: &str, buffer: Vec<u8>) -> GcovReaderBuf {
        GcovReaderBuf {
            stem: stem.to_string(),
            buffer,
            pos: 0,
        }
    }
}

impl GcovReader for GcovReaderBuf {
    fn get_stem(&self) -> &str {
        &self.stem
    }

    #[inline(always)]
    fn skip_u32(&mut self) -> Result<(), GcovError> {
        skip!(u32, self)
    }

    #[inline(always)]
    fn skip_u64(&mut self) -> Result<(), GcovError> {
        skip!(u64, self)
    }

    fn read_string(&mut self) -> Result<String, GcovError> {
        let mut len = 0;
        while len == 0 {
            len = read_u_le!(u32, self)?;
        }
        let len = len as usize * 4;
        let start = self.pos;
        self.pos += len;
        if self.pos <= self.buffer.len() {
            let bytes = &self.buffer[start..self.pos];
            let i = len - bytes.iter().rev().position(|&x| x != 0).unwrap();
            Ok(unsafe { std::str::from_utf8_unchecked(&bytes[..i]).to_string() })
        } else {
            Err(GcovError::Str(format!(
                "Not enough data in buffer: cannot read string in {}",
                self.get_stem()
            )))
        }
    }

    #[inline(always)]
    fn read_u32(&mut self) -> Result<u32, GcovError> {
        read_u_le!(u32, self)
    }

    #[inline(always)]
    fn read_u64(&mut self) -> Result<u64, GcovError> {
        read_u_le!(u64, self)
    }

    #[inline(always)]
    fn read_counter(&mut self) -> Result<u64, GcovError> {
        let lo = read_u_le!(u32, self)?;
        let hi = read_u_le!(u32, self)?;

        Ok(u64::from(hi) << 32 | u64::from(lo))
    }

    fn read_version(&mut self) -> Result<u32, GcovError> {
        let i = self.pos;
        if i + 4 <= self.buffer.len() {
            self.pos += 4;
            if self.buffer[i] == b'*' {
                let zero = u32::from('0');
                let zero = zero + 10 * (zero + 10 * zero);
                Ok(u32::from(self.buffer[i + 1])
                    + 10 * (u32::from(self.buffer[i + 2]) + 10 * u32::from(self.buffer[i + 3]))
                    - zero)
            } else {
                let bytes = &self.buffer[i..i + 4];
                Err(GcovError::Str(format!(
                    "Unexpected version: {} in {}",
                    std::str::from_utf8(&bytes).unwrap(),
                    self.get_stem()
                )))
            }
        } else {
            Err(GcovError::Str(format!(
                "Not enough data in buffer: Cannot read version in {}",
                self.get_stem()
            )))
        }
    }

    fn check_type(&mut self, typ: [u8; 4]) -> Result<(), GcovError> {
        let i = self.pos;
        if i + 4 <= self.buffer.len() {
            self.pos += 4;
            if self.buffer[i] == typ[0]
                && self.buffer[i + 1] == typ[1]
                && self.buffer[i + 2] == typ[2]
                && self.buffer[i + 3] == typ[3]
            {
                Ok(())
            } else {
                let bytes = &self.buffer[i..i + 4];
                Err(GcovError::Str(format!(
                    "Unexpected file type: {} in {}.",
                    std::str::from_utf8(&bytes).unwrap(),
                    self.get_stem()
                )))
            }
        } else {
            Err(GcovError::Str(format!(
                "Not enough data in buffer: Cannot compare types in {}",
                self.get_stem()
            )))
        }
    }

    #[inline(always)]
    fn get_pos(&self) -> usize {
        self.pos
    }
}

impl Display for GcovError {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        match self {
            GcovError::Io(e) => write!(f, "{}", e),
            GcovError::Str(e) => write!(f, "{}", e),
        }
    }
}

impl Debug for GCNO {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        for fun in &self.functions {
            writeln!(
                f,
                "===== {} ({}) @ {}:{}",
                fun.name, fun.identifier, fun.file_name, fun.line_number
            )?;
            for block in &fun.blocks {
                writeln!(f, "Block : {} Counter : {}", block.no, block.counter)?;
                if let Some((last, elmts)) = block.source.split_last() {
                    write!(f, "\tSource Edges : ")?;
                    for edge in elmts.iter().map(|i| &fun.edges[*i]) {
                        write!(f, "{} ({}), ", edge.source, edge.counter)?;
                    }
                    let edge = &fun.edges[*last];
                    writeln!(f, "{} ({}), ", edge.source, edge.counter)?;
                }
                if let Some((last, elmts)) = block.destination.split_last() {
                    write!(f, "\tDestination Edges : ")?;
                    for edge in elmts.iter().map(|i| &fun.edges[*i]) {
                        write!(f, "{} ({}), ", edge.destination, edge.counter)?;
                    }
                    let edge = &fun.edges[*last];
                    writeln!(f, "{} ({}), ", edge.destination, edge.counter)?;
                }
                if let Some((last, elmts)) = block.lines.split_last() {
                    write!(f, "\tLines : ")?;
                    for i in elmts {
                        write!(f, "{},", i)?;
                    }
                    writeln!(f, "{},", last)?;
                }
            }
        }

        Ok(())
    }
}

impl GCNO {
    pub fn new() -> Self {
        GCNO {
            version: 0,
            checksum: 0,
            runcounts: 0,
            functions: Vec::new(),
        }
    }

    #[inline]
    pub fn compute(
        stem: &str,
        gcno_buf: Vec<u8>,
        mut gcda_bufs: Vec<Vec<u8>>,
        branch_enabled: bool,
    ) -> Result<Vec<(String, CovResult)>, GcovError> {
        let mut gcno = GCNO::new();
        gcno.read(GcovReaderBuf::new(stem, gcno_buf))?;
        for gcda_buf in gcda_bufs.drain(..) {
            gcno.read_gcda(GcovReaderBuf::new(stem, gcda_buf))?;
        }
        Ok(gcno.finalize(branch_enabled))
    }

    pub fn read<T: GcovReader>(&mut self, mut reader: T) -> Result<(), GcovError> {
        reader.check_type([b'o', b'n', b'c', b'g'])?;
        self.version = reader.read_version()?;
        self.checksum = reader.read_u32()?;
        self.read_functions(&mut reader)
    }

    fn read_edges(fun: &mut GcovFunction, reader: &mut GcovReader) -> Result<u32, GcovError> {
        let mut tag = reader.read_u32()?;
        let edges = &mut fun.edges;
        let blocks = &mut fun.blocks;
        let mut edges_count = 0;
        while tag == 0x0143_0000 {
            let count = reader.read_u32()?;
            let count = ((count - 1) / 2) as usize;
            let block_no = reader.read_u32()? as usize;
            if block_no <= blocks.len() {
                edges.reserve(count);
                blocks[block_no].destination.reserve(count);
                for _ in 0..count {
                    let dst_block_no = reader.read_u32()? as usize;
                    let _flags = reader.read_u32()?;
                    edges.push(GcovEdge {
                        source: block_no,
                        destination: dst_block_no,
                        counter: 0,
                        cycles: 0,
                    });
                    let i = match blocks[block_no]
                        .destination
                        .binary_search_by(|x| edges[*x].destination.cmp(&dst_block_no))
                    {
                        Ok(i) => i,
                        Err(i) => i,
                    };
                    blocks[block_no].destination.insert(i, edges_count);
                    blocks[dst_block_no].source.push(edges_count);
                    edges_count += 1;
                }
                tag = reader.read_u32()?;
            } else {
                return Err(GcovError::Str(format!(
                    "Unexpected block number: {} (in {}) in {}",
                    block_no,
                    fun.name,
                    reader.get_stem()
                )));
            }
        }
        Ok(tag)
    }

    fn read_lines(
        fun: &mut GcovFunction,
        reader: &mut GcovReader,
        tag: u32,
    ) -> Result<u32, GcovError> {
        let mut tag = tag;
        while tag == 0x0145_0000 {
            let len = reader.read_u32()? - 3;
            let block_no = reader.read_u32()? as usize;
            if block_no <= fun.blocks.len() {
                if len > 0 {
                    // Read the word that pads the beginning of the line table. This may be a
                    // flag of some sort, but seems to always be zero.
                    let _dummy = reader.skip_u32()?;

                    let file_name = reader.read_string()?;
                    let len = len - 2 - ((file_name.len() as u32 / 4) + 1);

                    if file_name != fun.file_name {
                        return Err(GcovError::Str(format!(
                            "Multiple sources for a single basic block: {} != {} (in {}) in {}",
                            fun.file_name,
                            file_name,
                            fun.name,
                            reader.get_stem()
                        )));
                    }
                    let block = &mut fun.blocks[block_no];
                    let lines = &mut block.lines;
                    lines.reserve(len as usize);
                    for _ in 0..len {
                        let line = reader.read_u32()?;
                        if line != 0 {
                            lines.push(line);
                            if line > block.line_max {
                                block.line_max = line;
                            }
                        }
                    }
                }
                // Just read 2 zeros
                let _dummy = reader.skip_u64()?;
                tag = reader.read_u32()?;
            } else {
                return Err(GcovError::Str(format!(
                    "Unexpected block number: {} (in {}).",
                    block_no, fun.name
                )));
            }
        }
        Ok(tag)
    }

    fn read_functions(&mut self, reader: &mut GcovReader) -> Result<(), GcovError> {
        let mut tag = reader.read_u32()?;
        while tag == 0x0100_0000 {
            let _dummy = reader.skip_u32()?;
            let identifier = reader.read_u32()?;
            let line_checksum = reader.read_u32()?;
            if self.version != 402 {
                let cfg_sum = reader.read_u32()?;
                if cfg_sum != self.checksum {
                    let fn_name = reader.read_string()?;
                    return Err(GcovError::Str(format!(
                        "File checksums do not match: {} != {} (in {}) in {}",
                        self.checksum,
                        cfg_sum,
                        fn_name,
                        reader.get_stem()
                    )));
                }
            }

            let name = reader.read_string()?;
            let file_name = reader.read_string()?;
            let line_number = reader.read_u32()?;
            let block_tag = reader.read_u32()?;

            if block_tag == 0x0141_0000 {
                let count = reader.read_u32()? as usize;
                let mut blocks: SmallVec<[GcovBlock; 16]> = SmallVec::with_capacity(count);
                for no in 0..count {
                    let _flags = reader.skip_u32()?;
                    blocks.push(GcovBlock {
                        no,
                        source: SmallVec::new(),
                        destination: SmallVec::new(),
                        lines: SmallVec::new(),
                        line_max: 0,
                        counter: 0,
                    });
                }
                let mut fun = GcovFunction {
                    identifier,
                    line_number,
                    line_checksum,
                    file_name,
                    name,
                    blocks,
                    edges: SmallVec::new(),
                    lines: FxHashMap::default(),
                    executed: false,
                };
                tag = GCNO::read_edges(&mut fun, reader)?;
                tag = GCNO::read_lines(&mut fun, reader, tag)?;
                self.functions.push(fun);
            } else {
                return Err(GcovError::Str(format!(
                    "Invalid function tag: {} in {}",
                    tag,
                    reader.get_stem()
                )));
            }
        }
        Ok(())
    }

    pub fn read_gcda<T: GcovReader>(&mut self, mut reader: T) -> Result<(), GcovError> {
        reader.check_type([b'a', b'd', b'c', b'g'])?;
        let version = reader.read_version()?;
        if version != self.version {
            Err(GcovError::Str(format!(
                "GCOV versions do not match in {}",
                reader.get_stem()
            )))
        } else {
            let checksum = reader.read_u32()?;
            if checksum != self.checksum {
                Err(GcovError::Str(format!(
                    "File checksums do not match: {} != {} in {}",
                    self.checksum,
                    checksum,
                    reader.get_stem()
                )))
            } else {
                for mut fun in &mut self.functions {
                    GCNO::read_gcda_function(self.version, self.checksum, &mut fun, &mut reader)?;
                }
                let object_tag = reader.read_u32()?;
                if object_tag == 0xa100_0000 {
                    reader.skip_u32()?;
                    reader.skip_u64()?;
                    self.runcounts += reader.read_u32()?;
                }
                Ok(())
            }
        }
    }

    fn read_gcda_function(
        version: u32,
        checksum: u32,
        fun: &mut GcovFunction,
        reader: &mut GcovReader,
    ) -> Result<(), GcovError> {
        let tag = reader.read_u32()?;
        if tag != 0x0100_0000 {
            return Err(GcovError::Str(format!(
                "Unexpected number of functions in {}",
                reader.get_stem()
            )));
        }

        let header_len = reader.read_u32()?;
        let end_pos = reader.get_pos() + (header_len as usize) * 4;
        let id = reader.read_u32()?;
        if id != fun.identifier {
            return Err(GcovError::Str(format!(
                "Function identifiers do not match: {} != {} (in {}) in {}",
                fun.identifier,
                id,
                fun.name,
                reader.get_stem()
            )));
        }

        let _chk_sum = reader.skip_u32()?;
        if version != 402 {
            let cfg_sum = reader.read_u32()?;
            if cfg_sum != checksum {
                return Err(GcovError::Str(format!(
                    "File checksums do not match: {} != {} (in {}) in {}",
                    checksum,
                    cfg_sum,
                    fun.name,
                    reader.get_stem()
                )));
            }
        }

        if reader.get_pos() < end_pos {
            let fun_name = reader.read_string()?;
            if fun.name != fun_name {
                return Err(GcovError::Str(format!(
                    "Function names do not match: {} != {} in {}",
                    fun.name,
                    fun_name,
                    reader.get_stem()
                )));
            }
        }

        let arc_tag = reader.read_u32()?;
        if arc_tag != 0x01a1_0000 {
            return Err(GcovError::Str(format!(
                "Arc tag not found (in {}) in {}",
                fun.name,
                reader.get_stem()
            )));
        }

        let count = reader.read_u32()?;
        let edges = &mut fun.edges;
        if edges.len() as u32 != count / 2 {
            return Err(GcovError::Str(format!(
                "Unexpected number of edges (in {}) in {}",
                fun.name,
                reader.get_stem()
            )));
        }

        if let Some((first, elmts)) = edges.split_first_mut() {
            // The first edge is between entry block and a block
            // so the entry block as the same counter as the
            // edge counter.
            let counter = reader.read_counter()?;
            first.counter += counter;
            fun.blocks[first.destination].counter += counter;
            fun.blocks[first.source].counter += counter;

            for edge in elmts {
                let counter = reader.read_counter()?;
                edge.counter += counter;
                fun.blocks[edge.destination].counter += counter;
            }
        }
        Ok(())
    }

    fn collect_lines(&self) -> FxHashMap<&str, FxHashMap<u32, u64>> {
        let mut results: FxHashMap<&str, FxHashMap<u32, u64>> = FxHashMap::default();
        for function in &self.functions {
            let lines = match results.entry(&function.file_name) {
                hash_map::Entry::Occupied(l) => l.into_mut(),
                hash_map::Entry::Vacant(p) => p.insert(FxHashMap::default()),
            };

            for (line, counter) in &function.lines {
                match lines.entry(*line) {
                    hash_map::Entry::Occupied(c) => {
                        *c.into_mut() += *counter;
                    }
                    hash_map::Entry::Vacant(p) => {
                        p.insert(*counter);
                    }
                }
            }
        }
        results
    }

    pub fn dump(
        &mut self,
        path: &PathBuf,
        file_name: &str,
        writer: &mut Write,
    ) -> Result<(), GcovError> {
        let file = File::open(path)?;
        let mut reader = BufReader::new(file);
        let mut source = String::new();

        for fun in &mut self.functions {
            fun.add_line_count();
        }

        let counters = self.collect_lines();
        let counters = &counters[file_name];
        reader.read_to_string(&mut source)?;
        let stem = PathBuf::from(file_name);
        let stem = stem.file_stem().unwrap().to_str().unwrap();
        let mut n: u32 = 0;
        let has_runs = self.runcounts != 0;

        writeln!(writer, "{:>9}:{:>5}:Source:{}", "-", 0, file_name)?;
        writeln!(writer, "{:>9}:{:>5}:Graph:{}.gcno", "-", 0, stem)?;
        if has_runs {
            writeln!(writer, "{:>9}:{:>5}:Data:{}.gcda", "-", 0, stem)?;
        } else {
            writeln!(writer, "{:>9}:{:>5}:Data:-", "-", 0)?;
        }
        writeln!(writer, "{:>9}:{:>5}:Runs:{}", "-", 0, self.runcounts)?;
        writeln!(
            writer,
            "{:>9}:{:>5}:Programs:{}",
            "-",
            0,
            if has_runs { 1 } else { 0 }
        )?;
        let mut iter = source.split('\n').peekable();
        while let Some(line) = iter.next() {
            if iter.peek().is_none() && line.is_empty() {
                // We're on the last line and it's empty
                break;
            }
            n += 1;
            if let Some(counter) = counters.get(&n) {
                if *counter == 0 {
                    writeln!(writer, "{:>9}:{:>5}:{}", "#####", n, line)?;
                } else {
                    writeln!(writer, "{:>9}:{:>5}:{}", *counter, n, line)?;
                }
            } else {
                writeln!(writer, "{:>9}:{:>5}:{}", "-", n, line)?;
            }
        }

        Ok(())
    }

    pub fn finalize(&mut self, branch_enabled: bool) -> Vec<(String, CovResult)> {
        let mut results: FxHashMap<&str, CovResult> = FxHashMap::default();
        for fun in &mut self.functions {
            fun.add_line_count();
            let res = match results.entry(&fun.file_name) {
                hash_map::Entry::Occupied(r) => r.into_mut(),
                hash_map::Entry::Vacant(p) => p.insert(CovResult {
                    lines: BTreeMap::new(),
                    branches: BTreeMap::new(),
                    functions: FxHashMap::default(),
                }),
            };
            res.functions.insert(
                fun.name.clone(),
                Function {
                    start: fun.line_number,
                    executed: fun.executed,
                },
            );
            if fun.executed {
                for (line, counter) in fun.lines.iter() {
                    match res.lines.entry(*line) {
                        btree_map::Entry::Occupied(c) => {
                            *c.into_mut() += *counter;
                        }
                        btree_map::Entry::Vacant(p) => {
                            p.insert(*counter);
                        }
                    }
                }
            } else {
                for line in fun.lines.keys() {
                    res.lines.entry(*line).or_insert(0);
                }
            }
            if branch_enabled {
                if fun.executed {
                    for block in &fun.blocks {
                        if block.destination.len() > 1 {
                            let line = block.line_max;
                            let taken = block
                                .destination
                                .iter()
                                .map(|no| fun.edges[*no].counter > 0);
                            match res.branches.entry(line) {
                                btree_map::Entry::Occupied(c) => {
                                    let v = c.into_mut();
                                    v.extend(taken);
                                }
                                btree_map::Entry::Vacant(p) => {
                                    p.insert(taken.collect());
                                }
                            }
                        }
                    }
                } else {
                    for block in &fun.blocks {
                        let n_dest = block.destination.len();
                        if n_dest > 1 {
                            let taken = vec![false; n_dest];
                            match res.branches.entry(block.line_max) {
                                btree_map::Entry::Occupied(c) => {
                                    let v = c.into_mut();
                                    v.extend(taken);
                                }
                                btree_map::Entry::Vacant(p) => {
                                    p.insert(taken);
                                }
                            }
                        }
                    }
                }
            }
        }
        let mut r = Vec::with_capacity(results.len());
        for (k, v) in results.drain() {
            r.push((k.to_string(), v));
        }
        r
    }
}

impl GcovFunction {
    fn get_cycle_count(edges: &mut [GcovEdge], path: &[usize]) -> u64 {
        let mut count = std::u64::MAX;
        for e in path.iter() {
            count = cmp::min(count, edges[*e].cycles);
        }
        for e in path {
            edges[*e].cycles -= count;
        }
        count
    }

    fn unblock(
        block: usize,
        blocked: &mut SmallVec<[usize; 4]>,
        block_lists: &mut SmallVec<[SmallVec<[usize; 2]>; 2]>,
    ) {
        if let Some(i) = blocked.iter().position(|x| *x == block) {
            blocked.remove(i);
            for b in block_lists.remove(i) {
                GcovFunction::unblock(b, blocked, block_lists);
            }
        }
    }

    fn look_for_circuit(
        fun_edges: &mut [GcovEdge],
        fun_blocks: &[GcovBlock],
        v: usize,
        start: usize,
        path: &mut SmallVec<[usize; 4]>,
        blocked: &mut SmallVec<[usize; 4]>,
        block_lists: &mut SmallVec<[SmallVec<[usize; 2]>; 2]>,
        blocks: &[usize],
    ) -> (bool, u64) {
        let mut count = 0;
        blocked.push(v);
        block_lists.push(SmallVec::new());
        let mut found = false;
        let dsts = &fun_blocks[v].destination;

        for e in dsts {
            let w = fun_edges[*e].destination;
            if w >= start && blocks.iter().any(|x| *x == w) {
                path.push(*e);
                if w == start {
                    count += GcovFunction::get_cycle_count(fun_edges, path);
                    found = true;
                } else if blocked.iter().all(|x| *x != w) {
                    let (f, c) = GcovFunction::look_for_circuit(
                        fun_edges,
                        fun_blocks,
                        w,
                        start,
                        path,
                        blocked,
                        block_lists,
                        blocks,
                    );
                    count += c;
                    if f {
                        found = true;
                    }
                }
                path.pop();
            }
        }

        if found {
            GcovFunction::unblock(v, blocked, block_lists);
        } else {
            for e in dsts {
                let w = fun_edges[*e].destination;
                if w >= start || blocks.iter().any(|x| *x == w) {
                    if let Some(i) = blocked.iter().position(|x| *x == w) {
                        let list = &mut block_lists[i];
                        if list.iter().all(|x| *x != v) {
                            list.push(v);
                        }
                    }
                }
            }
        }

        (found, count)
    }

    fn get_cycles_count(
        fun_edges: &mut [GcovEdge],
        fun_blocks: &[GcovBlock],
        blocks: &[usize],
    ) -> u64 {
        let mut count: u64 = 0;
        let mut path: SmallVec<[usize; 4]> = SmallVec::new();
        let mut blocked: SmallVec<[usize; 4]> = SmallVec::new();
        let mut block_lists: SmallVec<[SmallVec<[usize; 2]>; 2]> = SmallVec::new();
        for b in blocks {
            path.clear();
            blocked.clear();
            block_lists.clear();
            let (_, c) = GcovFunction::look_for_circuit(
                fun_edges,
                fun_blocks,
                *b,
                *b,
                &mut path,
                &mut blocked,
                &mut block_lists,
                blocks,
            );
            count += c;
        }
        count
    }

    fn get_line_count(
        fun_edges: &mut [GcovEdge],
        fun_blocks: &[GcovBlock],
        blocks: &[usize],
    ) -> u64 {
        let mut count: u64 = 0;
        for b in blocks {
            let block = &fun_blocks[*b];
            if block.source.is_empty() {
                count += block.counter;
            } else {
                for e in &block.source {
                    let e = &fun_edges[*e];
                    let w = e.source;
                    if !blocks.iter().any(|x| *x == w) {
                        count += e.counter;
                    }
                }
            }
            for e in &block.destination {
                let e = &mut fun_edges[*e];
                e.cycles = e.counter;
            }
        }

        count + GcovFunction::get_cycles_count(fun_edges, fun_blocks, blocks)
    }

    fn add_line_count(&mut self) {
        self.executed = self.edges.first().unwrap().counter > 0;
        if self.executed {
            let mut lines_to_block: FxHashMap<u32, Vec<usize>> = FxHashMap::default();
            for block in &self.blocks {
                let n = block.no;
                for line in &block.lines {
                    match lines_to_block.entry(*line) {
                        hash_map::Entry::Occupied(vec) => {
                            vec.into_mut().push(n);
                        }
                        hash_map::Entry::Vacant(v) => {
                            v.insert(vec![n]);
                        }
                    }
                }
            }
            self.lines.reserve(lines_to_block.len());
            for (line, blocks) in lines_to_block {
                let count = if blocks.len() == 1 {
                    self.blocks[blocks[0]].counter
                } else {
                    GcovFunction::get_line_count(&mut self.edges, &self.blocks, &blocks)
                };
                self.lines.insert(line, count);
            }
        } else {
            for block in &self.blocks {
                for line in &block.lines {
                    self.lines.entry(*line).or_insert(0);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::defs::FunctionMap;

    fn get_input_string(path: &str) -> String {
        let path = PathBuf::from(path);
        let mut f = File::open(path).unwrap();
        let mut input = String::new();
        f.read_to_string(&mut input).unwrap();
        input
    }

    fn get_input_vec(path: &str) -> Vec<u8> {
        let path = PathBuf::from(path);
        let mut f = File::open(path).unwrap();
        let mut input = Vec::new();
        f.read_to_end(&mut input).unwrap();
        input
    }

    #[test]
    fn test_reader_gcno() {
        let mut gcno = GCNO::new();
        gcno.read(GcovReaderBuf::from("test/llvm/reader.gcno"))
            .unwrap();
        let output = format!("{:?}", gcno);
        let input = get_input_string("test/llvm/reader.gcno.0.dump");

        assert!(input == output);
    }

    #[test]
    fn test_reader_gcno_gcda() {
        let mut gcno = GCNO::new();
        gcno.read(GcovReaderBuf::from("test/llvm/reader.gcno"))
            .unwrap();
        gcno.read_gcda(GcovReaderBuf::from("test/llvm/reader.gcda"))
            .unwrap();
        let output = format!("{:?}", gcno);
        let input = get_input_string("test/llvm/reader.gcno.1.dump");

        assert!(input == output);
    }

    #[test]
    fn test_reader_gcno_gcda_gcda() {
        let mut gcno = GCNO::new();
        gcno.read(GcovReaderBuf::from("test/llvm/reader.gcno"))
            .unwrap();
        for _ in 0..2 {
            gcno.read_gcda(GcovReaderBuf::from("test/llvm/reader.gcda"))
                .unwrap();
        }
        let output = format!("{:?}", gcno);
        let input = get_input_string("test/llvm/reader.gcno.2.dump");

        assert!(input == output);
    }

    #[test]
    fn test_reader_gcno_counter() {
        let mut gcno = GCNO::new();
        gcno.read(GcovReaderBuf::from("test/llvm/reader.gcno"))
            .unwrap();
        let mut output = Vec::new();
        gcno.dump(
            &PathBuf::from("test/llvm/reader.c"),
            "reader.c",
            &mut output,
        )
        .unwrap();
        let input = get_input_vec("test/llvm/reader.c.0.gcov");

        assert!(input == output);
    }

    #[test]
    fn test_reader_gcno_gcda_counter() {
        let mut gcno = GCNO::new();
        gcno.read(GcovReaderBuf::from("test/llvm/reader.gcno"))
            .unwrap();
        gcno.read_gcda(GcovReaderBuf::from("test/llvm/reader.gcda"))
            .unwrap();
        let mut output = Vec::new();
        gcno.dump(
            &PathBuf::from("test/llvm/reader.c"),
            "reader.c",
            &mut output,
        )
        .unwrap();
        let input = get_input_vec("test/llvm/reader.c.1.gcov");

        assert!(input == output);
    }

    #[test]
    fn test_reader_gcno_gcda_gcda_counter() {
        let mut gcno = GCNO::new();
        gcno.read(GcovReaderBuf::from("test/llvm/reader.gcno"))
            .unwrap();
        for _ in 0..2 {
            gcno.read_gcda(GcovReaderBuf::from("test/llvm/reader.gcda"))
                .unwrap();
        }
        let mut output = Vec::new();
        gcno.dump(
            &PathBuf::from("test/llvm/reader.c"),
            "reader.c",
            &mut output,
        )
        .unwrap();
        let input = get_input_vec("test/llvm/reader.c.2.gcov");

        assert!(input == output);
    }

    #[test]
    fn test_reader_finalize_file() {
        let mut gcno = GCNO::new();
        gcno.read(GcovReaderBuf::from("test/llvm/file.gcno"))
            .unwrap();
        gcno.read_gcda(GcovReaderBuf::from("test/llvm/file.gcda"))
            .unwrap();
        let result = gcno.finalize(true);

        let mut lines: BTreeMap<u32, u64> = BTreeMap::new();
        lines.insert(2, 1);
        let mut functions: FunctionMap = FxHashMap::default();
        functions.insert(
            String::from("main"),
            Function {
                start: 1,
                executed: true,
            },
        );
        let branches: BTreeMap<u32, Vec<bool>> = BTreeMap::new();
        let expected = vec![(
            String::from("file.c"),
            CovResult {
                lines: lines,
                branches: branches,
                functions: functions,
            },
        )];

        assert_eq!(result, expected);
    }

    #[test]
    fn test_reader_finalize_file_branch() {
        let mut gcno = GCNO::new();
        gcno.read(GcovReaderBuf::from("test/llvm/file_branch.gcno"))
            .unwrap();
        gcno.read_gcda(GcovReaderBuf::from("test/llvm/file_branch.gcda"))
            .unwrap();
        let result = gcno.finalize(true);

        let mut lines: BTreeMap<u32, u64> = BTreeMap::new();
        [
            (2, 2),
            (3, 1),
            (4, 1),
            (5, 1),
            (6, 1),
            (8, 1),
            (10, 2),
            (13, 1),
            (14, 0),
            (16, 1),
            (18, 1),
            (21, 0),
            (22, 0),
            (24, 0),
            (25, 0),
            (26, 0),
            (28, 0),
            (32, 1),
        ]
        .iter()
        .for_each(|x| {
            lines.insert(x.0, x.1);
        });

        let mut functions: FunctionMap = FxHashMap::default();
        functions.insert(
            String::from("foo"),
            Function {
                start: 1,
                executed: true,
            },
        );
        functions.insert(
            String::from("bar"),
            Function {
                start: 12,
                executed: true,
            },
        );
        functions.insert(
            String::from("oof"),
            Function {
                start: 20,
                executed: false,
            },
        );
        functions.insert(
            String::from("main"),
            Function {
                start: 31,
                executed: true,
            },
        );
        let mut branches: BTreeMap<u32, Vec<bool>> = BTreeMap::new();
        [
            (2, vec![true, true]),
            (3, vec![true, false]),
            (13, vec![false, true]),
            (21, vec![false, false, false, false]),
        ]
        .iter()
        .for_each(|x| {
            branches.insert(x.0, x.1.clone());
        });

        let expected = vec![(
            String::from("file_branch.c"),
            CovResult {
                lines: lines,
                branches: branches,
                functions: functions,
            },
        )];

        assert_eq!(result, expected);
    }
}
