use rustc_hash::{FxHashMap, FxHashSet};
use smallvec::SmallVec;
use std::cmp;
use std::collections::{btree_map, hash_map, BTreeMap};
use std::convert::From;
use std::fmt::{Debug, Display, Formatter};
use std::fs::File;
use std::io::{BufReader, Error, Read, Write};
use std::marker::PhantomData;
use std::path::Path;
use std::path::PathBuf;
use std::result::Result;

use crate::defs::{CovResult, Function};

const GCOV_ARC_ON_TREE: u32 = 1 << 0;
const GCOV_ARC_FAKE: u32 = 1 << 1;
//const GCOV_ARC_FALLTHROUGH: u32 = 1 << 2;
const GCOV_TAG_FUNCTION: u32 = 0x0100_0000;
const GCOV_TAG_BLOCKS: u32 = 0x0141_0000;
const GCOV_TAG_ARCS: u32 = 0x0143_0000;
const GCOV_TAG_LINES: u32 = 0x0145_0000;
const GCOV_TAG_COUNTER_ARCS: u32 = 0x01a1_0000;
const GCOV_TAG_OBJECT_SUMMARY: u32 = 0xa100_0000;
const GCOV_TAG_PROGRAM_SUMMARY: u32 = 0xa300_0000;

#[derive(Debug)]
pub enum GcovReaderError {
    Io(std::io::Error),
    Str(String),
}

impl From<Error> for GcovReaderError {
    fn from(err: Error) -> GcovReaderError {
        GcovReaderError::Str(format!("Reader error: {}", err))
    }
}

pub trait Endian {
    fn is_little_endian() -> bool;
}

pub trait GcovReader<E: Endian> {
    fn read_string(&mut self) -> Result<String, GcovReaderError>;
    fn read_u32(&mut self) -> Result<u32, GcovReaderError>;
    fn read_counter(&mut self) -> Result<u64, GcovReaderError>;
    fn get_version(&self, buf: &[u8]) -> u32;
    fn read_version(&mut self) -> Result<u32, GcovReaderError>;
    fn get_pos(&self) -> usize;
    fn get_stem(&self) -> &str;
    fn skip_u32(&mut self) -> Result<(), GcovReaderError>;
    fn skip(&mut self, len: usize) -> Result<(), GcovReaderError>;
    fn is_little_endian(&self) -> bool {
        E::is_little_endian()
    }
}

pub struct LittleEndian;
impl Endian for LittleEndian {
    fn is_little_endian() -> bool {
        true
    }
}

pub struct BigEndian;
impl Endian for BigEndian {
    fn is_little_endian() -> bool {
        false
    }
}

enum FileType {
    Gcno,
    Gcda,
}

#[derive(Default)]
pub struct Gcno {
    version: u32,
    checksum: u32,
    #[allow(dead_code)]
    cwd: Option<String>,
    programcounts: u32,
    runcounts: u32,
    functions: Vec<GcovFunction>,
    ident_to_fun: FxHashMap<u32, usize>,
}

#[derive(Debug)]
struct GcovFunction {
    identifier: u32,
    start_line: u32,
    #[allow(dead_code)]
    start_column: u32,
    end_line: u32,
    #[allow(dead_code)]
    end_column: u32,
    #[allow(dead_code)]
    artificial: u32,
    line_checksum: u32,
    cfg_checksum: u32,
    file_name: String,
    name: String,
    blocks: SmallVec<[GcovBlock; 16]>,
    edges: SmallVec<[GcovEdge; 16]>,
    real_edge_count: usize,
    lines: FxHashMap<u32, u64>,
    executed: bool,
}

#[derive(Debug)]
struct GcovBlock {
    no: usize,
    source: SmallVec<[usize; 2]>,
    destination: SmallVec<[usize; 2]>,
    lines: SmallVec<[u32; 16]>,
    line_max: u32,
    counter: u64,
}

#[derive(Debug)]
struct GcovEdge {
    source: usize,
    destination: usize,
    flags: u32,
    counter: u64,
    cycles: u64,
}

impl GcovEdge {
    fn is_on_tree(&self) -> bool {
        (self.flags & GCOV_ARC_ON_TREE) != 0
    }

    fn is_fake(&self) -> bool {
        (self.flags & GCOV_ARC_FAKE) != 0
    }

    fn get_tree_mark(&self) -> &'static str {
        if self.is_on_tree() {
            "*"
        } else {
            ""
        }
    }
}

impl GcovBlock {
    fn new(no: usize) -> Self {
        Self {
            no,
            source: SmallVec::new(),
            destination: SmallVec::new(),
            lines: SmallVec::new(),
            line_max: 0,
            counter: 0,
        }
    }
}

pub struct GcovReaderBuf<E: Endian> {
    stem: String,
    buffer: Vec<u8>,
    pos: usize,
    phantom: PhantomData<E>,
}

macro_rules! read_u {
    ($ty: ty, $buf: expr) => {{
        let size = std::mem::size_of::<$ty>();
        let start = $buf.pos;
        $buf.pos += size;
        if $buf.pos <= $buf.buffer.len() {
            let val: $ty = unsafe {
                // data are aligned so it's safe to do that
                #[allow(clippy::transmute_ptr_to_ptr)]
                *std::mem::transmute::<*const u8, *const $ty>($buf.buffer[start..].as_ptr())
            };
            Ok(if $buf.is_little_endian() {
                val.to_le()
            } else {
                val.to_be()
            })
        } else {
            Err(GcovReaderError::Str(format!(
                "Not enough data in buffer: cannot read integer in {}",
                $buf.get_stem()
            )))
        }
    }};
}

macro_rules! skip {
    ($size: expr, $buf: expr) => {{
        $buf.pos += $size;
        if $buf.pos < $buf.buffer.len() {
            Ok(())
        } else {
            Err(GcovReaderError::Str(format!(
                "Not enough data in buffer: cannot skip {} bytes in {}",
                $size,
                $buf.get_stem()
            )))
        }
    }};
}

impl<E: Endian> GcovReaderBuf<E> {
    pub fn new(stem: &str, buffer: Vec<u8>) -> GcovReaderBuf<E> {
        GcovReaderBuf {
            stem: stem.to_string(),
            buffer,
            pos: 4, // we already read gcno or gcda
            phantom: PhantomData,
        }
    }
}

impl<E: Endian> GcovReader<E> for GcovReaderBuf<E> {
    fn get_stem(&self) -> &str {
        &self.stem
    }

    #[inline(always)]
    fn skip_u32(&mut self) -> Result<(), GcovReaderError> {
        skip!(std::mem::size_of::<u32>(), self)
    }

    #[inline(always)]
    fn skip(&mut self, len: usize) -> Result<(), GcovReaderError> {
        skip!(len, self)
    }

    fn read_string(&mut self) -> Result<String, GcovReaderError> {
        let len = read_u!(u32, self)?;
        if len == 0 {
            return Ok("".to_string());
        }
        let len = len as usize * 4;
        let start = self.pos;
        self.pos += len;
        if self.pos <= self.buffer.len() {
            let bytes = &self.buffer[start..self.pos];
            let i = len - bytes.iter().rev().position(|&x| x != 0).unwrap();
            Ok(unsafe { std::str::from_utf8_unchecked(&bytes[..i]).to_string() })
        } else {
            Err(GcovReaderError::Str(format!(
                "Not enough data in buffer: cannot read string in {}",
                self.get_stem()
            )))
        }
    }

    #[inline(always)]
    fn read_u32(&mut self) -> Result<u32, GcovReaderError> {
        read_u!(u32, self)
    }

    #[inline(always)]
    fn read_counter(&mut self) -> Result<u64, GcovReaderError> {
        let lo = read_u!(u32, self)?;
        let hi = read_u!(u32, self)?;

        Ok(u64::from(hi) << 32 | u64::from(lo))
    }

    fn get_version(&self, buf: &[u8]) -> u32 {
        if buf[2] >= b'A' {
            100 * u32::from(buf[2] - b'A')
                + 10 * u32::from(buf[1] - b'0')
                + u32::from(buf[0] - b'0')
        } else {
            10 * u32::from(buf[2] - b'0') + u32::from(buf[0] - b'0')
        }
    }

    fn read_version(&mut self) -> Result<u32, GcovReaderError> {
        let i = self.pos;
        if i + 4 <= self.buffer.len() {
            self.pos += 4;
            if self.is_little_endian() && self.buffer[i] == b'*' {
                Ok(self.get_version(&self.buffer[i + 1..i + 4]))
            } else if !self.is_little_endian() && self.buffer[i + 3] == b'*' {
                let buf = [self.buffer[i + 2], self.buffer[i + 1], self.buffer[i]];
                Ok(self.get_version(&buf))
            } else {
                let bytes = &self.buffer[i..i + 4];
                Err(GcovReaderError::Str(format!(
                    "Unexpected version: {} in {}",
                    String::from_utf8_lossy(bytes),
                    self.get_stem()
                )))
            }
        } else {
            Err(GcovReaderError::Str(format!(
                "Not enough data in buffer: Cannot read version in {}",
                self.get_stem()
            )))
        }
    }

    #[inline(always)]
    fn get_pos(&self) -> usize {
        self.pos
    }
}

impl Display for GcovReaderError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            GcovReaderError::Io(e) => write!(f, "{}", e),
            GcovReaderError::Str(e) => write!(f, "{}", e),
        }
    }
}

impl Debug for Gcno {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        for fun in &self.functions {
            writeln!(
                f,
                "===== {} ({}) @ {}:{}",
                fun.name, fun.identifier, fun.file_name, fun.start_line
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
                        write!(
                            f,
                            "{}{} ({}), ",
                            edge.get_tree_mark(),
                            edge.destination,
                            edge.counter
                        )?;
                    }
                    let edge = &fun.edges[*last];
                    writeln!(
                        f,
                        "{}{} ({}), ",
                        edge.get_tree_mark(),
                        edge.destination,
                        edge.counter
                    )?;
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

impl Gcno {
    pub fn new() -> Self {
        Gcno {
            version: 0,
            checksum: 0,
            cwd: None,
            programcounts: 0,
            runcounts: 0,
            functions: Vec::new(),
            ident_to_fun: FxHashMap::default(),
        }
    }

    fn guess_endianness(
        mut typ: [u8; 4],
        buffer: &[u8],
        stem: &str,
    ) -> Result<bool, GcovReaderError> {
        if 4 <= buffer.len() {
            let bytes = &buffer[..4];
            if bytes == typ {
                // Little endian
                Ok(true)
            } else {
                typ.reverse();
                if bytes == typ {
                    // Big endian
                    Ok(false)
                } else {
                    Err(GcovReaderError::Str(format!(
                        "Unexpected file type: {} in {}.",
                        std::str::from_utf8(bytes).unwrap(),
                        stem
                    )))
                }
            }
        } else {
            Err(GcovReaderError::Str(format!(
                "Not enough data in buffer: Cannot compare types in {}",
                stem
            )))
        }
    }

    fn read(&mut self, typ: FileType, buf: Vec<u8>, stem: &str) -> Result<(), GcovReaderError> {
        let little_endian = Self::guess_endianness(
            match typ {
                FileType::Gcno => *b"oncg",
                _ => *b"adcg",
            },
            &buf,
            stem,
        )?;
        if little_endian {
            match typ {
                FileType::Gcno => self.read_gcno(GcovReaderBuf::<LittleEndian>::new(stem, buf)),
                _ => self.read_gcda(GcovReaderBuf::<LittleEndian>::new(stem, buf)),
            }
        } else {
            match typ {
                FileType::Gcno => self.read_gcno(GcovReaderBuf::<BigEndian>::new(stem, buf)),
                _ => self.read_gcda(GcovReaderBuf::<BigEndian>::new(stem, buf)),
            }
        }
    }

    pub fn compute(
        stem: &str,
        gcno_buf: Vec<u8>,
        gcda_bufs: Vec<Vec<u8>>,
        branch_enabled: bool,
    ) -> Result<Vec<(String, CovResult)>, GcovReaderError> {
        let mut gcno = Self::new();
        gcno.read(FileType::Gcno, gcno_buf, stem)?;
        for gcda_buf in gcda_bufs.into_iter() {
            gcno.read(FileType::Gcda, gcda_buf, stem)?;
        }
        gcno.stop();
        Ok(gcno.finalize(branch_enabled))
    }

    pub fn stop(&mut self) {
        for fun in self.functions.iter_mut() {
            fun.count_on_tree(self.version);
        }
    }

    pub fn read_gcno<E: Endian, T: GcovReader<E>>(
        &mut self,
        mut reader: T,
    ) -> Result<(), GcovReaderError> {
        self.version = reader.read_version()?;
        self.checksum = reader.read_u32()?;
        if self.version >= 90 {
            self.cwd = Some(reader.read_string()?);
        }
        if self.version >= 80 {
            // hasUnexecutedBlocks
            reader.skip_u32()?;
        }

        self.read_functions(&mut reader)
    }

    fn read_edges<E: Endian, T: GcovReader<E> + Sized>(
        fun: &mut GcovFunction,
        count: u32,
        reader: &mut T,
    ) -> Result<(), GcovReaderError> {
        let edges = &mut fun.edges;
        let blocks = &mut fun.blocks;
        let count = ((count - 1) / 2) as usize;
        let block_no = reader.read_u32()? as usize;
        if block_no <= blocks.len() {
            blocks[block_no].destination.reserve(count);
            for _ in 0..count {
                let dst_block_no = reader.read_u32()? as usize;
                let flags = reader.read_u32()?;
                let edges_count = edges.len();
                edges.push(GcovEdge {
                    source: block_no,
                    destination: dst_block_no,
                    flags,
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
                if (flags & GCOV_ARC_ON_TREE) == 0 {
                    fun.real_edge_count += 1;
                }
            }
        } else {
            return Err(GcovReaderError::Str(format!(
                "Unexpected block number: {} (in {}) in {}",
                block_no,
                fun.name,
                reader.get_stem()
            )));
        }
        Ok(())
    }

    fn read_lines<E: Endian, T: GcovReader<E> + Sized>(
        fun: &mut GcovFunction,
        version: u32,
        reader: &mut T,
    ) -> Result<(), GcovReaderError> {
        let block_no = reader.read_u32()? as usize;
        let mut must_take = true;
        if block_no <= fun.blocks.len() {
            let block = &mut fun.blocks[block_no];
            let lines = &mut block.lines;
            loop {
                let line = reader.read_u32()?;
                if line != 0 {
                    if !must_take
                        || (version >= 80 && (line < fun.start_line || line > fun.end_line))
                    {
                        continue;
                    }

                    lines.push(line);
                    if line > block.line_max {
                        block.line_max = line;
                    }
                } else {
                    let filename = reader.read_string()?;
                    if filename.is_empty() {
                        break;
                    }
                    must_take = filename == fun.file_name;
                    // some lines in the block can come from an other file
                    // TODO
                }
            }
        } else {
            return Err(GcovReaderError::Str(format!(
                "Unexpected block number: {} (in {}).",
                block_no, fun.name
            )));
        }
        Ok(())
    }

    fn read_blocks<E: Endian, T: GcovReader<E> + Sized>(
        fun: &mut GcovFunction,
        length: u32,
        version: u32,
        reader: &mut T,
    ) -> Result<(), GcovReaderError> {
        if version < 80 {
            let length = length as usize;
            for no in 0..length {
                // flags, currently unused
                reader.skip_u32()?;
                fun.blocks.push(GcovBlock::new(no));
            }
        } else {
            let length = reader.read_u32()? as usize;
            for no in 0..length {
                fun.blocks.push(GcovBlock::new(no));
            }
        }

        Ok(())
    }

    fn read_functions<E: Endian, T: GcovReader<E> + Sized>(
        &mut self,
        reader: &mut T,
    ) -> Result<(), GcovReaderError> {
        while let Ok(tag) = reader.read_u32() {
            if tag == 0 {
                break;
            }
            let length = reader.read_u32()?;

            if tag == GCOV_TAG_FUNCTION {
                let identifier = reader.read_u32()?;
                let line_checksum = reader.read_u32()?;
                let cfg_checksum = if self.version >= 47 {
                    reader.read_u32()?
                } else {
                    0
                };

                let name = reader.read_string()?;
                let (artificial, file_name, start_line, start_column, end_line, end_column) =
                    if self.version < 80 {
                        (0, reader.read_string()?, reader.read_u32()?, 0, 0, 0)
                    } else {
                        (
                            reader.read_u32()?,
                            reader.read_string()?,
                            reader.read_u32()?,
                            reader.read_u32()?,
                            reader.read_u32()?,
                            if self.version >= 90 {
                                reader.read_u32()?
                            } else {
                                0
                            },
                        )
                    };
                let pos = self.functions.len();
                self.functions.push(GcovFunction {
                    identifier,
                    start_line,
                    start_column,
                    end_line,
                    end_column,
                    artificial,
                    line_checksum,
                    cfg_checksum,
                    file_name,
                    name,
                    blocks: SmallVec::new(),
                    edges: SmallVec::new(),
                    real_edge_count: 0,
                    lines: FxHashMap::default(),
                    executed: false,
                });
                self.ident_to_fun.insert(identifier, pos);
            } else if tag == GCOV_TAG_BLOCKS {
                let fun = if let Some(fun) = self.functions.last_mut() {
                    fun
                } else {
                    continue;
                };
                Gcno::read_blocks(fun, length, self.version, reader)?;
            } else if tag == GCOV_TAG_ARCS {
                let fun = if let Some(fun) = self.functions.last_mut() {
                    fun
                } else {
                    continue;
                };
                Gcno::read_edges(fun, length, reader)?;
            } else if tag == GCOV_TAG_LINES {
                let fun = if let Some(fun) = self.functions.last_mut() {
                    fun
                } else {
                    continue;
                };
                Gcno::read_lines(fun, self.version, reader)?;
            }
        }
        Ok(())
    }

    pub fn read_gcda<E: Endian, T: GcovReader<E>>(
        &mut self,
        mut reader: T,
    ) -> Result<(), GcovReaderError> {
        let version = reader.read_version()?;
        if version != self.version {
            Err(GcovReaderError::Str(format!(
                "GCOV versions do not match in {}",
                reader.get_stem()
            )))
        } else {
            let checksum = reader.read_u32()?;
            if checksum != self.checksum {
                Err(GcovReaderError::Str(format!(
                    "File checksums do not match: {} != {} in {}",
                    self.checksum,
                    checksum,
                    reader.get_stem()
                )))
            } else {
                let mut current_fun_id: Option<usize> = None;
                while let Ok(tag) = reader.read_u32() {
                    if tag == 0 {
                        break;
                    }
                    let length = reader.read_u32()?;
                    let mut pos = reader.get_pos();

                    if tag == GCOV_TAG_FUNCTION {
                        if length == 0 {
                            continue;
                        }

                        if length == 1 {
                            return Err(GcovReaderError::Str(format!(
                                "Invalid header length in {}",
                                reader.get_stem()
                            )));
                        }

                        let id = reader.read_u32()?;
                        let line_sum = reader.read_u32()?;
                        let cfg_sum = if version >= 47 { reader.read_u32()? } else { 0 };
                        if let Some(fun_id) = self.ident_to_fun.get(&id) {
                            let fun = &self.functions[*fun_id];
                            if line_sum != fun.line_checksum || cfg_sum != fun.cfg_checksum {
                                return Err(GcovReaderError::Str(format!(
                                    "Checksum mismatch ({}, {}) != ({}, {}) in {}",
                                    line_sum,
                                    fun.line_checksum,
                                    cfg_sum,
                                    fun.cfg_checksum,
                                    reader.get_stem()
                                )));
                            }
                            current_fun_id = Some(*fun_id);
                        } else {
                            return Err(GcovReaderError::Str(format!(
                                "Invalid function identifier {} in {}",
                                id,
                                reader.get_stem()
                            )));
                        }
                    } else if tag == GCOV_TAG_COUNTER_ARCS {
                        let fun = if let Some(fun_id) = &current_fun_id {
                            &mut self.functions[*fun_id]
                        } else {
                            continue;
                        };

                        let count = length;
                        let edges = &mut fun.edges;
                        if fun.real_edge_count as u32 != count / 2 {
                            return Err(GcovReaderError::Str(format!(
                                "Unexpected number of edges (in {}) in {}",
                                fun.name,
                                reader.get_stem()
                            )));
                        }

                        for edge in edges.iter_mut() {
                            if edge.is_on_tree() {
                                continue;
                            }
                            let counter = reader.read_counter()?;
                            edge.counter += counter;
                            fun.blocks[edge.source].counter += counter;
                        }
                    } else if tag == GCOV_TAG_OBJECT_SUMMARY {
                        let runcounts = reader.read_u32()?;
                        reader.skip_u32()?;
                        self.runcounts += if length == 9 {
                            reader.read_u32()?
                        } else {
                            runcounts
                        };
                    } else if tag == GCOV_TAG_PROGRAM_SUMMARY {
                        if length > 0 {
                            reader.skip_u32()?;
                            reader.skip_u32()?;
                            self.runcounts += reader.read_u32()?;
                        }
                        self.programcounts += 1;
                    }
                    pos += 4 * (length as usize);
                    reader.skip(pos - reader.get_pos())?;
                }

                Ok(())
            }
        }
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
        path: &Path,
        file_name: &str,
        writer: &mut dyn Write,
    ) -> Result<(), GcovReaderError> {
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
            i32::from(has_runs)
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
                    start: fun.start_line,
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
                for block in &fun.blocks {
                    let line = if block.lines.is_empty() {
                        let mut line_max = 0;
                        for edge_no in block.source.iter() {
                            let source = &fun.blocks[fun.edges[*edge_no].source];
                            line_max = line_max.max(source.line_max);
                        }
                        line_max
                    } else {
                        block.line_max
                    };
                    if line == 0 {
                        continue;
                    }

                    let taken: Vec<_> = block
                        .destination
                        .iter()
                        .filter_map(|no| {
                            let edge = &fun.edges[*no];
                            if edge.is_fake() {
                                None
                            } else {
                                Some(fun.executed && edge.counter > 0)
                            }
                        })
                        .collect();
                    if taken.len() <= 1 {
                        continue;
                    }
                    match res.branches.entry(line) {
                        btree_map::Entry::Occupied(c) => {
                            let v = c.into_mut();
                            v.extend_from_slice(&taken);
                        }
                        btree_map::Entry::Vacant(p) => {
                            p.insert(taken);
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
        let mut count = u64::MAX;
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
            if block.no == 0 {
                count = block
                    .destination
                    .iter()
                    .fold(count, |acc, e| acc + fun_edges[*e].counter);
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

    fn count_on_tree(&mut self, version: u32) {
        if self.blocks.len() >= 2 {
            let src_no = 0;
            let sink_no = if version < 48 {
                self.blocks.len() - 1
            } else {
                1
            };
            let edges_count = self.edges.len();
            self.edges.push(GcovEdge {
                source: sink_no,
                destination: src_no,
                flags: GCOV_ARC_ON_TREE,
                counter: 0,
                cycles: 0,
            });
            let i = match self.blocks[sink_no]
                .destination
                .binary_search_by(|x| self.edges[*x].destination.cmp(&src_no))
            {
                Ok(i) => i,
                Err(i) => i,
            };
            self.blocks[sink_no].destination.insert(i, edges_count);
            self.blocks[src_no].source.push(edges_count);

            let mut visited = FxHashSet::default();
            for block_no in 0..self.blocks.len() {
                Self::propagate_counts(&self.blocks, &mut self.edges, block_no, None, &mut visited);
            }
            for edge in self.edges.iter().rev() {
                if edge.is_on_tree() {
                    self.blocks[edge.source].counter += edge.counter;
                }
            }
        }
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

    fn propagate_counts(
        blocks: &SmallVec<[GcovBlock; 16]>,
        edges: &mut SmallVec<[GcovEdge; 16]>,
        block_no: usize,
        pred_arc: Option<usize>,
        visited: &mut FxHashSet<usize>,
    ) -> u64 {
        // For each basic block, the sum of incoming edge counts equals the sum of
        // outgoing edge counts by Kirchoff's circuit law. If the unmeasured arcs form a
        // spanning tree, the count for each unmeasured arc (GCOV_ARC_ON_TREE) can be
        // uniquely identified.

        // Prevent infinite recursion
        if !visited.insert(block_no) {
            return 0;
        }
        let mut positive_excess = 0;
        let mut negative_excess = 0;
        let block = &blocks[block_no];
        for edge_id in block.source.iter() {
            if pred_arc.map_or(true, |x| *edge_id != x) {
                let edge = &edges[*edge_id];
                positive_excess += if edge.is_on_tree() {
                    let source = edge.source;
                    Self::propagate_counts(blocks, edges, source, Some(*edge_id), visited)
                } else {
                    edge.counter
                };
            }
        }
        for edge_id in block.destination.iter() {
            if pred_arc.map_or(true, |x| *edge_id != x) {
                let edge = &edges[*edge_id];
                negative_excess += if edge.is_on_tree() {
                    let destination = edge.destination;
                    Self::propagate_counts(blocks, edges, destination, Some(*edge_id), visited)
                } else {
                    edge.counter
                };
            }
        }
        let excess = if positive_excess >= negative_excess {
            positive_excess - negative_excess
        } else {
            negative_excess - positive_excess
        };
        if let Some(id) = pred_arc {
            let edge = &mut edges[id];
            edge.counter = excess;
        }
        excess
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::defs::FunctionMap;

    fn from_path(gcno: &mut Gcno, typ: FileType, path: &str) {
        let path = PathBuf::from(path);
        let mut f = File::open(&path).unwrap();
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        gcno.read(typ, buf, path.to_str().unwrap()).unwrap();
    }

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
        let mut gcno = Gcno::new();
        from_path(&mut gcno, FileType::Gcno, "test/llvm/reader.gcno");
        let output = format!("{:?}", gcno);
        let input = get_input_string("test/llvm/reader.gcno.0.dump");

        assert_eq!(output, input);
    }

    #[test]
    fn test_reader_gcno_gcda() {
        let mut gcno = Gcno::new();
        from_path(&mut gcno, FileType::Gcno, "test/llvm/reader.gcno");
        from_path(&mut gcno, FileType::Gcda, "test/llvm/reader.gcda");
        gcno.stop();
        let output = format!("{:?}", gcno);
        let input = get_input_string("test/llvm/reader.gcno.1.dump");

        assert_eq!(output, input);
    }

    #[test]
    fn test_reader_gcno_gcda_gcc6() {
        let mut gcno = Gcno::new();
        from_path(&mut gcno, FileType::Gcno, "test/reader_gcc-6.gcno");
        from_path(&mut gcno, FileType::Gcda, "test/reader_gcc-6.gcda");
        gcno.stop();
        let output = format!("{:?}", gcno);
        let input = get_input_string("test/reader_gcc-6.gcno.1.dump");

        assert_eq!(output, input);
    }

    #[test]
    fn test_reader_gcno_gcda_gcc7() {
        let mut gcno = Gcno::new();
        from_path(&mut gcno, FileType::Gcno, "test/reader_gcc-7.gcno");
        from_path(&mut gcno, FileType::Gcda, "test/reader_gcc-7.gcda");
        gcno.stop();
        let output = format!("{:?}", gcno);
        let input = get_input_string("test/reader_gcc-7.gcno.1.dump");

        assert_eq!(output, input);
    }

    #[test]
    fn test_reader_gcno_gcda_gcc8() {
        let mut gcno = Gcno::new();
        from_path(&mut gcno, FileType::Gcno, "test/reader_gcc-8.gcno");
        from_path(&mut gcno, FileType::Gcda, "test/reader_gcc-8.gcda");
        gcno.stop();
        let output = format!("{:?}", gcno);
        let input = get_input_string("test/reader_gcc-8.gcno.1.dump");

        assert_eq!(output, input);
    }

    #[test]
    fn test_reader_gcno_gcda_gcc9() {
        let mut gcno = Gcno::new();
        from_path(&mut gcno, FileType::Gcno, "test/reader_gcc-9.gcno");
        from_path(&mut gcno, FileType::Gcda, "test/reader_gcc-9.gcda");
        gcno.stop();
        let output = format!("{:?}", gcno);
        let input = get_input_string("test/reader_gcc-9.gcno.1.dump");

        assert_eq!(output, input);
    }

    #[test]
    fn test_reader_gcno_gcda_gcc10() {
        let mut gcno = Gcno::new();
        from_path(&mut gcno, FileType::Gcno, "test/reader_gcc-10.gcno");
        from_path(&mut gcno, FileType::Gcda, "test/reader_gcc-10.gcda");
        gcno.stop();
        let output = format!("{:?}", gcno);
        let input = get_input_string("test/reader_gcc-10.gcno.1.dump");

        assert_eq!(output, input);
    }

    #[test]
    fn test_reader_gcno_gcda_gcda() {
        let mut gcno = Gcno::new();
        from_path(&mut gcno, FileType::Gcno, "test/llvm/reader.gcno");
        for _ in 0..2 {
            from_path(&mut gcno, FileType::Gcda, "test/llvm/reader.gcda");
        }
        gcno.stop();
        let output = format!("{:?}", gcno);
        let input = get_input_string("test/llvm/reader.gcno.2.dump");

        assert_eq!(output, input);
    }

    #[test]
    fn test_reader_gcno_counter() {
        let mut gcno = Gcno::new();
        from_path(&mut gcno, FileType::Gcno, "test/llvm/reader.gcno");
        gcno.stop();
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
        let mut gcno = Gcno::new();
        from_path(&mut gcno, FileType::Gcno, "test/llvm/reader.gcno");
        from_path(&mut gcno, FileType::Gcda, "test/llvm/reader.gcda");
        gcno.stop();
        let mut output = Vec::new();
        gcno.dump(
            &PathBuf::from("test/llvm/reader.c"),
            "reader.c",
            &mut output,
        )
        .unwrap();
        let input = get_input_vec("test/llvm/reader.c.1.gcov");

        assert_eq!(output, input);
    }

    #[test]
    fn test_reader_gcno_gcda_gcda_counter() {
        let mut gcno = Gcno::new();
        from_path(&mut gcno, FileType::Gcno, "test/llvm/reader.gcno");
        for _ in 0..2 {
            from_path(&mut gcno, FileType::Gcda, "test/llvm/reader.gcda");
        }
        gcno.stop();
        let mut output = Vec::new();
        gcno.dump(
            &PathBuf::from("test/llvm/reader.c"),
            "reader.c",
            &mut output,
        )
        .unwrap();
        let input = get_input_vec("test/llvm/reader.c.2.gcov");

        assert_eq!(output, input);
    }

    #[test]
    fn test_reader_finalize_file() {
        let mut gcno = Gcno::new();
        from_path(&mut gcno, FileType::Gcno, "test/llvm/file.gcno");
        from_path(&mut gcno, FileType::Gcda, "test/llvm/file.gcda");
        gcno.stop();
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
                lines,
                branches,
                functions,
            },
        )];

        assert_eq!(result, expected);
    }

    #[test]
    fn test_reader_finalize_file_branch() {
        let mut gcno = Gcno::new();
        from_path(&mut gcno, FileType::Gcno, "test/llvm/file_branch.gcno");
        from_path(&mut gcno, FileType::Gcda, "test/llvm/file_branch.gcda");
        gcno.stop();
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
                lines,
                branches,
                functions,
            },
        )];

        assert_eq!(result, expected);
    }
}
