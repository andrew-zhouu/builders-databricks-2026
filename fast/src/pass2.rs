//! Pass 2: deduplicate one event-id partition and scatter the surviving events
//! to person partitions.
//!
//! Dedup comes before validation: of all well-formed copies of an event the one
//! with the smallest ingest_ts is kept, and only if that copy is valid does the
//! event survive. `w0 = key << 21 | ingest`, so comparing `w0` of two copies of
//! one event compares their ingest times.

use crate::parse::{event_hash, INGEST_BITS, LOCAL_BITS, PURCHASE, SLOT_BITS, TS_MASK};
use crate::part::PartWriter;
use crate::Ctx;

#[derive(Default)]
pub struct Scratch {
    recs: Vec<u64>,
    tab: Vec<u32>,
    dead: Vec<u8>,
}

/// A survivor waiting for its person (the slot's root), prefetched when queued.
#[derive(Clone, Copy, Default)]
struct Pending {
    ts: u64,
    extra: u64,
    slot: u32,
    /// 0 = empty, 1 = event, 2 = purchase
    kind: u32,
}

const RING: usize = 16;

#[inline(always)]
fn emit(ctx: &Ctx, s: &mut PartWriter, ps: &mut PartWriter, e: &Pending) {
    if e.kind != 0 {
        let person = ctx.table.person(e.slot) as u64;
        let rec = (person & ((1 << LOCAL_BITS) - 1)) << 31 | e.ts;
        let sp = (person >> LOCAL_BITS) as usize;
        s.push1(&ctx.pool, sp, rec);
        if e.kind == 2 {
            ps.push2(&ctx.pool, sp, rec, e.extra);
        }
    }
}

pub fn run(ctx: &Ctx, sc: &mut Scratch, s: &mut PartWriter, ps: &mut PartWriter, p: usize) {
    sc.recs.clear();
    for t in 0..ctx.nthreads {
        let d = ctx.d(t);
        d.copy_part(&ctx.pool, p, &mut sc.recs);
        // These blocks are never read again: reuse them for this thread's output.
        s.free.extend_from_slice(d.part_blocks(p));
    }
    let recs = &sc.recs[..];
    let n = recs.len() / 2;
    if n == 0 {
        return;
    }
    let tsize = (2 * n).next_power_of_two().max(16);
    sc.tab.clear();
    sc.tab.resize(tsize, 0);
    sc.dead.clear();
    sc.dead.resize(n, 0);
    let mask = tsize - 1;
    let tab = &mut sc.tab[..];
    let dead = &mut sc.dead[..];

    let mut i = 0;
    while i < n {
        let w0 = recs[2 * i];
        let w1 = recs[2 * i + 1];
        let ek = w0 >> INGEST_BITS;
        let mut slot = (event_hash(ek) >> 8) as usize & mask;
        loop {
            let e = tab[slot];
            if e == 0 {
                tab[slot] = i as u32 + 1;
                break;
            }
            let o = (e - 1) as usize;
            let ow0 = recs[2 * o];
            if ow0 >> INGEST_BITS == ek {
                if w0 < ow0 {
                    dead[o] = 1;
                    tab[slot] = i as u32 + 1;
                } else {
                    dead[i] = 1;
                }
                break;
            }
            slot = (slot + 1) & mask;
        }
        i += 1 + (w1 & PURCHASE != 0) as usize;
    }

    let mut ring = [Pending::default(); RING];
    let mut at = 0;
    let mut i = 0;
    while i < n {
        let w1 = recs[2 * i + 1];
        let purchase = w1 & PURCHASE != 0;
        if dead[i] == 0 && (w1 as i64) < 0 {
            let slot = ((w1 >> 31) & ((1 << SLOT_BITS) - 1)) as u32;
            ctx.table.prefetch_person(slot);
            let new = Pending {
                ts: w1 & TS_MASK,
                extra: if purchase { recs[2 * i + 2] } else { 0 },
                slot,
                kind: 1 + purchase as u32,
            };
            let old = std::mem::replace(&mut ring[at], new);
            at = (at + 1) & (RING - 1);
            emit(ctx, s, ps, &old);
        }
        i += 1 + purchase as usize;
    }
    for e in &ring {
        emit(ctx, s, ps, e);
    }
}
