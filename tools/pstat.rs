//! pstat: user-mode cycles and instructions of a whole command (all threads),
//! plus wall, user and system time. A minimal `perf stat`.
//!
//! Instruction counts do not depend on machine load and user-mode cycles depend
//! on it far less than wall time does, so this is the tool for comparing two
//! binaries on a shared machine.
//!
//! Usage: pstat PROGRAM [ARGS...]
//! Prints one line to stderr: wall_ms user_ms sys_ms Mcycles Minstr maxrss_MB

use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_long, c_void};

extern "C" {
    fn fork() -> c_int;
    fn pipe(fds: *mut c_int) -> c_int;
    fn execvp(file: *const c_char, argv: *const *const c_char) -> c_int;
    fn wait4(pid: c_int, status: *mut c_int, options: c_int, rusage: *mut i64) -> c_int;
    fn read(fd: c_int, buf: *mut c_void, n: usize) -> isize;
    fn write(fd: c_int, buf: *const c_void, n: usize) -> isize;
    fn close(fd: c_int) -> c_int;
    fn syscall(num: c_long, ...) -> c_long;
    fn _exit(code: c_int) -> !;
}

fn open(config: u64, pid: c_int) -> c_int {
    let mut attr = [0u64; 8];
    attr[0] = 64u64 << 32; // PERF_TYPE_HARDWARE, size 64
    attr[1] = config;
    // disabled | inherit | exclude_kernel | exclude_hv | enable_on_exec
    attr[5] = 1 | (1 << 1) | (1 << 5) | (1 << 6) | (1 << 12);
    unsafe { syscall(298, attr.as_ptr(), pid, -1 as c_int, -1 as c_int, 0u64) as c_int }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: pstat PROGRAM [ARGS...]");
        std::process::exit(2);
    }
    let cargs: Vec<CString> = args.iter().map(|a| CString::new(a.as_str()).unwrap()).collect();
    let mut argv: Vec<*const c_char> = cargs.iter().map(|a| a.as_ptr()).collect();
    argv.push(std::ptr::null());
    unsafe {
        let mut go = [0 as c_int; 2];
        pipe(go.as_mut_ptr());
        let pid = fork();
        if pid == 0 {
            close(go[1]);
            let mut b = 0u8;
            read(go[0], &mut b as *mut u8 as *mut c_void, 1);
            execvp(argv[0], argv.as_ptr());
            _exit(127);
        }
        close(go[0]);
        let (cycles, instr) = (open(0, pid), open(1, pid));
        let start = std::time::Instant::now();
        write(go[1], b"x".as_ptr() as *const c_void, 1);
        let mut status = 0;
        let mut ru = [0i64; 18];
        wait4(pid, &mut status, 0, ru.as_mut_ptr());
        let wall = start.elapsed().as_secs_f64() * 1e3;
        let value = |fd: c_int| {
            let mut v = 0u64;
            if fd >= 0 {
                read(fd, &mut v as *mut u64 as *mut c_void, 8);
            }
            v
        };
        let ms = |sec: i64, usec: i64| sec as f64 * 1e3 + usec as f64 / 1e3;
        eprintln!(
            "{:.1} {:.1} {:.1} {:.1} {:.1} {:.0}",
            wall,
            ms(ru[0], ru[1]),
            ms(ru[2], ru[3]),
            value(cycles) as f64 / 1e6,
            value(instr) as f64 / 1e6,
            ru[4] as f64 / 1024.0
        );
        std::process::exit(if status & 0x7f == 0 { (status >> 8) & 0xff } else { 128 });
    }
}
