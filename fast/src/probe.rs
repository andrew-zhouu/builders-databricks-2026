//! Microbenchmarks for the diagnostic image (cargo feature `probe`, which only
//! that image is built with): what the scoring machine's page cache, clock
//! speed, huge pages, page faults and memory are like, in numbers that can be
//! compared with the same probes on a development machine.
//!
//! This is the only place in the crate with 512-bit instructions: one probe
//! measures how much using them lowers the clock for the code around them.

use core::arch::x86_64::*;
use std::hint::black_box;
use std::os::fd::AsRawFd;
use std::os::raw::{c_int, c_void};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

extern "C" {
    fn mmap(addr: *mut c_void, len: usize, prot: c_int, flags: c_int, fd: c_int, off: i64) -> *mut c_void;
    fn munmap(addr: *mut c_void, len: usize) -> c_int;
    fn madvise(addr: *mut c_void, len: usize, advice: c_int) -> c_int;
    fn mincore(addr: *mut c_void, len: usize, vec: *mut u8) -> c_int;
}

const MB: usize = 1 << 20;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

fn files(dir: &Path) -> Vec<(PathBuf, u64)> {
    let mut v: Vec<(PathBuf, u64)> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().filter_map(|e| e.metadata().ok().filter(|m| m.is_file()).map(|m| (e.path(), m.len()))).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn map_file(path: &Path, len: usize) -> Option<*mut u8> {
    let f = std::fs::File::open(path).ok()?;
    let p = unsafe { mmap(std::ptr::null_mut(), len, 1, 2, f.as_raw_fd(), 0) };
    (p as isize != -1).then_some(p as *mut u8)
}

fn map_anon(len: usize, advice: c_int) -> *mut u8 {
    let p = unsafe { mmap(std::ptr::null_mut(), len, 3, 0x22, -1, 0) };
    assert!(p as isize != -1, "mmap");
    unsafe { madvise(p, len, advice) };
    p as *mut u8
}

fn anon_huge_kb() -> u64 {
    std::fs::read_to_string("/proc/self/smaps_rollup")
        .ok()
        .and_then(|t| t.lines().find(|l| l.starts_with("AnonHugePages:")).and_then(|l| l.split_whitespace().nth(1)?.parse().ok()))
        .unwrap_or(0)
}

/// How much of the input is in the page cache right now. Call before anything reads it.
pub fn page_cache(data_dir: &Path, out: &mut Vec<String>) {
    for dir in ["events", "clicks", "links"] {
        let (mut pages, mut resident, mut failed) = (0u64, 0u64, 0u64);
        let t = Instant::now();
        for (path, len) in files(&data_dir.join(dir)) {
            let len = len as usize;
            if len == 0 {
                continue;
            }
            let Some(p) = map_file(&path, len) else {
                failed += 1;
                continue;
            };
            let n = len.div_ceil(4096);
            let mut vec = vec![0u8; n];
            if unsafe { mincore(p as *mut c_void, len, vec.as_mut_ptr()) } == 0 {
                pages += n as u64;
                resident += vec.iter().filter(|&&b| b & 1 != 0).count() as u64;
            } else {
                failed += 1;
            }
            unsafe { munmap(p as *mut c_void, len) };
        }
        out.push(format!(
            "page cache {dir}: {resident} of {pages} pages resident ({failed} files not checked), checked in {:.1} ms",
            ms(t)
        ));
    }
}

/// A chain of dependent shift-xor-multiply steps: 5 cycles a step on every
/// Intel core since Haswell (1 + 1 + 3), and nothing a compiler can shortcut,
/// so its time is the clock period.
const CHAIN_CYCLES: f64 = 5.0;
#[inline(never)]
fn chain(n: u64, seed: u64) -> u64 {
    let mut x = seed | 1;
    for _ in 0..n {
        x = (x ^ (x >> 7)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
    x
}

#[inline(never)]
unsafe fn ymm_touch(buf: &[u8; 4096]) -> u64 {
    let c = _mm256_set1_epi8(b',' as i8);
    let mut acc = 0u64;
    for i in (0..4096).step_by(32) {
        let v = _mm256_loadu_si256(buf.as_ptr().add(i) as *const __m256i);
        acc = acc.wrapping_add(_mm256_movemask_epi8(_mm256_cmpeq_epi8(v, c)) as u32 as u64);
    }
    acc
}

#[inline(never)]
#[target_feature(enable = "avx512f,avx512bw")]
unsafe fn zmm_touch(buf: &[u8; 4096]) -> u64 {
    let c = _mm512_set1_epi8(b',' as i8);
    let mut acc = 0u64;
    for i in (0..4096).step_by(64) {
        let v = _mm512_loadu_si512(buf.as_ptr().add(i) as *const __m512i);
        acc = acc.wrapping_add(_mm512_cmpeq_epi8_mask(v, c));
    }
    acc
}

/// Nanoseconds per chain step while a little vector work of the given width
/// (0 = none, 256, 512) runs between the timed stretches.
fn clock(width: u32, millis: u64) -> f64 {
    let mut buf = [b'x'; 4096];
    buf[77] = b',';
    let start = Instant::now();
    let (mut acc, mut spent, mut steps) = (1u64, 0u128, 0u64);
    loop {
        let elapsed = start.elapsed().as_millis() as u64;
        if elapsed >= millis {
            break;
        }
        acc ^= match width {
            256 => unsafe { ymm_touch(black_box(&buf)) },
            512 => unsafe { zmm_touch(black_box(&buf)) },
            _ => 0,
        };
        let t = Instant::now();
        acc = chain(20_000, black_box(acc));
        // The first stretches let the core settle into its licence.
        if elapsed >= millis / 4 {
            spent += t.elapsed().as_nanos();
            steps += 20_000;
        }
    }
    black_box(acc);
    spent as f64 / steps.max(1) as f64
}

fn clocks(threads: usize, out: &mut Vec<String>) {
    let has512 = is_x86_feature_detected!("avx512f") && is_x86_feature_detected!("avx512bw");
    let widths: &[u32] = if has512 { &[0, 256, 512, 0] } else { &[0, 256, 0] };
    // The TSC ticks at the nominal frequency; the chain shows the real one.
    let (t, c0) = (Instant::now(), unsafe { _rdtsc() });
    chain(30_000_000, 1);
    let tsc_ghz = (unsafe { _rdtsc() } - c0) as f64 / t.elapsed().as_nanos() as f64;
    out.push(format!("tsc {tsc_ghz:.3} GHz; avx512 {}", if has512 { "present" } else { "ABSENT" }));
    for n in [1, threads] {
        let mut line = format!("clock, {n} thread(s) busy (GHz, slowest..fastest thread):");
        for &w in widths {
            let ns: Vec<f64> = std::thread::scope(|s| {
                let hs: Vec<_> = (0..n).map(|_| s.spawn(move || clock(w, 80))).collect();
                hs.into_iter().map(|h| h.join().unwrap()).collect()
            });
            let (lo, hi) = ns.iter().fold((f64::MAX, 0f64), |a, &x| (a.0.min(x), a.1.max(x)));
            let name = if w == 0 { "scalar".to_string() } else { format!("+{w}-bit") };
            line += &format!("  [{name}] {:.2}..{:.2}", CHAIN_CYCLES / hi, CHAIN_CYCLES / lo);
        }
        out.push(line);
    }
}

unsafe fn touch(p: *mut u8, len: usize) {
    for off in (0..len).step_by(4096) {
        p.add(off).write_volatile(1);
    }
}

/// Dependent random reads over `len` bytes (a power of two) at `p`: ns per read.
unsafe fn chase(p: *mut u8, len: usize, steps: usize) -> f64 {
    let lines = len / 64;
    for i in 0..lines {
        (p.add(i * 64) as *mut u64).write((i as u64 ^ 0xABCD).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    }
    // Each address depends on the value just read; the step counter keeps the
    // walk from falling into a short cycle.
    let mut v = 0u64;
    let t = Instant::now();
    for step in 0..steps as u64 {
        let idx = ((v >> 20) ^ step.wrapping_mul(0xBF58_476D_1CE4_E5B9) >> 30) as usize & (lines - 1);
        v = (p.add(idx * 64) as *const u64).read();
    }
    black_box(v);
    t.elapsed().as_nanos() as f64 / steps as f64
}

fn memory(threads: usize, out: &mut Vec<String>) {
    const LEN: usize = 256 * MB;
    unsafe {
        // Fresh memory: what a page fault costs with and without huge pages.
        let before = anon_huge_kb();
        let huge = map_anon(LEN, 14);
        let t = Instant::now();
        touch(huge, LEN);
        let huge_ms = ms(t);
        let got = anon_huge_kb().saturating_sub(before);
        let small = map_anon(LEN, 15);
        let t = Instant::now();
        touch(small, LEN);
        let small_ms = ms(t);
        out.push(format!(
            "first touch of 256 MB: {huge_ms:.1} ms with MADV_HUGEPAGE ({} MB came as huge pages), {small_ms:.1} ms with 4K pages",
            got / 1024
        ));
        // Cache-missing reads: memory latency, and what the page walk adds on 4K pages.
        let (h, s) = (chase(huge, LEN, 2_000_000), chase(small, LEN, 2_000_000));
        out.push(format!("random read in 256 MB: {h:.1} ns on huge pages, {s:.1} ns on 4K pages"));
        // Streaming.
        let t = Instant::now();
        std::ptr::copy_nonoverlapping(huge, small, LEN / 2);
        let copy_ms = ms(t);
        let t = Instant::now();
        let mut sum = 0u64;
        for i in (0..LEN).step_by(8) {
            sum = sum.wrapping_add((huge.add(i) as *const u64).read());
        }
        black_box(sum);
        out.push(format!(
            "one thread: copy {:.1} GB/s, read {:.1} GB/s",
            (LEN / 2) as f64 / copy_ms / 1e6,
            LEN as f64 / ms(t) / 1e6
        ));
        munmap(huge as *mut c_void, LEN);
        munmap(small as *mut c_void, LEN);
        // All threads faulting fresh huge-page memory at once, as the parser does.
        let t = Instant::now();
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| {
                    let p = map_anon(64 * MB, 14);
                    touch(p, 64 * MB);
                    munmap(p as *mut c_void, 64 * MB);
                });
            }
        });
        out.push(format!("{threads} threads, first touch of 64 MB each (huge pages asked): {:.1} ms", ms(t)));
    }
}

/// Reading the input: page faults on a mapping against read().
fn input(data_dir: &Path, out: &mut Vec<String>) {
    let Some((path, len)) = files(&data_dir.join("events")).into_iter().max_by_key(|f| f.1) else { return };
    let len = (len as usize).min(64 * MB) & !4095;
    if len == 0 {
        return;
    }
    unsafe {
        let Some(p) = map_file(&path, len) else { return };
        let t = Instant::now();
        let mut sum = 0u64;
        for off in (0..len).step_by(4096) {
            sum += p.add(off).read_volatile() as u64;
        }
        let fault_ms = ms(t);
        let t = Instant::now();
        for i in (0..len).step_by(8) {
            sum = sum.wrapping_add((p.add(i) as *const u64).read_unaligned());
        }
        let scan_ms = ms(t);
        munmap(p as *mut c_void, len);
        let f = std::fs::File::open(&path).unwrap();
        let mut buf = vec![1u8; MB];
        let t = Instant::now();
        let mut at = 0;
        while at < len {
            let n = f.read_at(&mut buf, at as u64).unwrap_or(0);
            if n == 0 {
                break;
            }
            sum += buf[0] as u64;
            at += n;
        }
        black_box(sum);
        out.push(format!(
            "largest events file, first {} MB: map and touch every page {fault_ms:.2} ms ({:.0} ns/page), then read it all {:.2} GB/s; read() instead {:.2} ms",
            len / MB,
            fault_ms * 1e6 / (len / 4096) as f64,
            len as f64 / scan_ms / 1e6,
            ms(t)
        ));
    }
}

/// What writing the output costs: `mb` megabytes to a new file in the output
/// directory, in 1 MB writes (then removed again).
fn output_file(dir: &Path, name: &str, mb: usize, sync: bool) -> String {
    use std::io::Write;
    let path = dir.join(name);
    let buf = vec![0x5Au8; MB];
    let t = Instant::now();
    let mut f = match std::fs::File::create(&path) {
        Ok(f) => f,
        Err(e) => return format!("output {name}: cannot create ({e})"),
    };
    let create_ms = ms(t);
    let t = Instant::now();
    for _ in 0..mb {
        if let Err(e) = f.write_all(&buf) {
            return format!("output {name}: write failed ({e})");
        }
    }
    let write_ms = ms(t);
    let t = Instant::now();
    let synced = if sync { f.sync_data().is_ok() } else { false };
    let sync_ms = ms(t);
    let t = Instant::now();
    drop(f);
    let close_ms = ms(t);
    let t = Instant::now();
    let _ = std::fs::remove_file(&path);
    format!(
        "output {name}: create {create_ms:.2} ms, write {mb} MB {write_ms:.1} ms ({:.0} MB/s){}, close {close_ms:.2} ms, remove {:.2} ms",
        mb as f64 * 1e3 / write_ms.max(1e-3),
        if sync { format!(", fdatasync {sync_ms:.1} ms (ok: {synced})") } else { String::new() },
        ms(t)
    )
}

/// The output directory: how fast it takes data, alone and from all threads,
/// and what forcing it to the device costs (a guide to what happens when the
/// container is torn down with unwritten output).
pub fn output(dir: &Path, threads: usize, out: &mut Vec<String>) {
    out.push(output_file(dir, ".probe-a", 8, false));
    out.push(output_file(dir, ".probe-b", 64, false));
    let t = Instant::now();
    std::thread::scope(|s| {
        for i in 0..threads {
            s.spawn(move || output_file(dir, &format!(".probe-t{i}"), 16, false));
        }
    });
    out.push(format!("output: {threads} threads writing 16 MB each: {:.1} ms in all", ms(t)));
    out.push(output_file(dir, ".probe-c", 16, true));
}

pub fn run(data_dir: &Path, threads: usize, out: &mut Vec<String>) {
    clocks(threads, out);
    memory(threads, out);
    input(data_dir, out);
}
