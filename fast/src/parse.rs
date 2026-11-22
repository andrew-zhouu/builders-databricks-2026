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

use crate::ids::{event_hash, hash33, hash9, hex32, raw_key_fast, ref_word, LONG_LEN};
use crate::lookup::ABSENT;
use crate::part::PartWriter;
use crate::util::{civil_from_days, days_from_civil, days_in_month, find_newline, parse_price, parse_ts, split, DAY_MS};
use crate::{Chunk, Ctx};

pub mod splitw;
mod wide;

pub const PERSON_BITS: u32 = 27;
pub const TS_MASK: u64 = (1 << 31) - 1;
pub const LOCAL_BITS: u32 = 13;
/// A click's first word is `person_local | ts_rel_ms(31) | campaign`: the
/// campaign gets the bits the other two leave.
pub const CAMPAIGN_BITS: u32 = 33 - LOCAL_BITS;
/// Where `person_local` starts in a click's first word.
pub const CLICK_SHIFT: u32 = 31 + CAMPAIGN_BITS;
/// Marks a cancellation among event and purchase records.
pub const CANCEL: u64 = 1 << (31 + LOCAL_BITS);

/// The second word of a trace: what the parser did with the line. Zero for a
/// line that failed validation (nothing was written for it); else `T_VALID`,
/// `T_PURCHASE` if it is a purchase, four more bits of the event_id's hash,
/// the person (27 bits from bit 31) and `ts_rel_ms` (31 bits).
pub const T_VALID: u64 = 1 << 63;
pub const T_PURCHASE: u64 = 1 << 62;

/// The trace's second word for a valid event of `person` at `ts_rel`.
#[inline(always)]
pub fn trace_valid(h4: u64, person: u64, ts_rel: u64) -> u64 {
    T_VALID | (h4 & 15) << 58 | person << 31 | ts_rel
}

/// From a valid trace's second word: the event's person partition and its
/// event record (the word the parser wrote there).
#[inline(always)]
pub fn trace_record(w2: u64) -> (u32, u64) {
    let person = w2 >> 31 & ((1 << PERSON_BITS) - 1);
    ((person >> LOCAL_BITS) as u32, (person & ((1 << LOCAL_BITS) - 1)) << 31 | (w2 & TS_MASK))
}

/// Bytes that must be readable after a line start for the fast paths.
const MARGIN: usize = 256;
/// Lines this close to the file start use the scalar path (backward loads).
const HEAD: usize = 16;

/// The per-thread writers an event can go to.
pub struct EventOut<'a> {
    /// Traces, partitioned by event-ID hash.
    pub k: &'a mut PartWriter,
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
    /// `YYYY-MM-DDT` (zero-padded to 16 bytes) and the epoch ms of the day
    /// with a given day of the month, among the days from one before the
    /// first a click may fall on to one after the event window (F2 writes
    /// local dates). Indexed by `tens * 10 + ones` of the low nibbles of the
    /// two characters. Empty (no text matches) for other indexes, and
    /// altogether when those days are too many to have distinct days of the
    /// month.
    dom_text: [[u8; 16]; 166],
    dom_ms: [i64; 166],
    /// The same table for the wide path, which cannot index it without a
    /// gather: the days in it fall in at most two months. For each, its
    /// `YYYY-MM-` as a little-endian word, the days of the month that are in
    /// the table (bit `dd`), and the epoch ms of the month's "day 0".
    /// `wide_ok`: the table is in use and this says all of it.
    ym: [u64; 2],
    ym_days: [u64; 2],
    ym_base: [i64; 2],
    wide_ok: bool,
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
            dom_text: [[0xFF; 16]; 166],
            dom_ms: [0; 166],
            ym: [0; 2],
            ym_days: [0; 2],
            ym_base: [0; 2],
            wide_ok: ndays + 3 <= 28,
        };
        if ndays + 3 <= 28 {
            let mut months = 0;
            for day in first_day - 1..=first_day + ndays as i64 + 1 {
                let (y, m, dd) = civil_from_days(day);
                if (0..=9999).contains(&y) {
                    let mut text = [0u8; 16];
                    text[..11].copy_from_slice(format!("{y:04}-{m:02}-{dd:02}T").as_bytes());
                    t.dom_text[dd as usize] = text;
                    t.dom_ms[dd as usize] = day * DAY_MS;
                    let ym = u64::from_le_bytes(text[..8].try_into().unwrap());
                    match (0..months).find(|&k| t.ym[k] == ym) {
                        Some(k) => t.ym_days[k] |= 1 << dd,
                        None if months < 2 => {
                            t.ym[months] = ym;
                            t.ym_days[months] = 1 << dd;
                            t.ym_base[months] = (day - dd) * DAY_MS;
                            months += 1;
                        }
                        // (Fewer than 28 days in a row are never in three months.)
                        None => t.wide_ok = false,
                    }
                }
            }
        }
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

/// `masks64` on a processor with AVX-512: one 64-byte register, three compares
/// straight into mask registers.
#[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq,avx512vbmi,avx512vbmi2")]
#[inline]
unsafe fn masks64_wide(p: *const u8) -> (u64, u64, u64) {
    let v = _mm512_loadu_si512(p as *const __m512i);
    (
        _mm512_cmpeq_epi8_mask(v, _mm512_set1_epi8(b',' as i8)),
        _mm512_cmpeq_epi8_mask(v, _mm512_set1_epi8(b'\n' as i8)),
        _mm512_cmpeq_epi8_mask(v, _mm512_set1_epi8(b'"' as i8)),
    )
}

/// Whether the wide path is taken where the processor has it, unless SS_SIMD
/// says otherwise (1: on, 0: off). Off until its decode keeps lines with long
/// IDs (ids.rs) on the fast path: as it stands it leaves them, a quarter of
/// all lines, to the scalar path.
const WIDE_DEFAULT: bool = false;

/// Whether this processor takes the wide path: AVX-512 with VBMI2, which
/// means Ice Lake or newer. (Skylake and Cascade Lake have AVX-512 as well,
/// but lower their clock when it is used; they keep the AVX2 path, as does
/// everything older.) The code of the wide path is reached through this test
/// and nowhere else. SS_SIMD=0 turns the wide path off.
pub fn wide() -> bool {
    static WIDE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *WIDE.get_or_init(|| {
        let asked = std::env::var("SS_SIMD").ok();
        (if WIDE_DEFAULT { asked.as_deref() != Some("0") } else { asked.as_deref() == Some("1") }) && wide_capable()
    })
}

/// Whether this processor has what the wide path needs.
pub fn wide_capable() -> bool {
    is_x86_feature_detected!("avx512f")
        && is_x86_feature_detected!("avx512bw")
        && is_x86_feature_detected!("avx512vl")
        && is_x86_feature_detected!("avx512dq")
        && is_x86_feature_detected!("avx512vbmi")
        && is_x86_feature_detected!("avx512vbmi2")
}

/// SS_SIMD_CHECK=1: the wide path also runs the ordinary one on every batch
/// and stops at the first line on which the two disagree.
fn wide_check() -> bool {
    static CHECK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CHECK.get_or_init(|| std::env::var("SS_SIMD_CHECK").is_ok_and(|v| v == "1"))
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

const RING: usize = 32;

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
    let h = event_hash(e.id);
    let (pk, trace) = trace_of(ctx, h);
    let mut w2 = 0;
    if e.valid {
        let (sp, rec, extra) = event_records(ctx, &e);
        w2 = trace_valid(h >> 40, (sp as u64) << LOCAL_BITS | rec >> 31, rec & TS_MASK);
        if e.purchase {
            out.ps.push2(&ctx.pool, sp, rec, extra);
            w2 |= T_PURCHASE;
        }
    }
    out.k.push2(&ctx.pool, pk, trace | loc, w2);
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

/// Whether the SIMD decoder may be used on the line at byte `pos` of a file of `len` bytes.
#[inline(always)]
pub fn can_decode(ctx: &Ctx, pos: usize, len: usize) -> bool {
    (pos >= HEAD) & (pos < fast_end(ctx, len))
}

// ------------------------------------------------------------------ events, staged
//
// The parser proper works on batches of lines, in three loops:
//
//   split    find each line's separators and check its structure;
//   decode   everything that depends on one line only: the trace of its
//            event_id, its user, timestamp and validity. The user's slot in
//            the user table is asked for here (prefetch) ...
//   emit     ... and read here, a batch later, when the event goes to its
//            person's partition.
//
// The first two contain no calls and no branch that depends on the data in an
// unpredictable way other than the form of the timestamp. What they cannot
// decide is marked and left to the scalar path, which implements the rules
// literally.

/// Lines per batch: their bytes and everything below stay in the L1 cache.
const NB: usize = 64;
/// The scalar path must take this line.
const F_DEAD: u8 = 1;
/// The event passed validation (and is not late, unless `F_INGEST` is set).
const F_VALID: u8 = 4;
const F_PURCHASE: u8 = 8;
/// ingest_ts is not on the UTC day of ts: the lateness rule needs its value.
const F_INGEST: u8 = 16;
/// The line is valid if its user_id, which is 33 bytes, is a long ID (ids.rs):
/// `stage_long` decides, and finds the key.
const F_LONG: u8 = 32;

const PURCHASE: u64 = u64::from_le_bytes(*b"purchase");
const VIEW: u32 = u32::from_le_bytes(*b"view");
const CART: u32 = u32::from_le_bytes(*b"cart");

#[repr(C, align(64))]
#[derive(Clone, Copy)]
struct Stage {
    /// Where each line starts.
    p: [*const u8; NB],
    /// Bytes 0..7: the offsets of the line's six commas and its newline; byte
    /// 7: bit `k` is set if field `k` is quoted.
    sep: [u64; NB],
    /// The trace's first word; `pk` is its partition, with four more bits of
    /// the hash from bit 16 (for the second word).
    trace: [u64; NB],
    /// The user's key; 0 = no user.
    key: [u64; NB],
    /// `price << 32 | category`, with `F_PURCHASE`.
    extra: [u64; NB],
    pk: [u32; NB],
    /// The user's home slot in the user table (prefetched).
    home: [u32; NB],
    /// `ts - t0`, meaningful with `F_VALID`.
    ts: [u32; NB],
    flags: [u8; NB],
    /// Scratch: the current line's field starts and lengths (see `fields`).
    sl: [u64; 2],
    /// Lines with more to do after `decode`: a lateness check on the value of
    /// ingest_ts, or a purchase. (The wide path appends eight bytes at a time.)
    more: [u8; NB + 8],
}

impl Stage {
    fn new() -> Stage {
        Stage {
            p: [std::ptr::null(); NB],
            sep: [0; NB],
            trace: [0; NB],
            key: [0; NB],
            extra: [0; NB],
            pk: [0; NB],
            home: [0; NB],
            ts: [0; NB],
            flags: [0; NB],
            sl: [0; 2],
            more: [0; NB + 8],
        }
    }
}

/// What `id_trace` needs to know about the run.
#[derive(Clone, Copy)]
struct IdParams {
    pd: u64,
    fp_mask: u64,
    loc_bits: u32,
}

impl IdParams {
    #[inline(always)]
    fn new(ctx: &Ctx) -> IdParams {
        IdParams { pd: ctx.pd as u64, fp_mask: ctx.fp_mask, loc_bits: ctx.loc_bits }
    }
}

/// How the fast paths turn an event ID into a trace. From the ID's first byte,
/// the rest of it as a little-endian word (zero-padded) and its length `n`:
/// the trace's partition (with four more bits of the hash from bit 16, for the
/// trace's second word), the trace's first word without the line's position,
/// and whether the ID is not of the regular shape (1 to 9 bytes), so that the
/// scalar path must take the line (`event_line`, which hashes any ID).
///
/// The wide path has the same computation on four lines at once
/// (`wide::id_trace4`); with `wide::ID_TRACE_WIDE` off it calls this one.
#[inline(always)]
fn id_trace(idp: &IdParams, first: u8, rest: u64, n: u64) -> (u32, u64, bool) {
    let (pk, trace) = trace_from(idp, hash9(first, rest));
    (pk, trace, n > 9)
}

/// A trace's partition (with four more bits of the hash from bit 16) and its
/// first word without the position, from the hash of the event ID.
#[inline(always)]
fn trace_from(idp: &IdParams, h: u64) -> (u32, u64) {
    ((((h >> 32) * idp.pd) >> 32) as u32 | ((h >> 40) as u32 & 15) << 16, (h & idp.fp_mask) << idp.loc_bits)
}

/// `id_trace` for the `n` bytes of an event ID at `a`, which also takes the
/// other usual shape, an ID of exactly 33 bytes (`ids::hash33`, as
/// `ids::event_hash` has it). Both hashes are computed and one is chosen: one
/// ID in eight is long, in no order a branch could learn.
#[inline(always)]
unsafe fn id_trace_at(idp: &IdParams, a: *const u8, n: u64) -> (u32, u64, bool) {
    let w = |k: usize| (a.add(1 + 8 * k) as *const u64).read_unaligned();
    let long = n == LONG_LEN as u64;
    let bits = (8 * n as u32).wrapping_sub(8);
    let x = w(0);
    let short = hash9(*a, _bzhi_u64(x, bits));
    // A NUL among a short ID's bytes: zero-padded, it would have the hash of
    // a shorter ID as well (see ids::event_hash, which hashes such an ID's
    // length with it): the scalar path takes it.
    let nul = (x.wrapping_sub(0x0101_0101_0101_0101) & !x & _bzhi_u64(0x8080_8080_8080_8080, bits)) | (*a == 0) as u64;
    let h = if long { hash33(*a, [x, w(1), w(2), w(3)]) } else { short };
    let (pk, trace) = trace_from(idp, h);
    (pk, trace, !long & ((n > 9) | (nul != 0)))
}

/// Every field's content, from a line's `sep` word: byte `k` of the first is
/// where field `k` starts (offset in the line), of the second its length.
#[inline(always)]
unsafe fn fields(s: u64) -> (u64, u64) {
    // One byte per field: 1 if it is quoted.
    let quoted = _pdep_u64(s >> 56, 0x0001_0101_0101_0101);
    let seps = s & 0x00FF_FFFF_FFFF_FFFF;
    // A field starts after the separator before it (the line start for field
    // 0), past its opening quote; it ends at its separator, before its closing
    // quote. No byte borrows from the next: lengths are >= 0.
    let start = (seps << 8).wrapping_add(0x0001_0101_0101_0100).wrapping_add(quoted);
    (start, seps.wrapping_sub(start).wrapping_sub(quoted))
}

/// Split the line at `p`: find its seven separators and check its quoting.
/// Returns the bytes the line takes (0: not understood, the scalar path
/// decides) and whether it became line `i` of the batch (a line with an empty
/// event_id is dropped here).
macro_rules! split_event_fn {
    ($(#[$attr:meta])* $name:ident, $masks:ident) => {
        $(#[$attr])*
        unsafe fn $name(st: &mut Stage, i: usize, p: *const u8) -> (usize, bool) {
            // Three windows of 64 bytes, each starting after a comma found in
            // the one before: an ID may be 33 bytes (35 with quotes), so the
            // first three fields do not fit one window, but an ID and its
            // comma do, and so do a user_id, a ts and their commas.
            // Window 1: event_id and its comma.
            let (cm, nm, qm) = $masks(p);
            let c0 = cm.trailing_zeros() as u64;
            // A comma (else c0 is 64), and no newline before it.
            let mut bad = (c0 >> 6) | _bzhi_u64(nm, c0 as u32);
            let q0 = _bzhi_u64(qm, c0 as u32);
            // The field starts with a quote; it ends with one (the byte before the comma).
            let f0 = q0 & 1;
            let l0 = (q0 << 1 >> (c0 & 63)) & 1;

            // Window 2: user_id, ts and their commas.
            let o1 = (c0 & 63) + 1;
            let (cm, nm, qm) = $masks(p.add(o1 as usize));
            let c1 = cm.trailing_zeros() as u64;
            let cm1 = _blsr_u64(cm);
            let c2 = cm1.trailing_zeros() as u64;
            // Two commas (else c2 is 64), and no newline before the second.
            bad |= (c2 >> 6) | _bzhi_u64(nm, c2 as u32);
            let qa = _bzhi_u64(qm, c2 as u32);
            let da = cm ^ _blsr_u64(cm1);
            // Fields that start with a quote, and fields that end with one.
            let fa = _pext_u64(qa >> 1, da) << 1 | (qa & 1);
            let la = _pext_u64(qa << 1, da);

            // Window 3: ingest_ts, event_type, product_id, price_cents and the newline.
            let o = o1 + (c2 & 63) + 1;
            let (c1, c2) = (o1 + c1, o1 + c2);
            let (cm, nm, qm) = $masks(p.add(o as usize));
            let nl = nm.trailing_zeros() as u64;
            let cmb = _bzhi_u64(cm, nl as u32);
            let c3 = cmb.trailing_zeros() as u64;
            let cmb1 = _blsr_u64(cmb);
            let c4 = cmb1.trailing_zeros() as u64;
            let cmb2 = _blsr_u64(cmb1);
            let c5 = cmb2.trailing_zeros() as u64;
            // The newline is in the window, with exactly three commas before it.
            bad |= (nl >> 6) | (cmb.count_ones() as u64 ^ 3);
            let qb = _bzhi_u64(qm, nl as u32);
            let db = cmb | 1u64.wrapping_shl(nl as u32);
            let fb = _pext_u64(qb >> 1, db) << 1 | (qb & 1);
            let lb = _pext_u64(qb << 1, db);

            // A field is quoted iff its first and last bytes are quotes; any other
            // quote makes the line malformed: the same fields start and end with a
            // quote, and there are exactly two quotes for each of them.
            let f = f0 | fa << 1 | fb << 3;
            bad |= (f ^ (l0 | la << 1 | lb << 3))
                | ((q0.count_ones() + qa.count_ones() + qb.count_ones()) as u64 ^ 2 * f.count_ones() as u64);
            if bad != 0 {
                return (0, false);
            }
            *st.p.get_unchecked_mut(i) = p;
            let d = st.sep.as_mut_ptr().add(i) as *mut u8;
            *d = c0 as u8;
            *d.add(1) = c1 as u8;
            *d.add(2) = c2 as u8;
            *d.add(3) = (o + c3) as u8;
            *d.add(4) = (o + c4) as u8;
            *d.add(5) = (o + c5) as u8;
            *d.add(6) = (o + nl) as u8;
            *d.add(7) = f as u8;
            // An empty event_id (nothing, or just two quotes): the line is dropped.
            ((o + nl + 1) as usize, c0 != 2 * f0)
        }
    };
}
split_event_fn!(#[inline(always)] split_event, masks64);
split_event_fn!(
    #[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq,avx512vbmi,avx512vbmi2")]
    #[inline]
    split_event_wide,
    masks64_wide
);

/// A `ts` field of `n` bytes at `a`. Returns (the scalar path must decide,
/// valid, epoch ms).
#[inline(always)]
unsafe fn event_ts(dt: &DateTab, a: *const u8, n: usize) -> (bool, bool, i64) {
    if (n == 20) | (n == 25) {
        // F1 or F2. The date is almost always one of the few days around the
        // window, which have distinct days of the month: look the day up by
        // that, and compare the text (`YYYY-MM-DDT`).
        let w = (a.add(8) as *const u16).read_unaligned() as usize;
        let dom = (w & 15) * 10 + (w >> 8 & 15);
        let date = _mm_cmpeq_epi8(
            _mm_loadu_si128(a as *const __m128i),
            _mm_loadu_si128(dt.dom_text.get_unchecked(dom).as_ptr() as *const __m128i),
        );
        // `HH:MM:SS`: digits within their ranges and the two colons.
        let d = (a.add(11) as *const u64).read_unaligned() ^ 0x3030_3A30_303A_3030;
        let over = (d.wrapping_add(0x767A_7F76_7A7F_767D) | d) & 0x8080_8080_8080_8080;
        let y = d.wrapping_mul(0xA01);
        let (hh, mm, ss) = (y >> 8 & 0xFF, y >> 32 & 0xFF, y >> 56);
        let ms = *dt.dom_ms.get_unchecked(dom) + ((hh * 3600 + mm * 60 + ss) * 1000) as i64;
        let s = *a.add(19);
        if (_mm_movemask_epi8(date) & 0x7FF != 0x7FF) | (over != 0) | (hh > 23) {
            // Another date (far from the window, or none at all), a bad time, a
            // run of digits: rare, and the scalar path knows all the rules.
            return (true, false, 0);
        }
        if n == 20 {
            return (false, s == b'Z', ms);
        }
        let h1 = (*a.add(20)).wrapping_sub(b'0') as i64;
        let h2 = (*a.add(21)).wrapping_sub(b'0') as i64;
        let m1 = (*a.add(23)).wrapping_sub(b'0') as i64;
        let m2 = (*a.add(24)).wrapping_sub(b'0') as i64;
        let (oh, om) = (h1 * 10 + h2, m1 * 10 + m2);
        let ok = ((s == b'+') | (s == b'-'))
            & (h1 <= 9)
            & (h2 <= 9)
            & (m1 <= 9)
            & (m2 <= 9)
            & (*a.add(22) == b':')
            & (oh <= 23)
            & (om <= 59);
        let off = (oh * 60 + om) * 60_000;
        return (false, ok, ms - if s == b'-' { -off } else { off });
    }
    if n.wrapping_sub(1) < 16 {
        // F3: digits only. Anything else of these lengths is simply invalid.
        let (digits, value) = digits16(a.add(n), n);
        return (false, digits, value as i64);
    }
    // No valid form has this length, except a long run of digits (F3 with
    // leading zeros): the scalar path decides what might be one.
    let maybe_digits = (n != 0) && ((*a).wrapping_sub(b'0') < 10) & ((*a.add(n - 1)).wrapping_sub(b'0') < 10);
    (maybe_digits, false, 0)
}

/// Decode the batch's lines (see the section comment). Conditions are kept as
/// integers that are zero when all is well and OR-ed together, so that a line
/// costs a handful of tests, not one per rule.
#[inline(never)]
unsafe fn stage_decode(ctx: &Ctx, st: &mut Stage, n: usize, loc0: u64) -> usize {
    const LOW: u64 = 0x0101_0101_0101_0101;
    const HIGH: u64 = 0x8080_8080_8080_8080;
    let dt = &ctx.dates;
    let (ws, t0, span) = (ctx.ws, ctx.t0, (ctx.we - ctx.ws) as u64);
    let idp = IdParams::new(ctx);
    let (first_day_ms, last_day) = (dt.first_day_ms, dt.day_text.len() as u64 - 1);
    let day_text = dt.day_text.as_ptr();
    let mut nmore = 0usize;
    for i in 0..n {
        let p = *st.p.get_unchecked(i);
        let (start, len) = fields(*st.sep.get_unchecked(i));
        // Through memory, so that each field's offset and length is one byte
        // load, not a shift and a mask of two registers that stay live.
        let sl = st.sl.as_mut_ptr() as *mut u64;
        sl.write_volatile(start);
        sl.add(1).write_volatile(len);
        let sl = sl as *const u8;
        let at = |k: usize| p.add(sl.add(k).read_volatile() as usize);
        let ln = |k: usize| sl.add(8 + k).read_volatile() as u64;

        // The trace: a hash of the event_id's bytes (1 to 9 of them, or 33;
        // see ids::event_hash).
        let n0 = ln(0);
        let a0 = at(0);
        let (pk, trace, id_dead) = id_trace_at(&idp, a0, n0);
        *st.pk.get_unchecked_mut(i) = pk;
        *st.trace.get_unchecked_mut(i) = trace | loc0.wrapping_add(p as u64);

        // The user: `u` and 1 to 8 ASCII bytes, none of them NUL, is its own
        // key (ids::raw_key). `user_bad` is zero for those. A user_id of 33
        // bytes is taken for a long ID here, and for good; `stage_long` looks
        // at it and finds its key.
        let n1 = ln(1);
        let a1 = at(1);
        let bits = (8 * n1 as u32).wrapping_sub(8);
        let raw = (a1.add(1) as *const u64).read_unaligned();
        let user_long = n1 == LONG_LEN as u64;
        let user_bad = if user_long {
            0
        } else {
            (((raw.wrapping_sub(LOW) & !raw) | raw) & _bzhi_u64(HIGH, bits)) | (*a1 ^ b'u') as u64 | (n1.wrapping_sub(2) > 7) as u64
        };
        let key = if (user_bad == 0) & !user_long { _bzhi_u64(raw, bits).swap_bytes() } else { 0 };
        *st.key.get_unchecked_mut(i) = key;
        *st.home.get_unchecked_mut(i) = ctx.table.prefetch_home(key) as u32;

        let (a2, n2) = (at(2), ln(2));
        let (ts_dead, ts_ok, ms) = event_ts(dt, a2, n2 as usize);
        let in_window = (ms.wrapping_sub(ws) as u64) < span;

        // The scalar path takes what this one cannot decide: a longer
        // event_id (its hash looks at more than one word), a user_id that
        // needs the dictionary, an ingest_ts that cannot be F1, a timestamp
        // out of the ordinary.
        let mut dead = id_dead as u64 | (ln(3) ^ 20) | ts_dead as u64;
        if user_bad != 0 {
            dead |= n1; // an empty user_id is just invalid
        }

        // view / cart / purchase, exactly.
        let n4 = ln(4);
        let w = (at(4) as *const u64).read_unaligned();
        let not_purchase = (n4 ^ 8) | (w ^ PURCHASE);
        let not_view = (n4 ^ 4) | (w as u32 ^ VIEW) as u64 * (w as u32 ^ CART) as u64;
        let invalid = dead | user_bad | not_purchase.min(not_view) | (ln(5) == 0) as u64 | !(ts_ok & in_window) as u64;
        let valid = invalid == 0;

        // Late? ingest_ts is a valid F1 timestamp by the rules, and it almost
        // always falls on the same UTC day as ts, which settles it without
        // parsing it: on the same day the two are less than 24 hours apart.
        // The day of ts as text: its own first ten bytes when it is F1, else
        // from the table of the window's days.
        let a3 = at(3);
        let day_of_ts = if n2 == 20 {
            _mm_loadu_si128(a2 as *const __m128i)
        } else {
            let day = ((ms.wrapping_sub(first_day_ms) as u64) / DAY_MS as u64).min(last_day) as usize;
            _mm_loadu_si128(day_text.add(day) as *const __m128i)
        };
        let same = _mm_cmpeq_epi8(_mm_loadu_si128(a3 as *const __m128i), day_of_ts);
        let other_day = (!_mm_movemask_epi8(same) as u32 & 0x3FF) | (*a3.add(19) ^ b'Z') as u32;
        let ingest = valid & (other_day != 0);
        let purchase = valid & (not_purchase == 0);

        *st.ts.get_unchecked_mut(i) = ms.wrapping_sub(t0) as u32;
        *st.flags.get_unchecked_mut(i) =
            (dead != 0) as u8 | (valid as u8) << 2 | (purchase as u8) << 3 | (ingest as u8) << 4 | ((valid & user_long) as u8) << 5;
        *st.more.get_unchecked_mut(nmore) = i as u8;
        nmore += (ingest | purchase) as usize;
    }
    nmore
}

/// The users of the valid lines that `decode` flagged `F_LONG`: 33 bytes,
/// which is a long ID if they are `u` and 32 lowercase hexadecimal digits
/// (ids.rs). Its key is its slot in the table of long IDs, which is asked for
/// in one loop and read in the next, where the key's home in the user table
/// is asked for in turn, for `emit`. Anything else of 33 bytes is for the
/// scalar path. Runs after `stage_more`, which may have found a line invalid.
#[inline(never)]
unsafe fn stage_long(ctx: &Ctx, st: &mut Stage, n: usize) {
    let mut list = [0u8; NB];
    let mut m = 0usize;
    for i in 0..n {
        *list.get_unchecked_mut(m) = i as u8;
        m += (*st.flags.get_unchecked(i) & (F_LONG | F_VALID | F_DEAD) == F_LONG | F_VALID) as usize;
    }
    if m == 0 {
        return;
    }
    let table = ctx.users.long.as_ref().unwrap_unchecked();
    let mut value = [(0u64, 0u64); NB];
    for k in 0..m {
        let i = *list.get_unchecked(k) as usize;
        let (start, _) = fields(*st.sep.get_unchecked(i));
        let a = (*st.p.get_unchecked(i)).add((start >> 8) as usize & 0xFF);
        let (hex, hi, lo) = hex32(a.add(1));
        if !hex | (*a != b'u') | (hi == 0) | (lo == 0) {
            *st.flags.get_unchecked_mut(i) = F_DEAD;
            continue;
        }
        table.prefetch(hi, lo);
        *value.get_unchecked_mut(k) = (hi, lo);
    }
    for k in 0..m {
        let i = *list.get_unchecked(k) as usize;
        if *st.flags.get_unchecked(i) & F_DEAD != 0 {
            continue;
        }
        let (hi, lo) = *value.get_unchecked(k);
        // (Neither half is zero, so there is a key.)
        let key = ctx.users.long_key(hi, lo).unwrap_unchecked();
        *st.key.get_unchecked_mut(i) = key;
        *st.home.get_unchecked_mut(i) = ctx.table.prefetch_home(key) as u32;
    }
}

/// The rest of validation for the lines `decode` listed: the lateness rule by
/// the value of ingest_ts, and a purchase's price and category.
#[inline(never)]
unsafe fn stage_more(ctx: &Ctx, st: &mut Stage, nmore: usize) {
    let dt = &ctx.dates;
    for k in 0..nmore {
        let i = st.more[k] as usize;
        let p = st.p[i];
        let (start, len) = fields(st.sep[i]);
        let at = |k: u32| p.add((start >> (8 * k)) as usize & 0xFF);
        let ln = |k: u32| (len >> (8 * k)) as usize & 0xFF;
        let mut fl = st.flags[i];
        if fl & F_INGEST != 0 {
            let a3 = at(3);
            let (shape, known, cal, ingest_s) = datetime(dt, a3);
            if !(shape & known & cal & (*a3.add(19) == b'Z')) {
                st.flags[i] = F_DEAD;
                continue;
            }
            if ingest_s * 1000 - (st.ts[i] as i64 + ctx.t0) > DAY_MS {
                fl &= !(F_VALID | F_PURCHASE);
            }
        }
        if fl & F_PURCHASE != 0 {
            match parse_price(std::slice::from_raw_parts(at(6), ln(6))) {
                Some(price) => {
                    let product = std::slice::from_raw_parts(at(5), ln(5));
                    st.extra[i] = (price as u64) << 32 | ctx.products().category(product) as u64;
                }
                None => fl &= !(F_VALID | F_PURCHASE),
            }
        }
        st.flags[i] = fl;
    }
}

/// Every line leaves its trace, which for a valid event names its person (the
/// user's slot was prefetched by `decode`) and time; pass 2 sends the event
/// there once the repeats are settled. Purchases also leave their price and
/// category with their person.
#[inline(never)]
unsafe fn stage_emit(ctx: &Ctx, out: &mut EventOut, st: &Stage, n: usize) {
    let (kw, pw) = (out.k.cursors(), out.ps.cursors());
    let (pool, table) = (&ctx.pool, &ctx.table);
    for i in 0..n {
        let fl = *st.flags.get_unchecked(i);
        if fl & F_DEAD != 0 {
            continue;
        }
        let pk = *st.pk.get_unchecked(i) as u64;
        let mut w2 = 0;
        if fl & F_VALID != 0 {
            let person = table.person_at(*st.home.get_unchecked(i) as usize, *st.key.get_unchecked(i)) as u64;
            let ts = *st.ts.get_unchecked(i) as u64;
            w2 = trace_valid(pk >> 16, person, ts);
            if fl & F_PURCHASE != 0 {
                let rec = (person & ((1 << LOCAL_BITS) - 1)) << 31 | ts;
                out.ps.push2_at(pw, pool, (person >> LOCAL_BITS) as usize, rec, *st.extra.get_unchecked(i));
                w2 |= T_PURCHASE;
            }
        }
        out.k.push2_at(kw, pool, pk as usize & 0xFFFF, *st.trace.get_unchecked(i), w2);
    }
}

/// SS_SPLIT_CHECK: what `splitw::split_batch` made of the lines from `from`
/// to `to` must be what `split_event` makes of them, line by line.
unsafe fn check_split(st: &Stage, data: &[u8], from: usize, to: usize, n: usize, took: usize) {
    let mut one = Stage::new();
    let (mut pos, mut m) = (from, 0usize);
    for line in 0..took {
        let p = data.as_ptr().add(pos);
        let (used, added) = split_event(&mut one, 0, p);
        let show = || String::from_utf8_lossy(&data[pos..(pos + 160).min(data.len())]).into_owned();
        assert!(used != 0, "batch split took line {line} at {pos}, which the line split refuses: {}", show());
        if added {
            assert!(m < n, "batch split left out line {line} at {pos}: {}", show());
            assert!(st.p[m] == p && st.sep[m] == one.sep[0], "line {line} at {pos}: sep {:016x}, line split {:016x}: {}", st.sep[m], one.sep[0], show());
            m += 1;
        }
        pos += used;
    }
    assert!(m == n && pos == to, "batch split: {n} lines to {to}, line split: {m} lines to {pos}");
}

macro_rules! events_chunk_fn {
    ($(#[$attr:meta])* $name:ident, $split:ident, $decode:path, $batch:expr) => {
        $(#[$attr])*
        unsafe fn $name(ctx: &Ctx, out: &mut EventOut, ch: &Chunk) -> usize {
            let data = ctx.files[ch.file as usize].slice();
            let len = data.len();
            let fast_hi = fast_end(ctx, len).min(ch.end);
            let base = ctx.file_base[ch.file as usize];
            // The input position of a line is `loc0 + its address`.
            let loc0 = base.wrapping_sub(data.as_ptr() as u64);
            let ahead = ctx.prefetch_ahead;
            let mut lines = 0usize;
            let Some(mut pos) = first_line(data, ch.start) else { return 0 };
            let mut st = Stage::new();
            let mut ws = splitw::Wide::new();
            // The batch splitter, in the instance for processors that have it.
            let batch = $batch & ctx.wide_split;
            while pos < ch.end {
                let mut n = 0usize;
                // The line at `pos` after the batch is for the scalar path.
                let mut stuck = true;
                if (pos >= HEAD) & batch {
                    if pos < fast_hi {
                        unsafe {
                            let from = pos;
                            let (m, to, took, odd) = splitw::split_batch(&mut st, &mut ws, data.as_ptr(), pos, fast_hi, ahead);
                            if ctx.split_check {
                                check_split(&st, data, from, to, m, took);
                            }
                            (n, pos, stuck) = (m, to, odd);
                            lines += took;
                        }
                    }
                } else if pos >= HEAD {
                    unsafe {
                        while (n < NB) & (pos < fast_hi) {
                            let p = data.as_ptr().add(pos);
                            // The hardware streamer stops at page boundaries; reach past them.
                            // A line is often longer than a cache line, so two requests
                            // per line: one alone would skip every few cache lines.
                            _mm_prefetch::<_MM_HINT_T0>(p.wrapping_add(ahead) as *const i8);
                            _mm_prefetch::<_MM_HINT_T0>(p.wrapping_add(ahead + 64) as *const i8);
                            let (used, added) = $split(&mut st, n, p);
                            if used == 0 {
                                break;
                            }
                            pos += used;
                            lines += 1;
                            n += added as usize;
                        }
                    }
                    stuck = n < NB;
                }
                if n > 0 {
                    unsafe {
                        let nmore = $decode(ctx, &mut st, n, loc0);
                        stage_more(ctx, &mut st, nmore);
                        stage_long(ctx, &mut st, n);
                        stage_emit(ctx, out, &st, n);
                        // The scalar path for what the stages could not decide.
                        if st.flags[..n].iter().fold(0, |a, &f| a | f) & F_DEAD != 0 {
                            for i in 0..n {
                                if st.flags[i] & F_DEAD != 0 {
                                    let line = std::slice::from_raw_parts(st.p[i], (st.sep[i] >> 48 & 0xFF) as usize);
                                    event_line(ctx, out, line, loc0.wrapping_add(st.p[i] as u64));
                                }
                            }
                        }
                    }
                }
                // The batch ended early: at a line the split did not understand, or
                // where the fast path may not read (the edges of the file).
                if stuck & (pos < ch.end) {
                    lines += 1;
                    let e = find_newline(data, pos).unwrap_or(len);
                    event_line(ctx, out, &data[pos..e], base + pos as u64);
                    pos = e + 1;
                }
            }
            lines
        }
    };
}
events_chunk_fn!(events_chunk_base, split_event, stage_decode, false);
events_chunk_fn!(
    #[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq,avx512vbmi,avx512vbmi2")]
    events_chunk_wide,
    split_event_wide,
    wide::decode,
    true
);

pub fn events_chunk(ctx: &Ctx, out: &mut EventOut, ch: &Chunk) -> usize {
    unsafe {
        if wide() {
            events_chunk_wide(ctx, out, ch)
        } else {
            events_chunk_base(ctx, out, ch)
        }
    }
}

// ------------------------------------------------------------------ clicks

#[inline(always)]
fn click_words(person: u64, tc: u64) -> (usize, u64) {
    ((person >> LOCAL_BITS) as usize, (person & ((1 << LOCAL_BITS) - 1)) << CLICK_SHIFT | tc)
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

/// A batch of click lines, for the same three loops as events (see "events,
/// staged").
#[repr(C, align(64))]
#[derive(Clone, Copy)]
struct ClickStage {
    p: [*const u8; NB],
    /// Bytes 0..4: the offsets of the line's three commas and its newline;
    /// byte 7: bit `k` is set if field `k` is quoted.
    sep: [u64; NB],
    user: [u64; NB],
    click: [u64; NB],
    /// `ts_rel << CAMPAIGN_BITS | campaign`
    tc: [u64; NB],
    home: [u32; NB],
    flags: [u8; NB],
    /// Where the `ts` field starts in its line, and its length.
    ts_at: [u8; NB],
    ts_len: [u8; NB],
    sl: [u64; 2],
}

/// Split the click line at `p`. Returns the bytes it takes (0: not understood,
/// the scalar path decides) and whether it became line `i` of the batch (a
/// line without a click_id or a user_id is dropped here).
macro_rules! split_click_fn {
    ($(#[$attr:meta])* $name:ident, $masks:ident) => {
        $(#[$attr])*
        unsafe fn $name(st: &mut ClickStage, i: usize, p: *const u8) -> (usize, bool) {
            // Three windows of 64 bytes, as in `split_event`: one for each ID
            // (33 bytes, 35 with quotes) and its comma, one for the rest.
            // Window 1: click_id and its comma (no newline before it).
            let (cm, nm, qm) = $masks(p);
            let c0 = cm.trailing_zeros() as u64;
            let mut bad = (c0 >> 6) | _bzhi_u64(nm, c0 as u32);
            let q0 = _bzhi_u64(qm, c0 as u32);
            // The field starts with a quote; it ends with one.
            let f0 = q0 & 1;
            let l0 = (q0 << 1 >> (c0 & 63)) & 1;

            // Window 2: user_id and its comma.
            let o1 = (c0 & 63) + 1;
            let (cm, nm, qm) = $masks(p.add(o1 as usize));
            let c1 = cm.trailing_zeros() as u64;
            bad |= (c1 >> 6) | _bzhi_u64(nm, c1 as u32);
            let q1 = _bzhi_u64(qm, c1 as u32);
            let f1 = q1 & 1;
            let l1 = (q1 << 1 >> (c1 & 63)) & 1;

            // Window 3: campaign_id, ts and the newline.
            let o = o1 + (c1 & 63) + 1;
            let (cm, nm, qm) = $masks(p.add(o as usize));
            let nl = nm.trailing_zeros() as u64;
            let cmb = _bzhi_u64(cm, nl as u32);
            let c2 = cmb.trailing_zeros() as u64;
            let q = _bzhi_u64(qm, nl as u32);
            let d = cmb | 1u64.wrapping_shl(nl as u32);
            let f = f0 | f1 << 1 | (_pext_u64(q >> 1, d) << 1 | (q & 1)) << 2;
            let l = l0 | l1 << 1 | _pext_u64(q << 1, d) << 2;
            // The newline is in the window, with exactly one comma before it, and
            // the quoting is sound (see `split_event`).
            bad |= (nl >> 6)
                | (cmb.count_ones() as u64 ^ 1)
                | (f ^ l)
                | ((q0.count_ones() + q1.count_ones() + q.count_ones()) as u64 ^ 2 * f.count_ones() as u64);
            if bad != 0 {
                return (0, false);
            }
            *st.p.get_unchecked_mut(i) = p;
            *st.sep.get_unchecked_mut(i) = c0 | (o1 + c1) << 8 | (o + c2) << 16 | (o + nl) << 24 | f << 56;
            ((o + nl + 1) as usize, (c0 != 2 * f0) & (c1 != 2 * f1))
        }
    };
}
split_click_fn!(#[inline(always)] split_click, masks64);
split_click_fn!(
    #[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq,avx512vbmi,avx512vbmi2")]
    #[inline]
    split_click_wide,
    masks64_wide
);

const C_DEAD: u8 = 1;
const C_VALID: u8 = 2;
/// With `C_VALID`: the click is valid if its user_id, which is 33 bytes, is a
/// long ID (ids.rs): `click_long` decides, and finds the key.
const C_LONG: u8 = 4;

/// Decode the batch's click lines, in two loops with few values each: the
/// IDs, then the timestamp. Conditions are integers that are zero when all is
/// well (see `stage_decode`).
#[inline(never)]
unsafe fn click_decode(ctx: &Ctx, st: &mut ClickStage, n: usize) {
    const LOW: u64 = 0x0101_0101_0101_0101;
    const HIGH: u64 = 0x8080_8080_8080_8080;
    let index = &ctx.campaigns().index;
    let table = &ctx.table;
    for i in 0..n {
        let p = *st.p.get_unchecked(i);
        let (start, len) = fields(*st.sep.get_unchecked(i));
        let sl = st.sl.as_mut_ptr() as *mut u64;
        sl.write_volatile(start);
        sl.add(1).write_volatile(len);
        let sl = sl as *const u8;
        let at = |k: usize| p.add(sl.add(k).read_volatile() as usize);
        let ln = |k: usize| sl.add(8 + k).read_volatile() as u64;

        // click_id: a letter and 1 to 8 ASCII bytes, none of them NUL, is its
        // own key (ids::raw_key); any other is keyed by where it is
        // (ids::ref_key; the line is at most 255 bytes).
        let (n0, a0) = (ln(0), at(0));
        let bits = (8 * n0 as u32).wrapping_sub(8);
        let raw = (a0.add(1) as *const u64).read_unaligned();
        let click_odd =
            (((raw.wrapping_sub(LOW) & !raw) | raw) & _bzhi_u64(HIGH, bits)) | (*a0 ^ b'k') as u64 | n0.wrapping_sub(2) >> 3;
        *st.click.get_unchecked_mut(i) = if click_odd == 0 { _bzhi_u64(raw, bits).swap_bytes() } else { ref_word(a0, n0) };

        // user_id: the same shape with `u` is its own key, and `odd` stays
        // zero for it; one of 33 bytes is taken for a long ID, for
        // `click_long` to look at.
        let (n1, a1) = (ln(1), at(1));
        let bits = (8 * n1 as u32).wrapping_sub(8);
        let raw = (a1.add(1) as *const u64).read_unaligned();
        let user_long = n1 == LONG_LEN as u64;
        let mut odd = if user_long {
            0
        } else {
            (((raw.wrapping_sub(LOW) & !raw) | raw) & _bzhi_u64(HIGH, bits)) | (*a1 ^ b'u') as u64 | n1.wrapping_sub(2) >> 3
        };
        let user = _bzhi_u64(raw, bits).swap_bytes();
        *st.user.get_unchecked_mut(i) = user;
        *st.home.get_unchecked_mut(i) = table.prefetch_home(user) as u32;

        // campaign_id: `c_` and 1 to 4 digits without a leading zero is a
        // number for the table. Anything else is rare (unknown campaigns have
        // this shape too) and left to the scalar path.
        let a2 = at(2);
        let m = (ln(2) as u32).wrapping_sub(2);
        let digits = _bzhi_u32((a2.add(2) as *const u32).read_unaligned() ^ 0x3030_3030, 8 * m);
        // The first digit is 1 to 9, the others 0 to 9: one less in the first
        // byte, and no byte may exceed 8, 9, 9, 9 (a first digit of 0 borrows
        // and sets its top bit).
        let x = digits.wrapping_sub(1);
        odd |= ((x.wrapping_add(0x7676_7677) | x) & 0x8080_8080) as u64;
        odd |= ((a2 as *const u16).read_unaligned() ^ u16::from_le_bytes(*b"c_")) as u64;
        odd |= (m.wrapping_sub(1) >> 2) as u64;
        // Most significant digit first: shift the digits to the top bytes, so
        // that the missing leading ones read as zeros.
        let t = digits.wrapping_shl(32u32.wrapping_sub(8 * m));
        let pairs = t.wrapping_mul(10).wrapping_add(t >> 8);
        let number = (pairs & 0xFF) * 100 + (pairs >> 16 & 0xFF);
        *st.tc.get_unchecked_mut(i) = index.by_number(number as usize) as u64;
        *st.flags.get_unchecked_mut(i) = (odd != 0) as u8 | (user_long as u8) << 2;
        *st.ts_at.get_unchecked_mut(i) = sl.add(3).read_volatile();
        *st.ts_len.get_unchecked_mut(i) = sl.add(11).read_volatile();
    }

    let dt = &ctx.dates;
    let (t0, span) = (ctx.t0, (ctx.we - ctx.t0) as u64);
    for i in 0..n {
        let a = (*st.p.get_unchecked(i)).add(*st.ts_at.get_unchecked(i) as usize);
        let (ts_dead, ts_ok, ms) = event_ts(dt, a, *st.ts_len.get_unchecked(i) as usize);
        let rel = ms.wrapping_sub(t0) as u64;
        let campaign = *st.tc.get_unchecked(i);
        let fl = *st.flags.get_unchecked(i);
        let dead = (fl & C_DEAD) | ts_dead as u8;
        let valid = ts_ok & (rel < span) & (campaign != ABSENT as u64) & (dead == 0);
        *st.tc.get_unchecked_mut(i) = rel << CAMPAIGN_BITS | campaign;
        // (`C_LONG` only on a valid line: nobody asks for an invalid click's user.)
        *st.flags.get_unchecked_mut(i) = dead | (valid as u8) << 1 | (fl & ((valid as u8) << 2));
    }
}

/// The users of the valid click lines flagged `C_LONG`, as `stage_long` has
/// them for events: a long ID gets its key and loses the flag, anything else
/// of 33 bytes is for the scalar path.
#[inline(never)]
unsafe fn click_long(ctx: &Ctx, st: &mut ClickStage, n: usize) {
    let mut list = [0u8; NB];
    let mut m = 0usize;
    for i in 0..n {
        *list.get_unchecked_mut(m) = i as u8;
        m += (*st.flags.get_unchecked(i) == C_VALID | C_LONG) as usize;
    }
    if m == 0 {
        return;
    }
    let table = ctx.users.long.as_ref().unwrap_unchecked();
    let mut value = [(0u64, 0u64); NB];
    for k in 0..m {
        let i = *list.get_unchecked(k) as usize;
        let (start, _) = fields(*st.sep.get_unchecked(i));
        let a = (*st.p.get_unchecked(i)).add((start >> 8) as usize & 0xFF);
        let (hex, hi, lo) = hex32(a.add(1));
        if !hex | (*a != b'u') | (hi == 0) | (lo == 0) {
            *st.flags.get_unchecked_mut(i) = C_DEAD;
            continue;
        }
        table.prefetch(hi, lo);
        *value.get_unchecked_mut(k) = (hi, lo);
    }
    for k in 0..m {
        let i = *list.get_unchecked(k) as usize;
        if *st.flags.get_unchecked(i) == C_DEAD {
            continue;
        }
        let (hi, lo) = *value.get_unchecked(k);
        let key = ctx.users.long_key(hi, lo).unwrap_unchecked();
        *st.user.get_unchecked_mut(i) = key;
        *st.home.get_unchecked_mut(i) = ctx.table.prefetch_home(key) as u32;
        *st.flags.get_unchecked_mut(i) = C_VALID;
    }
}

#[inline(never)]
unsafe fn click_emit(ctx: &Ctx, c: &mut PartWriter, st: &ClickStage, n: usize) {
    let cw = c.cursors();
    let (pool, table) = (&ctx.pool, &ctx.table);
    for i in 0..n {
        if *st.flags.get_unchecked(i) != C_VALID {
            continue;
        }
        let person = table.person_at(*st.home.get_unchecked(i) as usize, *st.user.get_unchecked(i)) as u64;
        let (part, w0) = click_words(person, *st.tc.get_unchecked(i));
        c.push2_at(cw, pool, part, w0, *st.click.get_unchecked(i));
    }
}

/// SS_SPLIT_CHECK for clicks: `splitw::split_clicks_batch` against `split_click`.
unsafe fn check_split_clicks(st: &ClickStage, data: &[u8], from: usize, to: usize, n: usize, took: usize) {
    let mut one: ClickStage = std::mem::zeroed();
    let (mut pos, mut m) = (from, 0usize);
    for line in 0..took {
        let p = data.as_ptr().add(pos);
        let (used, added) = split_click(&mut one, 0, p);
        let show = || String::from_utf8_lossy(&data[pos..(pos + 120).min(data.len())]).into_owned();
        assert!(used != 0, "batch split took click line {line} at {pos}, which the line split refuses: {}", show());
        if added {
            assert!(m < n, "batch split left out click line {line} at {pos}: {}", show());
            assert!(st.p[m] == p && st.sep[m] == one.sep[0], "click line {line} at {pos}: sep {:016x}, line split {:016x}: {}", st.sep[m], one.sep[0], show());
            m += 1;
        }
        pos += used;
    }
    assert!(m == n && pos == to, "batch split of clicks: {n} lines to {to}, line split: {m} lines to {pos}");
}

macro_rules! clicks_chunk_fn {
    ($(#[$attr:meta])* $name:ident, $split:ident, $decode:path, $batch:expr) => {
        $(#[$attr])*
        unsafe fn $name(ctx: &Ctx, c: &mut PartWriter, ch: &Chunk) -> usize {
            let data = ctx.files[ch.file as usize].slice();
            let len = data.len();
            let fast_hi = fast_end(ctx, len).min(ch.end);
            let ahead = ctx.prefetch_ahead;
            let mut lines = 0usize;
            let Some(mut pos) = first_line(data, ch.start) else { return 0 };
            let mut st = ClickStage {
                p: [std::ptr::null(); NB],
                sep: [0; NB],
                user: [0; NB],
                click: [0; NB],
                tc: [0; NB],
                home: [0; NB],
                flags: [0; NB],
                ts_at: [0; NB],
                ts_len: [0; NB],
                sl: [0; 2],
            };
            let mut ws = splitw::Wide::new();
            // The batch splitter, in the instance for processors that have it.
            let batch = $batch & ctx.wide_split;
            while pos < ch.end {
                let mut n = 0usize;
                // The line at `pos` after the batch is for the scalar path.
                let mut stuck = true;
                if (pos >= HEAD) & batch {
                    if pos < fast_hi {
                        unsafe {
                            let from = pos;
                            let (m, to, took, odd) =
                                splitw::split_clicks_batch(&mut st, &mut ws, data.as_ptr(), pos, fast_hi, ahead);
                            if ctx.split_check {
                                check_split_clicks(&st, data, from, to, m, took);
                            }
                            (n, pos, stuck) = (m, to, odd);
                            lines += took;
                        }
                    }
                } else if pos >= HEAD {
                    unsafe {
                        while (n < NB) & (pos < fast_hi) {
                            let p = data.as_ptr().add(pos);
                            _mm_prefetch::<_MM_HINT_T0>(p.wrapping_add(ahead) as *const i8);
                            let (used, added) = $split(&mut st, n, p);
                            if used == 0 {
                                break;
                            }
                            pos += used;
                            lines += 1;
                            n += added as usize;
                        }
                    }
                    stuck = n < NB;
                }
                if n > 0 {
                    unsafe {
                        $decode(ctx, &mut st, n);
                        click_long(ctx, &mut st, n);
                        click_emit(ctx, c, &st, n);
                        // The scalar path for what the stages could not decide.
                        if st.flags[..n].iter().fold(0, |a, &f| a | f) & C_DEAD != 0 {
                            for i in 0..n {
                                if st.flags[i] & C_DEAD != 0 {
                                    click_line(ctx, c, std::slice::from_raw_parts(st.p[i], (st.sep[i] >> 24 & 0xFF) as usize));
                                }
                            }
                        }
                    }
                }
                if stuck & (pos < ch.end) {
                    lines += 1;
                    let e = find_newline(data, pos).unwrap_or(len);
                    click_line(ctx, c, &data[pos..e]);
                    pos = e + 1;
                }
            }
            lines
        }
    };
}
clicks_chunk_fn!(clicks_chunk_base, split_click, click_decode, false);
clicks_chunk_fn!(
    #[target_feature(enable = "avx512f,avx512bw,avx512vl,avx512dq,avx512vbmi,avx512vbmi2")]
    clicks_chunk_wide,
    split_click_wide,
    wide::click_decode,
    true
);

pub fn clicks_chunk(ctx: &Ctx, c: &mut PartWriter, ch: &Chunk) -> usize {
    unsafe {
        if wide() {
            clicks_chunk_wide(ctx, c, ch)
        } else {
            clicks_chunk_base(ctx, c, ch)
        }
    }
}
