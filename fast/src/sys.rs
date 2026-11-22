//! Thin OS layer: anonymous and file mappings, madvise, immediate exit.

use std::fs::File;
use std::os::fd::AsRawFd;
use std::os::raw::{c_int, c_void};

extern "C" {
    fn mmap(addr: *mut c_void, len: usize, prot: c_int, flags: c_int, fd: c_int, off: i64) -> *mut c_void;
    fn madvise(addr: *mut c_void, len: usize, advice: c_int) -> c_int;
    fn sync_file_range(fd: c_int, pos: i64, len: i64, flags: std::os::raw::c_uint) -> c_int;
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

/// With SS_WRITEBACK=1 (an experiment, off otherwise): ask the kernel to
/// start writing a finished output file's pages to disk now, without waiting
/// for it.
///
/// Why one might: when a container exits, Docker unmounts its root, which
/// makes the kernel write out everything still unwritten on that disk before
/// `docker run` returns, and on our test machine that wait disappears when
/// the output was written early. Why it is off: the official clock does not
/// include that wait (a build with this on scored no better than one without
/// and three times the unwritten output), and disk activity during the run
/// can only cost time.
///
/// Starting the writeback can itself wait (for the file system's journal), so
/// a thread of its own does it: the caller only hands the file over.
pub fn start_writeback(f: File) {
    use std::sync::mpsc::{channel, Sender};
    use std::sync::{Mutex, OnceLock};
    static QUEUE: OnceLock<Option<Mutex<Sender<File>>>> = OnceLock::new();
    let queue = QUEUE.get_or_init(|| {
        if !std::env::var("SS_WRITEBACK").is_ok_and(|v| v == "1") {
            return None;
        }
        let (tx, rx) = channel::<File>();
        let run = move || {
            const SYNC_FILE_RANGE_WRITE: std::os::raw::c_uint = 2;
            for f in rx {
                unsafe { sync_file_range(f.as_raw_fd(), 0, 0, SYNC_FILE_RANGE_WRITE) };
            }
        };
        std::thread::Builder::new().name("writeback".into()).spawn(run).ok().map(|_| Mutex::new(tx))
    });
    if let Some(queue) = queue {
        if let Ok(tx) = queue.lock() {
            let _ = tx.send(f);
        }
    }
}

const PROT_READ: c_int = 1;
const PROT_WRITE: c_int = 2;
const MAP_PRIVATE: c_int = 2;
const MAP_ANONYMOUS: c_int = 0x20;
const MAP_NORESERVE: c_int = 0x4000;
const MADV_DONTNEED: c_int = 4;
const MADV_HUGEPAGE: c_int = 14;

/// Experiment switch (environment): SS_NOHUGE = no MADV_HUGEPAGE.
pub static NOHUGE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Zeroed, lazily committed memory, or `None` if the kernel refuses the mapping
/// (possible for very large reservations under strict overcommit). Never
/// returned to the OS before exit.
pub fn try_map_anon(len: usize, huge: bool) -> Option<*mut u8> {
    let len = len.max(4096);
    let p = unsafe {
        mmap(std::ptr::null_mut(), len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0)
    };
    if p as isize == -1 {
        return None;
    }
    if huge && !NOHUGE.load(std::sync::atomic::Ordering::Relaxed) {
        unsafe { madvise(p, len, MADV_HUGEPAGE) };
    }
    Some(p as *mut u8)
}

/// As `try_map_anon`, for memory the program cannot run without.
pub fn map_anon(len: usize, huge: bool) -> *mut u8 {
    try_map_anon(len, huge).unwrap_or_else(|| panic!("mmap of {len} bytes failed"))
}

/// Fault in the pages of a page-aligned range, writable, without changing
/// what is in them (MADV_POPULATE_WRITE, Linux 5.14). False if the kernel
/// cannot or will not.
pub fn populate(ptr: *mut u8, len: usize) -> bool {
    const MADV_POPULATE_WRITE: c_int = 23;
    unsafe { madvise(ptr as *mut c_void, len, MADV_POPULATE_WRITE) == 0 }
}

/// Drop the fully covered pages of an anonymous range (their contents are lost).
pub fn zap(ptr: *mut u8, len: usize) {
    let a = (ptr as usize + 4095) & !4095;
    let b = (ptr as usize + len) & !4095;
    if b > a {
        unsafe { madvise(a as *mut c_void, b - a, MADV_DONTNEED) };
    }
}

/// A read-only private mapping of a whole input file. Empty files map to an
/// empty slice. The mapping stays for the life of the process: pass 2 reads
/// lines again through it.
pub struct FileMap {
    pub ptr: *const u8,
    pub len: usize,
}
unsafe impl Send for FileMap {}
unsafe impl Sync for FileMap {}

impl FileMap {
    pub fn open(path: &std::path::Path) -> std::io::Result<FileMap> {
        let f = File::open(path)?;
        let len = f.metadata()?.len() as usize;
        if len == 0 {
            return Ok(FileMap { ptr: std::ptr::NonNull::<u8>::dangling().as_ptr(), len });
        }
        let p = unsafe { mmap(std::ptr::null_mut(), len, PROT_READ, MAP_PRIVATE, f.as_raw_fd(), 0) };
        if p as isize == -1 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(FileMap { ptr: p as *const u8, len })
    }

    #[inline]
    pub fn slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }

    /// Drop the page-table entries of fully covered pages in `[start, end)`; the
    /// pages stay in the page cache. Used at the end, to spread the unmapping
    /// over the worker threads.
    pub fn release(&self, start: usize, end: usize) {
        let a = (start + 4095) & !4095;
        let b = end & !4095;
        if b > a {
            unsafe { madvise(self.ptr.add(a) as *mut c_void, b - a, MADV_DONTNEED) };
        }
    }
}
