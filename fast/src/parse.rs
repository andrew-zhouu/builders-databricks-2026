//! Chunk parsers: links -> union-find, clicks -> person partitions,
//! events -> event-id partitions (deduplicated in pass 2).
//!
//! Record layouts (64-bit words):
//!   event   w0 = event_key(43) << 21 | ingest_rel_seconds(21)
//!           w1 = valid(1) | purchase(1) | 0(4) | person(27) | ts_rel_ms(31)
//!           a valid purchase is followed by a second record: (price << 32 | category, 0)
//!   click   w0 = person_local(12) | ts_rel_ms(31) | campaign(21)
//!           w1 = click key (order-preserving, see ids.rs)

use std::sync::atomic::Ordering::Relaxed;

use crate::ids::{event_key, EVENT_IRREGULAR};
use crate::lookup::ABSENT;
use crate::part::PartWriter;
use crate::util::{for_each_line, parse_price, parse_ts, split, DAY_MS};
use crate::{Chunk, Ctx};

pub const VALID: u64 = 1 << 63;
pub const PURCHASE: u64 = 1 << 62;
pub const INGEST_BITS: u32 = 21;
pub const INGEST_MAX: i64 = (1 << INGEST_BITS) - 1;
pub const PERSON_BITS: u32 = 27;
pub const TS_MASK: u64 = (1 << 31) - 1;
pub const LOCAL_BITS: u32 = 12;
pub const CAMPAIGN_BITS: u32 = 21;

#[inline(always)]
pub fn event_hash(ek: u64) -> u64 {
    ek.wrapping_mul(0xD6E8_FEB8_6659_FD93)
}

pub fn links_chunk(ctx: &Ctx, ch: &Chunk) {
    let file = &ctx.files[ch.file as usize];
    for_each_line(file.slice(), ch.start, ch.end, |line| {
        if let Some([a, b]) = split::<2>(line) {
            if a.is_empty() || b.is_empty() {
                return;
            }
            let sa = ctx.table.insert(ctx.users.key(a));
            let sb = ctx.table.insert(ctx.users.key(b));
            if sa != sb {
                ctx.table.union(sa, sb, |x, y| ctx.users.less(x, y));
            }
        }
    });
    file.release(ch.start, ch.end);
}

#[inline]
fn event_line(ctx: &Ctx, d: &mut PartWriter, line: &[u8]) {
    let Some(f) = split::<7>(line) else { return };
    if f[0].is_empty() {
        return;
    }
    // ingest_ts is always a valid F1 timestamp by the rules; a line where it is
    // not cannot be ordered against its copies, so it is treated as malformed.
    let Some(ingest) = parse_ts(f[3]) else { return };
    let ek = match event_key(f[0]) {
        Some(k) => k,
        None => EVENT_IRREGULAR | ctx.event_dict.intern(f[0]),
    };
    let ingest_rel = ingest.div_euclid(1000) - ctx.ingest_base_s;
    if ingest_rel < 0 {
        // Too far before the window to order exactly in 21 bits: leave the
        // fast path altogether (never happens with generated data).
        ctx.bail.store(true, Relaxed);
    }
    let w0 = ek << INGEST_BITS | ingest_rel.clamp(0, INGEST_MAX) as u64;
    let part = (event_hash(ek) >> 40) as usize & ctx.pd_mask;

    let mut valid = !f[1].is_empty() && !f[5].is_empty();
    let mut ts = 0i64;
    match parse_ts(f[2]) {
        Some(t) if t >= ctx.ws && t < ctx.we => ts = t,
        _ => valid = false,
    }
    let purchase = match f[4] {
        b"view" | b"cart" => false,
        b"purchase" => true,
        _ => {
            valid = false;
            false
        }
    };
    let mut price = 0u32;
    if purchase {
        match parse_price(f[6]) {
            Some(p) => price = p,
            None => valid = false,
        }
    }
    if valid && ingest - ts > DAY_MS {
        valid = false;
    }
    if !valid {
        d.push2(&ctx.pool, part, w0, 0);
        return;
    }
    let slot = ctx.table.insert(ctx.users.key(f[1]));
    let person = ctx.table.person(slot) as u64;
    let w1 = VALID | person << 31 | (ts - ctx.t0) as u64;
    if purchase {
        let cat = ctx.products.category(f[5]) as u64;
        d.push2(&ctx.pool, part, w0, w1 | PURCHASE);
        d.push2(&ctx.pool, part, (price as u64) << 32 | cat, 0);
    } else {
        d.push2(&ctx.pool, part, w0, w1);
    }
}

pub fn events_chunk(ctx: &Ctx, d: &mut PartWriter, ch: &Chunk) {
    let file = &ctx.files[ch.file as usize];
    for_each_line(file.slice(), ch.start, ch.end, |line| event_line(ctx, d, line));
    file.release(ch.start, ch.end);
}

#[inline]
fn click_line(ctx: &Ctx, c: &mut PartWriter, line: &[u8]) {
    let Some(f) = split::<4>(line) else { return };
    if f[0].is_empty() || f[1].is_empty() {
        return;
    }
    let campaign = ctx.campaigns.index.get(f[2]);
    if campaign == ABSENT {
        return;
    }
    let ts = match parse_ts(f[3]) {
        Some(t) if t >= ctx.t0 && t < ctx.we => t,
        _ => return,
    };
    let slot = ctx.table.insert(ctx.users.key(f[1]));
    let person = ctx.table.person(slot) as u64;
    let local = person & ((1 << LOCAL_BITS) - 1);
    let w0 = local << 52 | ((ts - ctx.t0) as u64) << CAMPAIGN_BITS | campaign as u64;
    c.push2(&ctx.pool, (person >> LOCAL_BITS) as usize, w0, ctx.clicks.key(f[0]));
}

pub fn clicks_chunk(ctx: &Ctx, c: &mut PartWriter, ch: &Chunk) {
    let file = &ctx.files[ch.file as usize];
    for_each_line(file.slice(), ch.start, ch.end, |line| click_line(ctx, c, line));
    file.release(ch.start, ch.end);
}
