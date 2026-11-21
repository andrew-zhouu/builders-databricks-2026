//! Pass 2: dedup, by exception, and events to their people.
//!
//! The parser left a two-word trace for every well-formed event line: a
//! fingerprint of the event ID with the position of the line, and what became
//! of the line (did it pass validation, and if so whose event is it and
//! when). Here, per trace partition:
//!
//! 1. Find the fingerprints that occur more than once, with a small table of
//!    "seen" and "seen again" bits. Most traces are read twice for this and
//!    nothing else happens to them.
//! 2. The usual repeat is settled from the traces alone: when every copy
//!    passed validation, for the same person and time, exactly one of them
//!    counts, whichever is the kept one. (Two different event IDs would have
//!    to share 47 bits of hash and their person, timestamp and validity to be
//!    taken for copies here.) Copies that all failed count for nothing.
//! 3. For every other repeated fingerprint, read its lines again and parse
//!    them by the literal rules. Lines whose IDs really are equal are copies
//!    of one event: the copy with the smallest ingest_ts is the kept one
//!    (copies with equal ingest_ts are identical, so any of them will do).
//!    Every other valid copy must not count. If the kept copy is itself
//!    invalid, nothing of the event remains, as the rules require (dedup
//!    comes before validation).
//! 4. Every valid event that is left is written to its person's partition,
//!    into the trace blocks of the partitions already done.

use core::arch::x86_64::{_bzhi_u64, _mm_prefetch, _MM_HINT_T0};

use crate::parse::{can_decode, decode_event, event_records, parse_event, trace_record, Decoded, CANCEL, LOCAL_BITS, T_PURCHASE};
use crate::part::PartWriter;
use crate::util::find_newline;
use crate::Ctx;

/// "Use the scalar path" (see `parse::Decoded`).
const TO_SCALAR_LINE: Decoded =
    Decoded { used: 0, id: (std::ptr::null(), 0), valid: false, user_key: 0, ts: 0, extra: 0, ingest_s: 0 };

/// One thread's purchase cancellations: (person partition, record words).
/// (A cancelled event simply never reaches its person; a purchase's price and
/// category were written there by the parser and are taken back with these.)
#[derive(Default)]
pub struct Cancels {
    purchases: Vec<(u32, [u64; 2])>,
    /// After `finish`: where each partition's entries start.
    purchase_start: Vec<u32>,
    /// Valid copies of events that were not the kept one.
    pub dropped: usize,
}

impl Cancels {
    /// Order by partition, once all of pass 2 is done.
    pub fn finish(&mut self, nparts: usize) {
        self.purchases.sort_unstable_by_key(|e| e.0);
        let mut s = vec![0u32; nparts + 1];
        for e in &self.purchases {
            s[e.0 as usize + 1] += 1;
        }
        for p in 0..nparts {
            s[p + 1] += s[p];
        }
        self.purchase_start = s;
    }

    pub fn purchases(&self, p: usize) -> &[(u32, [u64; 2])] {
        &self.purchases[self.purchase_start[p] as usize..self.purchase_start[p + 1] as usize]
    }

    pub fn len(&self) -> usize {
        self.dropped
    }
}

/// One copy of a possibly repeated event, read again from the input.
struct Copy {
    /// The trace's first word (which names the line), and its fingerprint.
    w1: u64,
    fp: u64,
    id: (*const u8, usize),
    /// The first 16 bytes of the event_id, zero-padded: settling compares
    /// these, because by then the line itself has left the cache.
    head: [u64; 2],
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
    seen: Vec<u64>,
    seen2: Vec<u64>,
    /// The first words of the traces whose events must not count, as a list,
    /// as an open-addressing set, and as a bitmap in front of that.
    dead: Vec<u64>,
    dead_set: Vec<u64>,
    dead_bits: Vec<u64>,
    cand: Vec<[u64; 2]>,
    tmp: Vec<[u64; 2]>,
    /// The first words of the traces whose lines must be read again.
    reread: Vec<u64>,
    copies: Vec<Copy>,
    pending: Vec<Pending>,
}

pub fn run(ctx: &Ctx, sc: &mut Scratch, out: &mut Cancels, s: &mut PartWriter, p: usize) {
    let t0 = ctx.now();
    // A trace is two words (parse::T_VALID and friends describe the second).
    let n: usize = (0..ctx.nthreads).map(|t| ctx.k(t).part_words(p)).sum::<usize>() / 2;
    if n == 0 {
        return;
    }
    sc.dead.clear();
    let t0 = if n >= 2 { repeats(ctx, sc, out, p, n, t0) } else { t0 };

    // The traces whose events must not count, as a set with a bitmap of 16
    // bits per entry in front of it.
    let ndead = sc.dead.len();
    out.dropped += ndead;
    let bits = (4 * ndead.max(16)).next_power_of_two().trailing_zeros();
    let mask = (1usize << bits) - 1;
    let fbits = (16 * ndead.max(16)).next_power_of_two().trailing_zeros().max(6);
    let mix = |w: u64| w.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let slot_of = |h: u64| (h >> (64 - bits)) as usize;
    let bit_of = |h: u64| ((h << 20) >> (64 - fbits)) as usize;
    if ndead > 0 {
        sc.dead_set.clear();
        sc.dead_set.resize(1 << bits, 0);
        sc.dead_bits.clear();
        sc.dead_bits.resize(1 << (fbits - 6), 0);
        for &w in &sc.dead {
            // (A trace's first word is never 0: a line of the first megabyte
            // of input would need a zero fingerprint too; harmless if so.)
            let h = mix(w);
            let mut i = slot_of(h);
            while sc.dead_set[i] != 0 {
                i = (i + 1) & mask;
            }
            sc.dead_set[i] = w;
            sc.dead_bits[bit_of(h) >> 6] |= 1u64 << (bit_of(h) & 63);
        }
    }
    let (set, filter) = (&sc.dead_set[..], &sc.dead_bits[..]);

    // Every valid event that is left goes to its person's partition.
    let sw = s.cursors();
    let pool = &ctx.pool;
    for t in 0..ctx.nthreads {
        ctx.k(t).for_each_run(pool, p, |run| {
            for r in run.chunks_exact(2) {
                let (w1, w2) = (r[0], r[1]);
                if w2 == 0 {
                    continue;
                }
                if ndead > 0 {
                    let h = mix(w1);
                    let b = bit_of(h);
                    if unsafe { *filter.get_unchecked(b >> 6) } & (1u64 << (b & 63)) != 0 {
                        let mut i = slot_of(h);
                        let mut dead = false;
                        loop {
                            let e = unsafe { *set.get_unchecked(i) };
                            if e == 0 {
                                break;
                            }
                            if e == w1 {
                                dead = true;
                                break;
                            }
                            i = (i + 1) & mask;
                        }
                        if dead {
                            continue;
                        }
                    }
                }
                let (part, rec) = trace_record(w2);
                unsafe { s.push1_at(sw, pool, part as usize, rec) };
            }
        });
    }
    // The partition's trace blocks are free now: the events of later
    // partitions are written into them.
    for t in 0..ctx.nthreads {
        s.free.extend_from_slice(ctx.k(t).part_blocks(p));
    }
    ctx.charge_rows(crate::P_D_GATHER, t0, n);
}

/// Find the event IDs that occur more than once among the `n` traces of
/// partition `p` and settle them: the first words of the traces whose events
/// must not count go to `sc.dead`, purchase cancellations to `out`.
fn repeats(ctx: &Ctx, sc: &mut Scratch, out: &mut Cancels, p: usize, n: usize, t0: crate::prof::Snap) -> crate::prof::Snap {
    // Which fingerprints occur more than once? A table of two bits per slot,
    // "seen" and "seen again", indexed by the fingerprint's low bits, with 8
    // slots per trace so that few unrelated traces share one. The traces are
    // read where the parser left them, twice.
    let slots = (8 * n).next_power_of_two().max(4096);
    sc.seen.clear();
    sc.seen.resize(slots / 32, 0);
    let tab = &mut sc.seen[..];
    let (shift, mask) = (ctx.loc_bits, slots as u64 - 1);
    for t in 0..ctx.nthreads {
        ctx.k(t).for_each_run(&ctx.pool, p, |run| {
            for r in run.chunks_exact(2) {
                let i = (r[0] >> shift) & mask;
                let (w, b) = ((i >> 5) as usize, 1u64 << (2 * (i & 31)));
                unsafe {
                    let v = *tab.get_unchecked(w);
                    *tab.get_unchecked_mut(w) = v | b | (v & b) << 1;
                }
            }
        });
    }
    // Collected without a branch: every trace is written, and the write
    // position only moves on past the ones that are kept.
    sc.cand.clear();
    sc.cand.reserve(n);
    let mut kept = 0usize;
    for t in 0..ctx.nthreads {
        ctx.k(t).for_each_run(&ctx.pool, p, |run| {
            for r in run.chunks_exact(2) {
                let i = (r[0] >> shift) & mask;
                unsafe {
                    let again = *tab.get_unchecked((i >> 5) as usize) >> (2 * (i & 31) + 1) & 1;
                    *sc.cand.as_mut_ptr().add(kept) = [r[0], r[1]];
                    kept += again as usize;
                }
            }
        });
    }
    unsafe { sc.cand.set_len(kept) };
    // About half of those only share a slot with an unrelated trace. The same
    // test again on other bits of the fingerprint removes nearly all of them,
    // so that only true repeats are sorted.
    if ctx.fp_mask >> 14 != 0 && sc.cand.len() >= 2 {
        let slots = (16 * sc.cand.len()).next_power_of_two().max(4096);
        let (shift2, mask2) = (shift + 13, (slots as u64 - 1) & (ctx.fp_mask >> 13));
        sc.seen2.clear();
        sc.seen2.resize(slots / 32, 0);
        let tab = &mut sc.seen2[..];
        for r in &sc.cand {
            let i = (r[0] >> shift2) & mask2;
            let (w, b) = ((i >> 5) as usize, 1u64 << (2 * (i & 31)));
            let v = tab[w];
            tab[w] = v | b | (v & b) << 1;
        }
        sc.cand.retain(|r| {
            let i = (r[0] >> shift2) & mask2;
            tab[(i >> 5) as usize] >> (2 * (i & 31) + 1) & 1 != 0
        });
    }
    let t0 = ctx.charge_rows(crate::P_D_DEDUP, t0, n);
    if sc.cand.is_empty() {
        return t0;
    }

    // Bring equal fingerprints together. A fingerprint that turns out to be
    // unique after all is done with. So is the usual repeat, which the traces
    // settle by themselves: every copy valid, of the same person at the same
    // time (and not a purchase, whose price the trace does not hold). Whichever
    // copy is the kept one, exactly one of those identical events counts: all
    // but the first are dropped. And copies that all failed validation count
    // for nothing anyway. Only what is left needs its lines read again:
    // `sc.reread`, groups together.
    sort_by_fp(&mut sc.cand, &mut sc.tmp, shift);
    sc.reread.clear();
    let cand = &sc.cand[..];
    let mut i = 0;
    while i < cand.len() {
        let fp = cand[i][0] >> shift;
        let w2 = cand[i][1];
        let mut same = true;
        let mut j = i + 1;
        while j < cand.len() && cand[j][0] >> shift == fp {
            same &= cand[j][1] == w2;
            j += 1;
        }
        if j - i >= 2 {
            if same & (w2 & T_PURCHASE == 0) {
                if w2 != 0 {
                    sc.dead.extend(cand[i + 1..j].iter().map(|r| r[0]));
                }
            } else {
                sc.reread.extend(cand[i..j].iter().map(|r| r[0]));
            }
        }
        i = j;
    }
    let t0 = ctx.charge_rows(crate::P_D_GROUP, t0, sc.cand.len());
    read_again(ctx, sc);
    let t0 = ctx.charge_rows(crate::P_D_READ, t0, sc.reread.len());
    settle(&mut sc.copies, out, &mut sc.dead);
    ctx.charge_rows(crate::P_D_EMIT, t0, sc.reread.len())
}

/// Order traces by fingerprint (the bits of their first word from `shift`
/// up): a stable LSD radix sort, 11 bits a pass.
fn sort_by_fp(v: &mut Vec<[u64; 2]>, tmp: &mut Vec<[u64; 2]>, shift: u32) {
    let n = v.len();
    if n < 256 || shift < 20 {
        v.sort_unstable_by_key(|r| r[0] >> shift);
        return;
    }
    tmp.clear();
    tmp.resize(n, [0; 2]);
    let mut hist = [0u32; 2048];
    let mut s = shift;
    while s < 64 {
        hist.fill(0);
        for r in v.iter() {
            hist[(r[0] >> s) as usize & 2047] += 1;
        }
        let mut total = 0u32;
        for c in hist.iter_mut() {
            let k = *c;
            *c = total;
            total += k;
        }
        for r in v.iter() {
            let c = &mut hist[(r[0] >> s) as usize & 2047];
            unsafe { *tmp.get_unchecked_mut(*c as usize) = *r };
            *c += 1;
        }
        std::mem::swap(v, tmp);
        s += 11;
    }
}

/// Read the lines of the traces in `sc.reread` again, into `sc.copies` (same
/// order). The lines are scattered over the input and each valid copy needs a
/// slot of the user table, so this is a pipeline: a line is asked for (two
/// cache lines) `AHEAD` traces before it is decoded, and the user's slot is
/// asked for then and read `LAG` valid copies later.
fn read_again(ctx: &Ctx, sc: &mut Scratch) {
    const AHEAD: usize = 32;
    const LAG: usize = 32;
    let (shift, loc_mask) = (ctx.loc_bits, (1u64 << ctx.loc_bits) - 1);
    let n = sc.reread.len();
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
            let (data, at) = ctx.locate(sc.reread[j] & loc_mask);
            let q = unsafe { data.as_ptr().add(at) };
            unsafe {
                _mm_prefetch::<_MM_HINT_T0>(q as *const i8);
                _mm_prefetch::<_MM_HINT_T0>(q.wrapping_add(64) as *const i8);
                // The decoder's second 64-byte window usually reaches into a third cache line.
                _mm_prefetch::<_MM_HINT_T0>(q.wrapping_add(128) as *const i8);
            }
            ring[j % AHEAD] = (q, at, data.len());
        }
        if j < AHEAD {
            continue;
        }
        let t = sc.reread[j - AHEAD];
        let mut copy = Copy { w1: t, fp: t >> shift, id: (p, 0), head: [0; 2], ingest: 0, valid: false, part: 0, rec: 0, extra: 0 };
        let d = if can_decode(ctx, start, len) { unsafe { decode_event::<true>(ctx, p) } } else { TO_SCALAR_LINE };
        if d.used != 0 {
            copy.id = d.id;
            // (At least `MARGIN` bytes are readable from a line this decoder takes.)
            copy.head = unsafe {
                let n = d.id.1 as u32;
                [
                    _bzhi_u64((d.id.0 as *const u64).read_unaligned(), 8 * n),
                    _bzhi_u64((d.id.0.add(8) as *const u64).read_unaligned(), 8 * n.saturating_sub(8)),
                ]
            };
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
                let mut head = [0u8; 16];
                let n = e.id.len().min(16);
                head[..n].copy_from_slice(&e.id[..n]);
                copy.head = [
                    u64::from_le_bytes(head[..8].try_into().unwrap()),
                    u64::from_le_bytes(head[8..].try_into().unwrap()),
                ];
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
    (a.id.1 == b.id.1) & (a.head == b.head) && (a.id.1 <= 16 || a.id() == b.id())
}

/// Copies of one event: every valid one that is not the kept one must not
/// count (and takes its purchase back).
#[inline(always)]
fn cancel(copies: &[Copy], out: &mut Cancels, dead: &mut Vec<u64>) {
    let mut kept = 0;
    for (at, c) in copies.iter().enumerate() {
        if c.ingest < copies[kept].ingest {
            kept = at;
        }
    }
    for (at, c) in copies.iter().enumerate() {
        if at != kept && c.valid {
            dead.push(c.w1);
            if c.extra != 0 {
                out.purchases.push((c.part, [c.rec | CANCEL, c.extra]));
            }
        }
    }
}

/// Group the copies by fingerprint and then by their actual event IDs, and
/// drop every valid copy that is not the kept one of its event.
fn settle(copies: &mut [Copy], out: &mut Cancels, dead: &mut Vec<u64>) {
    let mut a = 0;
    while a < copies.len() {
        let mut b = a + 1;
        while b < copies.len() && copies[b].fp == copies[a].fp {
            b += 1;
        }
        let group = &mut copies[a..b];
        a = b;
        // Nearly always one event's copies.
        if group[1..].iter().all(|c| same_id(c, &group[0])) {
            cancel(group, out, dead);
            continue;
        }
        if group.len() == 2 {
            continue;
        }
        // Different events share the fingerprint: bring equal IDs together
        // (any order that does will do; stable, so copies stay in input order).
        group.sort_by(|x, y| {
            (x.id.1, x.head).cmp(&(y.id.1, y.head)).then_with(|| {
                if x.id.1 > 16 {
                    x.id().cmp(y.id())
                } else {
                    std::cmp::Ordering::Equal
                }
            })
        });
        let mut i = 0;
        while i < group.len() {
            let mut j = i + 1;
            while j < group.len() && same_id(&group[j], &group[i]) {
                j += 1;
            }
            if j - i >= 2 {
                cancel(&group[i..j], out, dead);
            }
            i = j;
        }
    }
}
