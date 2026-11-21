//! Shopping Sessions: clean, link, sessionize and attribute click-stream CSVs
//! into two daily Parquet tables. See DESIGN.md for the pipeline.

mod ids;
mod lookup;
mod parquet;
mod parse;
mod part;
mod pass2;
mod pass3;
mod slow;
mod sys;
mod table;
mod util;

use std::cell::UnsafeCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::Barrier;
use std::time::Instant;

use ids::{Dict, KeySpace};
use lookup::{Campaigns, Products};
use parquet::{Column, Kind};
use part::{BlockPool, PartWriter};
use pass3::CampCell;
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
    pub ingest_base_s: i64,
    /// `day0` is the UTC day number of `t0`; `day_off` its offset into that day.
    pub day0: i64,
    pub day_off: u64,
    pub ndays: usize,
    pub nthreads: usize,
    pub products: Products,
    /// Category names as Parquet PLAIN values (4-byte length + bytes), by index.
    pub cat_recs: Vec<Vec<u8>>,
    pub campaigns: Campaigns,
    pub users: KeySpace,
    pub clicks: KeySpace,
    pub event_dict: Dict,
    pub dates: parse::DateTab,
    /// Debug switch: send every line through the scalar parsers.
    pub scalar_only: bool,
    /// Software prefetch distance in bytes for the event parser (0 = off).
    pub prefetch_ahead: usize,
    /// Release the big memory regions from all threads before exiting.
    zap: bool,
    pub table: Table,
    pub pool: BlockPool,
    pub files: Vec<FileMap>,
    link_chunks: Vec<Chunk>,
    main_chunks: Vec<Chunk>,
    next_link: AtomicUsize,
    next_compress: AtomicUsize,
    next_main: AtomicUsize,
    next_d: AtomicUsize,
    next_s: AtomicUsize,
    /// Event partitions (a power of two) and the bits of an event key stored within one.
    pub pd: usize,
    pub rem_bits: u32,
    pub ps: usize,
    d: Vec<Shared<PartWriter>>,
    c: Vec<Shared<PartWriter>>,
    s: Vec<Shared<PartWriter>>,
    psw: Vec<Shared<PartWriter>>,
    acc: Vec<Shared<Vec<CampCell>>>,
    barrier: Barrier,
    /// Thread CPU nanoseconds per profiled section, summed over threads (TIMING only).
    pub prof: [std::sync::atomic::AtomicU64; 12],
    /// Set when the data does not fit the fast path's packed layouts.
    pub bail: AtomicBool,
    out_dir: PathBuf,
    timing: bool,
    t_start: Instant,
}

impl Ctx {
    // Writers are mutated only by their owning thread during the phase that
    // fills them, and only read (by anyone) after the barrier that follows.
    #[inline(always)]
    pub fn d(&self, t: usize) -> &PartWriter {
        unsafe { &*self.d[t].0.get() }
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

    /// Profiling: the current thread CPU time, or 0 when not timing.
    #[inline]
    pub fn now(&self) -> u64 {
        if self.timing {
            sys::thread_cpu_ns()
        } else {
            0
        }
    }

    /// Profiling: charge the time since `since` to `section`; returns the new "now".
    #[inline]
    pub fn charge(&self, section: usize, since: u64) -> u64 {
        if self.timing {
            let t = sys::thread_cpu_ns();
            self.prof[section].fetch_add(t - since, Relaxed);
            t
        } else {
            0
        }
    }

    fn failed(&self) -> bool {
        self.bail.load(Relaxed) || self.table.overflow.load(Relaxed)
    }

    fn sync(&self, what: &str) {
        if self.barrier.wait().is_leader() && self.timing {
            eprintln!("[{:8.2} ms] {what}", self.t_start.elapsed().as_secs_f64() * 1e3);
        }
    }
}

// Profiled sections (thread CPU time, printed with TIMING=1).
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
const P_NAMES: [&str; 10] = [
    "parse events", "parse clicks", "finalize", "dedup gather", "dedup table", "dedup emit", "sessions gather",
    "sessions sort", "sessions people", "write user_daily",
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
        *ctx.d[tid].0.get() = PartWriter::new(ctx.pd);
        *ctx.c[tid].0.get() = PartWriter::new(ctx.ps);
        *ctx.s[tid].0.get() = PartWriter::new(ctx.ps);
        *ctx.psw[tid].0.get() = PartWriter::new(ctx.ps);
    }
    // ---- links: insert both IDs, join them
    loop {
        let i = ctx.next_link.fetch_add(1, Relaxed);
        if i >= ctx.link_chunks.len() {
            break;
        }
        parse::links_chunk(ctx, &ctx.link_chunks[i]);
    }
    ctx.sync("links parsed");
    loop {
        let from = ctx.next_compress.fetch_add(COMPRESS_STEP, Relaxed);
        if from >= ctx.table.cap {
            break;
        }
        ctx.table.compress(from, (from + COMPRESS_STEP).min(ctx.table.cap));
    }
    ctx.sync("people resolved");

    // ---- parse clicks and events
    {
        let d = unsafe { &mut *ctx.d[tid].0.get() };
        let c = unsafe { &mut *ctx.c[tid].0.get() };
        let mut t = ctx.now();
        loop {
            let i = ctx.next_main.fetch_add(1, Relaxed);
            if i >= ctx.main_chunks.len() {
                break;
            }
            let ch = &ctx.main_chunks[i];
            match ch.kind {
                FileKind::Events => {
                    parse::events_chunk(ctx, d, ch);
                    t = ctx.charge(P_EVENTS, t);
                }
                _ => {
                    parse::clicks_chunk(ctx, c, ch);
                    t = ctx.charge(P_CLICKS, t);
                }
            }
        }
        d.finalize(&ctx.pool);
        c.finalize(&ctx.pool);
        ctx.charge(P_FINALIZE, t);
    }
    ctx.sync("events and clicks parsed");
    if ctx.failed() {
        return;
    }

    // ---- dedup, scatter survivors by person
    {
        let s = unsafe { &mut *ctx.s[tid].0.get() };
        let ps = unsafe { &mut *ctx.psw[tid].0.get() };
        let mut sc = pass2::Scratch::default();
        loop {
            let p = ctx.next_d.fetch_add(1, Relaxed);
            if p >= ctx.pd {
                break;
            }
            pass2::run(ctx, &mut sc, s, ps, p);
        }
        s.finalize(&ctx.pool);
        ps.finalize(&ctx.pool);
        // Never free: unmapping costs time and the process is about to exit.
        std::mem::forget(sc);
    }
    ctx.sync("deduplicated");

    // ---- sessions, attribution, rows
    let mut sc = pass3::Scratch::new(ctx);
    loop {
        let p = ctx.next_s.fetch_add(1, Relaxed);
        if p >= ctx.ps {
            break;
        }
        pass3::run(ctx, &mut sc, p);
    }
    let t = ctx.now();
    write_user_daily(ctx, &sc.rows, tid);
    ctx.charge(P_WRITE, t);
    unsafe { *ctx.acc[tid].0.get() = std::mem::take(&mut sc.acc) };
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
    } else {
        std::mem::forget(sc);
    }
    ctx.sync("campaign_daily written, memory released");
}

/// `--bench`: best-of-N thread CPU time per line for each parser on a small
/// region of the largest file of its kind. Small and repeated so that the
/// minimum is meaningful even on a busy machine.
fn bench(ctx: &Ctx) {
    sys::NORELEASE.store(true, Relaxed);
    for ch in &ctx.link_chunks {
        parse::links_chunk(ctx, ch);
    }
    ctx.table.compress(0, ctx.table.cap);
    let region: usize = std::env::var("BENCH_BYTES").ok().and_then(|v| v.parse().ok()).unwrap_or(1 << 19);
    let iters: usize = std::env::var("BENCH_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(400);
    for kind in [FileKind::Events, FileKind::Clicks, FileKind::Links] {
        let chunks = if kind == FileKind::Links { &ctx.link_chunks } else { &ctx.main_chunks };
        let Some(file) = chunks
            .iter()
            .filter(|c| c.kind == kind)
            .map(|c| c.file)
            .max_by_key(|&f| ctx.files[f as usize].len)
        else {
            continue;
        };
        let data = ctx.files[file as usize].slice();
        let end = region.min(data.len());
        let lines = data[..end].iter().filter(|&&c| c == b'\n').count().max(1);
        let ch = Chunk { file, kind, start: 0, end };
        let mut best = u64::MAX;
        let mut free: Vec<u32> = Vec::new();
        for _ in 0..iters {
            let nparts = if kind == FileKind::Events { ctx.pd } else { ctx.ps };
            let mut w = PartWriter::new(nparts);
            w.free = std::mem::take(&mut free);
            let t = sys::thread_cpu_ns();
            match kind {
                FileKind::Events => parse::events_chunk(ctx, &mut w, &ch),
                FileKind::Clicks => parse::clicks_chunk(ctx, &mut w, &ch),
                FileKind::Links => parse::links_chunk(ctx, &ch),
            }
            best = best.min(sys::thread_cpu_ns() - t);
            w.finalize(&ctx.pool);
            free = w.all_blocks().to_vec();
            free.extend_from_slice(&w.free);
        }
        let name = ["events", "clicks", "links"][kind as usize];
        eprintln!("bench {name}: {:.1} ns/line ({lines} lines, {} bytes/line)", best as f64 / lines as f64, end / lines);
    }
}

fn write_user_daily(ctx: &Ctx, rows: &pass3::Rows, tid: usize) {
    let n = rows.n;
    if n == 0 && tid != 0 {
        return;
    }
    let levels = parquet::def_levels_header(&rows.valid);
    let (ids, cats) = (&rows.ids, &rows.cats);
    let columns = [
        Column { name: "user_id", ptype: parquet::BYTE_ARRAY, kind: Kind::Str, optional: false, payload: vec![ids] },
        Column { name: "day", ptype: parquet::INT32, kind: Kind::Date, optional: false, payload: vec![parquet::as_bytes(&rows.day)] },
        Column { name: "sessions", ptype: parquet::INT32, kind: Kind::Int, optional: false, payload: vec![parquet::as_bytes(&rows.sessions)] },
        Column { name: "events", ptype: parquet::INT32, kind: Kind::Int, optional: false, payload: vec![parquet::as_bytes(&rows.events)] },
        Column { name: "revenue_cents", ptype: parquet::INT64, kind: Kind::Int, optional: false, payload: vec![parquet::as_bytes(&rows.revenue)] },
        Column { name: "top_category", ptype: parquet::BYTE_ARRAY, kind: Kind::Str, optional: true, payload: vec![&levels, &rows.valid, cats] },
    ];
    let path = ctx.out_dir.join("user_daily").join(format!("part-{tid:03}.parquet"));
    parquet::write(&path, n, &columns).expect("write user_daily");
}

fn write_campaign_daily(ctx: &Ctx) {
    let ncamp = ctx.campaigns.ids.len();
    let ndays = ctx.ndays;
    let mut total = vec![CampCell::default(); ncamp * ndays];
    for t in 0..ctx.nthreads {
        let acc = unsafe { &*ctx.acc[t].0.get() };
        for (a, b) in total.iter_mut().zip(acc.iter()) {
            a.sessions += b.sessions;
            a.purchasing += b.purchasing;
            a.assisted += b.assisted;
            a.revenue += b.revenue;
        }
    }
    let (mut ids, mut channels) = (Vec::<u8>::new(), Vec::<u8>::new());
    let (mut day, mut sessions, mut purchasing, mut assisted) = (Vec::<i32>::new(), Vec::<i64>::new(), Vec::<i64>::new(), Vec::<i64>::new());
    let mut revenue = Vec::<i64>::new();
    for camp in 0..ncamp {
        for d in 0..ndays {
            let cell = &total[camp * ndays + d];
            if cell.sessions == 0 && cell.assisted == 0 {
                continue;
            }
            for (buf, s) in [(&mut ids, &ctx.campaigns.ids[camp]), (&mut channels, &ctx.campaigns.channels[camp])] {
                buf.extend_from_slice(&(s.len() as u32).to_le_bytes());
                buf.extend_from_slice(s);
            }
            day.push((ctx.day0 + d as i64) as i32);
            sessions.push(cell.sessions as i64);
            purchasing.push(cell.purchasing as i64);
            revenue.push(cell.revenue as i64);
            assisted.push(cell.assisted as i64);
        }
    }
    let columns = [
        Column { name: "campaign_id", ptype: parquet::BYTE_ARRAY, kind: Kind::Str, optional: false, payload: vec![&ids] },
        Column { name: "channel", ptype: parquet::BYTE_ARRAY, kind: Kind::Str, optional: false, payload: vec![&channels] },
        Column { name: "day", ptype: parquet::INT32, kind: Kind::Date, optional: false, payload: vec![parquet::as_bytes(&day)] },
        Column { name: "sessions", ptype: parquet::INT64, kind: Kind::Int, optional: false, payload: vec![parquet::as_bytes(&sessions)] },
        Column { name: "purchasing_sessions", ptype: parquet::INT64, kind: Kind::Int, optional: false, payload: vec![parquet::as_bytes(&purchasing)] },
        Column { name: "revenue_cents", ptype: parquet::INT64, kind: Kind::Int, optional: false, payload: vec![parquet::as_bytes(&revenue)] },
        Column { name: "assisted_sessions", ptype: parquet::INT64, kind: Kind::Int, optional: false, payload: vec![parquet::as_bytes(&assisted)] },
    ];
    let path = ctx.out_dir.join("campaign_daily").join("part-000.parquet");
    parquet::write(&path, day.len(), &columns).expect("write campaign_daily");
}

fn main() {
    let t_start = Instant::now();
    let all_args: Vec<String> = std::env::args().collect();
    let force_slow = all_args.iter().any(|a| a == "--slow") || std::env::var("SLOW").is_ok();
    let force_bench = all_args.iter().any(|a| a == "--bench");
    let args: Vec<String> = all_args.into_iter().filter(|a| !a.starts_with("--")).collect();
    let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
    let data_dir = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| env("DATA_DIR", "/data")));
    let out_dir = PathBuf::from(args.get(2).cloned().unwrap_or_else(|| env("OUTPUT_DIR", "/output")));
    let timing = std::env::var("TIMING").is_ok();
    sys::NOHUGE.store(std::env::var("NOHUGE").is_ok(), Relaxed);
    sys::NORELEASE.store(std::env::var("NORELEASE").is_ok(), Relaxed);
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
    let products = Products::load(&std::fs::read(data_dir.join("products.csv")).unwrap_or_default());
    mark("products loaded");
    let campaigns = Campaigns::load(&std::fs::read(data_dir.join("campaigns.csv")).unwrap_or_default());
    mark("campaigns loaded");

    let mut files: Vec<FileMap> = Vec::new();
    let mut link_chunks: Vec<Chunk> = Vec::new();
    let mut main_chunks: Vec<Chunk> = Vec::new();
    let mut bytes = [0usize; 3];
    for (kind, dir, step) in [
        (FileKind::Links, "links", LINK_CHUNK),
        (FileKind::Clicks, "clicks", CHUNK),
        (FileKind::Events, "events", CHUNK),
    ] {
        for path in list_csv(&data_dir.join(dir)) {
            let map = FileMap::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let len = map.len;
            bytes[kind as usize] += len;
            let file = files.len() as u32;
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

    // Sizes. The user table holds every distinct user ID; aim for a load of at most ~0.5.
    let est_ids = links_bytes / 16 + events_bytes / 2200 + clicks_bytes / 1500 + 2048;
    let cap = (est_ids * 2).next_multiple_of(1 << parse::LOCAL_BITS);
    let ps = cap >> parse::LOCAL_BITS;
    // Event partitions: about 0.5 MB of records each, but never so many that a
    // thread's write-combining lines (64 bytes per partition) crowd its L2.
    let pd = (events_bytes / 2_400_000).next_power_of_two().clamp(1 << parse::MIN_PD_BITS, 2048);
    let pool_blocks = (events_bytes * 5 / 2 + clicks_bytes * 2) / part::BLOCK_BYTES
        + (pd + 3 * ps) * nthreads * 2
        + 4096;

    let t0 = ws - REACH_MS;
    let day0 = t0.div_euclid(DAY_MS);
    let window_s = (we - ws) / 1000;
    let ingest_base_s = ws.div_euclid(1000) - (parse::INGEST_MAX - (window_s + 86_400 + 2));
    let fits = ws < we
        && we - t0 < (1i64 << 31)
        && parse::INGEST_MAX - (window_s + 86_400 + 2) >= 86_400
        && cap < (1 << parse::PERSON_BITS)
        && campaigns.ids.len() < (1 << parse::CAMPAIGN_BITS);
    if !fits {
        eprintln!("input does not fit the fast path: using the plain implementation");
        slow::run(&data_dir, &out_dir);
        unsafe { sys::_exit(0) }
    }

    let ctx = Ctx {
        ws,
        we,
        t0,
        ingest_base_s,
        day0,
        day_off: (t0 - day0 * DAY_MS) as u64,
        ndays: ((we - 1).div_euclid(DAY_MS) - day0 + 1) as usize,
        nthreads,
        cat_recs: products
            .categories
            .iter()
            .map(|name| [&(name.len() as u32).to_le_bytes()[..], name].concat())
            .collect(),
        products,
        campaigns,
        users: KeySpace::new(),
        clicks: KeySpace::new(),
        event_dict: Dict::new(),
        dates: parse::DateTab::new(ws),
        scalar_only: std::env::var("SCALAR").is_ok(),
        prefetch_ahead: std::env::var("PREFETCH").ok().and_then(|v| v.parse().ok()).unwrap_or(0),
        zap: std::env::var("NOZAP").is_err(),
        table: Table::new(cap),
        pool: BlockPool::new(pool_blocks),
        files,
        link_chunks,
        main_chunks,
        next_link: AtomicUsize::new(0),
        next_compress: AtomicUsize::new(0),
        next_main: AtomicUsize::new(0),
        next_d: AtomicUsize::new(0),
        next_s: AtomicUsize::new(0),
        pd,
        rem_bits: ids::EVENT_KEY_BITS - pd.trailing_zeros(),
        ps,
        d: (0..nthreads).map(|_| Shared::new(PartWriter::new(0))).collect(),
        c: (0..nthreads).map(|_| Shared::new(PartWriter::new(0))).collect(),
        s: (0..nthreads).map(|_| Shared::new(PartWriter::new(0))).collect(),
        psw: (0..nthreads).map(|_| Shared::new(PartWriter::new(0))).collect(),
        acc: (0..nthreads).map(|_| Shared::new(Vec::new())).collect(),
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
        let total: u64 = ctx.prof.iter().map(|p| p.load(Relaxed)).sum();
        for (name, p) in P_NAMES.iter().zip(&ctx.prof) {
            let ns = p.load(Relaxed);
            eprintln!("    cpu {:8.2} ms {:5.1}%  {name}", ns as f64 / 1e6, 100.0 * ns as f64 / total.max(1) as f64);
        }
        eprintln!(
            "[{:8.2} ms] done; table {} of {} slots used, pool used {} MB, irregular ids: users {}, events {}, clicks {}",
            t_start.elapsed().as_secs_f64() * 1e3,
            ctx.table.used.load(Relaxed),
            ctx.table.cap,
            ctx.pool.used_bytes() >> 20,
            ctx.users.dict.len(),
            ctx.event_dict.len(),
            ctx.clicks.dict.len()
        );
    }
    // Skip destructors and unmapping: the kernel reclaims everything at exit.
    unsafe { sys::_exit(0) }
}
