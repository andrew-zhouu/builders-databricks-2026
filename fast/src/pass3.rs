//! Pass 3: one person partition -> sessions, attribution, and output rows.
//!
//! A partition holds the people whose user-table slot falls in one 4096-slot
//! range, so a person is a 12-bit local index. Events are bucketed by person
//! with a counting sort (two passes straight over the partition's blocks), then
//! each person's timestamps are sorted on their own.

use crate::parse::{CAMPAIGN_BITS, CANCEL, CLICK_SHIFT, LOCAL_BITS, TS_MASK};
use crate::util::{DAY_MS, GAP_MS, REACH_MS};
use crate::Ctx;

const LOCALS: usize = 1 << LOCAL_BITS;

/// Per (campaign, day) counters of the sessions credited to the campaign.
#[derive(Clone, Copy, Default)]
pub struct Credit {
    pub sessions: u32,
    pub purchasing: u32,
    pub revenue: u64,
}

/// One thread's campaign counters, indexed by `campaign * ndays + day`. Assists
/// are counted far more often than credits (a session has one credit but
/// often ten assists), so they have a compact array of their own.
#[derive(Default)]
pub struct CampAcc {
    pub assisted: Vec<u32>,
    pub credit: Vec<Credit>,
}

/// user_daily rows produced by one thread, column by column, in the shape the
/// Parquet file wants them: the file is dictionary-encoded, which makes it
/// less than half the size of plain values.
#[derive(Default)]
pub struct Rows {
    pub n: usize,
    /// The user_id dictionary: 4-byte length + bytes, once per person, in
    /// order of appearance; `people` of them.
    pub ids: Vec<u8>,
    pub people: u32,
    /// Per row: the person's number in `ids`.
    pub id_idx: Vec<u32>,
    /// Per row: the day, counted from the first day of the click window (`Ctx::day0`).
    pub day: Vec<i32>,
    pub sessions: Vec<i32>,
    pub events: Vec<i32>,
    pub revenue: Vec<i64>,
    /// top_category: one validity bit per row (LSB first), and the category
    /// index of each row that has one.
    pub valid: Vec<u8>,
    pub cats: Vec<u32>,
}

impl Rows {
    /// Roughly what the rows take in a file.
    pub fn bytes(&self) -> usize {
        self.ids.len() + 6 * self.n
    }

    /// Start over (after the rows were written), keeping the memory.
    pub fn clear(&mut self) {
        self.n = 0;
        self.people = 0;
        self.ids.clear();
        self.id_idx.clear();
        self.day.clear();
        self.sessions.clear();
        self.events.clear();
        self.revenue.clear();
        self.valid.clear();
        self.cats.clear();
    }
}

/// The length-prefixed bytes of a regular user key (see ids.rs: `u`, then the
/// key's bytes), written into `out`; returns their total length (4 +
/// characters). Bytes after it are junk.
#[inline(always)]
fn id_record(key: u64, out: &mut [u8; 16]) -> usize {
    let n = 1 + crate::ids::raw_len(key);
    out[0..4].copy_from_slice(&(n as u32).to_le_bytes());
    out[4] = b'u';
    out[5..13].copy_from_slice(&key.to_be_bytes());
    4 + n
}

/// An event record is `person_local(12) | ts_rel_ms(31)`. Its slot is the top
/// 11 bits of the timestamp: slots are 2^20 ms (about 17.5 minutes) wide, which
/// must not exceed the 30-minute session gap (see `run`).
/// How many people ahead of the walk a person's user-table slot is prefetched.
const PEOPLE_AHEAD: usize = 4;
const SLOT_SHIFT: u32 = 20;
const SLOTS: usize = 1 << (31 - SLOT_SHIFT);
const _: () = assert!((1u32 << SLOT_SHIFT) <= GAP_MS);

pub struct Scratch {
    /// `start[l]..start[l + 1]` are the events of person `l` in the sorted buffer.
    start: Vec<u32>,
    /// Radix sort buffers and histograms (slots, then persons).
    a: Vec<u64>,
    b: Vec<u64>,
    h: Vec<u32>,
    pu: Vec<u64>,
    cl: Vec<u64>,
    tmp: Vec<u64>,
    starts: Vec<u32>,
    sdays: Vec<u32>,
    dfirst: Vec<u32>,
    stamp: Vec<u32>,
    tag: u32,
    cat_sum: Vec<u64>,
    touched: Vec<u32>,
    /// Scratch space for a person's purchases once the cancelled ones are removed.
    kept_pu: Vec<[u64; 2]>,
    /// The partition's people who have events.
    present: Vec<u16>,
    pub rows: Rows,
    pub acc: CampAcc,
}

impl Scratch {
    pub fn new(ctx: &Ctx) -> Scratch {
        Scratch {
            start: vec![0; LOCALS + 1],
            a: Vec::new(),
            b: Vec::new(),
            h: vec![0; SLOTS + LOCALS],
            pu: Vec::new(),
            cl: Vec::new(),
            tmp: Vec::new(),
            starts: Vec::new(),
            sdays: Vec::new(),
            dfirst: Vec::new(),
            stamp: vec![0; ctx.campaigns().ids.len()],
            tag: 0,
            cat_sum: vec![0; ctx.products().categories.len()],
            touched: Vec::new(),
            kept_pu: Vec::new(),
            present: Vec::new(),
            rows: Rows::default(),
            acc: CampAcc {
                assisted: vec![0; ctx.campaigns().ids.len() * ctx.ndays],
                credit: vec![Credit::default(); ctx.campaigns().ids.len() * ctx.ndays],
            },
        }
    }
}

#[inline(always)]
fn click_ts(w0: u64) -> u32 {
    ((w0 >> CAMPAIGN_BITS) & TS_MASK) as u32
}
#[inline(always)]
fn click_campaign(w0: u64) -> usize {
    (w0 & ((1 << CAMPAIGN_BITS) - 1)) as usize
}

fn pairs(v: &mut [u64]) -> &mut [[u64; 2]] {
    unsafe { std::slice::from_raw_parts_mut(v.as_mut_ptr() as *mut [u64; 2], v.len() / 2) }
}

/// Sort the two-word records in `v` by their first word, given that bits
/// `shift..shift + 12` of it are the person and everything above is zero or
/// constant: a counting sort by person, then each person's few records by
/// insertion (or the library sort when there are many). `tmp` and `hist` are
/// scratch space.
fn sort_pairs(v: &mut Vec<u64>, tmp: &mut Vec<u64>, hist: &mut [u32], shift: u32) {
    let n = v.len() / 2;
    if n < 2 {
        return;
    }
    if n < 1024 {
        // Few records (a partition's purchases): the per-person tables below
        // would cost more than sorting them outright: by person, then by the
        // whole first word (a cancellation's flag is above the person's bits,
        // so it must not lead). Records with equal first words may come out in
        // any order; nothing depends on it.
        pairs(v).sort_unstable_by_key(|r| ((r[0] >> shift) & (LOCALS as u64 - 1), r[0]));
        return;
    }
    let hist = &mut hist[..LOCALS];
    hist.iter_mut().for_each(|c| *c = 0);
    for r in v.chunks_exact(2) {
        unsafe { *hist.get_unchecked_mut((r[0] >> shift) as usize & (LOCALS - 1)) += 1 };
    }
    prefix(hist);
    tmp.clear();
    tmp.reserve(2 * n);
    unsafe {
        tmp.set_len(2 * n);
        let dst = tmp.as_mut_ptr();
        for r in v.chunks_exact(2) {
            let c = hist.get_unchecked_mut((r[0] >> shift) as usize & (LOCALS - 1));
            let at = 2 * *c as usize;
            *dst.add(at) = r[0];
            *dst.add(at + 1) = r[1];
            *c += 1;
        }
    }
    // `hist[l]` is now the end of person `l`'s records, and so the start of the next person's.
    let out = pairs(tmp);
    let mut from = 0usize;
    for &end in hist.iter() {
        let end = end as usize;
        let group = &mut out[from..end];
        if group.len() > 24 {
            group.sort_unstable_by_key(|r| r[0]);
        } else {
            for i in 1..group.len() {
                let x = group[i];
                let mut j = i;
                while j > 0 && group[j - 1][0] > x[0] {
                    group[j] = group[j - 1];
                    j -= 1;
                }
                group[j] = x;
            }
        }
        from = end;
    }
    std::mem::swap(v, tmp);
}

/// Turn bucket counts into start offsets; returns the total.
#[inline(always)]
fn prefix(h: &mut [u32]) -> u32 {
    let mut total = 0u32;
    for c in h.iter_mut() {
        let n = *c;
        *c = total;
        total += n;
    }
    total
}

/// One stable LSD pass: scatter `src` into `dst` by `(w >> shift) & mask`.
#[inline(always)]
unsafe fn scatter(src: &[u64], dst: *mut u64, cursor: &mut [u32], shift: u32, mask: u64) {
    for &w in src {
        let c = cursor.get_unchecked_mut(((w >> shift) & mask) as usize);
        *dst.add(*c as usize) = w;
        *c += 1;
    }
}

pub fn run(ctx: &Ctx, sc: &mut Scratch, p: usize) {
    let t0 = ctx.now();
    // Group the partition's events by person and, within a person, by time
    // slot: a two-pass LSD radix sort on `slot(11)` then `person(12)`. A full
    // sort by timestamp is not needed: two events in one slot are less than
    // 30 minutes apart (a slot is narrower than that), so a session can only
    // break between slots, and whether it does depends only on the latest
    // timestamp of one slot and the earliest of the next (see `person`).
    // Histograms are taken while gathering.
    sc.h.iter_mut().for_each(|c| *c = 0);
    sc.a.clear();
    {
        let (hs, hp) = sc.h.split_at_mut(SLOTS);
        let a = &mut sc.a;
        for t in 0..ctx.nthreads {
            let s = ctx.s(t);
            a.reserve(s.part_words(p));
            s.for_each_run(&ctx.pool, p, |run| unsafe {
                let mut at = a.as_mut_ptr().add(a.len());
                for &w in run {
                    *hs.get_unchecked_mut((w >> SLOT_SHIFT) as usize & (SLOTS - 1)) += 1;
                    *hp.get_unchecked_mut((w >> 31) as usize & (LOCALS - 1)) += 1;
                    *at = w;
                    at = at.add(1);
                }
                a.set_len(at.offset_from(a.as_ptr()) as usize);
            });
        }
        let total = a.len();
        if total == 0 {
            return;
        }
        prefix(hs);
        // Person boundaries come from the second histogram.
        let mut at = 0u32;
        for l in 0..LOCALS {
            sc.start[l] = at;
            at += hp[l];
            hp[l] = sc.start[l];
        }
        sc.start[LOCALS] = at;
        sc.b.clear();
        sc.b.reserve(total);
        unsafe {
            sc.b.set_len(total);
            scatter(a, sc.b.as_mut_ptr(), hs, SLOT_SHIFT, SLOTS as u64 - 1);
            scatter(&sc.b, a.as_mut_ptr(), hp, 31, LOCALS as u64 - 1);
        }
    }
    sc.pu.clear();
    sc.cl.clear();
    for t in 0..ctx.nthreads {
        ctx.ps(t).copy_part(&ctx.pool, p, &mut sc.pu);
        for &(_, w) in ctx.x(t).purchases(p) {
            sc.pu.extend_from_slice(&w);
        }
        ctx.c(t).copy_part(&ctx.pool, p, &mut sc.cl);
    }
    let t0 = ctx.charge_rows(crate::P_S_GATHER, t0, sc.a.len());

    {
        // At most one row per event (see `flush_day`).
        let (r, n) = (&mut sc.rows, sc.a.len());
        r.day.reserve(n);
        r.sessions.reserve(n);
        r.events.reserve(n);
        r.revenue.reserve(n);
        r.id_idx.reserve(n);
    }
    let ts = std::mem::take(&mut sc.a);
    let mut pu = std::mem::take(&mut sc.pu);
    let mut cl = std::mem::take(&mut sc.cl);
    let mut kept_pu = std::mem::take(&mut sc.kept_pu);
    // Purchases by (person, ts), a person's cancellations after their
    // purchases; clicks by (person, ts, campaign).
    sort_pairs(&mut pu, &mut sc.b, &mut sc.h, 31);
    sort_pairs(&mut cl, &mut sc.b, &mut sc.h, CLICK_SHIFT);
    {
        let pu = pairs(&mut pu);
        let cl = pairs(&mut cl);
        let mut t0 = ctx.charge_rows(crate::P_S_SORT, t0, pu.len() + cl.len());
        let (mut pi, mut ci) = (0, 0);
        // The people with events, so that the walk has no hard-to-predict
        // skip, and so that each one's name (a slot of the user table, far
        // from any cache) can be asked for a few people ahead.
        let mut present = std::mem::take(&mut sc.present);
        present.clear();
        present.reserve(LOCALS + PEOPLE_AHEAD);
        unsafe {
            let mut n = 0;
            for local in 0..LOCALS {
                *present.as_mut_ptr().add(n) = local as u16;
                n += (sc.start[local] != sc.start[local + 1]) as usize;
            }
            present.set_len(n);
        }
        let base = (p << LOCAL_BITS) as u32;
        for at in 0..present.len() {
            if let Some(&next) = present.get(at + PEOPLE_AHEAD) {
                ctx.table.prefetch_slot(base | next as u32);
            }
            let local = present[at] as usize;
            let (from, to) = (sc.start[local] as usize, sc.start[local + 1] as usize);
            let events = &ts[from..to];
            // Skip the records of people without events: someone whose events
            // were all cancelled still has their (cancelled) purchases here.
            while pi < pu.len() && ((pu[pi][0] >> 31) as usize & (LOCALS - 1)) < local {
                pi += 1;
            }
            let mut pj = pi;
            while pj < pu.len() && (pu[pj][0] >> 31) as usize & (LOCALS - 1) == local {
                pj += 1;
            }
            let mut purchases = &pu[pi..pj];
            pi = pj;
            let local = local as u64;
            while ci < cl.len() && cl[ci][0] >> CLICK_SHIFT < local {
                ci += 1;
            }
            let mut cj = ci;
            while cj < cl.len() && cl[cj][0] >> CLICK_SHIFT == local {
                cj += 1;
            }
            let clicks = &cl[ci..cj];
            ci = cj;
            // A person's cancelled purchases sort after their purchases.
            if purchases.last().is_some_and(|r| r[0] & CANCEL != 0) {
                apply_purchase_cancels(purchases, &mut kept_pu);
                purchases = &kept_pu;
            }
            person(ctx, sc, base | local as u32, events, purchases, clicks);
        }
        sc.present = present;
        t0 = ctx.charge_rows(crate::P_S_PEOPLE, t0, ts.len());
        let _ = t0;
    }
    sc.a = ts;
    sc.pu = pu;
    sc.cl = cl;
    sc.kept_pu = kept_pu;
}

/// One person's purchases (in time order, cancellations last) with the
/// cancellations applied: each removes one purchase with its timestamp, price
/// and category.
fn apply_purchase_cancels(records: &[[u64; 2]], kept: &mut Vec<[u64; 2]>) {
    let first_cancel = records.partition_point(|r| r[0] & CANCEL == 0);
    kept.clear();
    kept.extend_from_slice(&records[..first_cancel]);
    for c in &records[first_cancel..] {
        if let Some(at) = kept.iter().position(|r| r[0] == c[0] & !CANCEL && r[1] == c[1]) {
            kept.remove(at);
        }
    }
}

/// The earliest and latest timestamp of the run of events that starts at `i`
/// and shares one time slot, and the index after the run. All of `ev` belongs
/// to one person, so equal bits above the slot shift mean an equal slot.
#[inline(always)]
fn slot_group(ev: &[u64], i: usize) -> (u32, u32, usize) {
    debug_assert!(i < ev.len());
    unsafe {
        let first = *ev.get_unchecked(i);
        let slot = first >> SLOT_SHIFT;
        let t = (first & TS_MASK) as u32;
        let (mut min, mut max) = (t, t);
        let mut j = i + 1;
        while j < ev.len() && *ev.get_unchecked(j) >> SLOT_SHIFT == slot {
            let t = (*ev.get_unchecked(j) & TS_MASK) as u32;
            min = min.min(t);
            max = max.max(t);
            j += 1;
        }
        (min, max, j)
    }
}

fn person(ctx: &Ctx, sc: &mut Scratch, slot: u32, ev: &[u64], pu: &[[u64; 2]], cl: &[[u64; 2]]) {
    // The person's name, once: the next entry of the user_id dictionary.
    let key = ctx.table.key(slot);
    {
        let r = &mut sc.rows;
        if key & crate::ids::IRREGULAR == 0 {
            let mut rec = [0u8; 16];
            let n = id_record(key, &mut rec);
            r.ids.reserve(16);
            unsafe {
                // Copy a fixed 16 bytes and keep only the record's length.
                let len = r.ids.len();
                std::ptr::copy_nonoverlapping(rec.as_ptr(), r.ids.as_mut_ptr().add(len), 16);
                r.ids.set_len(len + n);
            }
        } else {
            let mut name = Vec::new();
            ctx.users.bytes(key, &mut name);
            r.ids.extend_from_slice(&(name.len() as u32).to_le_bytes());
            r.ids.extend_from_slice(&name);
        }
        r.people += 1;
    }
    let ndays = ctx.ndays;
    let organic = ctx.campaigns().organic as usize;
    let n = ev.len();
    let nc = cl.len();
    // With many sessions and many clicks, per-session scans of the click window
    // are quadratic; assists are then counted per campaign instead (below).
    let big = (n as u64) * (nc as u64) > 20_000;
    sc.starts.clear();
    sc.sdays.clear();

    let (mut lo, mut hi, mut pk) = (0usize, 0usize, 0usize);
    let mut cur_day = u32::MAX;
    let (mut d_sessions, mut d_events, mut d_revenue) = (0i32, 0i32, 0i64);
    // Sessions: `ev` is ordered by slot, and slots are in time order, so the
    // event before a slot's earliest one is the previous slot's latest one. A
    // session runs on through the slots for as long as that gap is at most 30
    // minutes; it starts at its first slot's earliest timestamp.
    let mut group = slot_group(ev, 0);
    let mut k = 0;
    loop {
        let start = group.0;
        let mut last = group.1;
        let mut m = group.2;
        let mut more = false;
        while m < n {
            group = slot_group(ev, m);
            if group.0 - last > GAP_MS {
                more = true;
                break;
            }
            last = group.1;
            m = group.2;
        }
        let day = ((start as u64 + ctx.day_off) / DAY_MS as u64) as u32;
        if day != cur_day {
            if cur_day != u32::MAX {
                flush_day(sc, cur_day, d_sessions, d_events, d_revenue);
            }
            cur_day = day;
            d_sessions = 0;
            d_events = 0;
            d_revenue = 0;
        }
        // Purchases of this session: every purchase with ts in [start, last].
        let mut revenue = 0u64;
        let mut purchasing = false;
        while pk < pu.len() && (unsafe { pu.get_unchecked(pk) }[0] & TS_MASK) as u32 <= last {
            let w = unsafe { pu.get_unchecked(pk) }[1];
            let price = w >> 32;
            let cat = w as u32;
            revenue += price;
            purchasing = true;
            let cell = unsafe { sc.cat_sum.get_unchecked_mut(cat as usize) };
            if *cell == 0 {
                sc.touched.push(cat);
            }
            *cell += price;
            pk += 1;
        }
        d_sessions += 1;
        d_events += (m - k) as i32;
        d_revenue += revenue as i64;

        // Credit: the latest click in [start - 7 days, start]; ties to the smallest click_id.
        let c0 = |i: usize| unsafe { cl.get_unchecked(i) }[0];
        while hi < nc && click_ts(c0(hi)) <= start {
            hi += 1;
        }
        while lo < hi && click_ts(c0(lo)) as i64 + REACH_MS < start as i64 {
            lo += 1;
        }
        let campaign = if hi > lo {
            let tmax = click_ts(c0(hi - 1));
            let mut best = hi - 1;
            let mut j = hi - 1;
            while j > lo && click_ts(c0(j - 1)) == tmax {
                j -= 1;
                if ctx.clicks.less(cl[j][1], cl[best][1]) {
                    best = j;
                }
            }
            click_campaign(c0(best))
        } else {
            organic
        };
        // (A campaign is an index into the campaign list and a day is inside
        // the window, by construction of the records.)
        let cell = unsafe { sc.acc.credit.get_unchecked_mut(campaign * ndays + day as usize) };
        cell.sessions += 1;
        cell.purchasing += purchasing as u32;
        cell.revenue += revenue;

        if big {
            sc.starts.push(start);
            sc.sdays.push(day);
        } else if hi > lo {
            // Assists: each distinct campaign in the window, once.
            sc.tag = sc.tag.wrapping_add(1);
            if sc.tag == 0 {
                sc.stamp.iter_mut().for_each(|s| *s = 0);
                sc.tag = 1;
            }
            for c in unsafe { cl.get_unchecked(lo..hi) } {
                let camp = click_campaign(c[0]);
                unsafe {
                    let stamp = sc.stamp.get_unchecked_mut(camp);
                    if *stamp != sc.tag {
                        *stamp = sc.tag;
                        *sc.acc.assisted.get_unchecked_mut(camp * ndays + day as usize) += 1;
                    }
                }
            }
        }
        if !more {
            break;
        }
        k = m;
    }
    flush_day(sc, cur_day, d_sessions, d_events, d_revenue);
    if big {
        assists_by_campaign(sc, cl, ndays);
    }
}

#[inline]
fn flush_day(sc: &mut Scratch, day: u32, sessions: i32, events: i32, revenue: i64) {
    // Top category: highest summed price, ties to the smallest name (= smallest index).
    let mut best = u32::MAX;
    let mut best_sum = 0u64;
    for &cat in &sc.touched {
        let sum = sc.cat_sum[cat as usize];
        if sum > best_sum || (sum == best_sum && cat < best) {
            best = cat;
            best_sum = sum;
        }
        sc.cat_sum[cat as usize] = 0;
    }
    sc.touched.clear();
    let r = &mut sc.rows;
    // `run` reserved room for one row per event of the partition in every
    // per-row column.
    unsafe {
        let n = r.n;
        debug_assert!(n < r.day.capacity() && n < r.revenue.capacity() && n < r.id_idx.capacity());
        *r.id_idx.as_mut_ptr().add(n) = r.people - 1;
        *r.day.as_mut_ptr().add(n) = day as i32;
        *r.sessions.as_mut_ptr().add(n) = sessions;
        *r.events.as_mut_ptr().add(n) = events;
        *r.revenue.as_mut_ptr().add(n) = revenue;
        r.id_idx.set_len(n + 1);
        r.day.set_len(n + 1);
        r.sessions.set_len(n + 1);
        r.events.set_len(n + 1);
        r.revenue.set_len(n + 1);
        if n & 7 == 0 {
            r.valid.push(0);
        }
        if best != u32::MAX {
            *r.valid.last_mut().unwrap_unchecked() |= 1 << (n & 7);
            r.cats.push(best);
        }
        r.n = n + 1;
    }
}

/// Assists for a person with many sessions and clicks. A campaign assists a
/// session iff the session start lies in `[t, t + 7 days]` for one of the
/// campaign's clicks at `t`; merge those intervals per campaign and count the
/// session starts they cover, day by day.
fn assists_by_campaign(sc: &mut Scratch, cl: &[[u64; 2]], ndays: usize) {
    let starts = &sc.starts[..];
    let sdays = &sc.sdays[..];
    // dfirst[d] = first session index whose day is >= d.
    sc.dfirst.clear();
    sc.dfirst.resize(ndays + 1, starts.len() as u32);
    for (i, &d) in sdays.iter().enumerate().rev() {
        sc.dfirst[d as usize] = i as u32;
    }
    for d in (0..ndays).rev() {
        if sc.dfirst[d] > sc.dfirst[d + 1] {
            sc.dfirst[d] = sc.dfirst[d + 1];
        }
    }
    sc.tmp.clear();
    sc.tmp.extend(cl.iter().map(|c| (click_campaign(c[0]) as u64) << 32 | click_ts(c[0]) as u64));
    sc.tmp.sort_unstable();
    let tmp = &sc.tmp[..];
    let reach = REACH_MS as u64;
    let mut i = 0;
    while i < tmp.len() {
        let camp = (tmp[i] >> 32) as usize;
        let a = tmp[i] & 0xFFFF_FFFF;
        let mut b = a + reach;
        i += 1;
        while i < tmp.len() && (tmp[i] >> 32) as usize == camp && (tmp[i] & 0xFFFF_FFFF) <= b {
            b = (tmp[i] & 0xFFFF_FFFF) + reach;
            i += 1;
        }
        let x = starts.partition_point(|&s| (s as u64) < a);
        let y = starts.partition_point(|&s| (s as u64) <= b);
        if y > x {
            for d in sdays[x] as usize..=sdays[y - 1] as usize {
                let from = (sc.dfirst[d] as usize).max(x);
                let to = (sc.dfirst[d + 1] as usize).min(y);
                if to > from {
                    sc.acc.assisted[camp * ndays + d] += (to - from) as u32;
                }
            }
        }
    }
}
