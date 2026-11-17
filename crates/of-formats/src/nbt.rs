//! Minecraft Java NBT: reading and writing, with every tag kept exactly as
//! found (including ones this program does not understand, the element type
//! of empty lists, and the order of compound entries).

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use std::fmt;
use std::io::{Read, Write};

#[derive(Debug, Clone, PartialEq)]
pub enum Tag {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<u8>),
    String(String),
    List(List),
    Compound(Compound),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

/// A list remembers its element type so that an empty list is written back
/// with the same type it was read with.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct List {
    pub elem: u8,
    pub items: Vec<Tag>,
}

/// Entries in file order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Compound(pub Vec<(String, Tag)>);

#[derive(Debug, Clone, PartialEq)]
pub struct NbtError(pub String);

impl fmt::Display for NbtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for NbtError {}

fn err<T>(msg: impl Into<String>) -> Result<T, NbtError> {
    Err(NbtError(msg.into()))
}

impl Tag {
    pub fn id(&self) -> u8 {
        match self {
            Tag::Byte(_) => 1,
            Tag::Short(_) => 2,
            Tag::Int(_) => 3,
            Tag::Long(_) => 4,
            Tag::Float(_) => 5,
            Tag::Double(_) => 6,
            Tag::ByteArray(_) => 7,
            Tag::String(_) => 8,
            Tag::List(_) => 9,
            Tag::Compound(_) => 10,
            Tag::IntArray(_) => 11,
            Tag::LongArray(_) => 12,
        }
    }
    /// Any whole-number tag as i64.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Tag::Byte(v) => Some(*v as i64),
            Tag::Short(v) => Some(*v as i64),
            Tag::Int(v) => Some(*v as i64),
            Tag::Long(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_i32(&self) -> Option<i32> {
        self.as_i64().and_then(|v| i32::try_from(v).ok())
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Tag::String(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_compound(&self) -> Option<&Compound> {
        match self {
            Tag::Compound(c) => Some(c),
            _ => None,
        }
    }
    pub fn as_list(&self) -> Option<&[Tag]> {
        match self {
            Tag::List(l) => Some(&l.items),
            _ => None,
        }
    }
    pub fn as_longs(&self) -> Option<&[i64]> {
        match self {
            Tag::LongArray(v) => Some(v),
            _ => None,
        }
    }
    pub fn as_ints(&self) -> Option<&[i32]> {
        match self {
            Tag::IntArray(v) => Some(v),
            _ => None,
        }
    }
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Tag::ByteArray(v) => Some(v),
            _ => None,
        }
    }
    pub fn list(items: Vec<Tag>) -> Tag {
        let elem = items.first().map(Tag::id).unwrap_or(0);
        Tag::List(List { elem, items })
    }
    pub fn string(s: impl Into<String>) -> Tag {
        Tag::String(s.into())
    }
}

impl Compound {
    pub fn new() -> Self {
        Self(Vec::new())
    }
    pub fn get(&self, key: &str) -> Option<&Tag> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn contains(&self, key: &str) -> bool {
        self.get(key).is_some()
    }
    /// Replace an entry in place, or add it at the end.
    pub fn set(&mut self, key: &str, value: Tag) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some(entry) => entry.1 = value,
            None => self.0.push((key.to_string(), value)),
        }
    }
    pub fn with(mut self, key: &str, value: Tag) -> Self {
        self.set(key, value);
        self
    }
    pub fn remove(&mut self, key: &str) -> Option<Tag> {
        let i = self.0.iter().position(|(k, _)| k == key)?;
        Some(self.0.remove(i).1)
    }
    pub fn i32(&self, key: &str) -> Option<i32> {
        self.get(key).and_then(Tag::as_i32)
    }
    pub fn str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Tag::as_str)
    }
    pub fn compound(&self, key: &str) -> Option<&Compound> {
        self.get(key).and_then(Tag::as_compound)
    }
    pub fn list(&self, key: &str) -> Option<&[Tag]> {
        self.get(key).and_then(Tag::as_list)
    }
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Tag)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }
    /// Entries whose keys are not in `known`, for carrying through untouched.
    pub fn without(&self, known: &[&str]) -> Compound {
        Compound(self.0.iter().filter(|(k, _)| !known.contains(&k.as_str())).cloned().collect())
    }
    /// Same entries regardless of order (compound order carries no meaning in game).
    pub fn same_content(&self, other: &Compound) -> bool {
        self.0.len() == other.0.len() && self.0.iter().all(|(k, v)| other.get(k).is_some_and(|o| tag_same(v, o)))
    }
}

/// Equality that ignores compound entry order and compares floats by bits.
pub fn tag_same(a: &Tag, b: &Tag) -> bool {
    match (a, b) {
        (Tag::Compound(x), Tag::Compound(y)) => x.same_content(y),
        (Tag::List(x), Tag::List(y)) => {
            x.items.len() == y.items.len() && x.items.iter().zip(&y.items).all(|(p, q)| tag_same(p, q))
        }
        (Tag::Float(x), Tag::Float(y)) => x.to_bits() == y.to_bits(),
        (Tag::Double(x), Tag::Double(y)) => x.to_bits() == y.to_bits(),
        _ => a == b,
    }
}

// ------------------------------------------------------------------ reading

const MAX_DEPTH: u32 = 512;

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], NbtError> {
        if self.data.len() - self.pos < n {
            return err(format!("file ends early at byte {}", self.pos));
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, NbtError> {
        Ok(self.take(1)?[0])
    }
    fn i16(&mut self) -> Result<i16, NbtError> {
        Ok(i16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> Result<i32, NbtError> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i64(&mut self) -> Result<i64, NbtError> {
        Ok(i64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn len(&mut self, item_bytes: usize) -> Result<usize, NbtError> {
        let n = self.i32()?;
        if n < 0 {
            return err(format!("negative length at byte {}", self.pos - 4));
        }
        let n = n as usize;
        // A length larger than the rest of the file means the file is damaged;
        // checking first avoids trying to reserve huge amounts of memory.
        if n.saturating_mul(item_bytes) > self.data.len() - self.pos {
            return err(format!("length {n} at byte {} runs past the end of the file", self.pos - 4));
        }
        Ok(n)
    }
    fn string(&mut self) -> Result<String, NbtError> {
        let n = self.i16()? as u16 as usize;
        Ok(decode_mutf8(self.take(n)?))
    }
    fn payload(&mut self, id: u8, depth: u32) -> Result<Tag, NbtError> {
        if depth > MAX_DEPTH {
            return err("tags are nested too deeply");
        }
        Ok(match id {
            1 => Tag::Byte(self.u8()? as i8),
            2 => Tag::Short(self.i16()?),
            3 => Tag::Int(self.i32()?),
            4 => Tag::Long(self.i64()?),
            5 => Tag::Float(f32::from_bits(self.i32()? as u32)),
            6 => Tag::Double(f64::from_bits(self.i64()? as u64)),
            7 => {
                let n = self.len(1)?;
                Tag::ByteArray(self.take(n)?.to_vec())
            }
            8 => Tag::String(self.string()?),
            9 => {
                let elem = self.u8()?;
                let n = self.len(if elem == 0 { 0 } else { 1 })?;
                if elem == 0 && n > 0 {
                    return err("list of 'end' tags is not empty");
                }
                let mut items = Vec::with_capacity(n);
                for _ in 0..n {
                    items.push(self.payload(elem, depth + 1)?);
                }
                Tag::List(List { elem, items })
            }
            10 => {
                let mut entries = Vec::new();
                loop {
                    let id = self.u8()?;
                    if id == 0 {
                        break;
                    }
                    let name = self.string()?;
                    entries.push((name, self.payload(id, depth + 1)?));
                }
                Tag::Compound(Compound(entries))
            }
            11 => {
                let n = self.len(4)?;
                let raw = self.take(n * 4)?;
                Tag::IntArray(raw.chunks_exact(4).map(|c| i32::from_be_bytes(c.try_into().unwrap())).collect())
            }
            12 => {
                let n = self.len(8)?;
                let raw = self.take(n * 8)?;
                Tag::LongArray(raw.chunks_exact(8).map(|c| i64::from_be_bytes(c.try_into().unwrap())).collect())
            }
            other => return err(format!("unknown tag type {other} at byte {}", self.pos)),
        })
    }
}

/// Expand gzip data if it is gzip; otherwise return it unchanged.
pub fn gunzip_if_needed(data: &[u8]) -> Result<Vec<u8>, NbtError> {
    if data.len() >= 2 && data[0] == 0x1f && data[1] == 0x8b {
        // Read in pieces, reserving each without aborting and checking the
        // memory limit between them: a large or hostile file is stopped,
        // not allowed to fill the memory (column bake plan, stage 6).
        const PIECE: usize = 4 << 20;
        let mut out = Vec::new();
        let mut dec = GzDecoder::new(data);
        loop {
            if !of_core::memory::within_limit() || out.try_reserve(PIECE).is_err() {
                return Err(NbtError(OVER_LIMIT.into()));
            }
            let start = out.len();
            out.resize(start + PIECE, 0);
            let n = read_full(&mut dec, &mut out[start..]).map_err(|e| NbtError(format!("damaged compressed data: {e}")))?;
            out.truncate(start + n);
            if n < PIECE {
                break;
            }
        }
        out.shrink_to_fit();
        Ok(out)
    } else {
        Ok(data.to_vec())
    }
}

/// What `NbtError` says when reading stopped at the memory limit.
pub const OVER_LIMIT: &str = "over the memory limit";

/// Fill `buf` from `r` as far as it goes; the count is short only at the end.
fn read_full(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut done = 0;
    while done < buf.len() {
        match r.read(&mut buf[done..]) {
            Ok(0) => break,
            Ok(n) => done += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(done)
}

pub fn gzip(data: &[u8]) -> Vec<u8> {
    let mut enc = GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(data).expect("writing to memory cannot fail");
    enc.finish().expect("writing to memory cannot fail")
}

/// Read a named root tag from raw (uncompressed) bytes. Returns the root
/// name, the root compound and how many bytes were used.
pub fn read_raw(data: &[u8]) -> Result<(String, Compound, usize), NbtError> {
    let mut r = Reader { data, pos: 0 };
    let id = r.u8()?;
    if id != 10 {
        return err("not an NBT file (it does not start with a compound tag)");
    }
    let name = r.string()?;
    match r.payload(10, 0)? {
        Tag::Compound(c) => Ok((name, c, r.pos)),
        _ => unreachable!(),
    }
}

/// Read a file that may or may not be gzip-compressed.
pub fn read(data: &[u8]) -> Result<(String, Compound), NbtError> {
    let raw = gunzip_if_needed(data)?;
    let (name, root, _) = read_raw(&raw)?;
    Ok((name, root))
}

// ------------------------------------------------------------------ writing

fn put_string(out: &mut Vec<u8>, s: &str) {
    let bytes = encode_mutf8(s);
    // Names and strings are limited to 65,535 bytes by the format.
    let n = bytes.len().min(u16::MAX as usize);
    out.extend_from_slice(&(n as u16).to_be_bytes());
    out.extend_from_slice(&bytes[..n]);
}

fn put_payload(out: &mut Vec<u8>, tag: &Tag) {
    match tag {
        Tag::Byte(v) => out.push(*v as u8),
        Tag::Short(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Int(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Long(v) => out.extend_from_slice(&v.to_be_bytes()),
        Tag::Float(v) => out.extend_from_slice(&v.to_bits().to_be_bytes()),
        Tag::Double(v) => out.extend_from_slice(&v.to_bits().to_be_bytes()),
        Tag::ByteArray(v) => {
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            out.extend_from_slice(v);
        }
        Tag::String(s) => put_string(out, s),
        Tag::List(l) => {
            let elem = l.items.first().map(Tag::id).unwrap_or(l.elem);
            out.push(elem);
            out.extend_from_slice(&(l.items.len() as i32).to_be_bytes());
            for item in &l.items {
                put_payload(out, item);
            }
        }
        Tag::Compound(c) => {
            for (name, value) in &c.0 {
                out.push(value.id());
                put_string(out, name);
                put_payload(out, value);
            }
            out.push(0);
        }
        Tag::IntArray(v) => {
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            out.reserve(v.len() * 4);
            for x in v {
                out.extend_from_slice(&x.to_be_bytes());
            }
        }
        Tag::LongArray(v) => {
            out.extend_from_slice(&(v.len() as i32).to_be_bytes());
            out.reserve(v.len() * 8);
            for x in v {
                out.extend_from_slice(&x.to_be_bytes());
            }
        }
    }
}

pub fn write_raw(name: &str, root: &Compound) -> Vec<u8> {
    let mut out = vec![10u8];
    put_string(&mut out, name);
    for (key, value) in &root.0 {
        out.push(value.id());
        put_string(&mut out, key);
        put_payload(&mut out, value);
    }
    out.push(0);
    out
}

pub fn write_gzip(name: &str, root: &Compound) -> Vec<u8> {
    gzip(&write_raw(name, root))
}

/// NBT written piece by piece straight into the compressor, so neither the
/// tag tree nor the uncompressed bytes of a large file are ever held whole.
/// The caller opens and closes compounds and lists in the right order; the
/// bytes come out the same as `write_gzip` would give for the same tree.
///
/// Why: a 1024 x 1024 sea's block data took more memory as a tag tree and
/// again as raw bytes than the finished, compressed file (stage 4 of the
/// column bake plan).
pub struct Stream {
    enc: GzEncoder<Vec<u8>>,
    buf: Vec<u8>,
}

impl Stream {
    /// Start a file whose root compound has this name.
    pub fn new(root_name: &str) -> Self {
        let mut s = Self { enc: GzEncoder::new(Vec::new(), flate2::Compression::default()), buf: Vec::with_capacity(1 << 16) };
        s.buf.push(10);
        put_string(&mut s.buf, root_name);
        s
    }
    fn spill(&mut self) {
        if self.buf.len() >= 1 << 16 {
            self.enc.write_all(&self.buf).expect("writing to memory cannot fail");
            self.buf.clear();
        }
    }
    fn named(&mut self, id: u8, name: &str) {
        self.buf.push(id);
        put_string(&mut self.buf, name);
    }
    /// One whole entry of the compound that is open.
    pub fn entry(&mut self, name: &str, tag: &Tag) {
        self.named(tag.id(), name);
        put_payload(&mut self.buf, tag);
        self.spill();
    }
    /// Open a compound entry; close it with `end`.
    pub fn begin_compound(&mut self, name: &str) {
        self.named(10, name);
    }
    /// Close the compound that is open (the root included).
    pub fn end(&mut self) {
        self.buf.push(0);
    }
    /// Open a list entry of `len` items of type `elem`; each item follows as `item`.
    pub fn begin_list(&mut self, name: &str, elem: u8, len: usize) {
        self.named(9, name);
        self.buf.push(elem);
        self.buf.extend_from_slice(&(len as i32).to_be_bytes());
    }
    /// One item of the list that is open, without a name.
    pub fn item(&mut self, tag: &Tag) {
        put_payload(&mut self.buf, tag);
        self.spill();
    }
    /// Open a long array entry of `len` values; each follows as `long`.
    pub fn begin_long_array(&mut self, name: &str, len: usize) {
        self.named(12, name);
        self.buf.extend_from_slice(&(len as i32).to_be_bytes());
    }
    pub fn long(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self.spill();
    }
    /// Open a byte array entry of `len` bytes; they follow as `bytes`.
    pub fn begin_byte_array(&mut self, name: &str, len: usize) {
        self.named(7, name);
        self.buf.extend_from_slice(&(len as i32).to_be_bytes());
    }
    pub fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
        self.spill();
    }
    /// The compressed file. The root must have been closed with `end`.
    pub fn finish(mut self) -> Vec<u8> {
        self.enc.write_all(&self.buf).expect("writing to memory cannot fail");
        self.enc.finish().expect("writing to memory cannot fail")
    }
}

// ------------------------------------------------- Java "modified UTF-8"

fn decode_mutf8(bytes: &[u8]) -> String {
    if bytes.is_ascii() {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    // Decode to UTF-16 units first: Java stores characters outside the basic
    // plane as two 3-byte surrogates, and NUL as C0 80.
    let mut units: Vec<u16> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let cont = |k: usize| bytes.get(i + k).filter(|c| *c & 0xC0 == 0x80).map(|c| (*c & 0x3F) as u16);
        if b < 0x80 {
            units.push(b as u16);
            i += 1;
        } else if b & 0xE0 == 0xC0 {
            match cont(1) {
                Some(c1) => {
                    units.push(((b as u16 & 0x1F) << 6) | c1);
                    i += 2;
                }
                None => {
                    units.push(0xFFFD);
                    i += 1;
                }
            }
        } else if b & 0xF0 == 0xE0 {
            match (cont(1), cont(2)) {
                (Some(c1), Some(c2)) => {
                    units.push(((b as u16 & 0x0F) << 12) | (c1 << 6) | c2);
                    i += 3;
                }
                _ => {
                    units.push(0xFFFD);
                    i += 1;
                }
            }
        } else {
            units.push(0xFFFD);
            i += 1;
        }
    }
    String::from_utf16_lossy(&units)
}

fn encode_mutf8(s: &str) -> Vec<u8> {
    if s.is_ascii() && !s.contains('\0') {
        return s.as_bytes().to_vec();
    }
    let mut out = Vec::with_capacity(s.len() + 8);
    for unit in s.encode_utf16() {
        match unit {
            0 => out.extend_from_slice(&[0xC0, 0x80]),
            1..=0x7F => out.push(unit as u8),
            0x80..=0x7FF => out.extend_from_slice(&[0xC0 | (unit >> 6) as u8, 0x80 | (unit & 0x3F) as u8]),
            _ => out.extend_from_slice(&[
                0xE0 | (unit >> 12) as u8,
                0x80 | ((unit >> 6) & 0x3F) as u8,
                0x80 | (unit & 0x3F) as u8,
            ]),
        }
    }
    out
}

// --------------------------------------------------------------- text dump

/// A readable outline of a tag tree, for diagnostics. Long arrays are
/// summarised rather than printed in full.
pub fn outline(root: &Compound, max_depth: usize) -> String {
    let mut out = String::new();
    outline_compound(&mut out, root, 0, max_depth);
    out
}

fn outline_compound(out: &mut String, c: &Compound, depth: usize, max_depth: usize) {
    for (k, v) in &c.0 {
        out.push_str(&"  ".repeat(depth));
        out.push_str(k);
        out.push_str(": ");
        outline_tag(out, v, depth, max_depth);
    }
}

fn outline_tag(out: &mut String, tag: &Tag, depth: usize, max_depth: usize) {
    use std::fmt::Write as _;
    match tag {
        Tag::Byte(v) => writeln!(out, "{v}b"),
        Tag::Short(v) => writeln!(out, "{v}s"),
        Tag::Int(v) => writeln!(out, "{v}"),
        Tag::Long(v) => writeln!(out, "{v}L"),
        Tag::Float(v) => writeln!(out, "{v}f"),
        Tag::Double(v) => writeln!(out, "{v}d"),
        Tag::String(s) => writeln!(out, "{s:?}"),
        Tag::ByteArray(v) => writeln!(out, "[{} bytes]", v.len()),
        Tag::IntArray(v) if v.len() <= 8 => writeln!(out, "{v:?} (ints)"),
        Tag::IntArray(v) => writeln!(out, "[{} ints]", v.len()),
        Tag::LongArray(v) => writeln!(out, "[{} longs]", v.len()),
        Tag::List(l) => {
            writeln!(out, "list of {} (type {})", l.items.len(), l.elem).ok();
            if depth < max_depth {
                for (i, item) in l.items.iter().take(3).enumerate() {
                    out.push_str(&"  ".repeat(depth + 1));
                    write!(out, "[{i}] ").ok();
                    outline_tag(out, item, depth + 1, max_depth);
                }
                if l.items.len() > 3 {
                    out.push_str(&"  ".repeat(depth + 1));
                    out.push_str("...\n");
                }
            }
            Ok(())
        }
        Tag::Compound(c) => {
            writeln!(out, "{{{} entries}}", c.0.len()).ok();
            if depth < max_depth {
                outline_compound(out, c, depth + 1, max_depth);
            }
            Ok(())
        }
    }
    .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file written piece by piece holds the same bytes as the tree written whole.
    #[test]
    fn stream_writes_what_the_tree_writer_writes() {
        use std::io::Read;
        let longs: Vec<i64> = (0..70_000).map(|i| i * 7919 - 3).collect();
        let bytes: Vec<u8> = (0..90_000).map(|i| (i * 31) as u8).collect();
        let tree = sample()
            .with("streamed_longs", Tag::LongArray(longs.clone()))
            .with("streamed_bytes", Tag::ByteArray(bytes.clone()))
            .with("items", Tag::list(vec![Tag::Compound(sample()), Tag::Compound(Compound::new())]))
            .with("open", Tag::Compound(Compound::new().with("inner", Tag::Int(5))));
        let mut s = Stream::new("root");
        for (k, v) in sample().iter() {
            s.entry(k, v);
        }
        s.begin_long_array("streamed_longs", longs.len());
        longs.iter().for_each(|v| s.long(*v));
        s.begin_byte_array("streamed_bytes", bytes.len());
        for part in bytes.chunks(4096) {
            s.bytes(part);
        }
        s.begin_list("items", 10, 2);
        s.item(&Tag::Compound(sample()));
        s.item(&Tag::Compound(Compound::new()));
        s.begin_compound("open");
        s.entry("inner", &Tag::Int(5));
        s.end();
        s.end();
        let mut raw = Vec::new();
        flate2::read::GzDecoder::new(&s.finish()[..]).read_to_end(&mut raw).unwrap();
        assert!(raw == write_raw("root", &tree));
    }

    fn sample() -> Compound {
        Compound::new()
            .with("byte", Tag::Byte(-3))
            .with("short", Tag::Short(-300))
            .with("int", Tag::Int(123456))
            .with("long", Tag::Long(-1 << 40))
            .with("float", Tag::Float(1.5))
            .with("double", Tag::Double(f64::NAN))
            .with("bytes", Tag::ByteArray(vec![0, 255, 7]))
            .with("text", Tag::string("水 wave \u{1F30A} nul\0end"))
            .with("empty_typed_list", Tag::List(List { elem: 10, items: vec![] }))
            .with("list", Tag::list(vec![Tag::Int(1), Tag::Int(2)]))
            .with("nested", Tag::Compound(Compound::new().with("mystery_tag", Tag::LongArray(vec![i64::MIN, 0, i64::MAX]))))
            .with("ints", Tag::IntArray(vec![-1, 0, 1]))
    }

    #[test]
    fn every_tag_type_survives_a_round_trip_byte_for_byte() {
        let bytes = write_raw("root name", &sample());
        let (name, back, used) = read_raw(&bytes).unwrap();
        assert_eq!(name, "root name");
        assert_eq!(used, bytes.len());
        assert_eq!(write_raw(&name, &back), bytes);
        assert!(back.same_content(&sample()));
    }

    #[test]
    fn gzip_is_detected_automatically() {
        let packed = write_gzip("", &sample());
        assert_eq!(&packed[..2], &[0x1f, 0x8b]);
        let (_, back) = read(&packed).unwrap();
        assert!(back.same_content(&sample()));
        let (_, plain) = read(&write_raw("", &sample())).unwrap();
        assert!(plain.same_content(&sample()));
    }

    #[test]
    fn java_string_encoding_is_used() {
        let bytes = encode_mutf8("a\0\u{1F30A}");
        // NUL becomes C0 80; the emoji becomes two 3-byte surrogates.
        assert_eq!(bytes.len(), 1 + 2 + 6);
        assert_eq!(decode_mutf8(&bytes), "a\0\u{1F30A}");
    }

    #[test]
    fn damaged_files_are_refused_not_crashed_on() {
        let bytes = write_raw("", &sample());
        for cut in [0, 1, 5, bytes.len() / 2, bytes.len() - 1] {
            assert!(read_raw(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        // A list claiming two billion entries must not allocate.
        let mut bad = vec![10, 0, 0, 9, 0, 1, b'x', 3];
        bad.extend_from_slice(&i32::MAX.to_be_bytes());
        assert!(read_raw(&bad).is_err());
        assert!(read(&[0x1f, 0x8b, 1, 2, 3]).is_err());
        assert!(read(b"hello").is_err());
    }

    #[test]
    fn compound_order_does_not_affect_content_comparison() {
        let a = Compound::new().with("x", Tag::Int(1)).with("y", Tag::Int(2));
        let b = Compound::new().with("y", Tag::Int(2)).with("x", Tag::Int(1));
        assert!(a.same_content(&b));
        assert!(!a.same_content(&b.clone().with("z", Tag::Int(3))));
    }
}
