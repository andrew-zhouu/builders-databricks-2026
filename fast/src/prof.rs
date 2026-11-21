//! Built-in profiling with the CPU's performance counters (perf_event_open).
//!
//! Two modes, both per thread and both off unless asked for:
//!   * counters: cycles, instructions, branch misses and cache misses, read at
//!     section boundaries, so every profiled section gets instructions and
//!     cycles per input row. Instruction counts do not depend on what else the
//!     machine is doing, which timings on a shared machine do.
//!   * sampling (`SS_PROF=cycles|instructions|branch-misses|cache-misses`):
//!     the kernel records the instruction pointer every N events; the
//!     histogram, relative to the load address, goes to `SS_PROF_OUT` for
//!     tools/icount_report.py to attribute to source lines.
//!
//! Everything here is best effort: where the kernel offers no counters, the
//! values are simply zero.

use std::cell::RefCell;
use std::collections::HashMap;
use std::os::raw::{c_int, c_long, c_void};
use std::sync::Mutex;

extern "C" {
    fn syscall(num: c_long, ...) -> c_long;
    fn read(fd: c_int, buf: *mut c_void, n: usize) -> isize;
    fn mmap(addr: *mut c_void, len: usize, prot: c_int, flags: c_int, fd: c_int, off: i64) -> *mut c_void;
}

const SYS_PERF_EVENT_OPEN: c_long = 298;
pub const NCOUNTERS: usize = 4;
pub const COUNTER_NAMES: [&str; NCOUNTERS] = ["cycles", "instructions", "branch-misses", "cache-misses"];
/// PERF_COUNT_HW_* for the four counters above.
const CONFIGS: [u64; NCOUNTERS] = [0, 1, 5, 3];
const RING_PAGES: usize = 1024;

fn open(config: u64, period: u64) -> c_int {
    // struct perf_event_attr, version 0 (64 bytes).
    let mut attr = [0u64; 8];
    attr[0] = 64u64 << 32; // type PERF_TYPE_HARDWARE (0), size 64
    attr[1] = config;
    attr[2] = period; // sample_period (0 = counting only)
    attr[3] = (period != 0) as u64; // sample_type: PERF_SAMPLE_IP
    attr[5] = (1 << 5) | (1 << 6); // exclude_kernel, exclude_hv
    unsafe { syscall(SYS_PERF_EVENT_OPEN, attr.as_ptr(), 0 as c_int, -1 as c_int, -1 as c_int, 0u64) as c_int }
}

struct Sampler {
    base: *mut u8,
    tail: u64,
    hist: HashMap<u64, u64>,
}

impl Sampler {
    fn new(config: u64, period: u64) -> Option<Sampler> {
        let fd = open(config, period);
        if fd < 0 {
            return None;
        }
        let len = (1 + RING_PAGES) * 4096;
        let base = unsafe { mmap(std::ptr::null_mut(), len, 3, 1 /* MAP_SHARED */, fd, 0) };
        if base as isize == -1 {
            return None;
        }
        Some(Sampler { base: base as *mut u8, tail: 0, hist: HashMap::new() })
    }

    /// Move the samples recorded so far from the kernel's ring into the histogram.
    fn drain(&mut self) {
        unsafe {
            let head = std::ptr::read_volatile(self.base.add(1024) as *const u64);
            let data = self.base.add(4096);
            let mask = (RING_PAGES * 4096 - 1) as u64;
            let word = |at: u64| std::ptr::read_unaligned(data.add((at & mask) as usize) as *const u64);
            while self.tail < head {
                let header = word(self.tail);
                let size = header >> 48;
                if size == 0 {
                    break;
                }
                if header as u32 == 9 {
                    // PERF_RECORD_SAMPLE: the instruction pointer follows
                    *self.hist.entry(word(self.tail + 8)).or_insert(0) += 1;
                }
                self.tail += size;
            }
            std::ptr::write_volatile(self.base.add(1032) as *mut u64, self.tail);
        }
    }
}

struct ThreadProf {
    fds: [c_int; NCOUNTERS],
    sampler: Option<Sampler>,
}

thread_local! {
    static THREAD: RefCell<Option<ThreadProf>> = const { RefCell::new(None) };
}

/// What to sample, parsed once from `SS_PROF` ("event" or "event:period").
fn sample_spec() -> Option<(u64, u64)> {
    let spec = std::env::var("SS_PROF").ok()?;
    let mut parts = spec.split(':');
    let name = parts.next()?;
    let config = CONFIGS[COUNTER_NAMES.iter().position(|n| *n == name)?];
    let default = match name {
        "cycles" | "instructions" => 200_003,
        _ => 1_009,
    };
    Some((config, parts.next().and_then(|p| p.parse().ok()).unwrap_or(default)))
}

fn with<R>(f: impl FnOnce(&mut ThreadProf) -> R) -> R {
    THREAD.with(|t| {
        let mut t = t.borrow_mut();
        let prof = t.get_or_insert_with(|| ThreadProf {
            fds: CONFIGS.map(|c| open(c, 0)),
            sampler: sample_spec().and_then(|(c, p)| Sampler::new(c, p)),
        });
        f(prof)
    })
}

/// Thread CPU time, the four (user-mode) counters, and from getrusage the
/// thread's kernel time and page-fault count, now.
#[derive(Clone, Copy, Default)]
pub struct Snap {
    pub ns: u64,
    pub c: [u64; NCOUNTERS],
    pub sys_ns: u64,
    pub faults: u64,
}

extern "C" {
    fn getrusage(who: c_int, usage: *mut i64) -> c_int;
}

/// (kernel-mode ns, minor + major faults, user-mode ns) of the calling thread
/// (`who` = 1) or the whole process (`who` = 0).
pub fn rusage(who: c_int) -> (u64, u64, u64) {
    // struct rusage: utime, stime (two timevals), then 14 longs; minflt and majflt are the 5th and 6th.
    let mut r = [0i64; 18];
    unsafe { getrusage(who, r.as_mut_ptr()) };
    let ns = |sec: i64, usec: i64| sec as u64 * 1_000_000_000 + usec as u64 * 1_000;
    (ns(r[2], r[3]), (r[8] + r[9]) as u64, ns(r[0], r[1]))
}

pub fn snap() -> Snap {
    with(|p| {
        let (sys_ns, faults, _) = rusage(1);
        let mut s = Snap { ns: crate::sys::thread_cpu_ns(), c: [0; NCOUNTERS], sys_ns, faults };
        for (v, &fd) in s.c.iter_mut().zip(&p.fds) {
            if fd >= 0 {
                unsafe { read(fd, v as *mut u64 as *mut c_void, 8) };
            }
        }
        s
    })
}

static SAMPLES: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());

/// Collect this thread's samples (call at phase boundaries and at the end).
pub fn drain() {
    with(|p| {
        if let Some(s) = &mut p.sampler {
            s.drain();
        }
    });
}

/// Hand this thread's histogram to the process-wide collection.
pub fn finish_thread() {
    with(|p| {
        if let Some(s) = &mut p.sampler {
            s.drain();
            SAMPLES.lock().unwrap().extend(s.hist.drain());
        }
    });
}

/// Write every thread's samples as "hex_offset count" lines (offsets relative
/// to the executable's load address), the format tools/icount_report.py reads.
pub fn write_samples() {
    let Ok(path) = std::env::var("SS_PROF_OUT") else { return };
    let samples = SAMPLES.lock().unwrap();
    if samples.is_empty() {
        return;
    }
    let exe = std::fs::read_link("/proc/self/exe").ok();
    let maps = std::fs::read_to_string("/proc/self/maps").unwrap_or_default();
    let base = maps
        .lines()
        .find(|l| exe.as_ref().and_then(|e| e.to_str()).is_some_and(|e| l.ends_with(e)))
        .and_then(|l| u64::from_str_radix(l.split('-').next()?, 16).ok())
        .filter(|&b| b >= 0x1000_0000)
        .unwrap_or(0);
    let mut hist: HashMap<u64, u64> = HashMap::new();
    for &(ip, n) in samples.iter() {
        *hist.entry(ip.wrapping_sub(base)).or_insert(0) += n;
    }
    let mut rows: Vec<_> = hist.into_iter().collect();
    rows.sort_unstable();
    let total: u64 = rows.iter().map(|r| r.1).sum();
    let mut out = String::new();
    for (a, n) in rows {
        out.push_str(&format!("{a:x} {n}\n"));
    }
    out.push_str(&format!("total {total}\n"));
    let _ = std::fs::write(path, out);
}
