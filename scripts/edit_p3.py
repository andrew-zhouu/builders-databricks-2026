#!/usr/bin/env python3
"""One-off source edit: campaign counters are updated through a short ring
(prefetch now, write later); block chains ask for their next block early."""
import sys

root = sys.argv[1]

def rep(q, old, new, count=1):
    assert q.count(old) == count, (old[:70], q.count(old))
    return q.replace(old, new)

q = open(f"{root}/pass3.rs").read()
q = rep(q, """use crate::parse::{CAMPAIGN_BITS, CANCEL, CLICK_SHIFT, LOCAL_BITS, TS_MASK};
""", """use core::arch::x86_64::{_mm_prefetch, _MM_HINT_ET0};

use crate::parse::{CAMPAIGN_BITS, CANCEL, CLICK_SHIFT, LOCAL_BITS, TS_MASK};
""")
q = rep(q, """pub struct Scratch {
    /// `start[l]..start[l + 1]` are the events of person `l` in the sorted buffer.
""", """/// Updates of the campaign counters wait in a ring of this many: a counter's
/// cache line is asked for when its update is known and written that many
/// updates later, when it has arrived. (The tables are far larger than what is
/// left of the cache beside a partition's events, and a session's campaign is
/// not known any earlier.)
const WAITING: usize = 16;

pub struct Scratch {
    wait_credit_at: [u32; WAITING],
    wait_credit: [Credit; WAITING],
    wait_assist: [u32; WAITING],
    /// How many updates have gone through each ring.
    credits: usize,
    assists: usize,
    /// `start[l]..start[l + 1]` are the events of person `l` in the sorted buffer.
""")
q = rep(q, """        Scratch {
            start: vec![0; LOCALS + 1],
""", """        Scratch {
            wait_credit_at: [0; WAITING],
            wait_credit: [Credit::default(); WAITING],
            wait_assist: [u32::MAX; WAITING],
            credits: 0,
            assists: 0,
            start: vec![0; LOCALS + 1],
""")
q = rep(q, """#[inline(always)]
fn click_ts(w0: u64) -> u32 {""", """impl Scratch {
    /// One session credited to the counters at `cell` (`campaign * ndays + day`).
    #[inline(always)]
    fn credit(&mut self, cell: usize, purchasing: bool, revenue: u64) {
        unsafe {
            _mm_prefetch::<_MM_HINT_ET0>(self.acc.credit.as_ptr().add(cell) as *const i8);
            let k = self.credits & (WAITING - 1);
            self.credits += 1;
            // The update that has waited longest (at first, nothing added to cell 0).
            let add = *self.wait_credit.get_unchecked(k);
            let c = self.acc.credit.get_unchecked_mut(*self.wait_credit_at.get_unchecked(k) as usize);
            c.sessions += add.sessions;
            c.purchasing += add.purchasing;
            c.revenue += add.revenue;
            *self.wait_credit_at.get_unchecked_mut(k) = cell as u32;
            *self.wait_credit.get_unchecked_mut(k) = Credit { sessions: 1, purchasing: purchasing as u32, revenue };
        }
    }

    /// One session assisted by the campaign and day of `cell`.
    #[inline(always)]
    fn assist(&mut self, cell: usize) {
        unsafe {
            _mm_prefetch::<_MM_HINT_ET0>(self.acc.assisted.as_ptr().add(cell) as *const i8);
            let k = self.assists & (WAITING - 1);
            self.assists += 1;
            let old = *self.wait_assist.get_unchecked(k);
            if old != u32::MAX {
                *self.acc.assisted.get_unchecked_mut(old as usize) += 1;
            }
            *self.wait_assist.get_unchecked_mut(k) = cell as u32;
        }
    }

    /// Write the updates still waiting.
    fn settle_counters(&mut self) {
        for k in 0..WAITING {
            let add = std::mem::take(&mut self.wait_credit[k]);
            let c = &mut self.acc.credit[self.wait_credit_at[k] as usize];
            c.sessions += add.sessions;
            c.purchasing += add.purchasing;
            c.revenue += add.revenue;
            let old = std::mem::replace(&mut self.wait_assist[k], u32::MAX);
            if old != u32::MAX {
                self.acc.assisted[old as usize] += 1;
            }
        }
    }
}

#[inline(always)]
fn click_ts(w0: u64) -> u32 {""")
q = rep(q, """        let cell = unsafe { sc.acc.credit.get_unchecked_mut(campaign * ndays + day as usize) };
        cell.sessions += 1;
        cell.purchasing += purchasing as u32;
        cell.revenue += revenue;
""", """        sc.credit(campaign * ndays + day as usize, purchasing, revenue);
""")
q = rep(q, """                    if *stamp != sc.tag {
                        *stamp = sc.tag;
                        *sc.acc.assisted.get_unchecked_mut(camp * ndays + day as usize) += 1;
                    }
""", """                    if *stamp != sc.tag {
                        *stamp = sc.tag;
                        sc.assist(camp * ndays + day as usize);
                    }
""")
q = rep(q, """        sc.present = present;
        t0 = ctx.charge_rows(crate::P_S_PEOPLE, t0, ts.len());
""", """        sc.present = present;
        sc.settle_counters();
        t0 = ctx.charge_rows(crate::P_S_PEOPLE, t0, ts.len());
""")
open(f"{root}/pass3.rs", "w").write(q)

p = open(f"{root}/part.rs").read()
p = rep(p, """    /// Calls `f` with each run of words of partition `p`, in order.
    #[inline(always)]
    pub fn for_each_run(&self, pool: &BlockPool, p: usize, mut f: impl FnMut(&[u64])) {
        let mut rem = self.words[p] as usize;
        for &b in self.part_blocks(p) {
            let n = rem.min(BLOCK_WORDS);
""", """    /// Calls `f` with each run of words of partition `p`, in order.
    #[inline(always)]
    pub fn for_each_run(&self, pool: &BlockPool, p: usize, mut f: impl FnMut(&[u64])) {
        let mut rem = self.words[p] as usize;
        let blocks = self.part_blocks(p);
        for (i, &b) in blocks.iter().enumerate() {
            ahead(pool, blocks, i);
            let n = rem.min(BLOCK_WORDS);
""")
p = rep(p, """        let mut rem = self.words[p] as usize;
        out.reserve(rem);
        for &b in self.part_blocks(p) {
            let n = rem.min(BLOCK_WORDS);
""", """        let mut rem = self.words[p] as usize;
        out.reserve(rem);
        let blocks = self.part_blocks(p);
        for (i, &b) in blocks.iter().enumerate() {
            ahead(pool, blocks, i);
            let n = rem.min(BLOCK_WORDS);
""")
p = rep(p, """#[repr(align(64))]
#[derive(Clone, Copy)]
struct Line([u64; 8]);
""", """#[repr(align(64))]
#[derive(Clone, Copy)]
struct Line([u64; 8]);

/// A chain's blocks lie anywhere in the pool, and the processor follows a
/// stream of reads only to the end of a block: about to read block `i`, ask
/// for the start of the one after it.
#[inline(always)]
fn ahead(pool: &BlockPool, blocks: &[u32], i: usize) {
    if let Some(&next) = blocks.get(i + 1) {
        let q = pool.ptr(next) as *const i8;
        for line in 0..4 {
            unsafe { _mm_prefetch::<_MM_HINT_T0>(q.add(64 * line)) };
        }
    }
}
""")
open(f"{root}/part.rs", "w").write(p)
print("ok")
