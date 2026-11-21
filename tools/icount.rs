//! icount: exact instruction counts per address for one run of a program.
//!
//! Single-steps the child with ptrace and histograms the instruction pointer,
//! relative to the load address of the executable. Slow (a few microseconds
//! per instruction) but deterministic: unlike timings, the result does not
//! depend on what else the machine is doing, which makes it the right tool for
//! comparing variants of throughput-bound code on a busy machine.
//!
//! Usage: icount OUT_FILE PROGRAM [ARGS...]      (the program must be single-threaded)
//! Output: "hex_offset count" lines, then "total N".

use std::collections::HashMap;
use std::ffi::CString;
use std::io::Write;
use std::os::raw::{c_char, c_int, c_long, c_void};

extern "C" {
    fn fork() -> c_int;
    fn execvp(file: *const c_char, argv: *const *const c_char) -> c_int;
    fn ptrace(request: c_int, pid: c_int, addr: *mut c_void, data: *mut c_void) -> c_long;
    fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
    fn _exit(code: c_int) -> !;
}

const PTRACE_TRACEME: c_int = 0;
const PTRACE_PEEKUSER: c_int = 3;
const PTRACE_SINGLESTEP: c_int = 9;
/// Offset of `rip` in `struct user_regs_struct` on x86-64.
const RIP: usize = 16 * 8;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: icount OUT_FILE PROGRAM [ARGS...]");
        std::process::exit(2);
    }
    let cargs: Vec<CString> = args[2..].iter().map(|a| CString::new(a.as_str()).unwrap()).collect();
    let mut argv: Vec<*const c_char> = cargs.iter().map(|a| a.as_ptr()).collect();
    argv.push(std::ptr::null());
    let null = std::ptr::null_mut::<c_void>();
    unsafe {
        let pid = fork();
        if pid == 0 {
            ptrace(PTRACE_TRACEME, 0, null, null);
            execvp(argv[0], argv.as_ptr());
            _exit(127);
        }
        let mut status = 0;
        waitpid(pid, &mut status, 0);
        // The executable is mapped now: its first mapping gives the load address.
        let exe = std::fs::canonicalize(&args[2]).unwrap();
        let maps = std::fs::read_to_string(format!("/proc/{pid}/maps")).unwrap();
        let base = maps
            .lines()
            .find(|l| l.ends_with(exe.to_str().unwrap()))
            .and_then(|l| u64::from_str_radix(l.split('-').next().unwrap(), 16).ok())
            .unwrap_or(0);
        // Non-PIE executables are linked at their load address already.
        let base = if base >= 0x1000_0000 { base } else { 0 };
        let mut hist: HashMap<u64, u64> = HashMap::new();
        let mut total = 0u64;
        loop {
            let rip = ptrace(PTRACE_PEEKUSER, pid, RIP as *mut c_void, null) as u64;
            *hist.entry(rip.wrapping_sub(base)).or_insert(0) += 1;
            total += 1;
            if ptrace(PTRACE_SINGLESTEP, pid, null, null) < 0 {
                break;
            }
            if waitpid(pid, &mut status, 0) < 0 || (status & 0xff) != 0x7f {
                break; // exited or killed
            }
        }
        let mut rows: Vec<(u64, u64)> = hist.into_iter().collect();
        rows.sort_unstable();
        let mut out = std::io::BufWriter::new(std::fs::File::create(&args[1]).unwrap());
        for (addr, count) in rows {
            writeln!(out, "{addr:x} {count}").unwrap();
        }
        writeln!(out, "total {total}").unwrap();
        eprintln!("icount: {total} instructions, exit status {:#x}", status);
    }
}
