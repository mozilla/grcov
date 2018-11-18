use std::cmp;
use byteorder::{LittleEndian, ReadBytesExt};
use std::fs::File;
use std::io::{BufReader, Error, Read, Write};
use std::convert::From;
use std::path::PathBuf;
use std::result::Result;
use std::fmt::{Debug, Display, Formatter};
use std::collections::{btree_map, hash_map, BTreeMap, HashMap};

use defs::{CovResult, Function};

struct GCNO {
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
    blocks: Vec<GcovBlock>,
    edges: Vec<GcovEdge>,
    lines: HashMap<u32, u64>,
    executed: bool,
}

#[derive(Debug)]
struct GcovBlock {
    no: usize,
    flags: u32,
    source: Vec<usize>,
    destination: Vec<usize>,
    lines: Vec<u32>,
    line_max: u32,
    counter: u64,
}

#[derive(Debug)]
struct GcovEdge {
    flags: u32,
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
        GcovError::Str(format!("Reader error: {:?}", err))
    }
}

impl Display for GcovError {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        match self {
            GcovError::Io(e) => {
                write!(f, "{:?}", e)
            },
            GcovError::Str(e) => {
                write!(f, "{}", e)
            }
        }
    }
}

impl Debug for GCNO {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        // TODO: print version
        //writeln!(f, "version: {}", self.version)?;
        for fun in &self.functions {
            writeln!(f, "===== {} ({}) @ {}:{}", fun.name, fun.identifier, fun.file_name, fun.line_number)?;
            for block in &fun.blocks {
                writeln!(f, "Block : {} Counter : {}", block.no, block.counter)?;
                if let Some((last, elmts)) = block.source.split_last() {
                    write!(f, "\tSource Edges : ")?;
                    for edge in elmts.iter().map(|i| &fun.edges[*i]) {
                        write!(f, "{} ({}), ", edge.source, edge.counter)?;
                    }
                    let edge = &fun.edges[*last];
                    // TODO: remove the final comma
                    writeln!(f, "{} ({}), ", edge.source, edge.counter)?;
                }
                if let Some((last, elmts)) = block.destination.split_last() {
                    write!(f, "\tDestination Edges : ")?;
                    for edge in elmts.iter().map(|i| &fun.edges[*i]) {
                        write!(f, "{} ({}), ", edge.destination, edge.counter)?;
                    }
                    let edge = &fun.edges[*last];
                    // TODO: remove the final comma
                    writeln!(f, "{} ({}), ", edge.destination, edge.counter)?;
                }
                if let Some((last, elmts)) = block.lines.split_last() {
                    write!(f, "\tLines : ")?;
                    for i in elmts {
                        // TODO: add space after comma
                        write!(f, "{},", i)?;
                    }
                    // TODO: remove the final comma
                    writeln!(f, "{},", last)?;
                }
            }
            // TODO: add a newline after a function 
            //writeln!(f)?;
        }
        //writeln!(f, "");

        Ok(())
    }
}

impl GCNO {

    fn read_string(reader: &mut Read) -> Result<String, GcovError> {
        let mut len = reader.read_u32::<LittleEndian>()?;
        while len == 0 {
            len = reader.read_u32::<LittleEndian>()?;
        }
        let len = len as usize * 4;
        let mut bytes: Vec<u8> = vec![0; len];
        reader.read_exact(&mut bytes)?;
        let i = bytes.len() - bytes.iter().rev().position(|&x| x != 0).unwrap();
        unsafe {
            Ok(std::str::from_utf8_unchecked(&bytes[..i]).to_string())
        }
    }

    fn read_counter(reader: &mut Read) -> Result<u64, GcovError> {
        let lo = reader.read_u32::<LittleEndian>()?;
        let hi = reader.read_u32::<LittleEndian>()?;

        Ok(u64::from(hi) << 32 | u64::from(lo))
    }

    fn check_type(reader: &mut Read, typ: [u8; 4]) -> Result<(), GcovError> {
        let mut bytes: [u8; 4] = [0; 4];
        reader.read_exact(&mut bytes)?;
        if bytes == typ {
            Ok(())
        } else {
            Err(GcovError::Str(format!("Unexpected file type: {}", std::str::from_utf8(&bytes).unwrap())))
        }
    }

    fn read_version(reader: &mut Read) -> Result<u32, GcovError> {
        let mut bytes: [u8; 4] = [0; 4];
        reader.read_exact(&mut bytes)?;
        if bytes[0] == b'*' {
            let version = u32::from(bytes[1] - b'0') +
                10 * (u32::from(bytes[2] - b'0') +
                      u32::from(bytes[3] - b'0') * 10);

            Ok(version)
        } else {
            Err(GcovError::Str(format!("Unexpected version: {}", std::str::from_utf8(&bytes).unwrap())))
        }
    }

    fn new() -> GCNO {
        GCNO { version: 0,
               checksum: 0,
               runcounts: 0,
               functions: Vec::new(),
        }
    }

    fn read(&mut self, reader: &mut Read) -> Result<(), GcovError> {
        GCNO::check_type(reader, [b'o', b'n', b'c', b'g'])?;
        self.version = GCNO::read_version(reader)?;
        self.checksum = reader.read_u32::<LittleEndian>()?;
        self.read_functions(reader)
    }

    fn read_edges(fun: &mut GcovFunction, reader: &mut Read) -> Result<u32, GcovError> {
        let mut tag = reader.read_u32::<LittleEndian>()?;
        let edges = &mut fun.edges;
        let blocks = &mut fun.blocks;
        let mut edges_count = 0;
        while tag == 0x0143_0000 {
            let count = reader.read_u32::<LittleEndian>()?;
            let count = ((count - 1) / 2) as usize;
            let block_no = reader.read_u32::<LittleEndian>()? as usize;
            if block_no <= blocks.len() {
                for _ in 0..count {
                    let dst_block_no = reader.read_u32::<LittleEndian>()? as usize;
                    let flags = reader.read_u32::<LittleEndian>()?;
                    edges.push(GcovEdge {
                        flags,
                        source: block_no,
                        destination: dst_block_no,
                        counter: 0,
                        cycles: 0,
                    });
                    blocks[block_no].destination.push(edges_count);
                    blocks[dst_block_no].source.push(edges_count);
                    edges_count += 1;
                }
                tag = reader.read_u32::<LittleEndian>()?;
            } else {
                return Err(GcovError::Str(format!("Unexpected block number: {} (in {}).", block_no, fun.name)))
            }
        }
        Ok(tag)
    }

    fn read_lines(fun: &mut GcovFunction, reader: &mut Read, tag: u32) -> Result<u32, GcovError> {
        let mut tag = tag;
        while tag == 0x0145_0000 {
            let len = reader.read_u32::<LittleEndian>()? - 3;
            let block_no = reader.read_u32::<LittleEndian>()? as usize;
            if block_no <= fun.blocks.len() {
                if len > 0 {
                    // Read the word that pads the beginning of the line table. This may be a
                    // flag of some sort, but seems to always be zero.
                    let _dummy = reader.read_u32::<LittleEndian>()?;

                    let file_name = GCNO::read_string(reader)?;
                    let len = len - 2 - ((file_name.len() as u32 / 4) + 1);

                    if file_name != fun.file_name {
                        return Err(GcovError::Str(format!("Multiple sources for a single basic block: {} != {} (in {}).", fun.file_name, file_name, fun.name)))
                    }
                    let block = &mut fun.blocks[block_no];
                    let lines = &mut block.lines;
                    lines.reserve(len as usize);

                    for _ in 0..len {
                        let line = reader.read_u32::<LittleEndian>()?;
                        if line != 0 {
                            lines.push(line);
                            if line > block.line_max {
                                block.line_max = line;
                            }
                        }
                    }
                }
                // Just read 2 zeros
                let _dummy = reader.read_u64::<LittleEndian>()?;
                tag = reader.read_u32::<LittleEndian>()?;
            } else {
                return Err(GcovError::Str(format!("Unexpected block number: {} (in {}).", block_no, fun.name)))
            }
        }
        Ok(tag)
    }

    fn read_functions(&mut self, reader: &mut Read) -> Result<(), GcovError> {
        let mut tag = reader.read_u32::<LittleEndian>()?;
        while tag == 0x0100_0000 {
            let _dummy = reader.read_u32::<LittleEndian>()?;
            let identifier = reader.read_u32::<LittleEndian>()?;
            let line_checksum = reader.read_u32::<LittleEndian>()?;
            if self.version != 402 {
                let cfg_sum = reader.read_u32::<LittleEndian>()?;
                if cfg_sum != self.checksum {
                    let fn_name = GCNO::read_string(reader)?;
                    return Err(GcovError::Str(format!("File checksums do not match: {} != {} (in {}).", self.checksum, cfg_sum, fn_name)));
                }
            }

            let name = GCNO::read_string(reader)?;
            let file_name = GCNO::read_string(reader)?;
            let line_number = reader.read_u32::<LittleEndian>()?;
            let block_tag = reader.read_u32::<LittleEndian>()?;

            if block_tag == 0x0141_0000 {
                let count = reader.read_u32::<LittleEndian>()? as usize;
                let mut blocks: Vec<GcovBlock> = Vec::with_capacity(count);
                for no in 0..count {
                    blocks.push(GcovBlock {
                        no,
                        flags: reader.read_u32::<LittleEndian>()?,
                        source: Vec::new(),
                        destination: Vec::new(),
                        lines: Vec::new(),
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
                    edges: Vec::new(),
                    lines: HashMap::new(),
                    executed: false,
                };
                tag = GCNO::read_edges(&mut fun, reader)?;
                tag = GCNO::read_lines(&mut fun, reader, tag)?;
                self.functions.push(fun);
            } else {
                return Err(GcovError::Str(format!("Invalid function tag: {:?}", tag)));
            }
        }
        Ok(())
    }

    fn read_gcda(&mut self, reader: &mut Read) -> Result<(), GcovError> {
        GCNO::check_type(reader, [b'a', b'd', b'c', b'g'])?;
        let version = GCNO::read_version(reader)?;
        if version != self.version {
            Err(GcovError::Str("GCOV versions do not match.".to_string()))
        } else {
            let checksum = reader.read_u32::<LittleEndian>()?;
            if checksum != self.checksum {
                Err(GcovError::Str(format!("File checksums do not match: {:?} != {:?}", self.checksum, checksum)))
            } else {
                for mut fun in &mut self.functions {
                    GCNO::read_gcda_function(self.version, self.checksum, &mut fun, reader)?;
                }
                let object_tag = reader.read_u32::<LittleEndian>()?;
                if object_tag == 0xa100_0000 {
                    reader.read_u32::<LittleEndian>()?;
                    reader.read_u64::<LittleEndian>()?;
                    self.runcounts += reader.read_u32::<LittleEndian>()?;
                }
                Ok(())
            }
        }
    }

    fn read_gcda_function(version: u32, checksum: u32, fun: &mut GcovFunction, reader: &mut Read) -> Result<(), GcovError> {
        let tag = reader.read_u32::<LittleEndian>()?;
        if tag != 0x0100_0000 {
            Err(GcovError::Str("Unexpected number of functions.".to_string()))
        } else {
            let _header_len = reader.read_u32::<LittleEndian>()?;
            let id = reader.read_u32::<LittleEndian>()?;
            if id != fun.identifier {
                Err(GcovError::Str(format!("Function identifiers do not match: {} != {} (in {}).", fun.identifier, id, fun.name)))
            } else {
                let _chk_sum = reader.read_u32::<LittleEndian>()?;
                if version != 402 {
                    let cfg_sum = reader.read_u32::<LittleEndian>()?;
                    if cfg_sum != checksum {
                        return Err(GcovError::Str(format!("File checksums do not match: {} != {} (in {}).", checksum, cfg_sum, fun.name)));
                    }
                }

                let fn_name = GCNO::read_string(reader)?;
                if fn_name != fun.name {
                    Err(GcovError::Str(format!("Function names do not match: {} != {}.", fun.name, fn_name)))
                } else {
                    let arc_tag = reader.read_u32::<LittleEndian>()?;
                    if arc_tag != 0x01a1_0000 {
                        Err(GcovError::Str(format!("Arc tag not found (in {}).", fn_name)))
                    } else {
                        let count = reader.read_u32::<LittleEndian>()?;
                        let edges = &mut fun.edges;
                        if edges.len() as u32 != count / 2 {
                            Err(GcovError::Str(format!("Unexpected number of edges (in {}).", fn_name)))
                        } else {
                            if let Some((first, elmts)) = edges.split_first_mut() {
                                // The first edge is between entry block and a block
                                // so the entry block as the same counter as the
                                // edge counter.
                                let counter = GCNO::read_counter(reader)?;
                                first.counter += counter;
                                fun.blocks[first.destination].counter += counter;
                                fun.blocks[first.source].counter += counter;

                                if !fun.executed && counter != 0 {
                                    fun.executed = true;
                                }
                                
                                for edge in elmts {
                                    let counter = GCNO::read_counter(reader)?;
                                    edge.counter += counter;
                                    fun.blocks[edge.destination].counter += counter;
                                    if !fun.executed && counter != 0 {
                                        fun.executed = true;
                                    }
                                }
                            }
                            Ok(())
                        }
                    }
                }
            }
        }
    }

    fn collect_lines(&self) -> HashMap<String, HashMap<u32, u64>> {
        let mut results: HashMap<String, HashMap<u32, u64>> = HashMap::new();
        for function in &self.functions {
            // TODO: could we remove the clone ?
            let mut lines = match results.entry(function.file_name.clone()) {
                hash_map::Entry::Occupied(l) => {
                        l.into_mut()
                }
                hash_map::Entry::Vacant(p) => {
                    p.insert(HashMap::new())
                }
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

    fn dump(&self, path: &PathBuf, file_name: &str, writer: &mut Write) -> Result<(), GcovError> {
        let file = File::open(path)?;
        let mut reader = BufReader::new(file);
        let mut source = String::new();
        let counters = self.collect_lines();
        let counters = &counters[file_name];
        reader.read_to_string(&mut source)?;
        let stem = PathBuf::from(file_name);
        let stem = stem.file_stem().unwrap().to_str().unwrap();
        let mut n: u32 = 0;
        let has_runs = self.runcounts != 0;
        
        writeln!(writer, "{:>9}:{:>5}:Source:{}", "-", 0, file_name);
        writeln!(writer, "{:>9}:{:>5}:Graph:{}.gcno", "-", 0, stem);
        if has_runs {
            writeln!(writer, "{:>9}:{:>5}:Data:{}.gcda", "-", 0, stem);
        } else {
            writeln!(writer, "{:>9}:{:>5}:Data:-", "-", 0);
        }
        writeln!(writer, "{:>9}:{:>5}:Runs:{}", "-", 0, self.runcounts);
        writeln!(writer, "{:>9}:{:>5}:Programs:{}", "-", 0, if has_runs { 1 } else { 0 });
        let mut iter = source.split('\n').peekable();
        while let Some(line) = iter.next() {
            if iter.peek().is_none() && line.is_empty() {
                // We're on the last line and it's empty
                break;
            }
            n += 1;
            if let Some(counter) = counters.get(&n) {
                if *counter == 0 {
                    writeln!(writer, "{:>9}:{:>5}:{}", "#####", n, line);
                } else {
                    writeln!(writer, "{:>9}:{:>5}:{}", *counter, n, line);
                }
            } else {
                writeln!(writer, "{:>9}:{:>5}:{}", "-", n, line);
            }
        }
        
        Ok(())
    }

    fn add_line_count(&mut self) {
        for fun in &mut self.functions {
            fun.add_line_count();
        }
    }
    
    fn finalize(&mut self, branch_enabled: bool) -> Vec<(String, CovResult)> {
        let mut results: HashMap<String, CovResult> = HashMap::new();
        for fun in &mut self.functions {
            fun.add_line_count();
            let mut res = match results.entry(fun.file_name.clone()) {
                hash_map::Entry::Occupied(r) => {
                    r.into_mut()
                }
                hash_map::Entry::Vacant(p) => {
                    p.insert(CovResult {
                        lines: BTreeMap::new(),
                        branches: BTreeMap::new(),
                        functions: HashMap::new(),
                    })
                }
            };
            res.functions.insert(fun.name.clone(), Function {
                start: fun.line_number,
                executed: fun.executed,
            });
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
                    let mut line_to_branches: HashMap<u32, Vec<bool>> = HashMap::new();
                    for block in &fun.blocks {
                        if block.destination.len() > 1 {
                            let line = block.line_max;
                            let taken = block.destination.iter().map(|no| fun.edges[*no].counter > 0);
                            match line_to_branches.entry(line) {
                                hash_map::Entry::Occupied(c) => {
                                    let mut v = c.into_mut();
                                    v.extend(taken);
                                }
                                hash_map::Entry::Vacant(p) => {
                                    p.insert(taken.collect());
                                }
                            }
                        }
                    }
                    for (line, taken_branches) in line_to_branches {
                        for (n, taken) in taken_branches.iter().enumerate() {
                            res.branches.insert((line, n as u32), *taken);
                        }                                    
                    }
                } else {
                    let mut line_to_branches: HashMap<u32, usize> = HashMap::new();
                    for block in &fun.blocks {
                        let n_dest = block.destination.len();
                        if n_dest > 1 {
                            match line_to_branches.entry(block.line_max) {
                                hash_map::Entry::Occupied(c) => {
                                    *c.into_mut() += n_dest;
                                }
                                hash_map::Entry::Vacant(p) => {
                                    p.insert(n_dest);
                                }
                            }
                        }
                    }
                    for (line, n) in line_to_branches {
                        for i in 0..n {
                            res.branches.insert((line, i as u32), false);
                        }                                    
                    }
                }
            }
        }
        let iter = results.drain();
        iter.collect()
    }
}

impl GcovFunction {

    fn get_cycle_count(edges: &mut [GcovEdge], path: &mut [usize]) -> u64 {
        let mut count = std::u64::MAX;
        for e in path.iter() {
            count = cmp::min(count, edges[*e].cycles);
        }
        for e in path {
            edges[*e].cycles -= count;
        }
        count
    }

    fn unblock(block: usize, blocked: &mut Vec<usize>, block_lists: &mut Vec<Vec<usize>>) {
        if let Some(i) = blocked.iter().position(|x| *x == block) {
            blocked.remove(i);
            for b in block_lists.remove(i) {
                GcovFunction::unblock(b, blocked, block_lists); 
            }
        }
    }
    
    fn look_for_circuit(fun_edges: &mut [GcovEdge],
                        fun_blocks: &[GcovBlock],
                        v: usize,
                        start: usize,
                        path: &mut Vec<usize>,
                        blocked: &mut Vec<usize>,
                        block_lists: &mut Vec<Vec<usize>>,
                        blocks: &[usize],
                        count: u64) -> (bool, u64) {
        let mut count = count;
        blocked.push(v);
        block_lists.push(Vec::new());
        let mut found = false;
        let dsts = &fun_blocks[v].destination;

        for e in dsts {
            let w = fun_edges[*e].destination;
            if w >= start && blocks.iter().any(|x| *x == w) {
                path.push(*e);
                if w == start {
                    count += GcovFunction::get_cycle_count(fun_edges, path);
                    found = true;
                } else if !blocked.iter().any(|x| *x == w) {
                    let (f, c) = GcovFunction::look_for_circuit(fun_edges, fun_blocks, w, start, path, blocked, block_lists, blocks, count);
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
                if w >= start {
                    if let Some(i) = blocks.iter().position(|x| *x == w) {
                        let list = &mut block_lists[i];
                        if !list.iter().any(|x| *x == v) {
                            list.push(v);
                        }
                    }
                }
            }
        }

        (found, count)
    }

    fn get_cycles_count(fun_edges: &mut [GcovEdge],
                        fun_blocks: &[GcovBlock],
                        blocks: &[usize]) -> u64 {
        let mut count: u64 = 0;
        let mut path: Vec<usize> = Vec::new();
        let mut blocked: Vec<usize> = Vec::new();
        let mut block_lists: Vec<Vec<usize>> = Vec::new();
        for b in blocks {
            path.clear();
            blocked.clear();
            block_lists.clear();
            let (_, c) = GcovFunction::look_for_circuit(fun_edges, fun_blocks, *b, *b, &mut path, &mut blocked, &mut block_lists, blocks, count);
            count += c;
        }
        count
    }

    fn get_line_count(fun_edges: &mut [GcovEdge],
                      fun_blocks: &[GcovBlock],
                      blocks: &[usize]) -> u64 {
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
        if self.executed {
            let mut lines_to_block: HashMap<u32, Vec<usize>> = HashMap::new();
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
            let lines_to_block = lines_to_block;
            for (line, blocks) in lines_to_block {
                let count = GcovFunction::get_line_count(&mut self.edges, &self.blocks, &blocks);
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
    use std::path::PathBuf;

    #[test]
    fn test_reader_gcno() {
        let path = PathBuf::from("test/llvm/reader.gcno");
        let mut f = File::open(path).unwrap();
        let mut gcno = GCNO::new();
        gcno.read(&mut f).unwrap();
        let output = format!("{:?}", gcno);
        let path = PathBuf::from("test/llvm/reader.gcno.0.dump");
        let mut f = File::open(path).unwrap();
        let mut input = String::new();
        f.read_to_string(&mut input).unwrap();

        assert!(input == output);
    }

    #[test]
    fn test_reader_gcno_gcda() {
        let path = PathBuf::from("test/llvm/reader.gcno");
        let mut f = File::open(path).unwrap();
        let mut gcno = GCNO::new();
        gcno.read(&mut f).unwrap();
        let path = PathBuf::from("test/llvm/reader.gcda");
        let mut f = File::open(path).unwrap();
        gcno.read_gcda(&mut f).unwrap();
        let output = format!("{:?}", gcno);
        let path = PathBuf::from("test/llvm/reader.gcno.1.dump");
        let mut f = File::open(path).unwrap();
        let mut input = String::new();
        f.read_to_string(&mut input).unwrap();

        assert!(input == output);
    }

    #[test]
    fn test_reader_gcno_gcda_gcda() {
        let path = PathBuf::from("test/llvm/reader.gcno");
        let mut f = File::open(path).unwrap();
        let mut gcno = GCNO::new();
        gcno.read(&mut f).unwrap();
        for _ in 0..2 {
            let path = PathBuf::from("test/llvm/reader.gcda");
            let mut f = File::open(path).unwrap();
            gcno.read_gcda(&mut f).unwrap();
        }
        let output = format!("{:?}", gcno);
        let path = PathBuf::from("test/llvm/reader.gcno.2.dump");
        let mut f = File::open(path).unwrap();
        let mut input = String::new();
        f.read_to_string(&mut input).unwrap();

        assert!(input == output);
    }

    #[test]
    fn test_reader_gcno_counter() {
        let path = PathBuf::from("test/llvm/reader.gcno");
        let mut f = File::open(path).unwrap();
        let mut gcno = GCNO::new();
        gcno.read(&mut f).unwrap();
        gcno.add_line_count();
        let mut output = Vec::new();
        gcno.dump(&PathBuf::from("test/llvm/reader.c"), "reader.c", &mut output).unwrap();
        let path = PathBuf::from("test/llvm/reader.c.0.gcov");
        let mut f = File::open(path).unwrap();
        let mut input = Vec::new();
        f.read_to_end(&mut input).unwrap();
        //eprintln!("{}", std::str::from_utf8(&output).unwrap());
        
        assert!(input == output);
    }
    
    #[test]
    fn test_reader_gcno_gcda_counter() {
        let path = PathBuf::from("test/llvm/reader.gcno");
        let mut f = File::open(path).unwrap();
        let mut gcno = GCNO::new();
        gcno.read(&mut f).unwrap();
        let path = PathBuf::from("test/llvm/reader.gcda");
        let mut f = File::open(path).unwrap();
        gcno.read_gcda(&mut f).unwrap();
        gcno.add_line_count();
        let mut output = Vec::new();
        gcno.dump(&PathBuf::from("test/llvm/reader.c"), "reader.c", &mut output).unwrap();
        let path = PathBuf::from("test/llvm/reader.c.1.gcov");
        let mut f = File::open(path).unwrap();
        let mut input = Vec::new();
        f.read_to_end(&mut input).unwrap();
        
        assert!(input == output);
    }

    #[test]
    fn test_reader_gcno_gcda_gcda_counter() {
        let path = PathBuf::from("test/llvm/reader.gcno");
        let mut f = File::open(path).unwrap();
        let mut gcno = GCNO::new();
        gcno.read(&mut f).unwrap();
        for _ in 0..2 {
            let path = PathBuf::from("test/llvm/reader.gcda");
            let mut f = File::open(path).unwrap();
            gcno.read_gcda(&mut f).unwrap();
        }
        gcno.add_line_count();
        let mut output = Vec::new();
        gcno.dump(&PathBuf::from("test/llvm/reader.c"), "reader.c", &mut output).unwrap();
        let path = PathBuf::from("test/llvm/reader.c.2.gcov");
        let mut f = File::open(path).unwrap();
        let mut input = Vec::new();
        f.read_to_end(&mut input).unwrap();
        
        assert!(input == output);
    }

    #[test]
    fn test_reader_finalize_file() {
        let path = PathBuf::from("test/llvm/file.gcno");
        let mut f = File::open(path).unwrap();
        let mut gcno = GCNO::new();
        gcno.read(&mut f).unwrap();
        let path = PathBuf::from("test/llvm/file.gcda");
        let mut f = File::open(path).unwrap();
        gcno.read_gcda(&mut f).unwrap();
        let result = gcno.finalize(true);

        let mut lines: BTreeMap<u32, u64> = BTreeMap::new();
        lines.insert(2, 1);
        let mut functions: HashMap<String, Function> = HashMap::new();
        functions.insert(
            String::from("main"),
            Function {
                start: 1,
                executed: true,
            },
        );
        let branches: BTreeMap<(u32, u32), bool> = BTreeMap::new();
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
        let path = PathBuf::from("test/llvm/file_branch.gcno");
        let mut f = File::open(path).unwrap();
        let mut gcno = GCNO::new();
        gcno.read(&mut f).unwrap();
        let path = PathBuf::from("test/llvm/file_branch.gcda");
        let mut f = File::open(path).unwrap();
        gcno.read_gcda(&mut f).unwrap();
        let result = gcno.finalize(true);

        let mut lines: BTreeMap<u32, u64> = BTreeMap::new();
        lines.insert(2, 2);
        lines.insert(3, 1);
        lines.insert(4, 1);
        lines.insert(5, 1);
        lines.insert(6, 1);
        lines.insert(8, 1);
        lines.insert(10, 2);
        lines.insert(13, 1);
        lines.insert(14, 0);
        lines.insert(16, 1);
        lines.insert(18, 1);
        lines.insert(21, 0);
        lines.insert(22, 0);
        lines.insert(24, 0);
        lines.insert(25, 0);
        lines.insert(26, 0);
        lines.insert(28, 0);
        lines.insert(32, 1);
        let mut functions: HashMap<String, Function> = HashMap::new();
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
        let mut branches: BTreeMap<(u32, u32), bool> = BTreeMap::new();
        branches.insert((2, 0), true);
        branches.insert((2, 1), true);
        branches.insert((3, 0), true);
        branches.insert((3, 1), false);
        branches.insert((13, 0), false);
        branches.insert((13, 1), true);
        branches.insert((21, 0), false);
        branches.insert((21, 1), false);
        branches.insert((21, 2), false);
        branches.insert((21, 3), false);
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
