//! Diagnostic mode (`SS_DIAG` set): report what kind of machine and kernel this
//! is, run the pipeline several times with its profile on, and then exit with
//! a failure code ON PURPOSE.
//!
//! It exists to learn about an environment whose logs are only shown for a
//! failed run. It is never active unless `SS_DIAG` is in the environment; the
//! submission image does not set it.

use std::time::Instant;

fn show(path: &str, keep: &dyn Fn(&str) -> bool, max: usize) {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            for line in text.lines().filter(|l| keep(l)).take(max) {
                let line: String = line.chars().take(300).collect();
                eprintln!("  {path}: {line}");
            }
        }
        Err(e) => eprintln!("  {path}: <{e}>"),
    }
}

fn dir_summary(path: &std::path::Path) -> (usize, u64, u64) {
    let (mut n, mut bytes, mut largest) = (0, 0, 0);
    if let Ok(rd) = std::fs::read_dir(path) {
        for e in rd.flatten() {
            if let Ok(m) = e.metadata() {
                if m.is_file() {
                    n += 1;
                    bytes += m.len();
                    largest = largest.max(m.len());
                }
            }
        }
    }
    (n, bytes, largest)
}

pub fn run(data_dir: &std::path::Path) -> ! {
    let all = |_: &str| true;
    // Measurements worth repeating at the end of the log, in case only one
    // end of it is shown.
    #[allow(unused_mut)]
    let mut key: Vec<String> = Vec::new();
    // First of all, before anything reads the input: is it in the page cache?
    #[cfg(feature = "probe")]
    crate::probe::page_cache(data_dir, &mut key);
    eprintln!("== diag: environment");
    show("/proc/cmdline", &all, 1);
    show("/proc/version", &all, 1);
    show("/proc/cpuinfo", &|l| l.starts_with("model name") || l.starts_with("cpu MHz") || l.starts_with("cache size"), 3);
    if let Ok(text) = std::fs::read_to_string("/proc/cpuinfo") {
        let n = text.lines().filter(|l| l.starts_with("processor")).count();
        let flags = text.lines().find(|l| l.starts_with("flags")).unwrap_or("");
        let has: Vec<&str> = ["avx2", "bmi2", "avx512f", "avx512bw", "avx512vbmi", "avx512vbmi2", "pdpe1gb", "hypervisor", "erms", "fsrm"]
            .into_iter()
            .filter(|f| flags.split_whitespace().any(|x| x == *f))
            .collect();
        eprintln!("  cpuinfo: {n} processors; flags of interest: {}", has.join(" "));
    }
    for i in 0..4 {
        let base = format!("/sys/devices/system/cpu/cpu0/cache/index{i}");
        let get = |f: &str| std::fs::read_to_string(format!("{base}/{f}")).unwrap_or_default().trim().to_string();
        eprintln!("  cache index{i}: L{} {} {} shared by cpus {}", get("level"), get("type"), get("size"), get("shared_cpu_list"));
    }
    for cpu in 0..16 {
        if let Ok(s) = std::fs::read_to_string(format!("/sys/devices/system/cpu/cpu{cpu}/topology/thread_siblings_list")) {
            eprintln!("  cpu{cpu} thread siblings: {}", s.trim());
        }
    }
    show("/sys/kernel/mm/transparent_hugepage/enabled", &all, 1);
    show("/sys/kernel/mm/transparent_hugepage/defrag", &all, 1);
    show("/sys/kernel/mm/transparent_hugepage/shmem_enabled", &all, 1);
    show(
        "/proc/meminfo",
        &|l| ["MemTotal", "MemFree", "MemAvailable", "Cached", "AnonHugePages", "HugePages_Total", "Hugepagesize"].iter().any(|k| l.starts_with(k)),
        8,
    );
    show("/proc/self/status", &|l| l.starts_with("Cpus_allowed_list") || l.starts_with("Mems_allowed_list"), 2);
    show("/proc/self/cgroup", &all, 3);
    for f in ["cpu.max", "cpuset.cpus.effective", "memory.max", "memory.high", "memory.swap.max", "pids.max"] {
        show(&format!("/sys/fs/cgroup/{f}"), &all, 1);
    }
    show("/proc/mounts", &|l| [" / ", " /data ", " /output ", " /scratch ", " /tmp "].iter().any(|m| l.contains(m)), 6);
    show("/proc/sys/kernel/perf_event_paranoid", &all, 1);
    show("/proc/sys/vm/overcommit_memory", &all, 1);
    show("/proc/loadavg", &all, 1);
    let s = crate::prof::snap();
    eprintln!("  perf counters readable: {}", s.c.iter().any(|&c| c != 0));
    eprintln!(
        "  env: NCPU={:?} MEM_GB={:?} DATA_DIR={:?} OUTPUT_DIR={:?} SCRATCH_DIR={:?}",
        std::env::var("NCPU").ok(),
        std::env::var("MEM_GB").ok(),
        std::env::var("DATA_DIR").ok(),
        std::env::var("OUTPUT_DIR").ok(),
        std::env::var("SCRATCH_DIR").ok()
    );
    for dir in ["events", "clicks", "links"] {
        let (n, bytes, largest) = dir_summary(&data_dir.join(dir));
        eprintln!("  {dir}: {n} files, {bytes} bytes, largest {largest}");
    }

    #[cfg(feature = "probe")]
    {
        let threads = std::env::var("NCPU")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .or_else(|| std::thread::available_parallelism().ok().map(|n| n.get()))
            .unwrap_or(8)
            .clamp(1, 64);
        crate::probe::run(data_dir, threads, &mut key);
    }
    eprintln!("== diag: probes");
    for line in &key {
        eprintln!("  {line}");
    }

    // The pipeline itself, several times: the first run shows a cold start (if
    // the page cache is cold), the rest the steady state and two variants.
    let exe = std::env::current_exe().unwrap_or_else(|_| "/proc/self/exe".into());
    let runs: [(&str, &[(&str, &str)]); 5] = [
        ("run 1 (default)", &[]),
        ("run 2 (default)", &[]),
        ("run 3 (default)", &[]),
        ("no huge pages", &[("SS_NOHUGE", "1")]),
        ("4 threads", &[("NCPU", "4")]),
    ];
    for (label, envs) in runs {
        eprintln!("== diag: {label}");
        let mut cmd = std::process::Command::new(&exe);
        cmd.args(std::env::args().skip(1)).env_remove("SS_DIAG").env("SS_TIMING", "1");
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let t = Instant::now();
        let status = cmd.status();
        let line = format!("{label}: {:.1} ms wall including spawn and exit, {:?}", t.elapsed().as_secs_f64() * 1e3, status.map(|s| s.code()));
        eprintln!("== diag: {line}");
        key.push(line);
    }
    eprintln!("== diag: summary");
    // Was the container held back by its CPU quota while all this ran?
    for f in ["/sys/fs/cgroup/cpu.stat", "/sys/fs/cgroup/cpu/cpu.stat", "/sys/fs/cgroup/cpu,cpuacct/cpu.stat"] {
        if std::path::Path::new(f).exists() {
            show(f, &|l| l.contains("throttled") || l.starts_with("nr_periods") || l.starts_with("usage_usec"), 6);
        }
    }
    show("/sys/fs/cgroup/cpu.max", &all, 1);
    show("/proc/pressure/cpu", &all, 2);
    if let Ok(text) = std::fs::read_to_string("/proc/cpuinfo") {
        eprintln!("  processors visible: {}", text.lines().filter(|l| l.starts_with("processor")).count());
    }
    show("/proc/cpuinfo", &|l| l.starts_with("model name"), 1);
    show("/proc/version", &all, 1);
    show("/sys/kernel/mm/transparent_hugepage/enabled", &all, 1);
    for line in &key {
        eprintln!("  {line}");
    }
    println!("diagnostic image: the report is on stderr; exiting with a failure code on purpose");
    eprintln!("== diag: done; exiting with a failure code on purpose so that this log is shown");
    std::process::exit(1)
}
