//! Radix partitioning into chains of 4 KB blocks.
//!
//! Each thread owns a `PartWriter` per stream. Records are 64-bit words; every
//! partition has one cache line of write-combining buffer that is flushed with a
//! non-temporal store when full. Blocks come from one global bump pool, or from
//! a writer's free list of blocks another stage has finished reading.

use core::arch::x86_64::*;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use crate::sys::map_anon;

pub const BLOCK_BYTES: usize = 4096;
pub const BLOCK_WORDS: usize = BLOCK_BYTES / 8;
const SPAN: usize = 16;

pub struct BlockPool {
    base: *mut u8,
    nblocks: usize,
    next: AtomicUsize,
}
unsafe impl Send for BlockPool {}
unsafe impl Sync for BlockPool {}

impl BlockPool {
    pub fn new(nblocks: usize) -> BlockPool {
        BlockPool { base: map_anon(nblocks * BLOCK_BYTES, true), nblocks, next: AtomicUsize::new(0) }
    }

    #[inline(always)]
    pub fn ptr(&self, block: u32) -> *mut u64 {
        unsafe { self.base.add(block as usize * BLOCK_BYTES) as *mut u64 }
    }

    fn alloc_span(&self) -> usize {
        let s = self.next.fetch_add(SPAN, Relaxed);
        if s + SPAN > self.nblocks {
            eprintln!("block pool exhausted");
            std::process::abort();
        }
        s
    }

    pub fn used_bytes(&self) -> usize {
        self.next.load(Relaxed) * BLOCK_BYTES
    }
}

#[repr(align(64))]
#[derive(Clone, Copy)]
struct Line([u64; 8]);

pub struct PartWriter {
    pub nparts: usize,
    wc: Vec<Line>,
    fill: Vec<u8>,
    cur: Vec<*mut u64>,
    lines: Vec<u32>,
    log: Vec<(u32, u32)>,
    span_next: usize,
    span_end: usize,
    /// Blocks free to reuse (filled by the stage that consumed them).
    pub free: Vec<u32>,
    // Built by `finalize`.
    pstart: Vec<u32>,
    pblocks: Vec<u32>,
    words: Vec<u32>,
}
unsafe impl Send for PartWriter {}
unsafe impl Sync for PartWriter {}

impl PartWriter {
    pub fn new(nparts: usize) -> PartWriter {
        PartWriter {
            nparts,
            wc: vec![Line([0; 8]); nparts],
            fill: vec![0; nparts],
            cur: vec![std::ptr::null_mut(); nparts],
            lines: vec![0; nparts],
            log: Vec::new(),
            span_next: 0,
            span_end: 0,
            free: Vec::new(),
            pstart: Vec::new(),
            pblocks: Vec::new(),
            words: Vec::new(),
        }
    }

    /// Append a two-word record to partition `p`.
    #[inline(always)]
    pub fn push2(&mut self, pool: &BlockPool, p: usize, a: u64, b: u64) {
        unsafe {
            let n = *self.fill.get_unchecked(p) as usize;
            let line = self.wc.get_unchecked_mut(p);
            *line.0.get_unchecked_mut(n) = a;
            *line.0.get_unchecked_mut(n + 1) = b;
            if n == 6 {
                self.flush(pool, p);
                *self.fill.get_unchecked_mut(p) = 0;
            } else {
                *self.fill.get_unchecked_mut(p) = (n + 2) as u8;
            }
        }
    }

    /// Append a one-word record to partition `p`. Do not mix with `push2`.
    #[inline(always)]
    pub fn push1(&mut self, pool: &BlockPool, p: usize, a: u64) {
        unsafe {
            let n = *self.fill.get_unchecked(p) as usize;
            *self.wc.get_unchecked_mut(p).0.get_unchecked_mut(n) = a;
            if n == 7 {
                self.flush(pool, p);
                *self.fill.get_unchecked_mut(p) = 0;
            } else {
                *self.fill.get_unchecked_mut(p) = (n + 1) as u8;
            }
        }
    }

    #[inline(always)]
    unsafe fn flush(&mut self, pool: &BlockPool, p: usize) {
        let mut c = *self.cur.get_unchecked(p);
        if (c as usize) & (BLOCK_BYTES - 1) == 0 {
            let b = self.take_block(pool);
            self.log.push((p as u32, b));
            c = pool.ptr(b);
        }
        // Two 256-bit halves: no 512-bit instructions anywhere (frequency licence).
        let src = self.wc.get_unchecked(p).0.as_ptr() as *const __m256i;
        _mm256_stream_si256(c as *mut __m256i, _mm256_load_si256(src));
        _mm256_stream_si256((c as *mut __m256i).add(1), _mm256_load_si256(src.add(1)));
        *self.cur.get_unchecked_mut(p) = c.add(8);
        *self.lines.get_unchecked_mut(p) += 1;
    }

    #[inline(never)]
    fn take_block(&mut self, pool: &BlockPool) -> u32 {
        if let Some(b) = self.free.pop() {
            return b;
        }
        if self.span_next == self.span_end {
            self.span_next = pool.alloc_span();
            self.span_end = self.span_next + SPAN;
        }
        let b = self.span_next;
        self.span_next += 1;
        b as u32
    }

    /// Flush partial lines and build the per-partition block index. After this
    /// the writer is read-only and other threads may read it.
    pub fn finalize(&mut self, pool: &BlockPool) {
        self.words = vec![0; self.nparts];
        for p in 0..self.nparts {
            let n = self.fill[p];
            self.words[p] = self.lines[p] * 8 + n as u32;
            if n > 0 {
                unsafe { self.flush(pool, p) };
                self.fill[p] = 0;
            }
        }
        unsafe { _mm_sfence() };
        let mut start = vec![0u32; self.nparts + 1];
        for &(p, _) in &self.log {
            start[p as usize + 1] += 1;
        }
        for p in 0..self.nparts {
            start[p + 1] += start[p];
        }
        let mut at = start.clone();
        let mut blocks = vec![0u32; self.log.len()];
        for &(p, b) in &self.log {
            blocks[at[p as usize] as usize] = b;
            at[p as usize] += 1;
        }
        self.pstart = start;
        self.pblocks = blocks;
    }

    #[inline]
    pub fn part_words(&self, p: usize) -> usize {
        self.words[p] as usize
    }

    #[inline]
    pub fn part_blocks(&self, p: usize) -> &[u32] {
        &self.pblocks[self.pstart[p] as usize..self.pstart[p + 1] as usize]
    }

    /// Append every word of partition `p` to `out`.
    pub fn copy_part(&self, pool: &BlockPool, p: usize, out: &mut Vec<u64>) {
        let mut rem = self.words[p] as usize;
        out.reserve(rem);
        for &b in self.part_blocks(p) {
            let n = rem.min(BLOCK_WORDS);
            unsafe {
                let dst = out.as_mut_ptr().add(out.len());
                std::ptr::copy_nonoverlapping(pool.ptr(b) as *const u64, dst, n);
                out.set_len(out.len() + n);
            }
            rem -= n;
        }
    }
}
