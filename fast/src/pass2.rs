//! Pass 2: dedup, by exception.
//!
//! The parser wrote every valid event to its person as if no event ID were
//! ever repeated, and left a trace (fingerprint of the ID, position of the
//! line) for every well-formed event line. Here, per trace partition:
//!
//! 1. Find the fingerprints that occur more than once, with two bitmaps
//!    ("seen" and "seen again") that fit in the L1 cache. Most partitions'
//!    traces are read twice and nothing else happens to them.
//! 2. For each such fingerprint, read its lines again from the input and parse
//!    them by the literal rules. Lines whose IDs really are equal are copies
//!    of one event: the copy with the smallest ingest_ts is the kept one
//!    (copies with equal ingest_ts are identical, so any of them will do).
//! 3. Every other copy that passed validation was written to a person by the
//!    parser and must not count: it gets a cancellation, which pass 3 applies
//!    to exactly one equal record. If the kept copy is itself invalid, nothing
//!    of the event remains, as the rules require (dedup comes before
//!    validation).

use core::arch::x86_64::{__m128i, _mm_cmpeq_epi8, _mm_loadu_si128, _mm_movemask_epi8, _mm_prefetch, _MM_HINT_T0};

use crate::parse::{can_decode, decode_event, event_records, parse_event, Decoded, CANCEL, LOCAL_BITS};
use crate::util::find_newline;
use crate::Ctx;

/// "Use the scalar path" (see `parse::Decoded`).
const TO_SCALAR_LINE: Decoded =
    Decoded { used: 0, id: (std::ptr::null(), 0), valid: false, user_key: 0, ts: 0, extra: 0, ingest_s: 0 };

/// One thread's cancellations: (person partition, record words).
#[derive(Default)]
pub struct Cancels {
    events: Vec<(u32, u64)>,
    purchases: Vec<(u32, [u64; 2])>,
    /// After `finish`: where each partition's entries start.
    event_start: Vec<u32>,
    purchase_start: Vec<u32>,
}

impl Cancels {
    /// Order by partition, once all of pass 2 is done.
    pub fn finish(&mut self, nparts: usize) {
        self.events.sort_unstable_by_key(|e| e.0);
        self.purchases.sort_unstable_by_key(|e| e.0);
        fn starts<T>(v: &[(u32, T)], nparts: usize) -> Vec<u32> {
            let mut s = vec![0u32; nparts + 1];
            for e in v {
                s[e.0 as usize + 1] += 1;
            }
            for p in 0..nparts {
                s[p + 1] += s[p];
            }
            s
        }
        self.event_start = starts(&self.events, nparts);
        self.purchase_start = starts(&self.purchases, nparts);
    }

    pub fn events(&self, p: usize) -> &[(u32, u64)] {
        &self.events[self.event_start[p] as usize..self.event_start[p + 1] as usize]
    }

    pub fn purchases(&self, p: usize) -> &[(u32, [u64; 2])] {
        &self.purchases[self.purchase_start[p] as usize..self.purchase_start[p + 1] as usize]
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }
}

/// One copy of a possibly repeated event, read again from the input.
struct Copy {
    fp: u64,
    id: (*const u8, usize),
    /// Epoch ms.
    ingest: i64,
    valid: bool,
    /// The records the parser wrote for it, if valid.
    part: u32,
    rec: u64,
    extra: u64,
}

impl Copy {
    fn id(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.id.0, self.id.1) }
    }
}

/// A valid copy waiting for its person (the user-table slot is being fetched).
struct Pending {
    copy: u32,
    home: u32,
    key: u64,
    ts: u32,
}

#[derive(Default)]
pub struct Scratch {
    recs: Vec<u64>,
    seen: Vec<u64>,
    again: Vec<u64>,
    cand: Vec<u64>,
    copies: Vec<Copy>,
    pending: Vec<Pending>,
    tmp: Vec<u64>,
}

pub fn run(ctx: &Ctx, sc: &mut Scratch, out: &mut Cancels, p: usize) {
    let t0 = ctx.now();
    sc.recs.clear();
    for t in 0..ctx.nthreads {
        ctx.k(t).copy_part(&ctx.pool, p, &mut sc.recs);
    }
    let recs = &sc.recs[..];
    let n = recs.len();
    if n < 2 {
        return;
    }
    let t0 = ctx.charge_rows(crate::P_D_GATHER, t0, n);

    // Which fingerprints occur more than once? Indexed by the fingerprint's low
    // bits, at 8 bits per trace, so few unrelated traces share a bit.
    let bits = (8 * n).next_power_of_two().max(4096);
    sc.seen.clear();
    sc.seen.resize(bits / 64, 0);
    sc.again.clear();
    sc.again.resize(bits / 64, 0);
    let (seen, again) = (&mut sc.seen[..], &mut sc.again[..]);
    let (shift, mask) = (ctx.loc_bits, bits as u64 - 1);
    for &r in recs {
        let i = (r >> shift) & mask;
        let (w, b) = ((i >> 6) as usize, 1u64 << (i & 63));
        unsafe {
            *again.get_unchecked_mut(w) |= *seen.get_unchecked(w) & b;
            *seen.get_unchecked_mut(w) |= b;
        }
    }
    sc.cand.clear();
    for &r in recs {
        let i = (r >> shift) & mask;
        if unsafe { *again.get_unchecked((i >> 6) as usize) } & (1u64 << (i & 63)) != 0 {
            sc.cand.push(r);
        }
    }
    // About half of those only share a bitmap bit with an unrelated trace. The
    // same test again on other bits of the fingerprint removes nearly all of
    // them, so that only true repeats are sorted and read again.
    if ctx.fp_mask >> 14 != 0 && sc.cand.len() >= 2 {
        let bits = (16 * sc.cand.len()).next_power_of_two().max(4096).min(64 * seen.len());
        let (shift2, mask2) = (shift + 13, (bits as u64 - 1) & (ctx.fp_mask >> 13));
        seen[..bits / 64].fill(0);
        again[..bits / 64].fill(0);
        for &r in &sc.cand {
            let i = (r >> shift2) & mask2;
            let (w, b) = ((i >> 6) as usize, 1u64 << (i & 63));
            again[w] |= seen[w] & b;
            seen[w] |= b;
        }
        sc.cand.retain(|&r| {
            let i = (r >> shift2) & mask2;
            again[(i >> 6) as usize] & (1u64 << (i & 63)) != 0
        });
    }
    let t0 = ctx.charge_rows(crate::P_D_DEDUP, t0, n);
    if sc.cand.is_empty() {
        return;
    }

    // Bring equal fingerprints together, and drop the traces whose fingerprint
    // turns out to be unique after all.
    sort_by_fp(&mut sc.cand, &mut sc.tmp, shift);
    let mut kept = 0;
    let mut i = 0;
    while i < sc.cand.len() {
        let fp = sc.cand[i] >> shift;
        let mut j = i + 1;
        while j < sc.cand.len() && sc.cand[j] >> shift == fp {
            j += 1;
        }
        if j - i >= 2 {
            sc.cand.copy_within(i..j, kept);
            kept += j - i;
        }
        i = j;
    }
    sc.cand.truncate(kept);
    read_again(ctx, sc);
    settle(&mut sc.copies, out);
    ctx.charge_rows(crate::P_D_EMIT, t0, sc.cand.len());
}

/// Order traces by fingerprint (their bits from `shift` up): a stable LSD
/// radix sort, 11 bits a pass.
fn sort_by_fp(v: &mut Vec<u64>, tmp: &mut Vec<u64>, shift: u32) {
    let n = v.len();
    if n < 256 || shift < 20 {
        v.sort_unstable();
        return;
    }
    tmp.clear();
    tmp.resize(n, 0);
    let mut hist = [0u32; 2048];
    let mut s = shift;
    while s < 64 {
        hist.fill(0);
        for &w in v.iter() {
            hist[(w >> s) as usize & 2047] += 1;
        }
        let mut total = 0u32;
        for c in hist.iter_mut() {
            let k = *c;
            *c = total;
            total += k;
        }
        for &w in v.iter() {
            let c = &mut hist[(w >> s) as usize & 2047];
            unsafe { *tmp.get_unchecked_mut(*c as usize) = w };
            *c += 1;
        }
        std::mem::swap(v, tmp);
        s += 11;
    }
}

/// Read the lines of the traces in `sc.cand` again, into `sc.copies` (same
/// order). The lines are scattered over the input and each valid copy needs a
/// slot of the user table, so this is a pipeline: a line is asked for (two
/// cache lines) `AHEAD` traces before it is decoded, and the user's slot is
/// asked for then and read `LAG` valid copies later.
fn read_again(ctx: &Ctx, sc: &mut Scratch) {
    const AHEAD: usize = 32;
    const LAG: usize = 32;
    let (shift, loc_mask) = (ctx.loc_bits, (1u64 << ctx.loc_bits) - 1);
    let n = sc.cand.len();
    sc.copies.clear();
    sc.copies.reserve(n);
    sc.pending.clear();
    let mut ring = [(std::ptr::null::<u8>(), 0usize, 0usize); AHEAD];
    let mut done = 0usize;
    let resolve = |copies: &mut Vec<Copy>, w: &Pending| {
        let person = ctx.table.person(ctx.table.insert_at(w.home as usize, w.key)) as u64;
        let copy = &mut copies[w.copy as usize];
        copy.part = (person >> LOCAL_BITS) as u32;
        copy.rec = (person & ((1 << LOCAL_BITS) - 1)) << 31 | w.ts as u64;
    };
    for j in 0..n + AHEAD {
        let (p, start, len) = ring[j % AHEAD];
        if j < n {
            let (data, at) = ctx.locate(sc.cand[j] & loc_mask);
            let q = unsafe { data.as_ptr().add(at) };
            unsafe {
                _mm_prefetch::<_MM_HINT_T0>(q as *const i8);
                _mm_prefetch::<_MM_HINT_T0>(q.wrapping_add(64) as *const i8);
            }
            ring[j % AHEAD] = (q, at, data.len());
        }
        if j < AHEAD {
            continue;
        }
        let t = sc.cand[j - AHEAD];
        let mut copy = Copy { fp: t >> shift, id: (p, 0), ingest: 0, valid: false, part: 0, rec: 0, extra: 0 };
        let d = if can_decode(ctx, start, len) { unsafe { decode_event::<true>(ctx, p) } } else { TO_SCALAR_LINE };
        if d.used != 0 {
            copy.id = d.id;
            copy.ingest = d.ingest_s * 1000;
            copy.valid = d.valid;
            copy.extra = d.extra;
            if d.valid {
                let home = ctx.table.prefetch_home(d.user_key) as u32;
                sc.pending.push(Pending { copy: sc.copies.len() as u32, home, key: d.user_key, ts: (d.ts - ctx.t0) as u32 });
            }
        } else {
            // The literal rules. Every trace was written for a line that
            // parsed, so this cannot fail.
            let line = unsafe { std::slice::from_raw_parts(p, len - start) };
            let line = &line[..find_newline(line, 0).unwrap_or(line.len())];
            if let Some(e) = parse_event(ctx, line) {
                copy.id = (e.id.as_ptr(), e.id.len());
                copy.ingest = e.ingest;
                copy.valid = e.valid;
                if e.valid {
                    let (part, rec, extra) = event_records(ctx, &e);
                    (copy.part, copy.rec, copy.extra) = (part as u32, rec, extra);
                }
            }
        }
        sc.copies.push(copy);
        if sc.pending.len() - done > LAG {
            resolve(&mut sc.copies, &sc.pending[done]);
            done += 1;
        }
    }
    while done < sc.pending.len() {
        resolve(&mut sc.copies, &sc.pending[done]);
        done += 1;
    }
}

/// Whether two copies have the same event_id.
#[inline(always)]
fn same_id(a: &Copy, b: &Copy) -> bool {
    let n = a.id.1;
    if n != b.id.1 {
        return false;
    }
    if n == 0 || n > 16 {
        return a.id() == b.id();
    }
    // A line that parsed has more than 16 bytes from the start of its
    // event_id (its ingest_ts alone is 20).
    unsafe {
        let x = _mm_loadu_si128(a.id.0 as *const __m128i);
        let y = _mm_loadu_si128(b.id.0 as *const __m128i);
        let differ = !(_mm_movemask_epi8(_mm_cmpeq_epi8(x, y)) as u32);
        differ & ((1u32 << n) - 1) == 0
    }
}

/// Copies of one event: cancel every valid one that is not the kept one.
#[inline(always)]
fn cancel(copies: &[Copy], out: &mut Cancels) {
    let mut kept = 0;
    for (at, c) in copies.iter().enumerate() {
        if c.ingest < copies[kept].ingest {
            kept = at;
        }
    }
    for (at, c) in copies.iter().enumerate() {
        if at != kept && c.valid {
            out.events.push((c.part, c.rec | CANCEL));
            if c.extra != 0 {
                out.purchases.push((c.part, [c.rec | CANCEL, c.extra]));
            }
        }
    }
}

/// Group the copies by fingerprint and then by their actual event IDs, and
/// cancel every valid copy that is not the kept one of its event.
fn settle(copies: &mut [Copy], out: &mut Cancels) {
    let mut a = 0;
    while a < copies.len() {
        let mut b = a + 1;
        while b < copies.len() && copies[b].fp == copies[a].fp {
            b += 1;
        }
        let group = &mut copies[a..b];
        a = b;
        if group.len() == 2 {
            if same_id(&group[0], &group[1]) {
                cancel(group, out);
            }
            continue;
        }
        // Stable: copies of one event stay in input order.
        group.sort_by(|x, y| x.id().cmp(y.id()));
        let mut i = 0;
        while i < group.len() {
            let mut j = i + 1;
            while j < group.len() && same_id(&group[j], &group[i]) {
                j += 1;
            }
            if j - i >= 2 {
                cancel(&group[i..j], out);
            }
            i = j;
        }
    }
}
