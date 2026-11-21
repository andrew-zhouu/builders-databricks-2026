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
    /// Unmap the big memory regions from all threads before exiting.
    zap: bool,
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
        self.bail.load(Relaxed) || self.table.overflow.load(Relaxed)
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
    loop {
        let i = ctx.next_s.fetch_add(1, Relaxed);
        if i >= ctx.ps {
            break;
        }
        pass3::run(ctx, &mut sc, order[i] as usize);
    }
    let t = ctx.now();
    write_user_daily(ctx, &sc.rows, tid);
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
        let n = ctx.nthreads;
        let share = |len: usize| (len / n * tid, if tid + 1 == n { len } else { len / n * (tid + 1) });
        let (a, b) = share(ctx.pool.used_bytes());
        ctx.pool.zap(a, b);
        let (a, b) = share(ctx.table.cap);
        ctx.table.zap(a, b);
        for (i, file) in ctx.files.iter().enumerate() {
            if i % n == tid {
                file.release(0, file.len);
            }
        }
    } else {
        std::mem::forget(sc);
    }
    ctx.sync("campaign_daily written, memory released");
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

fn write_user_daily(ctx: &Ctx, rows: &pass3::Rows, tid: usize) {
    use parquet::{as_bytes, index_width, pack_indices, small_dict};
    let n = rows.n;
    if n == 0 && tid != 0 {
        return;
    }
    // Every column as a dictionary and bit-packed indexes into it.
    let id_idx = pack_indices(rows.id_idx.iter().copied(), n, index_width(rows.people as usize));
    let days: Vec<i32> = (0..ctx.ndays as i64).map(|d| (ctx.day0 + d) as i32).collect();
    let day_idx = pack_indices(rows.day.iter().map(|&d| d as u32), n, index_width(days.len()));
    // Small counts: the distinct values. (Plain, should one ever be huge.)
    let small = |values: &[i32]| small_dict(values).map(|(dict, idx)| (pack_indices(idx.iter().copied(), n, index_width(dict.len())), dict));
    let (sessions, events) = (small(&rows.sessions), small(&rows.events));
    // Revenue is zero for most rows and anything for the rest.
    let mut amounts: Vec<i64> = vec![0];
    let mut seen: HashMap<i64, u32> = HashMap::new();
    let revenue_at: Vec<u32> = rows
        .revenue
        .iter()
        .map(|&v| {
            if v == 0 {
                0
            } else {
                *seen.entry(v).or_insert_with(|| {
                    amounts.push(v);
                    amounts.len() as u32 - 1
                })
            }
        })
        .collect();
    let revenue_idx = pack_indices(revenue_at.iter().copied(), n, index_width(amounts.len()));
    let levels = parquet::def_levels_header(&rows.valid);
    let categories: Vec<u8> = ctx.cat_recs().concat();
    let cat_idx = pack_indices(rows.cats.iter().copied(), rows.cats.len(), index_width(ctx.cat_recs().len()));

    let int = |name: &'static str, values: &'_ [i32], coded: &'_ Option<(Vec<u8>, Vec<i32>)>| match coded {
        Some((idx, dict)) => Column {
            name,
            ptype: parquet::INT32,
            kind: Kind::Int,
            optional: false,
            payload: vec![unsafe { std::slice::from_raw_parts(idx.as_ptr(), idx.len()) }],
            dict: Some((vec![unsafe { std::slice::from_raw_parts(dict.as_ptr() as *const u8, 4 * dict.len()) }], dict.len())),
        },
        None => Column {
            name,
            ptype: parquet::INT32,
            kind: Kind::Int,
            optional: false,
            payload: vec![unsafe { std::slice::from_raw_parts(values.as_ptr() as *const u8, 4 * values.len()) }],
            dict: None,
        },
    };
    let columns = [
        Column {
            name: "user_id",
            ptype: parquet::BYTE_ARRAY,
            kind: Kind::Str,
            optional: false,
            payload: vec![&id_idx],
            dict: Some((vec![&rows.ids], rows.people as usize)),
        },
        Column {
            name: "day",
            ptype: parquet::INT32,
            kind: Kind::Date,
            optional: false,
            payload: vec![&day_idx],
            dict: Some((vec![as_bytes(&days)], days.len())),
        },
        int("sessions", &rows.sessions, &sessions),
        int("events", &rows.events, &events),
        Column {
            name: "revenue_cents",
            ptype: parquet::INT64,
            kind: Kind::Int,
            optional: false,
            payload: vec![&revenue_idx],
            dict: Some((vec![as_bytes(&amounts)], amounts.len())),
        },
        Column {
            name: "top_category",
            ptype: parquet::BYTE_ARRAY,
            kind: Kind::Str,
            optional: true,
            payload: vec![&levels, &rows.valid, &cat_idx],
            dict: Some((vec![&categories], ctx.cat_recs().len())),
        },
    ];
    let path = ctx.out_dir.join("user_daily").join(format!("part-{tid:03}.parquet"));
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
    let (mut ids, mut channels) = (Vec::<u8>::new(), Vec::<u8>::new());
    let (mut day, mut sessions, mut purchasing, mut assisted) = (Vec::<i32>::new(), Vec::<i64>::new(), Vec::<i64>::new(), Vec::<i64>::new());
    let mut revenue = Vec::<i64>::new();
    for camp in 0..ncamp {
        for d in 0..ndays {
            let cell = &total.credit[camp * ndays + d];
            let assists = total.assisted[camp * ndays + d];
            if cell.sessions == 0 && assists == 0 {
                continue;
            }
            for (buf, s) in [(&mut ids, &ctx.campaigns().ids[camp]), (&mut channels, &ctx.campaigns().channels[camp])] {
                buf.extend_from_slice(&(s.len() as u32).to_le_bytes());
                buf.extend_from_slice(s);
            }
            day.push((ctx.day0 + d as i64) as i32);
            sessions.push(cell.sessions as i64);
            purchasing.push(cell.purchasing as i64);
            revenue.push(cell.revenue as i64);
            assisted.push(assists as i64);
        }
    }
    let columns = [
        Column { name: "campaign_id", ptype: parquet::BYTE_ARRAY, kind: Kind::Str, optional: false, payload: vec![&ids], dict: None },
        Column { name: "channel", ptype: parquet::BYTE_ARRAY, kind: Kind::Str, optional: false, payload: vec![&channels], dict: None },
        Column { name: "day", ptype: parquet::INT32, kind: Kind::Date, optional: false, payload: vec![parquet::as_bytes(&day)], dict: None },
        Column { name: "sessions", ptype: parquet::INT64, kind: Kind::Int, optional: false, payload: vec![parquet::as_bytes(&sessions)], dict: None },
        Column { name: "purchasing_sessions", ptype: parquet::INT64, kind: Kind::Int, optional: false, payload: vec![parquet::as_bytes(&purchasing)], dict: None },
        Column { name: "revenue_cents", ptype: parquet::INT64, kind: Kind::Int, optional: false, payload: vec![parquet::as_bytes(&revenue)], dict: None },
        Column { name: "assisted_sessions", ptype: parquet::INT64, kind: Kind::Int, optional: false, payload: vec![parquet::as_bytes(&assisted)], dict: None },
    ];
    let path = ctx.out_dir.join("campaign_daily").join("part-000.parquet");
    parquet::write(&path, day.len(), &columns).expect("write campaign_daily");
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
        users: KeySpace::new(b'u'),
        clicks: KeySpace::new(b'k'),
        dates: parse::DateTab::new(ws, day0, ndays),
        scalar_only: std::env::var("SS_SCALAR").is_ok(),
        prefetch_ahead: std::env::var("SS_PREFETCH").ok().and_then(|v| v.parse().ok()).unwrap_or(2048),
        zap: std::env::var("SS_NOZAP").is_err(),
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
            "[{:8.2} ms] done; table {} of {} slots used, pool used {} MB, irregular ids: users {}, clicks {}; {} cancellations",
            t_start.elapsed().as_secs_f64() * 1e3,
            ctx.table.used.load(Relaxed),
            ctx.table.cap,
            ctx.pool.used_bytes() >> 20,
            ctx.users.dict.len(),
            ctx.clicks.dict.len(),
            cancels
        );
    }
    // Skip destructors and unmapping: the kernel reclaims everything at exit.
    unsafe { sys::_exit(0) }
}
