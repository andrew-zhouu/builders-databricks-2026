//! Thin OS layer: anonymous and file mappings, madvise, immediate exit.

use std::fs::File;
use std::os::fd::AsRawFd;
use std::os::raw::{c_int, c_void};

extern "C" {
    fn mmap(addr: *mut c_void, len: usize, prot: c_int, flags: c_int, fd: c_int, off: i64) -> *mut c_void;
    fn madvise(addr: *mut c_void, len: usize, advice: c_int) -> c_int;
    pub fn _exit(code: c_int) -> !;
}

#[repr(C)]
struct Timespec {
    sec: i64,
    nsec: i64,
}
extern "C" {
    fn clock_gettime(clock: c_int, ts: *mut Timespec) -> c_int;
}

/// CPU time consumed by the calling thread, in nanoseconds.
pub fn thread_cpu_ns() -> u64 {
    let mut ts = Timespec { sec: 0, nsec: 0 };
    unsafe { clock_gettime(3, &mut ts) };
    ts.sec as u64 * 1_000_000_000 + ts.nsec as u64
}

const PROT_READ: c_int = 1;
const PROT_WRITE: c_int = 2;
const MAP_PRIVATE: c_int = 2;
const MAP_ANONYMOUS: c_int = 0x20;
const MAP_NORESERVE: c_int = 0x4000;
const MADV_DONTNEED: c_int = 4;
const MADV_HUGEPAGE: c_int = 14;

/// Experiment switches (environment): NOHUGE = no MADV_HUGEPAGE,
/// NORELEASE = keep parsed input mapped until exit.
pub static NOHUGE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub static NORELEASE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Zeroed, lazily committed memory. Never returned to the OS before exit.
pub fn map_anon(len: usize, huge: bool) -> *mut u8 {
    let len = len.max(4096);
    let p = unsafe {
        mmap(std::ptr::null_mut(), len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0)
    };
    if p as isize == -1 {
        panic!("mmap of {len} bytes failed");
    }
    if huge && !NOHUGE.load(std::sync::atomic::Ordering::Relaxed) {
        unsafe { madvise(p, len, MADV_HUGEPAGE) };
    }
    p as *mut u8
}

/// Drop the fully covered pages of an anonymous range (their contents are lost).
pub fn zap(ptr: *mut u8, len: usize) {
    let a = (ptr as usize + 4095) & !4095;
    let b = (ptr as usize + len) & !4095;
    if b > a {
        unsafe { madvise(a as *mut c_void, b - a, MADV_DONTNEED) };
    }
}

/// An input file: open for positioned reads, and (unless `map` is false) a
/// read-only private mapping of the whole of it. Empty files map to an empty slice.
pub struct FileMap {
    pub ptr: *const u8,
    pub len: usize,
    file: File,
}
unsafe impl Send for FileMap {}
unsafe impl Sync for FileMap {}

impl FileMap {
    pub fn open(path: &std::path::Path, map: bool) -> std::io::Result<FileMap> {
        let f = File::open(path)?;
        let len = f.metadata()?.len() as usize;
        if len == 0 || !map {
            return Ok(FileMap { ptr: std::ptr::NonNull::<u8>::dangling().as_ptr(), len, file: f });
        }
        let p = unsafe { mmap(std::ptr::null_mut(), len, PROT_READ, MAP_PRIVATE, f.as_raw_fd(), 0) };
        if p as isize == -1 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(FileMap { ptr: p as *const u8, len, file: f })
    }

    /// Fill `buf` from byte `offset` of the file; returns the bytes read
    /// (short only at end of file).
    pub fn read_at(&self, buf: &mut [u8], offset: usize) -> usize {
        use std::os::unix::fs::FileExt;
        let mut done = 0;
        while done < buf.len() {
            match self.file.read_at(&mut buf[done..], (offset + done) as u64) {
                Ok(0) => break,
                Ok(n) => done += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => panic!("read failed: {e}"),
            }
        }
        done
    }

    #[inline]
    pub fn slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }

    /// Drop the page-table entries of fully covered pages in `[start, end)`; the
    /// pages stay in the page cache. Spreads unmap work over the worker threads.
    pub fn release(&self, start: usize, end: usize) {
        if NORELEASE.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let a = (start + 4095) & !4095;
        let b = end & !4095;
        if b > a {
            unsafe { madvise(self.ptr.add(a) as *mut c_void, b - a, MADV_DONTNEED) };
        }
    }
}
