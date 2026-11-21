//! Chunk parsers: links -> union-find; clicks and events -> person partitions.
//!
//! Every line is first tried on a SIMD fast path that only accepts lines it
//! understands completely (field count, quoting, regular IDs, timestamp forms
//! it can decide). Anything else goes to the scalar functions, which implement
//! the rules literally. Both paths emit the same records.
//!
//! Dedup is by exception. An event that passes validation is written straight
//! to its person's partition, as if no event ID were ever repeated. Every
//! well-formed event line, valid or not, also leaves an 8-byte *trace* in a
//! partition chosen by a hash of its event ID: a fingerprint of the ID and the
//! position of the line in the input. Pass 2 looks only at the traces: where a
//! fingerprint repeats it reads those lines again, applies the dedup rule
//! exactly, and writes a cancellation for every valid copy that is not the
//! kept one. Most event IDs are unique and cost nothing beyond their trace.
//!
//! Records (64-bit words):
//!   trace     fingerprint << loc_bits | position of the line among all event files
//!   event     [cancel(1) at bit 43] | person_local(12) | ts_rel_ms(31)
//!   purchase  the event word, then price << 32 | category
//!   click     person_local(12) | ts_rel_ms(31) | campaign(21), then the click key
//!
//! Vector code is AVX2 only, and multiplies stay in 128-bit registers: wider
//! or heavier instructions lower the core's frequency licence on Cascade Lake.

use core::arch::x86_64::*;

use crate::ids::{event_hash, mix16, raw_key_fast};
use crate::lookup::ABSENT;
use crate::part::PartWriter;
use crate::util::{civil_from_days, days_from_civil, days_in_month, find_newline, parse_price, parse_ts, split, DAY_MS};
use crate::{Chunk, Ctx};

pub const PERSON_BITS: u32 = 27;
pub const TS_MASK: u64 = (1 << 31) - 1;
pub const LOCAL_BITS: u32 = 12;
pub const CAMPAIGN_BITS: u32 = 21;
/// Marks a cancellation among event and purchase records.
pub const CANCEL: u64 = 1 << (31 + LOCAL_BITS);

/// Bytes that must be readable after a line start for the fast paths.
const MARGIN: usize = 256;
/// Lines this close to the file start use the scalar path (backward loads).
const HEAD: usize = 16;

/// The per-thread writers an event can go to.
pub struct EventOut<'a> {
    /// Traces, partitioned by event-ID hash.
    pub k: &'a mut PartWriter,
    /// Events, partitioned by person.
    pub s: &'a mut PartWriter,
    /// Purchases, partitioned by person.
    pub ps: &'a mut PartWriter,
}

/// The trace partition and the trace word (without the position) of an event-ID hash.
#[inline(always)]
pub fn trace_of(ctx: &Ctx, h: u64) -> (usize, u64) {
    ((((h >> 32) * ctx.pd as u64) >> 32) as usize, (h & ctx.fp_mask) << ctx.loc_bits)
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
    /// `YYYY-MM-DD` (zero-padded to 16 bytes) of each UTC day from the first
    /// day a click may fall on to the last day of the event window.
    day_text: Vec<[u8; 16]>,
    /// Epoch ms of the start of the first of those days.
    first_day_ms: i64,
}

impl DateTab {
    pub fn new(ws: i64, first_day: i64, ndays: usize) -> DateTab {
        let (y, _, _) = civil_from_days(ws.div_euclid(DAY_MS));
        let mut t = DateTab {
            years: [u32::MAX; 4],
            row: [3; 16],
            days: [[BAD_DAY; 512]; 4],
            day_text: Vec::new(),
            first_day_ms: first_day * DAY_MS,
        };
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
        for d in 0..ndays as i64 + 1 {
            let (y, m, dd) = civil_from_days(first_day + d);
            let mut text = [0u8; 16];
            // Years outside 0000-9999 cannot be written as F1: leave the text
            // empty, so that no ingest date ever compares equal to it.
            if (0..=9999).contains(&y) {
                text[..10].copy_from_slice(format!("{y:04}-{m:02}-{dd:02}").as_bytes());
            }
            t.day_text.push(text);
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

/// Where the SIMD paths may run in a file of `len` bytes.
#[inline(always)]
fn fast_end(ctx: &Ctx, len: usize) -> usize {
    if ctx.scalar_only {
        0
    } else {
        len.saturating_sub(MARGIN)
    }
}

#[inline(always)]
fn prefetch_input(ctx: &Ctx, data: &[u8], pos: usize) {
    if ctx.prefetch_ahead != 0 {
        // The hardware streamer stops at page boundaries; reach past them.
        unsafe { _mm_prefetch::<_MM_HINT_T0>(data.as_ptr().wrapping_add(pos + ctx.prefetch_ahead) as *const i8) };
    }
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
    let (r0, k0) = raw_key_fast(a0, n0, b'u');
    let (r1, k1) = raw_key_fast(a1, n1, b'u');
    if !(r0 & r1) {
        return 0;
    }
    ring.push(ctx, (k0, k1));
    nl + 1
}

const RING: usize = 8;

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

pub fn links_chunk(ctx: &Ctx, ch: &Chunk) -> usize {
    let data = ctx.files[ch.file as usize].slice();
    let (len, fast_end) = (data.len(), fast_end(ctx, data.len()));
    let mut lines = 0usize;
    if let Some(mut pos) = first_line(data, ch.start) {
        let mut ring = LinkRing::new();
        while pos < ch.end {
            lines += 1;
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
    lines
}

// ------------------------------------------------------------------ events

/// One well-formed event line, by the rules.
pub struct Event<'a> {
    pub id: &'a [u8],
    /// Epoch ms.
    pub ingest: i64,
    /// Passed validation and is not late.
    pub valid: bool,
    pub user: &'a [u8],
    /// Epoch ms; meaningful when valid.
    pub ts: i64,
    pub purchase: bool,
    pub price: u32,
    pub product: &'a [u8],
}

/// The rules for one event line, literally. `None`: the line is dropped before
/// dedup (malformed, or an empty event_id).
pub fn parse_event<'a>(ctx: &Ctx, line: &'a [u8]) -> Option<Event<'a>> {
    let f = split::<7>(line)?;
    if f[0].is_empty() {
        return None;
    }
    // ingest_ts is always a valid F1 timestamp by the rules; a line where it is
    // not cannot be ordered against its copies, so it is treated as malformed.
    if f[3].len() != 20 {
        return None;
    }
    let ingest = parse_ts(f[3])?;
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
    Some(Event { id: f[0], ingest, valid, user: f[1], ts, purchase: purchase && valid, price, product: f[5] })
}

/// The event and purchase records of a valid event: (person partition, event word, purchase word).
#[inline]
pub fn event_records(ctx: &Ctx, e: &Event) -> (usize, u64, u64) {
    let person = ctx.table.person(ctx.table.insert(ctx.users.key(e.user))) as u64;
    let rec = (person & ((1 << LOCAL_BITS) - 1)) << 31 | (e.ts - ctx.t0) as u64;
    let extra = if e.purchase { (e.price as u64) << 32 | ctx.products().category(e.product) as u64 } else { 0 };
    ((person >> LOCAL_BITS) as usize, rec, extra)
}

/// The scalar path for one event line that starts at input position `loc`.
#[inline(never)]
fn event_line(ctx: &Ctx, out: &mut EventOut, line: &[u8], loc: u64) {
    let Some(e) = parse_event(ctx, line) else { return };
    let (pk, trace) = trace_of(ctx, event_hash(e.id));
    out.k.push1(&ctx.pool, pk, trace | loc);
    if e.valid {
        let (sp, rec, extra) = event_records(ctx, &e);
        out.s.push1(&ctx.pool, sp, rec);
        if e.purchase {
            out.ps.push2(&ctx.pool, sp, rec, extra);
        }
    }
}

const BATCH: usize = 32;

/// Valid events waiting for their person: the user table is the one randomly
/// accessed structure of the parser, so its slots are prefetched when an event
/// is queued and read a batch later.
struct EventBatch {
    n: usize,
    key: [u64; BATCH],
    home: [u32; BATCH],
    ts: [u32; BATCH],
    /// `price << 32 | category` of a purchase, 0 otherwise (a price is at least 1).
    extra: [u64; BATCH],
}

impl EventBatch {
    fn new() -> EventBatch {
        EventBatch { n: 0, key: [0; BATCH], home: [0; BATCH], ts: [0; BATCH], extra: [0; BATCH] }
    }

    #[inline(always)]
    fn add(&mut self, ctx: &Ctx, out: &mut EventOut, key: u64, ts: u32, extra: u64) {
        let i = self.n;
        unsafe {
            *self.key.get_unchecked_mut(i) = key;
            *self.home.get_unchecked_mut(i) = ctx.table.prefetch_home(key) as u32;
            *self.ts.get_unchecked_mut(i) = ts;
            *self.extra.get_unchecked_mut(i) = extra;
        }
        self.n = i + 1;
        if self.n == BATCH {
            self.flush(ctx, out);
        }
    }

    #[inline(never)]
    fn flush(&mut self, ctx: &Ctx, out: &mut EventOut) {
        for i in 0..self.n {
            let person = ctx.table.person(ctx.table.insert_at(self.home[i] as usize, self.key[i])) as u64;
            let rec = (person & ((1 << LOCAL_BITS) - 1)) << 31 | self.ts[i] as u64;
            let sp = (person >> LOCAL_BITS) as usize;
            out.s.push1(&ctx.pool, sp, rec);
            if self.extra[i] != 0 {
                out.ps.push2(&ctx.pool, sp, rec, self.extra[i]);
            }
        }
        self.n = 0;
    }
}

/// What the SIMD path made of one event line.
pub struct Decoded {
    /// Bytes consumed; 0 if the line needs the scalar path.
    pub used: usize,
    /// The event_id; an empty one means the line is dropped.
    pub id: (*const u8, usize),
    /// Passed validation and is not late.
    pub valid: bool,
    /// The user's key (meaningful when valid).
    pub user_key: u64,
    /// Epoch ms (meaningful when valid).
    pub ts: i64,
    /// `price << 32 | category` of a valid purchase, else 0.
    pub extra: u64,
    /// Epoch seconds of ingest_ts; only computed when `FULL`.
    pub ingest_s: i64,
}

const TO_SCALAR: Decoded = Decoded { used: 0, id: (std::ptr::null(), 0), valid: false, user_key: 0, ts: 0, extra: 0, ingest_s: 0 };

/// Decode the event line at `p`, which must be at least `HEAD` bytes into its
/// file and at least `MARGIN` bytes before its end. With `FULL`, ingest_ts is
/// always parsed (pass 2 needs its value); without, only when the lateness
/// rule cannot be decided from its date.
#[inline(always)]
pub unsafe fn decode_event<const FULL: bool>(ctx: &Ctx, p: *const u8) -> Decoded {
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
        return TO_SCALAR;
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

    // What sends the line to the scalar path: an event_id longer than 16
    // bytes, an irregular user_id, an ingest_ts of the wrong length, a
    // timestamp this path cannot decide.
    let mut out = Decoded { used, id: (a0, n0), valid: false, user_key: 0, ts: 0, extra: 0, ingest_s: 0 };
    if n0 == 0 {
        return out; // no event_id: dropped
    }
    let (user_ok, user_key) = raw_key_fast(a1, n1, b'u');
    if (n0 > 16) | !(user_ok | (n1 == 0)) | (n3 != 20) {
        return TO_SCALAR;
    }
    let dt = &ctx.dates;
    let (ts_decided, ts_valid, ts) = timestamp(dt, a2, n2);
    if !ts_decided {
        return TO_SCALAR;
    }

    let w = (a4 as *const u64).read_unaligned();
    let purchase = (n4 == 8) & (w == u64::from_le_bytes(*b"purchase"));
    let view = (n4 == 4) & ((w as u32 == u32::from_le_bytes(*b"view")) | (w as u32 == u32::from_le_bytes(*b"cart")));
    let mut valid = (n1 != 0) & (n5 != 0) & (purchase | view) & ts_valid & (ts >= ctx.ws) & (ts < ctx.we);
    if FULL {
        let (shape, known, cal, ingest_s) = datetime(dt, a3);
        if !(shape & known & cal & (*a3.add(19) == b'Z')) {
            return TO_SCALAR;
        }
        out.ingest_s = ingest_s;
        valid &= ingest_s * 1000 - ts <= DAY_MS;
    } else if valid {
        // Late? ingest_ts is a valid F1 timestamp by the rules, and it almost
        // always falls on the same UTC day as ts, which settles it without
        // parsing it: on the same day the two are less than 24 hours apart.
        let day = ((ts - dt.first_day_ms) as u64 / DAY_MS as u64) as usize;
        let same = _mm_cmpeq_epi8(
            _mm_loadu_si128(a3 as *const __m128i),
            _mm_loadu_si128(dt.day_text.get_unchecked(day).as_ptr() as *const __m128i),
        );
        if (_mm_movemask_epi8(same) & 0x3FF != 0x3FF) | (*a3.add(19) != b'Z') {
            let (shape, known, cal, ingest_s) = datetime(dt, a3);
            if !(shape & known & cal & (*a3.add(19) == b'Z')) {
                return TO_SCALAR;
            }
            valid = ingest_s * 1000 - ts <= DAY_MS;
        }
    }
    let mut extra = 0u64;
    if valid & purchase {
        let q6 = q(fb, 3);
        let price = std::slice::from_raw_parts(p2.add(c5 + 1 + q6), nl - c5 - 1 - 2 * q6);
        match parse_price(price) {
            Some(price) => {
                let cat = ctx.products().category(std::slice::from_raw_parts(p2.add(c4 + 1 + q5), n5)) as u64;
                extra = (price as u64) << 32 | cat;
            }
            None => valid = false,
        }
    }
    out.valid = valid;
    out.user_key = user_key;
    out.ts = ts;
    out.extra = extra;
    out
}

/// The parser's fast path for the event line at `p`, whose input position is
/// `loc`. Returns the bytes consumed, or 0 if the line needs the scalar path.
#[inline(always)]
unsafe fn event_fast(ctx: &Ctx, out: &mut EventOut, batch: &mut EventBatch, p: *const u8, loc: u64) -> usize {
    let d = decode_event::<false>(ctx, p);
    let (a0, n0) = d.id;
    if (d.used == 0) | (n0 == 0) {
        return d.used;
    }
    // The trace: a hash of the event_id's bytes (ids::event_hash).
    let lo = _bzhi_u64((a0 as *const u64).read_unaligned(), 8 * n0 as u32);
    let hi = _bzhi_u64((a0.add(8) as *const u64).read_unaligned(), 8 * (n0 as u32).saturating_sub(8));
    let (pk, trace) = trace_of(ctx, mix16(lo, hi, n0));
    out.k.push1(&ctx.pool, pk, trace | loc);
    if d.valid {
        batch.add(ctx, out, d.user_key, (d.ts - ctx.t0) as u32, d.extra);
    }
    d.used
}

/// Whether the SIMD decoder may be used on the line at byte `pos` of a file of `len` bytes.
#[inline(always)]
pub fn can_decode(ctx: &Ctx, pos: usize, len: usize) -> bool {
    (pos >= HEAD) & (pos < fast_end(ctx, len))
}

pub fn events_chunk(ctx: &Ctx, out: &mut EventOut, ch: &Chunk) -> usize {
    let data = ctx.files[ch.file as usize].slice();
    let (len, fast_end) = (data.len(), fast_end(ctx, data.len()));
    let base = ctx.file_base[ch.file as usize];
    let mut lines = 0usize;
    if let Some(mut pos) = first_line(data, ch.start) {
        let mut batch = EventBatch::new();
        while pos < ch.end {
            lines += 1;
            if (pos >= HEAD) & (pos < fast_end) {
                prefetch_input(ctx, data, pos);
                let used = unsafe { event_fast(ctx, out, &mut batch, data.as_ptr().add(pos), base + pos as u64) };
                if used != 0 {
                    pos += used;
                    continue;
                }
            }
            let e = find_newline(data, pos).unwrap_or(len);
            event_line(ctx, out, &data[pos..e], base + pos as u64);
            pos = e + 1;
        }
        batch.flush(ctx, out);
    }
    lines
}

// ------------------------------------------------------------------ clicks

#[inline(always)]
fn click_words(person: u64, tc: u64) -> (usize, u64) {
    ((person >> LOCAL_BITS) as usize, (person & ((1 << LOCAL_BITS) - 1)) << 52 | tc)
}

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
    let person = ctx.table.person(ctx.table.insert(ctx.users.key(f[1]))) as u64;
    let (part, w0) = click_words(person, ((ts - ctx.t0) as u64) << CAMPAIGN_BITS | campaign as u64);
    c.push2(&ctx.pool, part, w0, ctx.clicks.key(f[0]));
}

/// Valid clicks waiting for their person (see `EventBatch`).
struct ClickBatch {
    n: usize,
    user: [u64; BATCH],
    home: [u32; BATCH],
    /// ts_rel << CAMPAIGN_BITS | campaign
    tc: [u64; BATCH],
    click: [u64; BATCH],
}

impl ClickBatch {
    fn new() -> ClickBatch {
        ClickBatch { n: 0, user: [0; BATCH], home: [0; BATCH], tc: [0; BATCH], click: [0; BATCH] }
    }

    #[inline(always)]
    fn add(&mut self, ctx: &Ctx, c: &mut PartWriter, user: u64, tc: u64, click: u64) {
        let i = self.n;
        unsafe {
            *self.user.get_unchecked_mut(i) = user;
            *self.home.get_unchecked_mut(i) = ctx.table.prefetch_home(user) as u32;
            *self.tc.get_unchecked_mut(i) = tc;
            *self.click.get_unchecked_mut(i) = click;
        }
        self.n = i + 1;
        if self.n == BATCH {
            self.flush(ctx, c);
        }
    }

    #[inline(never)]
    fn flush(&mut self, ctx: &Ctx, c: &mut PartWriter) {
        for i in 0..self.n {
            let person = ctx.table.person(ctx.table.insert_at(self.home[i] as usize, self.user[i])) as u64;
            let (part, w0) = click_words(person, self.tc[i]);
            c.push2(&ctx.pool, part, w0, self.click[i]);
        }
        self.n = 0;
    }
}

/// Returns the bytes consumed, or 0 if the line needs the scalar path.
#[inline(always)]
unsafe fn click_fast(ctx: &Ctx, c: &mut PartWriter, batch: &mut ClickBatch, p: *const u8) -> usize {
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
    let (click_ok, click_key) = raw_key_fast(a0, n0, b'k');
    let (user_ok, user_key) = raw_key_fast(a1, n1, b'u');
    if !(click_ok & user_ok) {
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
        batch.add(ctx, c, user_key, ((ts - ctx.t0) as u64) << CAMPAIGN_BITS | campaign as u64, click_key);
    }
    used
}

pub fn clicks_chunk(ctx: &Ctx, c: &mut PartWriter, ch: &Chunk) -> usize {
    let data = ctx.files[ch.file as usize].slice();
    let (len, fast_end) = (data.len(), fast_end(ctx, data.len()));
    let mut lines = 0usize;
    if let Some(mut pos) = first_line(data, ch.start) {
        let mut batch = ClickBatch::new();
        while pos < ch.end {
            lines += 1;
            if (pos >= HEAD) & (pos < fast_end) {
                prefetch_input(ctx, data, pos);
                let used = unsafe { click_fast(ctx, c, &mut batch, data.as_ptr().add(pos)) };
                if used != 0 {
                    pos += used;
                    continue;
                }
            }
            let e = find_newline(data, pos).unwrap_or(len);
            click_line(ctx, c, &data[pos..e]);
            pos = e + 1;
        }
        batch.flush(ctx, c);
    }
    lines
}
