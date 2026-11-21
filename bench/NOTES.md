# Benchmark notes

## After the first official score (0.85 s)

Method. Other jobs on the machine make wall time unreliable, so comparisons are
interleaved (`scripts/abx.sh`) and read off hardware counters where possible:
instructions are deterministic and user-mode cycles depend on load far less than wall
time. `PIN=0-7` pins 8 threads to 4 cores x 2 hyperthreads, the scorer's shape; in
that configuration the scored binary takes 0.69 s (best) to 0.85 s (median) on the 20M
set, matching its official 0.85 s.

Profile of the scored design, 5M events (`scripts/cyc.sh`, `scripts/prof.sh`):

| Section | instr/row | cycles/row, 1 thread | notes |
| --- | --- | --- | --- |
| parse events | 512 to 515 | 200 to 270 | 160 to 170 in the cache-resident micro-benchmark |
| parse clicks | 353 | 160 to 195 | |
| dedup gather / table / emit | 0.2 / 31 / 36 | 7 / 27 / 20 | |
| sessions gather / people | 53 / 88 | 42 / 56 to 77 | before this round |
| sort clicks and purchases | 142 | 62 | before this round |
| links | 238 | 180 to 230 | 3.6 cache misses per line |

Kernel time on the development machine (5M, 8 threads): about 0.5 s of 2.0 s CPU, the
same within noise for mmap or pread, with or without huge pages, with or without
releasing input pages or the parallel teardown (`scripts/sysvar.sh`, medians of 7).
On the 1M set it is 58 ms with 4 threads and 130 to 165 ms with 8.

Changes and their measured effect (5M, 1 thread, min of 3 unless noted):

| Change | Effect |
| --- | --- |
| Prefetch distance for user-table slots 8 -> 16 -> 32 lines | parse events 214 -> 206 -> 206 cycles/row at 5M; at 20M (8 threads) total user cycles 13237 -> 13069 -> 12941 M (min) |
| User table slots per ID 2 -> 1.25 | worse: parse events 214 -> 225 cycles/row, +8 instr/row |
| User table slots per ID 2 -> 3 or 4 | parse events -3 to -6%, total unchanged (more partitions downstream) |
| Sessions: 2-pass sort by (person, 2^20 ms slot) instead of 4-pass by (person, ts) | gather 42 -> 20 cycles/row, people 56 -> 65; net about -14 cycles per event |
| Clicks and purchases: counting sort per person instead of comparison sort | 142 -> 109 instr/row, cycles unchanged within noise |
| Input prefetch distance 1 KB / 2 KB / 4 KB | no difference |

Candidate (`dev`) against the scored binary (`scripts/abx.sh`, interleaved, 8 threads
pinned to 0-7, min / median):

| Data, rounds, load | wall ms scored | wall ms candidate | user Mcycles scored | user Mcycles candidate | Minstr scored / candidate |
| --- | --- | --- | --- | --- | --- |
| 20M, 7, about 4 | 688 / 849 | 668 / 785 | 13622 / 14468 | 12941 / 13374 | 17854 / 17482 |
| 20M, 9, about 11 | 796 / 933 | 858 / 934 | 13674 / 14624 | 13912 / 14724 | 17854 / 17482 |
| 20M, 9, about 12 | 815 / 897 | 764 / 893 | 13351 / 14201 | 13332 / 14218 | 17853 / 17482 |
| 5M, 11, about 7 | 206 / 222 | 204 / 211 | 2688 / 2870 | 2646 / 2780 | 4563 / 4471 |
| 5M, 11, about 13 | 223 / 330 | 216 / 308 | 3328 / 3476 | 3199 / 3411 | 4562 / 4470 |
| 1M, 15, about 13 | 150 / 196 | 148 / 192 | 686 / 714 | 645 / 703 | 955 / 937 |

Reading: 2.1% fewer instructions, always. Fewer cycles on a quiet machine (5 to 8% at
20M in the one quiet series), nothing measurable under load. Not promoted to `main`.

In the container with the scorer's flags (`scripts/docker_ab.sh`, 20M, 5 interleaved
rounds, load about 26, time to done min / median): default 1879 / 2254 ms; no huge
pages 2432 / 3012; 4 threads 2505 / 2615; no input release and no parallel teardown
1366 / 2036 with kernel time 1294 / 1301 ms against 1538 / 1981. The last one is an
open lead, not yet a decision (see HANDOFF.md).

A lesson about the method: single-thread cycle counts (`scripts/cyc.sh`) showed these
changes as worth 4 to 5%, and they mostly evaporated at 8 threads on 4 cores. Changes
have to be judged in the scorer's shape (`PIN=0-7`, 20M), and anything under about 5%
needs a quiet machine and many rounds to see at all.

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
