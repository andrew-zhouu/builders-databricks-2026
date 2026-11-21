//! Pass 2: deduplicate one event-id partition and scatter the surviving events
//! to person partitions.
//!
//! Dedup comes before validation: of all well-formed copies of an event the one
//! with the smallest ingest_ts is kept, and only if that copy is valid does the
//! event survive. `w0 = key << 21 | ingest`, so comparing `w0` of two copies of
//! one event compares their ingest times.

use crate::parse::{INGEST_BITS, LOCAL_BITS, PERSON_BITS, PURCHASE, TS_MASK};
use crate::part::PartWriter;
use crate::Ctx;

#[derive(Default)]
pub struct Scratch {
    recs: Vec<u64>,
    tab: Vec<u32>,
    dead: Vec<u8>,
}

pub fn run(ctx: &Ctx, sc: &mut Scratch, s: &mut PartWriter, ps: &mut PartWriter, p: usize) {
    let t0 = ctx.now();
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
    let t0 = ctx.charge_rows(crate::P_D_GATHER, t0, n);
    let tsize = (2 * n).next_power_of_two().max(16);
    sc.tab.clear();
    sc.tab.resize(tsize, 0);
    sc.dead.clear();
    sc.dead.resize(n, 0);
    let mask = tsize - 1;
    let shift = 64 - tsize.trailing_zeros();
    let tab = &mut sc.tab[..];
    let dead = &mut sc.dead[..];

    let mut i = 0;
    while i < n {
        let w0 = recs[2 * i];
        let w1 = recs[2 * i + 1];
        // The stored remainder identifies the event within this partition.
        let ek = w0 >> INGEST_BITS;
        let mut slot = (ek.wrapping_mul(0xD6E8_FEB8_6659_FD93) >> shift) as usize;
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

    let t0 = ctx.charge_rows(crate::P_D_DEDUP, t0, n);
    let mut i = 0;
    while i < n {
        let w1 = recs[2 * i + 1];
        let purchase = w1 & PURCHASE != 0;
        if dead[i] == 0 && (w1 as i64) < 0 {
            let person = (w1 >> 31) & ((1 << PERSON_BITS) - 1);
            let rec = (person & ((1 << LOCAL_BITS) - 1)) << 31 | (w1 & TS_MASK);
            let sp = (person >> LOCAL_BITS) as usize;
            s.push1(&ctx.pool, sp, rec);
            if purchase {
                ps.push2(&ctx.pool, sp, rec, recs[2 * i + 2]);
            }
        }
        i += 1 + purchase as usize;
    }
    ctx.charge_rows(crate::P_D_EMIT, t0, n);
}
