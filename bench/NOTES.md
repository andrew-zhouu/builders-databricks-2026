# Benchmark notes

All local numbers come from a shared development machine (i7-11700, 8 cores / 16
threads, WSL2) that other jobs kept at a load average of 15 to 50 for the whole
period. Absolute times are therefore pessimistic and noisy; only interleaved A/B
comparisons (`scripts/phase_ab.sh`, `scripts/sys_ab.sh`: variants alternate within one
run, min and median over many rounds) and the cache-sized micro-benchmark
(`scripts/bench.sh`: best-of-400 thread CPU time) are trustworthy, and only as relative
numbers. The scorer's times are the ones that count.

## Whole runs (8 threads)

| Data | Result | Conditions |
| --- | --- | --- |
| 1M dev | 0.11 to 0.20 s wall | load ~20 |
| 5M (seed 21) | 0.23 s best, 0.28 s median wall; 1.3 s user + 0.4 s sys | load ~16, 11 interleaved runs |
| 20M (seed 20) | 1.3 s wall; 7.6 s user + 1.1 s sys | load ~21, one run |

Single-threaded CPU per event is flat from 1M to 20M (about 265 to 350 ns under load),
so nothing in the pipeline degrades with size.

## Where the CPU goes (20M, thread CPU time)

parse events about 50%, parse clicks 12%, dedup 11%, sessions and attribution 16%,
writing user_daily 4%.

## Parser micro-benchmark (ns per line, best case, one thread)

| Step | events | clicks | links |
| --- | --- | --- | --- |
| scalar parser (whole-run figure, not the micro-benchmark) | ~250 | | |
| first SIMD version (AVX-512 masks) | 69 to 96 | 54 | 46 |
| AVX2 only, `pext` ID keys | 48 to 53 | 39 | 29 |
| key + parent in one table slot, form-specific timestamp parse | 57 to 61 | 42 | 26 |

(The last two rows were measured at different machine loads; they are equal within noise.)

## Decisions and their evidence

| Question | Finding | Decision |
| --- | --- | --- |
| AVX-512 or AVX2 | With AVX-512 enabled LLVM fused pairs of 256-bit compares into 512-bit ones. Light 512-bit code costs about 10% of core frequency on Cascade Lake and saves about 3% of instructions. Cannot be measured locally (Rocket Lake has no such penalty). | AVX2 only; the gate fails on any zmm or mask-register instruction. |
| Comparison sort vs radix in sessionization | sort section 32 ms -> 8 ms per 1M events | 4-pass LSD radix per partition |
| User -> person lookup | key and union-find parent in one 16-byte slot: one cache miss per row instead of two, and none in the dedup pass | adopted |
| Link phase | two-stage prefetch ring + loading products/campaigns on thread 0 meanwhile: 28.3 -> 20.3 ms median to "people resolved" (5M) | adopted |
| Software prefetch of the input (2 KB ahead) | 5M whole run, 21 rounds: median 288 to 317 ms with, 373 ms without | on |
| mmap vs pread into an L2-sized buffer | 5M, 15 rounds: 275/643 ms (min/median) vs 310/667 ms | mmap; pread kept behind `IO=read` |
| Transparent huge pages for the table and block pool | user time 1.23 -> 1.35 s without (5M) | on (`madvise`) |
| Releasing memory from all threads before exit | no measurable difference here (THP makes teardown cheap) | on; matters where huge pages are unavailable |
| products.csv load | sorting 50,000 category strings took 9 to 12 ms | dedupe first (about 5 ms), and moved off the serial path |
