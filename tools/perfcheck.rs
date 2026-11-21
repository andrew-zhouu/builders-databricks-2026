//! perfcheck: does this kernel give user space hardware performance counters?
//! Opens instructions / cycles / branch-misses for the current thread and
//! measures a small loop. Prints the counts, or the error per counter.

use std::os::raw::{c_int, c_long};

extern "C" {
    fn syscall(num: c_long, ...) -> c_long;
    fn read(fd: c_int, buf: *mut u8, n: usize) -> isize;
    fn __errno_location() -> *mut c_int;
}

const SYS_PERF_EVENT_OPEN: c_long = 298;

fn open(config: u64) -> Result<c_int, c_int> {
    // struct perf_event_attr, version 0 (64 bytes): type, size, config, ..., flags at word 5.
    let mut attr = [0u64; 8];
    attr[0] = 0 /* PERF_TYPE_HARDWARE */ | (64u64 << 32);
    attr[1] = config;
    attr[5] = (1 << 5) | (1 << 6); // exclude_kernel, exclude_hv
    let fd = unsafe { syscall(SYS_PERF_EVENT_OPEN, attr.as_ptr(), 0 as c_int, -1 as c_int, -1 as c_int, 0u64) };
    if fd < 0 {
        Err(unsafe { *__errno_location() })
    } else {
        Ok(fd as c_int)
    }
}

fn value(fd: c_int) -> u64 {
    let mut v = 0u64;
    unsafe { read(fd, &mut v as *mut u64 as *mut u8, 8) };
    v
}

fn main() {
    let names = [("cycles", 0u64), ("instructions", 1), ("branch-misses", 5), ("cache-misses", 3)];
    let fds: Vec<_> = names.iter().map(|&(_, c)| open(c)).collect();
    let before: Vec<u64> = fds.iter().map(|f| f.map_or(0, value)).collect();
    let mut x = 1u64;
    for i in 0..10_000_000u64 {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(i | 1);
    }
    std::hint::black_box(x);
    for ((name, _), (fd, b)) in names.iter().zip(fds.iter().zip(before)) {
        match fd {
            Ok(fd) => println!("{name:14} {}", value(*fd) - b),
            Err(e) => println!("{name:14} unavailable (errno {e})"),
        }
    }
}
