//! A minimal Parquet writer: one row group, one uncompressed v1 data page per
//! column, PLAIN or dictionary-encoded (a PLAIN dictionary page, then indices
//! in the RLE / bit-packed hybrid: what every Parquet writer produces by
//! default, so every reader takes it). Page payloads are borrowed slices,
//! written without copying.

use std::fs::File;
use std::io::Write;
use std::path::Path;

pub const INT32: i32 = 1;
pub const INT64: i32 = 2;
pub const BYTE_ARRAY: i32 = 6;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Int,
    Date,
    Str,
}

pub struct Column<'a> {
    pub name: &'static str,
    pub ptype: i32,
    pub kind: Kind,
    /// OPTIONAL column: `payload` then starts with the definition levels section.
    pub optional: bool,
    /// The data page body, in order: levels, then PLAIN values or, with
    /// `dict`, the dictionary indices (`pack_indices`).
    pub payload: Vec<&'a [u8]>,
    /// For a dictionary-encoded column: the PLAIN-encoded dictionary values
    /// and how many there are.
    pub dict: Option<(Vec<&'a [u8]>, usize)>,
}

/// The body of a dictionary-indices data page for `values`, each below
/// `2^width`: the bit width, then one bit-packed run (the last group of eight
/// padded with zeros).
pub fn pack_indices(values: impl Iterator<Item = u32>, count: usize, width: u32) -> Vec<u8> {
    let width = width.clamp(1, 32);
    let mut out = Vec::with_capacity(count.div_ceil(8) * width as usize + 16);
    out.push(width as u8);
    packed_run(&mut out, values, count, width);
    out
}

/// Append one run-length run of the RLE / bit-packed hybrid to a page body:
/// `count` times `value`, for values of `width` bits. (Nothing for no values.)
#[inline(always)]
pub fn rle_run(out: &mut Vec<u8>, value: u32, count: usize, width: u32) {
    if count == 0 {
        return;
    }
    // This runs once per person: straight into `out`, with no buffer of its
    // own (every thread would queue up at the allocator) and no call to copy
    // a byte or two. A run takes at most 10 + 4 bytes.
    out.reserve(16);
    unsafe {
        let mut at = out.as_mut_ptr().add(out.len());
        let mut v = (count as u64) << 1;
        while v >= 0x80 {
            *at = v as u8 | 0x80;
            at = at.add(1);
            v >>= 7;
        }
        *at = v as u8;
        (at.add(1) as *mut u32).write_unaligned(value);
        at = at.add(1 + (width.clamp(1, 32) as usize).div_ceil(8));
        out.set_len(at.offset_from(out.as_ptr()) as usize);
    }
}

#[inline]
fn varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// Append one bit-packed run of the hybrid to a page body: `count` values of
/// `width` bits (1 to 32), the last group of eight padded with zeros. It must
/// be the page's last run unless `count` is a multiple of eight. (Nothing for
/// no values.)
pub fn packed_run(out: &mut Vec<u8>, values: impl Iterator<Item = u32>, count: usize, width: u32) {
    if count == 0 {
        return;
    }
    let groups = count.div_ceil(8);
    varint(out, (groups as u64) << 1 | 1);
    let start = out.len();
    let bytes = groups * width as usize;
    // Room for whole-word writes past the end.
    out.resize(start + bytes + 8, 0);
    // At most `count` values are taken, so the writes stay inside the buffer:
    // 32 bits at a time while packing, and one last word of up to 8 bytes.
    let (mut acc, mut nbits) = (0u64, 0u32);
    unsafe {
        let mut at = out.as_mut_ptr().add(start);
        for v in values.take(count) {
            acc |= (v as u64) << nbits;
            nbits += width;
            if nbits >= 32 {
                (at as *mut u32).write_unaligned(acc as u32);
                at = at.add(4);
                acc >>= 32;
                nbits -= 32;
            }
        }
        (at as *mut u64).write_unaligned(acc);
    }
    out.truncate(start + bytes);
}

/// The number of bits that hold every index below `n`.
pub fn index_width(n: usize) -> u32 {
    (usize::BITS - n.saturating_sub(1).leading_zeros()).max(1)
}

/// A dictionary for small non-negative integers: the distinct values in order
/// of first appearance, and a table from value to index. `None` if a value is
/// negative or too large for a direct table.
pub fn small_dict(values: &[i32]) -> Option<(Vec<i32>, Vec<u32>)> {
    let (mut min, mut max) = (i32::MAX, i32::MIN);
    for &v in values {
        min = min.min(v);
        max = max.max(v);
    }
    if values.is_empty() || min < 0 || max >= 1 << 22 {
        return None;
    }
    let mut index = vec![u32::MAX; max as usize + 1];
    let mut dict = Vec::new();
    for &v in values {
        let slot = unsafe { index.get_unchecked_mut(v as usize) };
        if *slot == u32::MAX {
            *slot = dict.len() as u32;
            dict.push(v);
        }
    }
    Some((dict, index))
}

struct Thrift {
    b: Vec<u8>,
}

impl Thrift {
    fn varint(&mut self, mut v: u64) {
        while v >= 0x80 {
            self.b.push(v as u8 | 0x80);
            v >>= 7;
        }
        self.b.push(v as u8);
    }
    fn zigzag(&mut self, v: i64) {
        self.varint(((v << 1) ^ (v >> 63)) as u64);
    }
    fn i32(&mut self, delta: u8, v: i32) {
        self.b.push(delta << 4 | 5);
        self.zigzag(v as i64);
    }
    fn i64(&mut self, delta: u8, v: i64) {
        self.b.push(delta << 4 | 6);
        self.zigzag(v);
    }
    fn binary(&mut self, delta: u8, s: &[u8]) {
        self.b.push(delta << 4 | 8);
        self.varint(s.len() as u64);
        self.b.extend_from_slice(s);
    }
    fn list(&mut self, delta: u8, elem: u8, n: usize) {
        self.b.push(delta << 4 | 9);
        if n < 15 {
            self.b.push((n as u8) << 4 | elem);
        } else {
            self.b.push(0xF0 | elem);
            self.varint(n as u64);
        }
    }
    fn begin(&mut self, delta: u8) {
        self.b.push(delta << 4 | 12);
    }
    fn end(&mut self) {
        self.b.push(0);
    }
}

fn dictionary_page_header(num_values: usize, size: usize) -> Vec<u8> {
    let mut t = Thrift { b: Vec::with_capacity(32) };
    t.i32(1, 2); // DICTIONARY_PAGE
    t.i32(1, size as i32);
    t.i32(1, size as i32);
    t.begin(4); // dictionary_page_header (field 7)
    t.i32(1, num_values as i32);
    t.i32(1, 0); // PLAIN
    t.end();
    t.end();
    t.b
}

fn page_header(num_values: usize, size: usize, encoding: i32) -> Vec<u8> {
    let mut t = Thrift { b: Vec::with_capacity(32) };
    t.i32(1, 0); // DATA_PAGE
    t.i32(1, size as i32);
    t.i32(1, size as i32);
    t.begin(2); // data_page_header (field 5)
    t.i32(1, num_values as i32);
    t.i32(1, encoding); // PLAIN (0) or RLE_DICTIONARY (8)
    t.i32(1, 3); // definition levels: RLE
    t.i32(1, 3); // repetition levels: RLE
    t.end();
    t.end();
    t.b
}

/// The definition-levels section of a v1 page for an OPTIONAL column: a 4-byte
/// length, then one bit-packed run (bit width 1) of `valid`, LSB first.
pub fn def_levels(valid: &[bool]) -> Vec<u8> {
    let groups = valid.len().div_ceil(8);
    let mut body = Thrift { b: Vec::with_capacity(groups + 8) };
    body.varint((groups as u64) << 1 | 1);
    let start = body.b.len();
    body.b.resize(start + groups, 0);
    for (i, &v) in valid.iter().enumerate() {
        if v {
            body.b[start + i / 8] |= 1 << (i % 8);
        }
    }
    let mut out = Vec::with_capacity(body.b.len() + 4);
    out.extend_from_slice(&(body.b.len() as u32).to_le_bytes());
    out.extend_from_slice(&body.b);
    out
}

/// The same section when the bit-packed bytes already exist: returns only the
/// 4-byte length and run header that go in front of `bits`.
pub fn def_levels_header(bits: &[u8]) -> Vec<u8> {
    let mut run = Thrift { b: Vec::with_capacity(8) };
    run.varint((bits.len() as u64) << 1 | 1);
    let mut out = Vec::with_capacity(12);
    out.extend_from_slice(&((run.b.len() + bits.len()) as u32).to_le_bytes());
    out.extend_from_slice(&run.b);
    out
}

pub fn write(path: &Path, rows: usize, columns: &[Column]) -> std::io::Result<()> {
    let mut f = File::create(path)?;
    let mut offset = 4usize;
    f.write_all(b"PAR1")?;
    // (chunk offset, data page offset, total size) per column
    let mut chunks: Vec<(usize, usize, usize)> = Vec::new();
    if rows > 0 {
        for c in columns {
            let start = offset;
            if let Some((dict, count)) = &c.dict {
                let size: usize = dict.iter().map(|p| p.len()).sum();
                let header = dictionary_page_header(*count, size);
                f.write_all(&header)?;
                for p in dict {
                    f.write_all(p)?;
                }
                offset += header.len() + size;
            }
            let size: usize = c.payload.iter().map(|p| p.len()).sum();
            let header = page_header(rows, size, if c.dict.is_some() { 8 } else { 0 });
            f.write_all(&header)?;
            for p in &c.payload {
                f.write_all(p)?;
            }
            chunks.push((start, offset, offset + header.len() + size - start));
            offset += header.len() + size;
        }
    }

    let mut t = Thrift { b: Vec::with_capacity(1024) };
    t.i32(1, 1); // version
    t.list(1, 12, columns.len() + 1); // schema
    t.binary(4, b"schema");
    t.i32(1, columns.len() as i32);
    t.end();
    for c in columns {
        t.i32(1, c.ptype);
        t.i32(2, c.optional as i32); // REQUIRED = 0, OPTIONAL = 1
        t.binary(1, c.name.as_bytes());
        match c.kind {
            Kind::Int => {}
            Kind::Str => {
                t.i32(2, 0); // converted_type UTF8
                t.begin(4); // logicalType
                t.begin(1); // STRING
                t.end();
                t.end();
            }
            Kind::Date => {
                t.i32(2, 6); // converted_type DATE
                t.begin(4); // logicalType
                t.begin(6); // DATE
                t.end();
                t.end();
            }
        }
        t.end();
    }
    t.i64(1, rows as i64); // num_rows
    if rows > 0 {
        t.list(1, 12, 1); // row_groups
        t.list(1, 12, columns.len()); // columns
        let mut total = 0usize;
        for (c, &(at, data_at, size)) in columns.iter().zip(&chunks) {
            total += size;
            t.i64(2, at as i64); // file_offset
            t.begin(1); // meta_data
            t.i32(1, c.ptype);
            if c.dict.is_some() {
                t.list(1, 5, 3);
                t.zigzag(0); // PLAIN (the dictionary)
                t.zigzag(3); // RLE (levels, and the hybrid the indices are in)
                t.zigzag(8); // RLE_DICTIONARY
            } else if c.optional {
                t.list(1, 5, 2);
                t.zigzag(0); // PLAIN
                t.zigzag(3); // RLE
            } else {
                t.list(1, 5, 1);
                t.zigzag(0);
            }
            t.list(1, 8, 1); // path_in_schema
            t.varint(c.name.len() as u64);
            t.b.extend_from_slice(c.name.as_bytes());
            t.i32(1, 0); // codec UNCOMPRESSED
            t.i64(1, rows as i64); // num_values
            t.i64(1, size as i64); // total_uncompressed_size
            t.i64(1, size as i64); // total_compressed_size
            t.i64(2, data_at as i64); // data_page_offset (field 9)
            if c.dict.is_some() {
                t.i64(2, at as i64); // dictionary_page_offset (field 11)
            }
            t.end();
            t.end();
        }
        t.i64(1, total as i64); // total_byte_size
        t.i64(1, rows as i64); // num_rows
        t.end();
    } else {
        t.list(1, 12, 0);
    }
    t.binary(2, b"shopping-sessions"); // created_by
    t.end();
    f.write_all(&t.b)?;
    f.write_all(&(t.b.len() as u32).to_le_bytes())?;
    f.write_all(b"PAR1")?;
    crate::sys::start_writeback(f);
    Ok(())
}

pub fn as_bytes<T: Copy>(v: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}
