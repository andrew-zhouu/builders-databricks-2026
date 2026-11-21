//! Chunk parsers: links -> union-find, clicks -> person partitions,
//! events -> event-id partitions (deduplicated in pass 2).
//!
//! Every line is first tried on a SIMD fast path that only accepts lines it
//! understands completely (field count, quoting, regular IDs, timestamp forms
//! it can decide). Anything else goes to the scalar functions, which implement
//! the rules literally. Both paths emit the same records.
//!
//! Record layouts (64-bit words):
//!   event   w0 = event_key_remainder(<= 43) << 21 | ingest_rel_seconds(21)
//!           w1 = valid(1) | purchase(1) | 0(4) | person(27) | ts_rel_ms(31)
//!           a valid purchase is followed by a second record: (price << 32 | category, 0)
//!   click   w0 = person_local(12) | ts_rel_ms(31) | campaign(21)
//!           w1 = click key (order-preserving, see ids.rs)
//!
//! An event key (49 bits) is multiplied by an odd constant modulo 2^49, a
//! bijection; the top bits of the product choose the partition and only the
//! rest is stored, so within a partition equal remainders mean equal IDs.
//!
//! Vector code is AVX2 only, and multiplies stay in 128-bit registers: wider
//! or heavier instructions lower the core's frequency licence on Cascade Lake.

use core::arch::x86_64::*;
use std::sync::atomic::Ordering::Relaxed;

use crate::ids::{event_key, EVENT_IRREGULAR, EVENT_KEY_BITS};
use crate::lookup::ABSENT;
use crate::part::PartWriter;
use crate::util::{civil_from_days, days_from_civil, days_in_month, find_newline, parse_price, parse_ts, split, DAY_MS};
use crate::{Chunk, Ctx};

pub const VALID: u64 = 1 << 63;
pub const PURCHASE: u64 = 1 << 62;
pub const INGEST_BITS: u32 = 21;
pub const INGEST_MAX: i64 = (1 << INGEST_BITS) - 1;
pub const PERSON_BITS: u32 = 27;
pub const TS_MASK: u64 = (1 << 31) - 1;
pub const LOCAL_BITS: u32 = 12;
pub const CAMPAIGN_BITS: u32 = 21;
/// Fewest event partitions: the remainder must fit in 64 - 21 bits.
pub const MIN_PD_BITS: u32 = EVENT_KEY_BITS + INGEST_BITS - 64;

/// Bytes that must be readable after a line start for the fast paths.
const MARGIN: usize = 256;
/// Lines this close to the file start use the scalar path (backward loads).
const HEAD: usize = 16;

/// Partition and stored remainder of an event key.
#[inline(always)]
fn place(ctx: &Ctx, ek: u64) -> (usize, u64) {
    let m = ek.wrapping_mul(0x9E37_79B9_7F4A_7C15) & ((1 << EVENT_KEY_BITS) - 1);
    ((m >> ctx.rem_bits) as usize, m & ((1 << ctx.rem_bits) - 1))
}

// ------------------------------------------------------------------ dates

const BAD_DAY: i32 = i32::MIN;

/// Day numbers for every valid date of three years around the window, indexed
/// by `month * 32 + day`. Timestamps in other years go to the scalar parser.
pub struct DateTab {
    /// ASCII of the three years (row 3 never matches).
    years: [u32; 4],
    /// Row for a year's last digit (3 = none).
    row: [u8; 16],
    days: [[i32; 512]; 4],
}

impl DateTab {
    pub fn new(ws: i64) -> DateTab {
        let (y, _, _) = civil_from_days(ws.div_euclid(DAY_MS));
        let mut t = DateTab { years: [u32::MAX; 4], row: [3; 16], days: [[BAD_DAY; 512]; 4] };
        for k in 0..3 {
            let yy = y - 1 + k as i64;
            if !(0..=9999).contains(&yy) {
                continue;
            }
            let s = format!("{yy:04}");
            t.years[k] = u32::from_le_bytes(s.as_bytes().try_into().unwrap());
            t.row[(yy % 10) as usize] = k as u8;
            for m in 1..=12 {
                for d in 1..=days_in_month(yy, m) {
                    t.days[k][(m * 32 + d) as usize] = days_from_civil(yy, m, d) as i32;
                }
            }
        }
        t
    }
}

/// Parses `YYYY-MM-DDTHH:MM:SS` at `a` (19 bytes; byte 19 is not looked at).
/// Returns (shape ok, year in table, calendar and clock values valid, epoch seconds).
#[inline(always)]
unsafe fn datetime(dt: &DateTab, a: *const u8) -> (bool, bool, bool, i64) {
    let y = (a as *const u32).read_unaligned();
    let row = *dt.row.get_unchecked((y >> 24 & 15) as usize) as usize;
    let known = y == *dt.years.get_unchecked(row);
    let v = _mm_loadu_si128(a.add(4) as *const __m128i);
    // "-MM-DDTHH:MM:SS?" minus the expected separators / '0'
    let pattern = _mm_setr_epi8(
        b'-' as i8, 48, 48, b'-' as i8, 48, 48, b'T' as i8, 48, 48, b':' as i8, 48, 48, b':' as i8, 48, 48, 0,
    );
    let limit = _mm_setr_epi8(0, 9, 9, 0, 9, 9, 0, 9, 9, 0, 9, 9, 0, 9, 9, -1);
    let t = _mm_sub_epi8(v, pattern);
    let shape = _mm_movemask_epi8(_mm_cmpeq_epi8(_mm_max_epu8(t, limit), limit)) == 0xFFFF;
    let digits = _mm_shuffle_epi8(t, _mm_setr_epi8(1, 2, 4, 5, 7, 8, 10, 11, 13, 14, -1, -1, -1, -1, -1, -1));
    // 16-bit lanes: month, day, hour, minute, second
    let pairs = _mm_maddubs_epi16(digits, _mm_setr_epi8(10, 1, 10, 1, 10, 1, 10, 1, 10, 1, 0, 0, 0, 0, 0, 0));
    let over = _mm_movemask_epi8(_mm_cmpgt_epi16(
        pairs,
        _mm_setr_epi16(12, 31, 23, 59, 59, i16::MAX, i16::MAX, i16::MAX),
    )) != 0;
    // 32-bit lanes: month * 32 + day, hour * 3600 + minute * 60, second, 0
    let q = _mm_madd_epi16(pairs, _mm_setr_epi16(32, 1, 3600, 60, 1, 0, 0, 0));
    // add the seconds into lane 1 so one move brings out (date index, second of day)
    let q = _mm_add_epi32(q, _mm_shuffle_epi32::<0b11_11_10_11>(q));
    let x = _mm_cvtsi128_si64(q) as u64;
    let days = *dt.days.get_unchecked(row).get_unchecked(x as usize & 511);
    (shape, known, !over & (days != BAD_DAY), days as i64 * 86_400 + (x >> 32) as i64)
}

const fn last_masks() -> [[u8; 16]; 17] {
    let mut t = [[0u8; 16]; 17];
    let mut n = 0;
    while n <= 16 {
        let mut i = 16 - n;
        while i < 16 {
            t[n][i] = 0xFF;
            i += 1;
        }
        n += 1;
    }
    t
}
/// `LAST[n]`: the last `n` of 16 bytes set.
static LAST: [[u8; 16]; 17] = last_masks();

/// The value of the `n` (1 to 16) ASCII digits that end at `end`.
/// Returns (all digits, value).
#[inline(always)]
unsafe fn digits16(end: *const u8, n: usize) -> (bool, u64) {
    let v = _mm_loadu_si128(end.sub(16) as *const __m128i);
    let t = _mm_sub_epi8(v, _mm_set1_epi8(48));
    let keep = _mm_loadu_si128(LAST.get_unchecked(n).as_ptr() as *const __m128i);
    let isdigit = _mm_cmpeq_epi8(_mm_min_epu8(t, _mm_set1_epi8(9)), t);
    let d = _mm_and_si128(t, keep);
    let a = _mm_maddubs_epi16(d, _mm_set1_epi16(0x010A));
    let b = _mm_madd_epi16(a, _mm_set1_epi32(0x0001_0064));
    let c = _mm_packus_epi32(b, b);
    let e = _mm_madd_epi16(c, _mm_set1_epi32(0x0001_2710));
    let x = _mm_cvtsi128_si64(e) as u64;
    (_mm_testc_si128(isdigit, keep) != 0, (x & 0xFFFF_FFFF) * 100_000_000 + (x >> 32))
}

/// A `ts` field of `n` bytes at `a`. Returns (decided here, valid, epoch ms).
/// Only the form the length allows is parsed.
#[inline(always)]
unsafe fn timestamp(dt: &DateTab, a: *const u8, n: usize) -> (bool, bool, i64) {
    if n.wrapping_sub(1) < 16 {
        // F3: digits only. Anything else of this length is simply invalid.
        let (digits, value) = digits16(a.add(n), n);
        return (true, digits, value as i64);
    }
    if (n == 20) | (n == 25) {
        // F1 / F2. A field of these lengths that is not shaped like a date may
        // still be all digits (F3), and an unknown year needs the full
        // calendar: both go to the scalar path.
        let (shape, known, cal, secs) = datetime(dt, a);
        let mut suffix = *a.add(19) == b'Z';
        let mut off = 0i64;
        if n == 25 {
            let s = *a.add(19);
            let h1 = (*a.add(20)).wrapping_sub(b'0') as i64;
            let h2 = (*a.add(21)).wrapping_sub(b'0') as i64;
            let m1 = (*a.add(23)).wrapping_sub(b'0') as i64;
            let m2 = (*a.add(24)).wrapping_sub(b'0') as i64;
            let (oh, om) = (h1 * 10 + h2, m1 * 10 + m2);
            suffix = ((s == b'+') | (s == b'-'))
                & (h1 <= 9)
                & (h2 <= 9)
                & (m1 <= 9)
                & (m2 <= 9)
                & (*a.add(22) == b':')
                & (oh <= 23)
                & (om <= 59);
            off = (oh * 60 + om) * 60_000;
            if s == b'-' {
                off = -off;
            }
        }
        return (shape & known, cal & suffix, secs * 1000 - off);
    }
    // Empty is invalid; longer digit strings need 64-bit care: scalar path.
    (n == 0, false, 0)
}

// ------------------------------------------------------------------ ids

const PEXT6: u64 = 0x5F5F_5F5F_5F5F_5F5F;

/// For two 16-byte strings (low and high half of the result): which bytes are in `[0-9a-z]`.
#[inline(always)]
unsafe fn alnum_bits(lo: *const u8, hi: *const u8) -> u32 {
    let v = _mm256_inserti128_si256::<1>(
        _mm256_castsi128_si256(_mm_loadu_si128(lo as *const __m128i)),
        _mm_loadu_si128(hi as *const __m128i),
    );
    let d = _mm256_sub_epi8(v, _mm256_set1_epi8(0x30));
    let l = _mm256_sub_epi8(v, _mm256_set1_epi8(0x61));
    let isd = _mm256_cmpeq_epi8(_mm256_min_epu8(d, _mm256_set1_epi8(9)), d);
    let isl = _mm256_cmpeq_epi8(_mm256_min_epu8(l, _mm256_set1_epi8(25)), l);
    _mm256_movemask_epi8(_mm256_or_si256(isd, isl)) as u32
}

/// The 54-bit key of the `n` (1 to 9) regular characters at `a` (see ids::key9).
#[inline(always)]
unsafe fn pack9(a: *const u8, n: usize) -> u64 {
    let w = _bzhi_u64((a as *const u64).read_unaligned(), 8u32.wrapping_mul(n as u32));
    let hi = _pext_u64(w.swap_bytes(), PEXT6);
    let c8 = _pext_u64(*a.add(8) as u64, 0x5F) & 0u64.wrapping_sub((n == 9) as u64);
    hi << 6 | c8
}

/// The 48-bit key of a regular event ID of `n` (1 to 9) bytes at `a` (see ids::event_key).
#[inline(always)]
unsafe fn pack_event(a: *const u8, n: usize) -> u64 {
    let w = (a.add(1) as *const u64).read_unaligned();
    _pext_u64(_bzhi_u64(w, 8u32.wrapping_mul((n as u32).wrapping_sub(1))), PEXT6)
}

// ------------------------------------------------------------------ scanning

#[inline(always)]
unsafe fn masks64(p: *const u8) -> (u64, u64, u64) {
    let v0 = _mm256_loadu_si256(p as *const __m256i);
    let v1 = _mm256_loadu_si256(p.add(32) as *const __m256i);
    let m = |c: u8| {
        let c = _mm256_set1_epi8(c as i8);
        (_mm256_movemask_epi8(_mm256_cmpeq_epi8(v0, c)) as u32 as u64)
            | (_mm256_movemask_epi8(_mm256_cmpeq_epi8(v1, c)) as u32 as u64) << 32
    };
    (m(b','), m(b'\n'), m(b'"'))
}

/// Quote check for the fields ended by the delimiters in `d`, given the quote
/// bits `q` of those fields. Returns (nonzero if malformed, per-field "is quoted" bits).
///
/// A field is quoted iff its first and last bytes are quotes; any other quote
/// makes the line malformed. With `f` = fields starting with a quote and `l` =
/// fields ending with one, the line is well-formed iff `f == l` and the number
/// of quotes is exactly `2 * popcount(f)`.
#[inline(always)]
unsafe fn quotes(q: u64, d: u64) -> (u64, u64) {
    let f = _pext_u64(q >> 1, d) << 1 | (q & 1);
    let l = _pext_u64(q << 1, d);
    ((f ^ l) | (q.count_ones() as u64 ^ 2 * f.count_ones() as u64), f)
}

#[inline(always)]
fn first_line(data: &[u8], start: usize) -> Option<usize> {
    find_newline(data, start.saturating_sub(1).min(data.len())).map(|i| i + 1)
}

// ------------------------------------------------------------------ links

#[inline(never)]
fn link_line(ctx: &Ctx, line: &[u8]) {
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
}

/// Returns the bytes consumed, or 0 if the line needs the scalar path.
#[inline(always)]
unsafe fn link_fast(ctx: &Ctx, ring: &mut LinkRing, p: *const u8) -> usize {
    let v = _mm256_loadu_si256(p as *const __m256i);
    let m = |c: u8| _mm256_movemask_epi8(_mm256_cmpeq_epi8(v, _mm256_set1_epi8(c as i8))) as u32 as u64;
    let (cm, nm, qm) = (m(b','), m(b'\n'), m(b'"'));
    let nl = nm.trailing_zeros() as usize;
    let below = 1u64.wrapping_shl(nl as u32).wrapping_sub(1);
    let cmb = cm & below;
    let c0 = cmb.trailing_zeros() as usize;
    let (bad, f) = quotes(qm & below, cmb | 1u64.wrapping_shl(nl as u32));
    if (nl >= 32) | (cmb == 0) | (cmb & cmb.wrapping_sub(1) != 0) | (bad != 0) {
        return 0;
    }
    let (f0, f1) = ((f & 1) as usize, (f >> 1 & 1) as usize);
    let (a0, n0) = (p.add(f0), c0 - 2 * f0);
    let (a1, n1) = (p.add(c0 + 1 + f1), nl - c0 - 1 - 2 * f1);
    if (n0 == 0) | (n1 == 0) {
        return nl + 1;
    }
    let need = _bzhi_u32(0xFFFF, n0 as u32) | _bzhi_u32(0xFFFF, n1 as u32) << 16;
    if (n0 > 9) | (n1 > 9) | (alnum_bits(a0, a1) & need != need) {
        return 0;
    }
    ring.push(ctx, (pack9(a0, n0), pack9(a1, n1)));
    nl + 1
}

/// Links on their way into the union-find, two stages behind the parser so
/// that each memory access has been prefetched: first the two home slots
/// (insert), then the two current parents (union).
struct LinkRing {
    /// Stage 1: the two user keys; `(0, 0)` = empty.
    keys: [(u64, u64); RING],
    /// Stage 2: the two slots; equal = nothing to do.
    slots: [(u32, u32); RING],
    at: usize,
}

impl LinkRing {
    fn new() -> LinkRing {
        LinkRing { keys: [(0, 0); RING], slots: [(0, 0); RING], at: 0 }
    }

    #[inline(always)]
    fn push(&mut self, ctx: &Ctx, e: (u64, u64)) {
        if e.0 != 0 {
            ctx.table.prefetch_for_insert(e.0);
            ctx.table.prefetch_for_insert(e.1);
        }
        let i = self.at;
        self.at = (i + 1) & (RING - 1);
        let k = std::mem::replace(unsafe { self.keys.get_unchecked_mut(i) }, e);
        let mut s = (0u32, 0u32);
        if k.0 != 0 {
            s = (ctx.table.insert(k.0), ctx.table.insert(k.1));
            ctx.table.prefetch_parent(s.0);
            ctx.table.prefetch_parent(s.1);
        }
        let old = std::mem::replace(unsafe { self.slots.get_unchecked_mut(i) }, s);
        if old.0 != old.1 {
            // Roots may be irregular IDs inserted by the scalar path: full comparison.
            ctx.table.union(old.0, old.1, |x, y| ctx.users.less(x, y));
        }
    }

    fn drain(&mut self, ctx: &Ctx) {
        for _ in 0..2 * RING {
            self.push(ctx, (0, 0));
        }
    }
}

pub fn links_chunk(ctx: &Ctx, ch: &Chunk) {
    let file = &ctx.files[ch.file as usize];
    let data = file.slice();
    let len = data.len();
    if let Some(mut pos) = first_line(data, ch.start) {
        let fast_end = if ctx.scalar_only { 0 } else { len.saturating_sub(MARGIN) };
        let mut ring = LinkRing::new();
        while pos < ch.end {
            if pos < fast_end {
                let used = unsafe { link_fast(ctx, &mut ring, data.as_ptr().add(pos)) };
                if used != 0 {
                    pos += used;
                    continue;
                }
            }
            let e = find_newline(data, pos).unwrap_or(len);
            link_line(ctx, &data[pos..e]);
            pos = e + 1;
        }
        ring.drain(ctx);
    }
    file.release(ch.start, ch.end);
}

// ------------------------------------------------------------------ events

/// An event waiting for its user-table slot (prefetched when queued).
#[derive(Clone, Copy)]
struct PendingEvent {
    w0: u64,
    w1: u64,
    key: u64,
    extra: u64,
    part: u32,
}

const RING: usize = 8;

struct EventRing {
    buf: [PendingEvent; RING],
    at: usize,
}

impl EventRing {
    fn new() -> EventRing {
        EventRing { buf: [PendingEvent { w0: 0, w1: 0, key: 0, extra: 0, part: 0 }; RING], at: 0 }
    }

    #[inline(always)]
    fn emit(ctx: &Ctx, d: &mut PartWriter, e: &PendingEvent) {
        if e.key != 0 {
            let person = ctx.table.person(ctx.table.insert(e.key)) as u64;
            d.push2(&ctx.pool, e.part as usize, e.w0, e.w1 | person << 31);
            if e.w1 & PURCHASE != 0 {
                d.push2(&ctx.pool, e.part as usize, e.extra, 0);
            }
        }
    }

    #[inline(always)]
    fn push(&mut self, ctx: &Ctx, d: &mut PartWriter, e: PendingEvent) {
        ctx.table.prefetch(e.key);
        let old = std::mem::replace(unsafe { self.buf.get_unchecked_mut(self.at) }, e);
        self.at = (self.at + 1) & (RING - 1);
        Self::emit(ctx, d, &old);
    }

    fn drain(&mut self, ctx: &Ctx, d: &mut PartWriter) {
        for i in 0..RING {
            let e = self.buf[(self.at + i) & (RING - 1)];
            Self::emit(ctx, d, &e);
            self.buf[(self.at + i) & (RING - 1)].key = 0;
        }
    }
}

#[inline(always)]
fn ingest_field(ctx: &Ctx, ingest_s: i64) -> u64 {
    let rel = ingest_s - ctx.ingest_base_s;
    if rel < 0 {
        // Too far before the window to order exactly in 21 bits: leave the
        // fast pipeline altogether (never happens with generated data).
        ctx.bail.store(true, Relaxed);
    }
    rel.clamp(0, INGEST_MAX) as u64
}

/// The literal rules for one event line.
#[inline(never)]
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
    let (part, rem) = place(ctx, ek);
    let w0 = rem << INGEST_BITS | ingest_field(ctx, ingest.div_euclid(1000));

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
    let person = ctx.table.person(ctx.table.insert(ctx.users.key(f[1]))) as u64;
    let w1 = VALID | person << 31 | (ts - ctx.t0) as u64;
    if purchase {
        let cat = ctx.products().category(f[5]) as u64;
        d.push2(&ctx.pool, part, w0, w1 | PURCHASE);
        d.push2(&ctx.pool, part, (price as u64) << 32 | cat, 0);
    } else {
        d.push2(&ctx.pool, part, w0, w1);
    }
}

/// Returns the bytes consumed, or 0 if the line needs the scalar path.
#[inline(always)]
unsafe fn event_fast(ctx: &Ctx, d: &mut PartWriter, ring: &mut EventRing, p: *const u8) -> usize {
    // Window 1: event_id, user_id, ts and their three commas.
    let (cm, nm, qm) = masks64(p);
    let c0 = cm.trailing_zeros() as usize;
    let cm1 = cm & cm.wrapping_sub(1);
    let c1 = cm1.trailing_zeros() as usize;
    let cm2 = cm1 & cm1.wrapping_sub(1);
    let c2 = cm2.trailing_zeros() as usize;
    let cm3 = cm2 & cm2.wrapping_sub(1);
    // Three commas, and no newline before the third.
    let mut bad = (nm.trailing_zeros() as usize <= c2) as u64;
    let (qbad, fa) = quotes(qm & 1u64.wrapping_shl(c2 as u32).wrapping_sub(1), cm ^ cm3);
    bad |= qbad;

    // Window 2: ingest_ts, event_type, product_id, price_cents and the newline.
    let p2 = p.add((c2 & 63) + 1);
    let (cm, nm, qm) = masks64(p2);
    let nl = nm.trailing_zeros() as usize;
    let below = 1u64.wrapping_shl(nl as u32).wrapping_sub(1);
    let cmb = cm & below;
    let c3 = cmb.trailing_zeros() as usize;
    let cmb1 = cmb & cmb.wrapping_sub(1);
    let c4 = cmb1.trailing_zeros() as usize;
    let cmb2 = cmb1 & cmb1.wrapping_sub(1);
    let c5 = cmb2.trailing_zeros() as usize;
    let cmb3 = cmb2 & cmb2.wrapping_sub(1);
    // The newline is in the window, with exactly three commas before it.
    bad |= ((nl == 64) | (cmb2 == 0) | (cmb3 != 0)) as u64;
    let (qbad, fb) = quotes(qm & below, cmb | 1u64.wrapping_shl(nl as u32));
    bad |= qbad;
    if bad != 0 {
        return 0;
    }

    let q = |f: u64, i: u32| (f >> i & 1) as usize;
    let (q0, q1, q2) = (q(fa, 0), q(fa, 1), q(fa, 2));
    let (q3, q4, q5) = (q(fb, 0), q(fb, 1), q(fb, 2));
    let (a0, n0) = (p.add(q0), c0 - 2 * q0);
    let (a1, n1) = (p.add(c0 + 1 + q1), c1 - c0 - 1 - 2 * q1);
    let (a2, n2) = (p.add(c1 + 1 + q2), c2 - c1 - 1 - 2 * q2);
    let (a3, n3) = (p2.add(q3), c3 - 2 * q3);
    let (a4, n4) = (p2.add(c3 + 1 + q4), c4 - c3 - 1 - 2 * q4);
    let n5 = c5 - c4 - 1 - 2 * q5;
    let used = c2 + 1 + nl + 1;

    // IDs: `e` + up to 8 regular characters, and up to 9 regular characters
    // (or an empty user_id, which only makes the event invalid).
    // Anything else goes to the scalar path: an empty or irregular event_id, an
    // irregular user_id, an ingest_ts that is not plain F1, a timestamp this
    // path cannot decide.
    let need = _bzhi_u32(0xFFFF, (n0 as u32).wrapping_sub(1)) | _bzhi_u32(0xFFFF, n1 as u32) << 16;
    if (n0.wrapping_sub(1) >= 9) | (*a0 != b'e') | (n1 > 9) | (alnum_bits(a0.add(1), a1) & need != need) {
        return 0;
    }
    let dt = &ctx.dates;
    let (i_shape, i_known, i_cal, ingest_s) = datetime(dt, a3);
    if !((n3 == 20) & i_shape & i_known & i_cal & (*a3.add(19) == b'Z')) {
        return 0;
    }
    let (ts_decided, ts_valid, ts) = timestamp(dt, a2, n2);
    if !ts_decided {
        return 0;
    }

    let w = (a4 as *const u64).read_unaligned();
    let purchase = (n4 == 8) & (w == u64::from_le_bytes(*b"purchase"));
    let view = (n4 == 4) & ((w as u32 == u32::from_le_bytes(*b"view")) | (w as u32 == u32::from_le_bytes(*b"cart")));
    let mut valid = (n1 != 0)
        & (n5 != 0)
        & (purchase | view)
        & ts_valid
        & (ts >= ctx.ws)
        & (ts < ctx.we)
        & (ingest_s * 1000 - ts <= DAY_MS);
    let (part, rem) = place(ctx, pack_event(a0, n0));
    let w0 = rem << INGEST_BITS | ingest_field(ctx, ingest_s);
    let mut extra = 0u64;
    let mut w1 = VALID | (ts.wrapping_sub(ctx.t0) as u64 & TS_MASK);
    if purchase {
        let q6 = q(fb, 3);
        let price = std::slice::from_raw_parts(p2.add(c5 + 1 + q6), nl - c5 - 1 - 2 * q6);
        match parse_price(price) {
            Some(price) => {
                let cat = ctx.products().category(std::slice::from_raw_parts(p2.add(c4 + 1 + q5), n5)) as u64;
                extra = (price as u64) << 32 | cat;
                w1 |= PURCHASE;
            }
            None => valid = false,
        }
    }
    if valid {
        ring.push(ctx, d, PendingEvent { w0, w1, key: pack9(a1, n1), extra, part: part as u32 });
    } else {
        d.push2(&ctx.pool, part, w0, 0);
    }
    used
}

pub fn events_chunk(ctx: &Ctx, d: &mut PartWriter, ch: &Chunk) {
    let file = &ctx.files[ch.file as usize];
    let data = file.slice();
    let len = data.len();
    if let Some(mut pos) = first_line(data, ch.start) {
        let fast_end = if ctx.scalar_only { 0 } else { len.saturating_sub(MARGIN) };
        let mut ring = EventRing::new();
        while pos < ch.end {
            if (pos >= HEAD) & (pos < fast_end) {
                if ctx.prefetch_ahead != 0 {
                    // The hardware streamer stops at page boundaries; reach past them.
                    unsafe {
                        _mm_prefetch::<_MM_HINT_T0>(data.as_ptr().wrapping_add(pos + ctx.prefetch_ahead) as *const i8)
                    };
                }
                let used = unsafe { event_fast(ctx, d, &mut ring, data.as_ptr().add(pos)) };
                if used != 0 {
                    pos += used;
                    continue;
                }
            }
            let e = find_newline(data, pos).unwrap_or(len);
            event_line(ctx, d, &data[pos..e]);
            pos = e + 1;
        }
        ring.drain(ctx, d);
    }
    file.release(ch.start, ch.end);
}

// ------------------------------------------------------------------ clicks

/// The literal rules for one click line.
#[inline(never)]
fn click_line(ctx: &Ctx, c: &mut PartWriter, line: &[u8]) {
    let Some(f) = split::<4>(line) else { return };
    if f[0].is_empty() || f[1].is_empty() {
        return;
    }
    let campaign = ctx.campaigns().index.get(f[2]);
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

/// A click waiting for its user-table slot (prefetched when queued).
#[derive(Clone, Copy)]
struct PendingClick {
    /// ts_rel << CAMPAIGN_BITS | campaign
    tc: u64,
    click: u64,
    /// The user key. 0 = empty.
    user: u64,
}

struct ClickRing {
    buf: [PendingClick; RING],
    at: usize,
}

impl ClickRing {
    fn new() -> ClickRing {
        ClickRing { buf: [PendingClick { tc: 0, click: 0, user: 0 }; RING], at: 0 }
    }

    #[inline(always)]
    fn step(&mut self, ctx: &Ctx, c: &mut PartWriter, new: PendingClick) {
        if new.user != 0 {
            ctx.table.prefetch(new.user);
        }
        let old = std::mem::replace(unsafe { self.buf.get_unchecked_mut(self.at) }, new);
        self.at = (self.at + 1) & (RING - 1);
        if old.user != 0 {
            let person = ctx.table.person(ctx.table.insert(old.user)) as u64;
            let local = person & ((1 << LOCAL_BITS) - 1);
            c.push2(&ctx.pool, (person >> LOCAL_BITS) as usize, local << 52 | old.tc, old.click);
        }
    }

    fn drain(&mut self, ctx: &Ctx, c: &mut PartWriter) {
        for _ in 0..RING {
            self.step(ctx, c, PendingClick { tc: 0, click: 0, user: 0 });
        }
    }
}

/// Returns the bytes consumed, or 0 if the line needs the scalar path.
#[inline(always)]
unsafe fn click_fast(ctx: &Ctx, c: &mut PartWriter, ring: &mut ClickRing, p: *const u8) -> usize {
    let (cm, nm, qm) = masks64(p);
    let nl = nm.trailing_zeros() as usize;
    let below = 1u64.wrapping_shl(nl as u32).wrapping_sub(1);
    let cmb = cm & below;
    let c0 = cmb.trailing_zeros() as usize;
    let cmb1 = cmb & cmb.wrapping_sub(1);
    let c1 = cmb1.trailing_zeros() as usize;
    let cmb2 = cmb1 & cmb1.wrapping_sub(1);
    let c2 = cmb2.trailing_zeros() as usize;
    let cmb3 = cmb2 & cmb2.wrapping_sub(1);
    let (qbad, f) = quotes(qm & below, cmb | 1u64.wrapping_shl(nl as u32));
    if (nl == 64) | (cmb2 == 0) | (cmb3 != 0) | (qbad != 0) {
        return 0;
    }
    let q = |i: u32| (f >> i & 1) as usize;
    let (q0, q1, q2, q3) = (q(0), q(1), q(2), q(3));
    let (a0, n0) = (p.add(q0), c0 - 2 * q0);
    let (a1, n1) = (p.add(c0 + 1 + q1), c1 - c0 - 1 - 2 * q1);
    let (a2, n2) = (p.add(c1 + 1 + q2), c2 - c1 - 1 - 2 * q2);
    let (a3, n3) = (p.add(c2 + 1 + q3), nl - c2 - 1 - 2 * q3);
    let used = nl + 1;
    if (n0 == 0) | (n1 == 0) {
        return used;
    }
    let need = _bzhi_u32(0xFFFF, n0 as u32) | _bzhi_u32(0xFFFF, n1 as u32) << 16;
    if (n0 > 9) | (n1 > 9) | (alnum_bits(a0, a1) & need != need) {
        return 0;
    }
    let (ts_decided, ts_valid, ts) = timestamp(&ctx.dates, a3, n3);
    if !ts_decided {
        return 0;
    }
    // `c_<n>` without a leading zero goes straight to the table.
    let (digits, number) = digits16(a2.add(n2), (n2.wrapping_sub(3) & 15) + 1);
    let canonical = (n2.wrapping_sub(3) < 7)
        & ((a2 as *const u16).read_unaligned() == u16::from_le_bytes(*b"c_"))
        & digits
        & (*a2.add(2) != b'0')
        & (number < crate::lookup::DIRECT_LIMIT as u64);
    let campaign = if canonical {
        ctx.campaigns().index.by_number(number as usize)
    } else {
        ctx.campaigns().index.get(std::slice::from_raw_parts(a2, n2))
    };
    if (campaign != ABSENT) & ts_valid & (ts >= ctx.t0) & (ts < ctx.we) {
        let tc = ((ts - ctx.t0) as u64) << CAMPAIGN_BITS | campaign as u64;
        ring.step(ctx, c, PendingClick { tc, click: pack9(a0, n0), user: pack9(a1, n1) });
    }
    used
}

pub fn clicks_chunk(ctx: &Ctx, c: &mut PartWriter, ch: &Chunk) {
    let file = &ctx.files[ch.file as usize];
    let data = file.slice();
    let len = data.len();
    if let Some(mut pos) = first_line(data, ch.start) {
        let fast_end = if ctx.scalar_only { 0 } else { len.saturating_sub(MARGIN) };
        let mut ring = ClickRing::new();
        while pos < ch.end {
            if (pos >= HEAD) & (pos < fast_end) {
                let used = unsafe { click_fast(ctx, c, &mut ring, data.as_ptr().add(pos)) };
                if used != 0 {
                    pos += used;
                    continue;
                }
            }
            let e = find_newline(data, pos).unwrap_or(len);
            click_line(ctx, c, &data[pos..e]);
            pos = e + 1;
        }
        ring.drain(ctx, c);
    }
    file.release(ch.start, ch.end);
}
