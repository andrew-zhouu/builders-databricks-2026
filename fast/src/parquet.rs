//! A minimal Parquet writer: one row group, one PLAIN uncompressed v1 data page
//! per column. Page payloads are borrowed slices, written without copying.

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
    /// The page body, in order (levels, then PLAIN values).
    pub payload: Vec<&'a [u8]>,
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

fn page_header(num_values: usize, size: usize) -> Vec<u8> {
    let mut t = Thrift { b: Vec::with_capacity(32) };
    t.i32(1, 0); // DATA_PAGE
    t.i32(1, size as i32);
    t.i32(1, size as i32);
    t.begin(2); // data_page_header (field 5)
    t.i32(1, num_values as i32);
    t.i32(1, 0); // PLAIN
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
    // (data page offset, total size) per column
    let mut chunks: Vec<(usize, usize)> = Vec::new();
    if rows > 0 {
        for c in columns {
            let size: usize = c.payload.iter().map(|p| p.len()).sum();
            let header = page_header(rows, size);
            f.write_all(&header)?;
            for p in &c.payload {
                f.write_all(p)?;
            }
            chunks.push((offset, header.len() + size));
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
        for (c, &(at, size)) in columns.iter().zip(&chunks) {
            total += size;
            t.i64(2, at as i64); // file_offset
            t.begin(1); // meta_data
            t.i32(1, c.ptype);
            if c.optional {
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
            t.i64(2, at as i64); // data_page_offset
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
    Ok(())
}

pub fn as_bytes<T: Copy>(v: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}
