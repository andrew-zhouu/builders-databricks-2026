//! Shopping Sessions: clean, link, sessionize and attribute click-stream CSVs
//! into two daily Parquet tables. See DESIGN.md for the pipeline.

mod ids;
mod lookup;
mod parquet;
mod parse;
mod part;
mod pass2;
mod pass3;
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
    pub campaigns: Campaigns,
    pub users: KeySpace,
    pub clicks: KeySpace,
    pub event_dict: Dict,
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
    pub pd: usize,
    pub pd_mask: usize,
    pub ps: usize,
    d: Vec<Shared<PartWriter>>,
    c: Vec<Shared<PartWriter>>,
    s: Vec<Shared<PartWriter>>,
    psw: Vec<Shared<PartWriter>>,
    acc: Vec<Shared<Vec<CampCell>>>,
    barrier: Barrier,
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

    fn failed(&self) -> bool {
        self.bail.load(Relaxed) || self.table.overflow.load(Relaxed)
    }

    fn sync(&self, what: &str) {
        if self.barrier.wait().is_leader() && self.timing {
            eprintln!("[{:8.2} ms] {what}", self.t_start.elapsed().as_secs_f64() * 1e3);
        }
    }
}

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
        loop {
            let i = ctx.next_main.fetch_add(1, Relaxed);
            if i >= ctx.main_chunks.len() {
                break;
            }
            let ch = &ctx.main_chunks[i];
            match ch.kind {
                FileKind::Events => parse::events_chunk(ctx, d, ch),
                _ => parse::clicks_chunk(ctx, c, ch),
            }
        }
        d.finalize(&ctx.pool);
        c.finalize(&ctx.pool);
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
    write_user_daily(ctx, &sc.rows, tid);
    unsafe { *ctx.acc[tid].0.get() = std::mem::take(&mut sc.acc) };
    ctx.sync("sessions built, user_daily written");
}

fn write_user_daily(ctx: &Ctx, rows: &pass3::Rows, tid: usize) {
    let n = rows.key.len();
    if n == 0 && tid != 0 {
        return;
    }
    let mut ids: Vec<u8> = Vec::with_capacity(n * 14);
    let mut tmp: Vec<u8> = Vec::with_capacity(64);
    for &k in &rows.key {
        tmp.clear();
        ctx.users.bytes(k, &mut tmp);
        ids.extend_from_slice(&(tmp.len() as u32).to_le_bytes());
        ids.extend_from_slice(&tmp);
    }
    let valid: Vec<bool> = rows.cat.iter().map(|&c| c != u32::MAX).collect();
    let levels = parquet::def_levels(&valid);
    let mut cats: Vec<u8> = Vec::new();
    for &c in &rows.cat {
        if c != u32::MAX {
            let name = &ctx.products.categories[c as usize];
            cats.extend_from_slice(&(name.len() as u32).to_le_bytes());
            cats.extend_from_slice(name);
        }
    }
    let columns = [
        Column { name: "user_id", ptype: parquet::BYTE_ARRAY, kind: Kind::Str, optional: false, payload: vec![&ids] },
        Column { name: "day", ptype: parquet::INT32, kind: Kind::Date, optional: false, payload: vec![parquet::as_bytes(&rows.day)] },
        Column { name: "sessions", ptype: parquet::INT32, kind: Kind::Int, optional: false, payload: vec![parquet::as_bytes(&rows.sessions)] },
        Column { name: "events", ptype: parquet::INT32, kind: Kind::Int, optional: false, payload: vec![parquet::as_bytes(&rows.events)] },
        Column { name: "revenue_cents", ptype: parquet::INT64, kind: Kind::Int, optional: false, payload: vec![parquet::as_bytes(&rows.revenue)] },
        Column { name: "top_category", ptype: parquet::BYTE_ARRAY, kind: Kind::Str, optional: true, payload: vec![&levels, &cats] },
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
    let args: Vec<String> = std::env::args().collect();
    let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
    let data_dir = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| env("DATA_DIR", "/data")));
    let out_dir = PathBuf::from(args.get(2).cloned().unwrap_or_else(|| env("OUTPUT_DIR", "/output")));
    let timing = std::env::var("TIMING").is_ok();
    let nthreads = std::env::var("NCPU")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(8, usize::from))
        .min(64);

    let (ws, we) = lookup::manifest(&std::fs::read(data_dir.join("manifest.json")).expect("manifest.json"))
        .expect("manifest window");
    let products = Products::load(&std::fs::read(data_dir.join("products.csv")).unwrap_or_default());
    let campaigns = Campaigns::load(&std::fs::read(data_dir.join("campaigns.csv")).unwrap_or_default());

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

    // Sizes. The user table holds every distinct user ID; aim for a load of at most ~0.5.
    let est_ids = links_bytes / 16 + events_bytes / 2200 + clicks_bytes / 1500 + 2048;
    let cap = (est_ids * 2).next_multiple_of(1 << parse::LOCAL_BITS);
    let ps = cap >> parse::LOCAL_BITS;
    let pd = (events_bytes / 1_800_000).next_power_of_two().clamp(1, 4096);
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
        eprintln!("input does not fit the fast path");
        std::process::exit(3);
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
        products,
        campaigns,
        users: KeySpace::new(),
        clicks: KeySpace::new(),
        event_dict: Dict::new(),
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
        pd_mask: pd - 1,
        ps,
        d: (0..nthreads).map(|_| Shared::new(PartWriter::new(pd))).collect(),
        c: (0..nthreads).map(|_| Shared::new(PartWriter::new(ps))).collect(),
        s: (0..nthreads).map(|_| Shared::new(PartWriter::new(ps))).collect(),
        psw: (0..nthreads).map(|_| Shared::new(PartWriter::new(ps))).collect(),
        acc: (0..nthreads).map(|_| Shared::new(Vec::new())).collect(),
        barrier: Barrier::new(nthreads),
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

    std::thread::scope(|scope| {
        for tid in 1..nthreads {
            let ctx = &ctx;
            scope.spawn(move || worker(ctx, tid));
        }
        worker(&ctx, 0);
    });
    if ctx.failed() {
        eprintln!("fast path bailed out");
        std::process::exit(3);
    }
    write_campaign_daily(&ctx);
    if timing {
        eprintln!(
            "[{:8.2} ms] done; pool used {} MB, irregular ids: users {}, events {}, clicks {}",
            t_start.elapsed().as_secs_f64() * 1e3,
            ctx.pool.used_bytes() >> 20,
            ctx.users.dict.len(),
            ctx.event_dict.len(),
            ctx.clicks.dict.len()
        );
    }
    // Skip destructors and unmapping: the kernel reclaims everything at exit.
    unsafe { sys::_exit(0) }
}
