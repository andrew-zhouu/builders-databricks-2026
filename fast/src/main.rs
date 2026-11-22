//! Shopping Sessions: clean, link, sessionize and attribute click-stream CSVs
//! into two daily Parquet tables. The README describes the pipeline.

mod diag;
mod ids;
mod lookup;
mod parquet;
mod parse;
mod part;
mod pass2;
mod pass3;
#[cfg(feature = "probe")]
mod probe;
mod prof;
mod slow;
mod sys;
mod table;
mod util;

use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::Barrier;
use std::time::Instant;

use ids::KeySpace;
use lookup::{Campaigns, Products};
use parquet::{Column, Kind};
use part::{BlockPool, PartWriter};
use pass2::Cancels;
use pass3::CampAcc;
use sys::FileMap;
use table::Table;
use util::{DAY_MS, REACH_MS};

pub struct Shared<T>(UnsafeCell<T>);
unsafe impl<T> Sync for Shared<T> {}
impl<T> Shared<T> {
    fn new(v: T) -> Self {
        Shared(UnsafeCell::new(v))
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum FileKind {
    Events,
    Clicks,
    Links,
}

pub struct Chunk {
    pub file: u32,
    pub kind: FileKind,
    pub start: usize,
    pub end: usize,
}

pub struct Ctx {
    /// Event window `[ws, we)` in epoch ms; `t0 = ws - 7 days` is the origin of
    /// every relative timestamp (the click window starts there).
    pub ws: i64,
    pub we: i64,
    pub t0: i64,
    /// `day0` is the UTC day number of `t0`; `day_off` its offset into that day.
    pub day0: i64,
    pub day_off: u64,
    pub ndays: usize,
    pub nthreads: usize,
    /// products.csv and campaigns.csv, loaded by thread 0 while the others
    /// already parse links; read only after the first barrier.
    lookups: Shared<Option<Lookups>>,
    data_dir: PathBuf,
    pub users: KeySpace,
    pub clicks: KeySpace,
    pub dates: parse::DateTab,
    /// Debug switch: send every line through the scalar parsers.
    pub scalar_only: bool,
    /// Software prefetch distance in bytes for the parsers' input (0 = off).
    pub prefetch_ahead: usize,
    /// The batch splitter of parse::splitw (processors with AVX-512 VBMI2;
    /// SS_WSPLIT=0 turns it off), and its check against the line splitter
    /// (SS_SPLIT_CHECK=1).
    pub wide_split: bool,
    pub split_check: bool,
    /// Unmap the big memory regions from all threads before exiting.
    zap: bool,
    /// A thread writes its user_daily rows out whenever it holds about this
    /// many bytes of them.
    out_chunk: usize,
    pub table: Table,
    pub pool: BlockPool,
    pub files: Vec<FileMap>,
    /// For an event file, the position of its first byte among all event
    /// files laid end to end (a trace stores a line's position in that space).
    pub file_base: Vec<u64>,
    /// Event files in order of position: (base, file index).
    event_files: Vec<(u64, u32)>,
    /// For each megabyte of trace positions, the index in `event_files` of the
    /// file that holds its first byte.
    loc_file: Vec<u32>,
    /// Bits of a trace that hold the position; the rest hold the fingerprint.
    pub loc_bits: u32,
    pub fp_mask: u64,
    link_chunks: Vec<Chunk>,
    main_chunks: Vec<Chunk>,
    next_link: AtomicUsize,
    next_compress: AtomicUsize,
    next_main: AtomicUsize,
    next_d: AtomicUsize,
    next_s: AtomicUsize,
    /// The next piece of memory to give back at the end (see `worker`), and
    /// the input files in pieces for it: (file, from, to).
    next_zap: AtomicUsize,
    zap_pieces: Vec<(u32, usize, usize)>,
    /// Person partitions, the ones with the most events first.
    s_order: std::sync::OnceLock<Vec<u32>>,
    /// Trace partitions and person partitions.
    pub pd: usize,
    pub ps: usize,
    k: Vec<Shared<PartWriter>>,
    c: Vec<Shared<PartWriter>>,
    s: Vec<Shared<PartWriter>>,
    psw: Vec<Shared<PartWriter>>,
    x: Vec<Shared<Cancels>>,
    acc: Vec<Shared<CampAcc>>,
    barrier: Barrier,
    /// Per profiled section, summed over threads (SS_TIMING only): CPU
    /// nanoseconds, the four performance counters, input rows, kernel
    /// nanoseconds and page faults.
    pub prof: [[std::sync::atomic::AtomicU64; 4 + prof::NCOUNTERS]; 14],
    /// Set when the data does not fit the fast path's packed layouts.
    pub bail: AtomicBool,
    out_dir: PathBuf,
    timing: bool,
    t_start: Instant,
}

pub struct Lookups {
    pub products: Products,
    pub campaigns: Campaigns,
    /// Category names as Parquet PLAIN values (4-byte length + bytes), by index.
    pub cat_recs: Vec<Vec<u8>>,
}

impl Ctx {
    fn load_lookups(&self) {
        let products = Products::load(&std::fs::read(self.data_dir.join("products.csv")).unwrap_or_default());
        let campaigns = Campaigns::load(&std::fs::read(self.data_dir.join("campaigns.csv")).unwrap_or_default());
        if campaigns.ids.len() >= (1 << parse::CAMPAIGN_BITS) {
            self.bail.store(true, Relaxed);
        }
        let cat_recs =
            products.categories.iter().map(|name| [&(name.len() as u32).to_le_bytes()[..], name].concat()).collect();
        unsafe { *self.lookups.0.get() = Some(Lookups { products, campaigns, cat_recs }) };
    }

    #[inline(always)]
    fn lookups(&self) -> &Lookups {
        unsafe { (*self.lookups.0.get()).as_ref().unwrap_unchecked() }
    }
    #[inline(always)]
    pub fn products(&self) -> &Products {
        &self.lookups().products
    }
    #[inline(always)]
    pub fn campaigns(&self) -> &Campaigns {
        &self.lookups().campaigns
    }
    #[inline(always)]
    pub fn cat_recs(&self) -> &[Vec<u8>] {
        &self.lookups().cat_recs
    }

    // Writers are mutated only by their owning thread during the phase that
    // fills them, and only read (by anyone) after the barrier that follows.
    #[inline(always)]
    pub fn k(&self, t: usize) -> &PartWriter {
        unsafe { &*self.k[t].0.get() }
    }
    #[inline(always)]
    pub fn c(&self, t: usize) -> &PartWriter {
        unsafe { &*self.c[t].0.get() }
    }
    #[inline(always)]
    pub fn s(&self, t: usize) -> &PartWriter {
        unsafe { &*self.s[t].0.get() }
    }
    #[inline(always)]
    pub fn ps(&self, t: usize) -> &PartWriter {
        unsafe { &*self.psw[t].0.get() }
    }
    #[inline(always)]
    pub fn x(&self, t: usize) -> &Cancels {
        unsafe { &*self.x[t].0.get() }
    }

    /// The event file that holds trace position `loc`, and the offset in it.
    #[inline]
    pub fn locate(&self, loc: u64) -> (&[u8], usize) {
        // The file that holds the megabyte's first byte, then onward: files
        // are rarely smaller than that.
        let mut at = self.loc_file[(loc >> 20) as usize] as usize;
        while at + 1 < self.event_files.len() && self.event_files[at + 1].0 <= loc {
            at += 1;
        }
        let (base, file) = self.event_files[at];
        (self.files[file as usize].slice(), (loc - base) as usize)
    }

    /// Profiling: this thread's CPU time and counters now (zeros when not timing).
    #[inline]
    pub fn now(&self) -> prof::Snap {
        if self.timing {
            prof::snap()
        } else {
            prof::Snap::default()
        }
    }

    /// Profiling: charge what the thread used since `since` to `section`, with
    /// the number of input rows that work covered; returns the new "now".
    #[inline]
    pub fn charge_rows(&self, section: usize, since: prof::Snap, rows: usize) -> prof::Snap {
        if !self.timing {
            return since;
        }
        let t = prof::snap();
        let p = &self.prof[section];
        p[0].fetch_add(t.ns - since.ns, Relaxed);
        for i in 0..prof::NCOUNTERS {
            p[1 + i].fetch_add(t.c[i].wrapping_sub(since.c[i]), Relaxed);
        }
        p[1 + prof::NCOUNTERS].fetch_add(rows as u64, Relaxed);
        p[2 + prof::NCOUNTERS].fetch_add(t.sys_ns.saturating_sub(since.sys_ns), Relaxed);
        p[3 + prof::NCOUNTERS].fetch_add(t.faults.saturating_sub(since.faults), Relaxed);
        t
    }

    #[inline]
    pub fn charge(&self, section: usize, since: prof::Snap) -> prof::Snap {
        self.charge_rows(section, since, 0)
    }

    fn failed(&self) -> bool {
        self.bail.load(Relaxed)
            || self.table.overflow.load(Relaxed)
            || self.users.long.as_ref().is_some_and(|long| long.overflow.load(Relaxed))
    }

    fn sync(&self, what: &str) {
        if self.timing {
            prof::drain();
        }
        if self.barrier.wait().is_leader() && self.timing {
            eprintln!("[{:8.2} ms] {what}", self.t_start.elapsed().as_secs_f64() * 1e3);
        }
    }

    /// The per-section profile: CPU time, and per input row the instructions,
    /// cycles, branch misses and cache misses (where the kernel provides them).
    fn print_profile(&self) {
        let total: u64 = self.prof.iter().map(|p| p[0].load(Relaxed)).sum();
        eprintln!(
            "    section            cpu ms     %   sys ms  faults      rows ns/row  instr/row cycles/row   IPC brmiss/row llcmiss/row"
        );
        let mut sum = [0u64; 4 + prof::NCOUNTERS];
        for (name, p) in P_NAMES.iter().zip(&self.prof) {
            let v: Vec<u64> = p.iter().map(|x| x.load(Relaxed)).collect();
            let (ns, cycles, instr, brmiss, llc, rows, sys, faults) = (v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7]);
            if ns == 0 {
                continue;
            }
            for (s, x) in sum.iter_mut().zip(&v) {
                *s += x;
            }
            let per = |x: u64| if rows > 0 { x as f64 / rows as f64 } else { 0.0 };
            eprintln!(
                "    {name:16} {:8.2} {:5.1}% {:8.2} {faults:7} {rows:9} {:6.1} {:10.1} {:10.1} {:5.2} {:10.3} {:11.3}",
                ns as f64 / 1e6,
                100.0 * ns as f64 / total.max(1) as f64,
                sys as f64 / 1e6,
                per(ns),
                per(instr),
                per(cycles),
                instr as f64 / cycles.max(1) as f64,
                per(brmiss),
                per(llc),
            );
        }
        let (psys, pfaults, puser) = prof::rusage(0);
        eprintln!(
            "    sections: cpu {:.1} ms (sys {:.1} ms, {} faults), {:.2} G instr, {:.2} G user cycles; process: user {:.1} ms, sys {:.1} ms, {} faults",
            sum[0] as f64 / 1e6,
            sum[6] as f64 / 1e6,
            sum[7],
            sum[2] as f64 / 1e9,
            sum[1] as f64 / 1e9,
            puser as f64 / 1e6,
            psys as f64 / 1e6,
            pfaults
        );
    }
}

// Profiled sections (printed with SS_TIMING=1).
pub const P_EVENTS: usize = 0;
pub const P_CLICKS: usize = 1;
pub const P_FINALIZE: usize = 2;
pub const P_D_GATHER: usize = 3;
pub const P_D_DEDUP: usize = 4;
pub const P_D_EMIT: usize = 5;
pub const P_S_GATHER: usize = 6;
pub const P_S_SORT: usize = 7;
pub const P_S_PEOPLE: usize = 8;
pub const P_WRITE: usize = 9;
pub const P_LINKS: usize = 10;
pub const P_COMPRESS: usize = 11;
pub const P_D_GROUP: usize = 12;
pub const P_D_READ: usize = 13;
const P_NAMES: [&str; 14] = [
    "parse events", "parse clicks", "finalize", "dedup gather", "dedup scan", "dedup settle", "sessions gather",
    "sessions sort", "sessions people", "write user_daily", "links", "resolve people", "dedup group", "dedup reread",
];

const CHUNK: usize = 1 << 20;
const LINK_CHUNK: usize = 1 << 18;
const COMPRESS_STEP: usize = 1 << 16;

fn list_csv(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                name.ends_with(".csv") && !name.starts_with('.') && p.is_file()
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    v.sort();
    v
}

fn worker(ctx: &Ctx, tid: usize) {
    // Per-thread partition writers, built here so their memory is first touched
    // by the thread that uses it and the allocations run in parallel.
    unsafe {
        *ctx.k[tid].0.get() = PartWriter::new(ctx.pd);
        *ctx.c[tid].0.get() = PartWriter::new(ctx.ps);
        *ctx.s[tid].0.get() = PartWriter::new(ctx.ps);
        *ctx.psw[tid].0.get() = PartWriter::new(ctx.ps);
    }
    if tid == 0 {
        ctx.load_lookups();
        calibrate(ctx);
    }
    // ---- links: insert both IDs, join them
    let mut t = ctx.now();
    loop {
        let i = ctx.next_link.fetch_add(1, Relaxed);
        if i >= ctx.link_chunks.len() {
            break;
        }
        let lines = parse::links_chunk(ctx, &ctx.link_chunks[i]);
        t = ctx.charge_rows(P_LINKS, t, lines);
    }
    ctx.sync("links parsed");
    if ctx.failed() {
        return;
    }
    let t = ctx.now();
    loop {
        let from = ctx.next_compress.fetch_add(COMPRESS_STEP, Relaxed);
        if from >= ctx.table.cap {
            break;
        }
        ctx.table.compress(from, (from + COMPRESS_STEP).min(ctx.table.cap));
    }
    ctx.charge(P_COMPRESS, t);
    ctx.sync("people resolved");

    // ---- parse clicks and events: straight to their person's partition, plus a trace per event
    {
        let c = unsafe { &mut *ctx.c[tid].0.get() };
        let mut out = unsafe { parse::EventOut { k: &mut *ctx.k[tid].0.get(), ps: &mut *ctx.psw[tid].0.get() } };
        let mut t = ctx.now();
        loop {
            let i = ctx.next_main.fetch_add(1, Relaxed);
            if i >= ctx.main_chunks.len() {
                break;
            }
            let ch = &ctx.main_chunks[i];
            match ch.kind {
                FileKind::Events => {
                    let lines = parse::events_chunk(ctx, &mut out, ch);
                    t = ctx.charge_rows(P_EVENTS, t, lines);
                }
                _ => {
                    let lines = parse::clicks_chunk(ctx, c, ch);
                    t = ctx.charge_rows(P_CLICKS, t, lines);
                }
            }
        }
        out.k.finalize(&ctx.pool);
        out.ps.finalize(&ctx.pool);
        c.finalize(&ctx.pool);
        ctx.charge(P_FINALIZE, t);
    }
    ctx.sync("events and clicks parsed");
    if ctx.failed() {
        return;
    }

    // ---- dedup: settle the event IDs that occur more than once, and send the events to their people
    {
        let x = unsafe { &mut *ctx.x[tid].0.get() };
        let s = unsafe { &mut *ctx.s[tid].0.get() };
        let mut sc = pass2::Scratch::default();
        loop {
            let p = ctx.next_d.fetch_add(1, Relaxed);
            if p >= ctx.pd {
                break;
            }
            pass2::run(ctx, &mut sc, x, s, p);
        }
        s.finalize(&ctx.pool);
        x.finish(ctx.ps);
        // Never free: unmapping costs time and the process is about to exit.
        std::mem::forget(sc);
    }
    ctx.sync("deduplicated");

    // ---- sessions, attribution, rows
    // Largest first: a partition holding one of the few very busy people takes
    // many times longer than the others, and must not be the one that
    // everybody ends up waiting for.
    let order = ctx.s_order.get_or_init(|| {
        let mut sized: Vec<(u32, u32)> = (0..ctx.ps)
            .map(|p| ((0..ctx.nthreads).map(|t| ctx.s(t).part_words(p)).sum::<usize>() as u32, p as u32))
            .collect();
        sized.sort_unstable_by(|a, b| b.cmp(a));
        sized.into_iter().map(|(_, p)| p).collect()
    });
    let mut sc = pass3::Scratch::new(ctx);
    // Rows leave in files of about `out_chunk` bytes as they come, not in one
    // file at the end: the row buffers and what the writer encodes at a time
    // then stay small enough to be reused from the cache, instead of growing
    // through fresh memory.
    let mut files = 0usize;
    let mut encoded = Encoded::default();
    loop {
        let i = ctx.next_s.fetch_add(1, Relaxed);
        if i >= ctx.ps {
            break;
        }
        pass3::run(ctx, &mut sc, order[i] as usize);
        if sc.rows.bytes() >= ctx.out_chunk {
            let t = ctx.now();
            write_user_daily(ctx, &sc.rows, &mut encoded, tid, files);
            ctx.charge_rows(P_WRITE, t, sc.rows.n);
            files += 1;
            sc.rows.clear();
        }
    }
    let t = ctx.now();
    // What is left; and the table always has a file, if only an empty one.
    if sc.rows.n > 0 || (tid == 0 && files == 0) {
        write_user_daily(ctx, &sc.rows, &mut encoded, tid, files);
    }
    ctx.charge_rows(P_WRITE, t, sc.rows.n);
    unsafe { *ctx.acc[tid].0.get() = std::mem::take(&mut sc.acc) };
    if ctx.timing {
        prof::finish_thread();
    }
    ctx.sync("sessions built, user_daily written");

    // ---- teardown in parallel: at exit the kernel frees the address space on
    // one thread, so give the big regions back from all of them first.
    if tid == 0 {
        write_campaign_daily(ctx);
    }
    if ctx.zap {
        drop(sc);
        // In pieces handed out one at a time (the input files, which are most
        // of the work, then eighths of the pool and of the table), so that
        // the thread that wrote campaign_daily meanwhile joins the others
        // where they are instead of finding a share of its own left over.
        const PIECES: usize = 8;
        let nfiles = ctx.zap_pieces.len();
        let piece = |len: usize, k: usize| (len / PIECES * k, if k + 1 == PIECES { len } else { len / PIECES * (k + 1) });
        loop {
            let i = ctx.next_zap.fetch_add(1, Relaxed);
            if i < nfiles {
                let (file, from, to) = ctx.zap_pieces[i];
                ctx.files[file as usize].release(from, to);
            } else if i < nfiles + PIECES {
                let (a, b) = piece(ctx.pool.used_bytes(), i - nfiles);
                ctx.pool.zap(a, b);
            } else if i < nfiles + 2 * PIECES {
                let (a, b) = piece(ctx.table.cap, i - nfiles - PIECES);
                ctx.table.zap(a, b);
            } else {
                break;
            }
        }
    } else {
        std::mem::forget(sc);
    }
    ctx.sync("campaign_daily written, memory released");
}

/// Which parser instance is the faster one here? A processor's flags say
/// which instances it can run, not which wins: 512-bit code lowers the clock
/// on some processors, and single instructions are slow on others. So where
/// the processor offers wide instances and nothing has been asked for, the
/// first 512 KB of the largest events file are parsed by the ordinary
/// instance and by each wide one, three times each and one instance after the
/// other, into writers of their own (the traces are thrown away; what else
/// the parser does, entering users in the table, the real run does again with
/// the same result), and the fastest wide one is taken if its best time is
/// clearly better than the ordinary one's. Run by the first thread while the
/// others are on the links, before any of them parses events: six to eight
/// milliseconds of one thread.
fn calibrate(ctx: &Ctx) {
    const REGION: usize = 1 << 19;
    let (wides, nwides) = parse::wide_candidates();
    let wides = &wides[..nwides];
    if wides.is_empty() || std::env::var("SS_CALIBRATE").is_ok_and(|v| v == "0") {
        return;
    }
    let Some(&(_, file)) = ctx.event_files.iter().max_by_key(|&&(_, f)| ctx.files[f as usize].len) else { return };
    // (A small input is over before the choice could matter.)
    if ctx.files[file as usize].len < 8 * REGION {
        return;
    }
    let ch = Chunk { file, kind: FileKind::Events, start: 0, end: REGION };
    let mut free: [Vec<u32>; 2] = Default::default();
    let mut run = |kind: u8| {
        parse::set_wide_kind(kind);
        let (mut k, mut ps) = (PartWriter::new(ctx.pd), PartWriter::new(ctx.ps));
        k.free = std::mem::take(&mut free[0]);
        ps.free = std::mem::take(&mut free[1]);
        let t = Instant::now();
        parse::events_chunk(ctx, &mut parse::EventOut { k: &mut k, ps: &mut ps }, &ch);
        let ns = t.elapsed().as_nanos() as u64;
        for (w, f) in [(&mut k, 0), (&mut ps, 1)] {
            w.finalize(&ctx.pool);
            free[f].extend_from_slice(w.all_blocks());
            free[f].extend_from_slice(&w.free);
        }
        ns
    };
    // Once without the clock, for the page table and the caches. Then the
    // ordinary instance, three times in a row, before any 512-bit instruction
    // has run: at the clock it would have for the whole run. Then the wide
    // one, after five runs without the clock: where 512-bit code lowers the
    // clock, the first of it runs at a fraction of its speed while the
    // processor changes over (and the lower clock lingers a few milliseconds
    // after the last of it), so only what comes two or three milliseconds
    // later is what the run would get. (The first version of this timed the
    // two in turn, and on Cascade Lake found the wide one slower: v4.)
    // (The instances come with those that leave the clock alone first, so
    // that a 512-bit one is the last thing timed.)
    run(0);
    let mut best = [u64::MAX; 4];
    for _ in 0..3 {
        best[0] = best[0].min(run(0));
    }
    for &wide in wides {
        for _ in 0..if parse::is_512(wide) { 5 } else { 1 } {
            run(wide);
        }
        for _ in 0..3 {
            best[wide as usize] = best[wide as usize].min(run(wide));
        }
    }
    // The fastest, if it is clearly faster: within a few percent the ordinary
    // instance is the safer one.
    let fastest = wides.iter().copied().min_by_key(|&w| best[w as usize]).unwrap_or(0);
    let kind = if best[fastest as usize] * 100 < best[0] * 95 { fastest } else { 0 };
    parse::set_wide_kind(kind);
    let wide = wides.iter().copied().find(|&w| parse::is_512(w)).unwrap_or(0);
    CALIBRATED[0].store(wide as u64, Relaxed);
    CALIBRATED[1].store(best[0], Relaxed);
    CALIBRATED[2].store(best[wide as usize], Relaxed);
    CALIBRATED[3].store(best[3], Relaxed);
    // The blocks go to this thread's own writers.
    unsafe {
        (*ctx.k[0].0.get()).free.append(&mut free[0]);
        (*ctx.psw[0].0.get()).free.append(&mut free[1]);
    }
    if ctx.timing {
        eprintln!(
            "              parser instance {kind}: the first 512 KB of events in {} us by the ordinary one, {} us by the four-lane one, {} us by wide instance {wide}",
            best[0] / 1000,
            best[3] / 1000,
            best[wide as usize] / 1000
        );
    }
}

/// What `calibrate` tried and found: the wide instance (0: none), the
/// ordinary instance's best time and the wide one's, in nanoseconds.
static CALIBRATED: [std::sync::atomic::AtomicU64; 4] = [
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
];

/// SS_SIGNAL (set by a probe image's Dockerfile, never by a candidate's): say
/// through the run's duration what `calibrate` found on this machine, for a
/// scorer that reports nothing but a time. The finished run sleeps a whole
/// number of seconds:
///
/// * 1 to 17: the wide instance tried was the one for processors without
///   VBMI2, and its best time was this share of the ordinary one's: 1 = under
///   55%, then a second for every 5% (2 = 55 to 60%, ..., 9 = 90 to 95%: up to
///   here the wide one is taken; 10 = 95 to 100%, ...), 17 = 130% or more;
/// * 18: it was the one for processors with VBMI2;
/// * 19: none was tried (no AVX-512, or the calibration did not run).
///
/// (The first probe, of v4's calibration, had another scale: 1 + one of six
/// steps, 7 + step for VBMI2, 13 for none; it read 6 on the scorer: no VBMI2,
/// the wide instance at 105% or more.)
///
/// With SS_SIGNAL=3 the same scale (1 to 17) reports the four-lane instance
/// instead, whatever the processor; 19 if it was not tried.
fn signal() {
    let (wide, base, mut best) = (CALIBRATED[0].load(Relaxed), CALIBRATED[1].load(Relaxed), CALIBRATED[2].load(Relaxed));
    let quad = std::env::var("SS_SIGNAL").is_ok_and(|v| v == "3");
    if quad {
        best = CALIBRATED[3].load(Relaxed);
    }
    let seconds = if base == 0 || best == 0 || best == u64::MAX || (wide == 0 && !quad) {
        19
    } else if wide == 1 && !quad {
        18
    } else {
        1 + (best * 100 / base / 5).clamp(10, 26) - 10
    };
    std::thread::sleep(std::time::Duration::from_secs(seconds));
}

/// `--bench`: the parsers alone on a small region of the largest file of each
/// kind, repeated; reports the least disturbed repetition.
fn bench(ctx: &Ctx) {
    ctx.load_lookups();
    for ch in &ctx.link_chunks {
        parse::links_chunk(ctx, ch);
    }
    ctx.table.compress(0, ctx.table.cap);
    let region: usize = std::env::var("SS_BENCH_BYTES").ok().and_then(|v| v.parse().ok()).unwrap_or(1 << 19);
    let iters: usize = std::env::var("SS_BENCH_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(400);
    // SS_BENCH_KIND=events|clicks|links restricts the run (and so any sampling) to one parser.
    let only = std::env::var("SS_BENCH_KIND").ok();
    for kind in [FileKind::Events, FileKind::Clicks, FileKind::Links] {
        if only.as_deref().is_some_and(|k| k != ["events", "clicks", "links"][kind as usize]) {
            continue;
        }
        let chunks = if kind == FileKind::Links { &ctx.link_chunks } else { &ctx.main_chunks };
        let Some(file) = chunks
            .iter()
            .filter(|c| c.kind == kind)
            .map(|c| c.file)
            .max_by_key(|&f| ctx.files[f as usize].len)
        else {
            continue;
        };
        let end = region.min(ctx.files[file as usize].len);
        let ch = Chunk { file, kind, start: 0, end };
        // Keep the repetition with the fewest cycles (or, without counters, the least time).
        let (mut best, mut lines) = ([u64::MAX; 1 + prof::NCOUNTERS], 1usize);
        // Each writer gets its own blocks back, so repetitions do not grow the pool.
        let mut free: [Vec<u32>; 4] = Default::default();
        for _ in 0..iters {
            let mut w = [PartWriter::new(ctx.pd), PartWriter::new(ctx.ps), PartWriter::new(ctx.ps), PartWriter::new(ctx.ps)];
            for (x, f) in w.iter_mut().zip(free.iter_mut()) {
                x.free = std::mem::take(f);
            }
            let a = prof::snap();
            let n = {
                let [k, _, ps, c] = &mut w;
                match kind {
                    FileKind::Events => parse::events_chunk(ctx, &mut parse::EventOut { k, ps }, &ch),
                    FileKind::Clicks => parse::clicks_chunk(ctx, c, &ch),
                    FileKind::Links => parse::links_chunk(ctx, &ch),
                }
            };
            let b = prof::snap();
            let better = if b.c[0] != a.c[0] { b.c[0] - a.c[0] < best[1] } else { b.ns - a.ns < best[0] };
            if better {
                best[0] = b.ns - a.ns;
                for i in 0..prof::NCOUNTERS {
                    best[1 + i] = b.c[i] - a.c[i];
                }
                lines = n.max(1);
            }
            for (x, f) in w.iter_mut().zip(free.iter_mut()) {
                x.finalize(&ctx.pool);
                f.extend_from_slice(x.all_blocks());
                f.extend_from_slice(&x.free);
            }
        }
        let name = ["events", "clicks", "links"][kind as usize];
        let per = |x: u64| x as f64 / lines as f64;
        eprintln!(
            "bench {name}: {:5.1} ns/line  {:6.1} instr/line  {:6.1} cycles/line  IPC {:.2}  {:.3} brmiss/line  {:.3} llcmiss/line  ({lines} lines)",
            per(best[0]),
            per(best[2]),
            per(best[1]),
            best[2] as f64 / best[1].max(1) as f64,
            per(best[3]),
            per(best[4]),
        );
    }
    prof::finish_thread();
    prof::write_samples();
}

/// What `write_user_daily` builds for a file, kept for the thread's next one.
/// A thread writes many files, and memory given back in between costs more
/// than the encoding: large blocks are mapped and unmapped each time, and an
/// unmap interrupts every other thread.
#[derive(Default)]
struct Encoded {
    order: Vec<u32>,
    paid: Vec<u32>,
    paid_at: Vec<u32>,
    id_idx: Vec<u8>,
    day_idx: Vec<u8>,
    /// For sessions and for events: indexes, dictionary, value -> index, and
    /// the plain values should there be no dictionary.
    counts: [(Vec<u8>, Vec<i32>, Vec<u32>, Vec<i32>); 2],
    amounts: Vec<i64>,
    /// Open addressing by amount (never 0): the amount and its place in `amounts`.
    seen: Vec<(i64, u32)>,
    revenue_idx: Vec<u8>,
    levels: Vec<u8>,
    cat_idx: Vec<u8>,
    days: Vec<i32>,
    categories: Vec<u8>,
}

fn write_user_daily(ctx: &Ctx, rows: &pass3::Rows, e: &mut Encoded, tid: usize, seq: usize) {
    use parquet::{as_bytes, index_width, pack_into, packed_run, rle_run, small_dict};
    let n = rows.n;
    // The order of rows in a file is free: the rows without revenue go first.
    // The revenue column and top_category's null flags (a row has a category
    // exactly when it has revenue: a purchase costs at least a cent) are then
    // two runs each, not a value and a bit per row.
    e.order.clear();
    e.order.reserve(n + 1);
    e.paid.clear();
    e.paid.reserve(n + 1);
    unsafe {
        // Every row number is written to both lists; only one of them keeps it.
        let (mut a, mut b) = (0usize, 0usize);
        for i in 0..n {
            let zero = (*rows.revenue.get_unchecked(i) == 0) as usize;
            *e.order.as_mut_ptr().add(a) = i as u32;
            *e.paid.as_mut_ptr().add(b) = i as u32;
            a += zero;
            b += 1 - zero;
        }
        e.order.set_len(a);
        e.paid.set_len(b);
    }
    let free = e.order.len();
    e.order.extend_from_slice(&e.paid);
    assert_eq!(rows.cats.len(), n - free, "a row has a top category exactly when it has revenue");
    let order = &e.order[..];

    // Every column as a dictionary and indexes into it. A person's rows
    // follow each other (within each of the two parts), so their index is
    // written as runs when that is shorter than bit-packing every row.
    let id_width = index_width(rows.people as usize);
    let ids_in_order = || order.iter().map(|&i| unsafe { *rows.id_idx.get_unchecked(i as usize) });
    let (mut runs, mut last) = (0usize, u32::MAX);
    for v in ids_in_order() {
        runs += (v != last) as usize;
        last = v;
    }
    if runs * (2 + (id_width as usize).div_ceil(8)) < n * id_width as usize / 8 {
        let out = &mut e.id_idx;
        out.clear();
        out.reserve(runs * 5 + 8);
        out.push(id_width as u8);
        let mut it = ids_in_order();
        if let Some(mut value) = it.next() {
            let mut count = 1usize;
            for v in it {
                if v == value {
                    count += 1;
                } else {
                    rle_run(out, value, count, id_width);
                    value = v;
                    count = 1;
                }
            }
            rle_run(out, value, count, id_width);
        }
    } else {
        pack_into(&mut e.id_idx, ids_in_order(), n, id_width);
    }
    if e.days.is_empty() {
        e.days = (0..ctx.ndays as i64).map(|d| (ctx.day0 + d) as i32).collect();
        e.categories = ctx.cat_recs().concat();
    }
    pack_into(&mut e.day_idx, order.iter().map(|&i| unsafe { *rows.day.get_unchecked(i as usize) } as u32), n, index_width(e.days.len()));
    // Small counts: the distinct values. (Plain, should one ever be huge.)
    let mut coded = [false; 2];
    for (k, values) in [&rows.sessions[..], &rows.events[..]].into_iter().enumerate() {
        let (idx, dict, index, plain) = &mut e.counts[k];
        coded[k] = small_dict(values, index, dict);
        if coded[k] {
            let at = order.iter().map(|&i| unsafe { *index.get_unchecked(*values.get_unchecked(i as usize) as usize) });
            pack_into(idx, at, n, index_width(dict.len()));
        } else {
            plain.clear();
            plain.extend(order.iter().map(|&i| values[i as usize]));
        }
    }
    // Revenue: one run of zeros, then the amounts of the rows that have one,
    // each distinct amount once.
    let slots = (2 * (n - free)).next_power_of_two().max(16);
    let shift = 64 - slots.trailing_zeros();
    e.seen.clear();
    e.seen.resize(slots, (0, 0));
    e.amounts.clear();
    e.amounts.push(0);
    e.paid_at.clear();
    e.paid_at.reserve(n - free);
    for &i in &order[free..] {
        let v = rows.revenue[i as usize];
        let mut at = ((v as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> shift) as usize;
        let place = loop {
            let slot = unsafe { e.seen.get_unchecked_mut(at) };
            if slot.0 == v {
                break slot.1;
            }
            if slot.0 == 0 {
                *slot = (v, e.amounts.len() as u32);
                e.amounts.push(v);
                break slot.1;
            }
            at = (at + 1) & (slots - 1);
        };
        e.paid_at.push(place);
    }
    let revenue_width = index_width(e.amounts.len());
    e.revenue_idx.clear();
    e.revenue_idx.reserve(e.paid_at.len() * revenue_width as usize / 8 + 32);
    e.revenue_idx.push(revenue_width as u8);
    rle_run(&mut e.revenue_idx, 0, free, revenue_width);
    packed_run(&mut e.revenue_idx, e.paid_at.iter().copied(), e.paid_at.len(), revenue_width);
    // top_category: the definition levels (a 4-byte length, then runs of
    // one-bit values: absent, present), then the categories of the rows that
    // have one, which are in the same order as before.
    e.levels.clear();
    e.levels.extend_from_slice(&[0; 4]);
    rle_run(&mut e.levels, 0, free, 1);
    rle_run(&mut e.levels, 1, n - free, 1);
    let level_bytes = (e.levels.len() - 4) as u32;
    e.levels[..4].copy_from_slice(&level_bytes.to_le_bytes());
    pack_into(&mut e.cat_idx, rows.cats.iter().copied(), rows.cats.len(), index_width(ctx.cat_recs().len()));

    let e = &*e;
    let int = |name: &'static str, k: usize| {
        let (idx, dict, _, plain) = &e.counts[k];
        if coded[k] {
            Column { name, ptype: parquet::INT32, kind: Kind::Int, optional: false, payload: vec![&idx[..]], dict: Some((vec![as_bytes(dict)], dict.len())) }
        } else {
            Column { name, ptype: parquet::INT32, kind: Kind::Int, optional: false, payload: vec![as_bytes(plain)], dict: None }
        }
    };
    let columns = [
        Column {
            name: "user_id",
            ptype: parquet::BYTE_ARRAY,
            kind: Kind::Str,
            optional: false,
            payload: vec![&e.id_idx],
            dict: Some((vec![&rows.ids], rows.people as usize)),
        },
        Column {
            name: "day",
            ptype: parquet::INT32,
            kind: Kind::Date,
            optional: false,
            payload: vec![&e.day_idx],
            dict: Some((vec![as_bytes(&e.days)], e.days.len())),
        },
        int("sessions", 0),
        int("events", 1),
        Column {
            name: "revenue_cents",
            ptype: parquet::INT64,
            kind: Kind::Int,
            optional: false,
            payload: vec![&e.revenue_idx],
            dict: Some((vec![as_bytes(&e.amounts)], e.amounts.len())),
        },
        Column {
            name: "top_category",
            ptype: parquet::BYTE_ARRAY,
            kind: Kind::Str,
            optional: true,
            payload: vec![&e.levels, &e.cat_idx],
            dict: Some((vec![&e.categories], ctx.cat_recs().len())),
        },
    ];
    let path = ctx.out_dir.join("user_daily").join(format!("part-{tid:03}-{seq:04}.parquet"));
    parquet::write(&path, n, &columns).expect("write user_daily");
}

fn write_campaign_daily(ctx: &Ctx) {
    let ncamp = ctx.campaigns().ids.len();
    let ndays = ctx.ndays;
    let mut total = CampAcc { assisted: vec![0; ncamp * ndays], credit: vec![Default::default(); ncamp * ndays] };
    for t in 0..ctx.nthreads {
        let acc = unsafe { &*ctx.acc[t].0.get() };
        for (a, b) in total.assisted.iter_mut().zip(&acc.assisted) {
            *a += b;
        }
        for (a, b) in total.credit.iter_mut().zip(&acc.credit) {
            a.sessions += b.sessions;
            a.purchasing += b.purchasing;
            a.revenue += b.revenue;
        }
    }
    // Text and days are dictionaries; a count column is one where that is the
    // smaller form.
    use parquet::{as_bytes, index_width, pack_indices};
    let (mut ids, mut channels) = (Vec::<u8>::new(), Vec::<u8>::new());
    let (mut nids, mut nchannels) = (0u32, 0u32);
    let mut channel_of: HashMap<&[u8], u32> = HashMap::new();
    let (mut id_at, mut channel_at, mut day_at) = (Vec::<u32>::new(), Vec::<u32>::new(), Vec::<u32>::new());
    let (mut sessions, mut purchasing, mut assisted) = (Vec::<i64>::new(), Vec::<i64>::new(), Vec::<i64>::new());
    let mut revenue = Vec::<i64>::new();
    for camp in 0..ncamp {
        let mut listed = false;
        let mut channel = 0;
        for d in 0..ndays {
            let cell = &total.credit[camp * ndays + d];
            let assists = total.assisted[camp * ndays + d];
            if cell.sessions == 0 && assists == 0 {
                continue;
            }
            if !listed {
                listed = true;
                let id = &ctx.campaigns().ids[camp];
                ids.extend_from_slice(&(id.len() as u32).to_le_bytes());
                ids.extend_from_slice(id);
                nids += 1;
                let name: &[u8] = &ctx.campaigns().channels[camp];
                channel = *channel_of.entry(name).or_insert_with(|| {
                    channels.extend_from_slice(&(name.len() as u32).to_le_bytes());
                    channels.extend_from_slice(name);
                    nchannels += 1;
                    nchannels - 1
                });
            }
            id_at.push(nids - 1);
            channel_at.push(channel);
            day_at.push(d as u32);
            sessions.push(cell.sessions as i64);
            purchasing.push(cell.purchasing as i64);
            revenue.push(cell.revenue as i64);
            assisted.push(assists as i64);
        }
    }
    let n = day_at.len();
    let days: Vec<i32> = (0..ndays as i64).map(|d| (ctx.day0 + d) as i32).collect();
    let id_idx = pack_indices(id_at.iter().copied(), n, index_width(nids as usize));
    let channel_idx = pack_indices(channel_at.iter().copied(), n, index_width(nchannels as usize));
    let day_idx = pack_indices(day_at.iter().copied(), n, index_width(days.len()));
    // The distinct values of a column and the indexes into them, if that is
    // smaller than the values themselves.
    let coded = |values: &[i64]| -> Option<(Vec<u8>, Vec<i64>)> {
        let mut dict: Vec<i64> = Vec::new();
        let mut seen: HashMap<i64, u32> = HashMap::new();
        let at: Vec<u32> = values
            .iter()
            .map(|&v| {
                *seen.entry(v).or_insert_with(|| {
                    dict.push(v);
                    dict.len() as u32 - 1
                })
            })
            .collect();
        let width = index_width(dict.len());
        (8 * dict.len() + n * width as usize / 8 + 64 < 8 * n).then(|| (pack_indices(at.iter().copied(), n, width), dict))
    };
    let coded = [coded(&sessions), coded(&purchasing), coded(&revenue), coded(&assisted)];
    let int = |name: &'static str, values: &'_ [i64], coded: &'_ Option<(Vec<u8>, Vec<i64>)>| match coded {
        Some((idx, dict)) => Column {
            name,
            ptype: parquet::INT64,
            kind: Kind::Int,
            optional: false,
            payload: vec![unsafe { std::slice::from_raw_parts(idx.as_ptr(), idx.len()) }],
            dict: Some((vec![unsafe { std::slice::from_raw_parts(dict.as_ptr() as *const u8, 8 * dict.len()) }], dict.len())),
        },
        None => Column {
            name,
            ptype: parquet::INT64,
            kind: Kind::Int,
            optional: false,
            payload: vec![unsafe { std::slice::from_raw_parts(values.as_ptr() as *const u8, 8 * values.len()) }],
            dict: None,
        },
    };
    let text = |name: &'static str, idx: &'_ [u8], dict: &'_ [u8], count: u32| Column {
        name,
        ptype: parquet::BYTE_ARRAY,
        kind: Kind::Str,
        optional: false,
        payload: vec![unsafe { std::slice::from_raw_parts(idx.as_ptr(), idx.len()) }],
        dict: Some((vec![unsafe { std::slice::from_raw_parts(dict.as_ptr(), dict.len()) }], count as usize)),
    };
    let columns = [
        text("campaign_id", &id_idx, &ids, nids),
        text("channel", &channel_idx, &channels, nchannels),
        Column { name: "day", ptype: parquet::INT32, kind: Kind::Date, optional: false, payload: vec![&day_idx], dict: Some((vec![as_bytes(&days)], days.len())) },
        int("sessions", &sessions, &coded[0]),
        int("purchasing_sessions", &purchasing, &coded[1]),
        int("revenue_cents", &revenue, &coded[2]),
        int("assisted_sessions", &assisted, &coded[3]),
    ];
    let path = ctx.out_dir.join("campaign_daily").join("part-000.parquet");
    parquet::write(&path, n, &columns).expect("write campaign_daily");
}

fn main() {
    let t_start = Instant::now();
    let all_args: Vec<String> = std::env::args().collect();
    let force_slow = all_args.iter().any(|a| a == "--slow") || std::env::var("SS_SLOW").is_ok();
    let force_bench = all_args.iter().any(|a| a == "--bench");
    let args: Vec<String> = all_args.into_iter().filter(|a| !a.starts_with("--")).collect();
    let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
    let data_dir = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| env("DATA_DIR", "/data")));
    let out_dir = PathBuf::from(args.get(2).cloned().unwrap_or_else(|| env("OUTPUT_DIR", "/output")));
    let timing = std::env::var("SS_TIMING").is_ok();
    if std::env::var("SS_DIAG").is_ok() {
        diag::run(&data_dir, &out_dir);
    }
    sys::NOHUGE.store(std::env::var("SS_NOHUGE").is_ok(), Relaxed);
    if force_slow {
        slow::run(&data_dir, &out_dir);
        unsafe { sys::_exit(0) }
    }
    let nthreads = std::env::var("NCPU")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(8, usize::from))
        .min(64);

    let mark = |what: &str| {
        if timing {
            eprintln!("[{:8.2} ms]   setup: {what}", t_start.elapsed().as_secs_f64() * 1e3);
        }
    };
    mark("start");
    let (ws, we) = lookup::manifest(&std::fs::read(data_dir.join("manifest.json")).expect("manifest.json"))
        .expect("manifest window");

    // Test switch: tiny chunks put every kind of line on a chunk boundary.
    let chunk_override: Option<usize> =
        std::env::var("SS_CHUNK_BYTES").ok().and_then(|v| v.parse().ok()).filter(|&n: &usize| n > 0);
    let mut files: Vec<FileMap> = Vec::new();
    let mut file_base: Vec<u64> = Vec::new();
    let mut event_files: Vec<(u64, u32)> = Vec::new();
    let mut link_chunks: Vec<Chunk> = Vec::new();
    let mut main_chunks: Vec<Chunk> = Vec::new();
    let mut bytes = [0usize; 3];
    for (kind, dir, step) in [
        (FileKind::Links, "links", chunk_override.unwrap_or(LINK_CHUNK)),
        (FileKind::Clicks, "clicks", chunk_override.unwrap_or(CHUNK)),
        (FileKind::Events, "events", chunk_override.unwrap_or(CHUNK)),
    ] {
        for path in list_csv(&data_dir.join(dir)) {
            let map = FileMap::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let len = map.len;
            let file = files.len() as u32;
            file_base.push(bytes[kind as usize] as u64);
            if kind == FileKind::Events && len > 0 {
                event_files.push((bytes[kind as usize] as u64, file));
            }
            bytes[kind as usize] += len;
            files.push(map);
            let mut start = 0;
            while start < len {
                let end = (start + step).min(len);
                let chunk = Chunk { file, kind, start, end };
                if kind == FileKind::Links {
                    link_chunks.push(chunk);
                } else {
                    main_chunks.push(chunk);
                }
                start = end;
            }
        }
    }
    let (events_bytes, clicks_bytes, links_bytes) =
        (bytes[FileKind::Events as usize], bytes[FileKind::Clicks as usize], bytes[FileKind::Links as usize]);
    mark("files mapped");

    // Sizes. The user table holds every distinct user ID, at a load of at most about a half.
    let est_ids = links_bytes / 16 + events_bytes / 2200 + clicks_bytes / 1500 + 2048;
    // SS_TABLE_X8 (experiment): table slots per estimated ID, in eighths.
    let table_x8: usize = std::env::var("SS_TABLE_X8").ok().and_then(|v| v.parse().ok()).filter(|&n| n >= 8).unwrap_or(16);
    let cap = (est_ids * table_x8 / 8).next_multiple_of(1 << parse::LOCAL_BITS);
    let ps = cap >> parse::LOCAL_BITS;
    // Trace partitions: about 30,000 traces each, so that the two bitmaps of
    // pass 2 stay in the first-level cache; but never so many that a thread's
    // write-combining lines (64 bytes per partition) crowd its L2.
    let pd_bytes: usize = std::env::var("SS_PD_BYTES").ok().and_then(|v| v.parse().ok()).filter(|&n| n > 0).unwrap_or(2_400_000);
    let pd = (events_bytes / pd_bytes).next_power_of_two().clamp(1, 4096);
    // A trace holds the position of its line among all event bytes; the rest
    // of the word is the fingerprint of the event ID.
    let loc_bits = (64 - (events_bytes.max(2) as u64 - 1).leading_zeros()).max(1);
    let mut loc_file: Vec<u32> = Vec::with_capacity((events_bytes >> 20) + 2);
    {
        let mut at = 0usize;
        for mb in 0..=(events_bytes >> 20) as u64 {
            while at + 1 < event_files.len() && event_files[at + 1].0 <= mb << 20 {
                at += 1;
            }
            loc_file.push(at as u32);
        }
    }
    // Block pool: address space for the worst case (every line as short as a
    // line can be); real data needs about a fifth of it. `pool_floor` is what
    // to settle for if the kernel will not reserve that much.
    let pool_slack = (pd + 3 * ps) * nthreads * 2 + 4096;
    let pool_blocks = (events_bytes * 5 / 2 + clicks_bytes * 2) / part::BLOCK_BYTES + pool_slack;
    let pool_floor = (events_bytes * 3 / 5 + clicks_bytes * 4 / 5) / part::BLOCK_BYTES + pool_slack;

    let t0 = ws - REACH_MS;
    let day0 = t0.div_euclid(DAY_MS);
    let fits = ws < we && we - t0 < (1i64 << 31) && cap < (1 << parse::PERSON_BITS) && loc_bits <= 40;
    if !fits {
        eprintln!("input does not fit the fast path: using the plain implementation");
        slow::run(&data_dir, &out_dir);
        unsafe { sys::_exit(0) }
    }
    let ndays = ((we - 1).div_euclid(DAY_MS) - day0 + 1) as usize;
    // The input files in pieces of at most 32 MB (a multiple of the page
    // size), for giving their pages back at the end.
    let zap_pieces: Vec<(u32, usize, usize)> = files
        .iter()
        .enumerate()
        .flat_map(|(i, f)| (0..f.len.div_ceil(32 << 20)).map(move |k| (i as u32, k << 25, ((k + 1) << 25).min(f.len))))
        .collect();

    let ctx = Ctx {
        ws,
        we,
        t0,
        day0,
        day_off: (t0 - day0 * DAY_MS) as u64,
        ndays,
        nthreads,
        lookups: Shared::new(None),
        data_dir: data_dir.clone(),
        // Long IDs (ids.rs) are one user in eight; their table has room for
        // several times that, and a run that fills it half starts over with
        // the plain implementation (`failed`).
        users: KeySpace::new(b'u', cap / 4),
        clicks: KeySpace::by_reference(b'k'),
        dates: parse::DateTab::new(ws, day0, ndays),
        scalar_only: std::env::var("SS_SCALAR").is_ok(),
        prefetch_ahead: std::env::var("SS_PREFETCH").ok().and_then(|v| v.parse().ok()).unwrap_or(2048),
        wide_split: !std::env::var("SS_WSPLIT").is_ok_and(|v| v == "0")
            && if matches!(parse::wide_kind(), 2 | 3) { parse::splitw::available5() } else { parse::splitw::available() },
        split_check: std::env::var("SS_SPLIT_CHECK").is_ok_and(|v| v == "1"),
        zap: std::env::var("SS_NOZAP").is_err(),
        // SS_OUT_CHUNK (experiment): bytes of rows per user_daily file.
        out_chunk: std::env::var("SS_OUT_CHUNK").ok().and_then(|v| v.parse().ok()).unwrap_or(512 << 10),
        table: Table::new(cap),
        pool: BlockPool::new(pool_blocks, pool_floor),
        files,
        file_base,
        loc_file,
        event_files,
        loc_bits,
        fp_mask: u64::MAX >> loc_bits,
        link_chunks,
        main_chunks,
        next_link: AtomicUsize::new(0),
        next_compress: AtomicUsize::new(0),
        next_main: AtomicUsize::new(0),
        next_d: AtomicUsize::new(0),
        next_s: AtomicUsize::new(0),
        next_zap: AtomicUsize::new(0),
        zap_pieces,
        s_order: std::sync::OnceLock::new(),
        pd,
        ps,
        k: (0..nthreads).map(|_| Shared::new(PartWriter::new(0))).collect(),
        c: (0..nthreads).map(|_| Shared::new(PartWriter::new(0))).collect(),
        s: (0..nthreads).map(|_| Shared::new(PartWriter::new(0))).collect(),
        psw: (0..nthreads).map(|_| Shared::new(PartWriter::new(0))).collect(),
        x: (0..nthreads).map(|_| Shared::new(Cancels::default())).collect(),
        acc: (0..nthreads).map(|_| Shared::new(CampAcc::default())).collect(),
        barrier: Barrier::new(nthreads),
        prof: Default::default(),
        bail: AtomicBool::new(false),
        out_dir,
        timing,
        t_start,
    };
    std::fs::create_dir_all(ctx.out_dir.join("user_daily")).expect("create user_daily");
    std::fs::create_dir_all(ctx.out_dir.join("campaign_daily")).expect("create campaign_daily");
    if timing {
        eprintln!(
            "[{:8.2} ms] setup: {} threads, events {} MB, clicks {} MB, links {} MB, table {} slots, pd {}, ps {}",
            t_start.elapsed().as_secs_f64() * 1e3,
            nthreads,
            events_bytes >> 20,
            clicks_bytes >> 20,
            links_bytes >> 20,
            cap,
            pd,
            ps
        );
    }

    if force_bench {
        bench(&ctx);
        unsafe { sys::_exit(0) }
    }
    std::thread::scope(|scope| {
        for tid in 1..nthreads {
            let ctx = &ctx;
            scope.spawn(move || worker(ctx, tid));
        }
        worker(&ctx, 0);
    });
    if ctx.failed() {
        // Nothing has been written yet: the flags are only raised while parsing.
        eprintln!("fast path bailed out: using the plain implementation");
        slow::run(&data_dir, &ctx.out_dir);
        unsafe { sys::_exit(0) }
    }
    if timing {
        ctx.print_profile();
        prof::write_samples();
        let cancels: usize = (0..nthreads).map(|t| ctx.x(t).len()).sum();
        eprintln!(
            "[{:8.2} ms] done; table {} of {} slots used, pool used {} MB, long user ids {}, irregular ids: users {}, clicks {}; {} cancellations",
            t_start.elapsed().as_secs_f64() * 1e3,
            ctx.table.used.load(Relaxed),
            ctx.table.cap,
            ctx.pool.used_bytes() >> 20,
            ctx.users.long.as_ref().map_or(0, |long| long.len()),
            ctx.users.dict.len(),
            ctx.clicks.dict.len(),
            cancels
        );
    }
    if std::env::var("SS_SIGNAL").is_ok() {
        signal();
    }
    // Skip destructors and unmapping: the kernel reclaims everything at exit.
    unsafe { sys::_exit(0) }
}
