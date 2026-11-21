# Handoff: where the work stands

Last updated 2026-11-21. Read this first when resuming. `bench/NOTES.md` has the
measurements behind every statement here.

## Commits that matter

| What | Commit | Where |
| --- | --- | --- |
| First official submission: **0.85 s**, 23rd of 130 (leader 0.59 s) | `f1459cc` | tag `scored-0.85`; first parent of the `main` tip. The fallback: never rewritten. |
| Tip of `main`, the default branch | `3201a41` | GitHub merge of pull request #1 (`dev` as of `9fb62dd` into `main`), made 2026-11-21 02:12 EST. Same tree as `9fb62dd`. |
| Second candidate: validated, **not promoted** (no demonstrated advantage) | `5aa33bc` | on `dev` (later `dev` commits touch only scripts and documents); see "Second candidate" below |
| Diagnostic image (fails on purpose, cannot score) | tip of `diag` | see "The diagnostic image" |

**Submit by commit SHA, never with the commit field empty.** An empty field scores the
newest commit on the default branch, which since the merge of pull request #1 is
`3201a41`, not the scored commit. `3201a41` runs the same default path as `f1459cc`
(it adds an optional pread mode, a faster hasher in the fallback, a graceful
block-pool reservation and scripts), and it has been put through `scripts/validate.sh`
after the fact, but it has never been scored.

A commit should only become the one we submit after it has passed
`scripts/validate.sh` (clean export, no-cache Docker build, every correctness check
run on the image's own binary, 5M and 20M scale checks, in-container runs with the
scorer's flags) **and** has a measured advantage over the scored one. To move `main`
again without rewriting anything, merge `main` into `dev` first (a no-op for the code:
`dev` already contains everything in it) and then fast-forward `main`, or open a pull
request from `dev`.

## What the official score told us

The local run that reproduces it: the scored binary on the 20M-event set, 8 threads
pinned to 4 cores x 2 hyperthreads (`PIN=0-7 scripts/abx.sh m20 7 SSBIN=scored`), takes
0.69 s at best and 0.85 s at the median. So the scorer's 0.85 s is essentially all our
own compute; there is no large fixed container overhead hiding in it, and a 31% gap to
the leader has to come out of the pipeline itself.

## Where the time goes (5M events, hardware counters)

Instructions are deterministic; of 4.5 G per run:

| Section | instr/row | share of instructions | cycles/row, 1 thread |
| --- | --- | --- | --- |
| parse events | 512 | 61% | 200 to 215 |
| parse clicks | 353 | 10% | 160 |
| dedup (gather, table, emit) | 66 | 8% | 50 |
| sessions (group, walk) | 129 | 14% | 85 |
| sort clicks and purchases | 109 | 3% | 54 |
| links | 238 | 1% | 180 |

* **The event parser is the target**: 72% of all instructions are CSV parsing. The
  fast path is already branch-free AVX2; what is left is the sheer amount of work per
  line (two 64-byte windows, quote validation, two timestamp parses, ID packing, the
  prefetch ring). A redesign aimed at about 300 instructions per line would cut total
  work by roughly a quarter. This is the main remaining opportunity and it is a
  rewrite of `event_fast`, not a tweak. `scripts/benchprof.sh` and
  `tools/linecount.py` show where its instructions are.
* At 8 threads on 4 cores (the scorer's shape) the same work costs about 25% more
  cycles than single-threaded: hyperthreads share a core, so instruction count is the
  thing to cut. Memory-stall fixes show up at 20M (the user table no longer fits in
  cache) more than at 5M.
* **Kernel time is 20 to 40% of CPU time on the development machine** and none of the
  switches (mmap or pread, huge pages or not, releasing input pages or not) changes
  it; it also grows with the thread count (1M set: 58 ms with 4 threads, 130 to 165 ms
  with 8). Whether the scorer's kernel behaves the same is unknown and cannot be
  measured locally. This is the question the diagnostic image answers.

## Second candidate

Changes since the scored commit that affect the default path:

1. Sessionization groups events by person and 2^20 ms slot with a 2-pass radix sort
   instead of fully sorting timestamps with 4 passes (`pass3.rs`; the argument for
   exactness is in the comment at the top of `run` and in the README).
2. Clicks and purchases are ordered by a counting sort per person instead of a
   comparison sort.
3. The user-table slot of an event or click is prefetched 32 lines ahead (was 8) and
   its home position is computed once.

And changes that do not affect a normal run: every experiment switch is now prefixed
`SS_` (no stray environment variable can alter a run), the block-pool reservation
degrades gracefully if the kernel refuses it, the plain fallback uses a faster hasher,
an optional pread input mode, the perf-counter profiler, the diagnostic mode.

It is correct: it passes the full validation (36 fixtures two ways, dev answer two
ways, six generated seeds, fuzzing in ten run modes, 5M and 20M scale checks, and the
same inside the image with the scorer's flags).

It is **not demonstrably faster**. Against the scored binary, interleaved rounds, 8
threads pinned to 4 cores x 2 hyperthreads, min / median:

| Series (20M events) | machine load | wall ms scored | wall ms candidate | user Mcycles scored | user Mcycles candidate |
| --- | --- | --- | --- | --- | --- |
| 1, 7 rounds | about 4 | 688 / 849 | 668 / 785 | 13622 / 14468 | 12941 / 13374 |
| 2, 9 rounds | about 11 | 796 / 933 | 858 / 934 | 13674 / 14624 | 13912 / 14724 |
| 3, 9 rounds | about 12 | 815 / 897 | 764 / 893 | 13351 / 14201 | 13332 / 14218 |

On the quietest series the candidate used 5 to 8% fewer cycles; on the two loaded
ones there is no difference. At 5M and 1M it is 2 to 6% ahead on best-case cycles and
level on medians. The only load-independent number is instructions: 17.48 G against
17.85 G at 20M, 2.1% fewer. A fair summary is "somewhere between no change and 5%
better", which is not enough to move `main`, and far below the 20% machine-to-machine
variation the organisers quote for practice scores.

The candidate can still be scored without moving `main`: the submission form takes a
commit SHA, and this commit is fully validated. Its result would be one more sample
of scorer noise around 0.8 s rather than evidence for or against these changes.

## The diagnostic image

Branch `diag` is the candidate plus one Dockerfile line (`ENV SS_DIAG=1`;
`scripts/mkdiag.sh` makes it). Submitted,
it prints the scorer's CPU model, cache sizes, hyperthread layout, huge-page settings,
cgroup limits, mounts, and seven profiled runs of the dev set (three default runs, then
without huge pages, with pread instead of mmap, without releasing memory, and with 4
threads), each with user time, kernel time and page faults per section. Then it exits
with code 1 **on purpose**: the rules show logs only for a submission that fails the
dev check, and a rejected submission never changes the best score. It costs one of the
two submissions per hour and nothing else.

What it would settle: whether transparent huge pages are available (the design leans
on them for the user table and the partition pool), whether kernel time is as large
there as here, and whether fewer threads than hyperthreads would be faster.

## An open lead: releasing memory may cost more than it saves

Inside the container with the scorer's flags (`scripts/docker_ab.sh`, 20M events, 5
interleaved rounds, min / median, on a very busy machine):

| Variant | time to done, ms | kernel ms |
| --- | --- | --- |
| default | 1879 / 2254 | 1538 / 1981 |
| `SS_NORELEASE=1,SS_NOZAP=1` (keep input pages mapped, no parallel teardown) | 1366 / 2036 | 1294 / 1301 |
| `SS_NOHUGE=1` | 2432 / 3012 | 1794 / 2317 |
| `NCPU=4` | 2505 / 2615 | 1319 / 1828 |

So huge pages and 8 threads are right, but the per-chunk `madvise` calls that release
parsed input (and the parallel teardown at the end) may be a net loss: every one of
them interrupts all the other threads to flush their TLBs. Outside the container on a
quieter machine at 5M the same switch made no measurable difference, so this needs a
20M series on a quiet machine, with the process exit included, before the default
changes. The diagnostic log includes this variant on the scorer's own kernel.

## Next steps, in order of expected value

1. Submit the diagnostic image once and read its log (decides items 3 and 4, and the
   open lead above).
2. Rewrite `event_fast` for instruction count (target about 300 per line), then
   `click_fast` the same way.
3. If the scorer shows large kernel time: test thread counts below 8 and pread there.
4. If huge pages are unavailable there: shrink the randomly accessed memory instead
   (smaller table slots, fewer partitions).
5. Dedup (`pass2.rs`): keep a hash tag in the table so that probing does not touch the
   records, use 16-bit indexes, or pre-filter unique IDs with a counting bitmap.
   Worth about 3 to 5%.
6. `pass3::person`: the per-thread campaign counters are a 1 MB array hit at random;
   split the hot assist counters from the rest. The product-category table can be one
   byte per product instead of four. A percent or two each.
7. Before the final (60M events): the write-combining buffers grow with the partition
   counts (2048 event partitions, about 2800 person partitions, 64 bytes each, several
   writers per thread) and will crowd a 1 MB L2. Consider 13 or 14 bits of person per
   partition (`LOCAL_BITS`). No 60M run has been possible on the development machine.

## Things tried that did not help

* A smaller user table (load 0.5 instead of 0.3): more probing, slower. A larger one:
  no measurable change.
* pread into an L2-sized buffer instead of mmap: same user-mode cost, more kernel time
  locally. Kept behind `SS_IO=read`.
* Input prefetch distances other than 2 KB; not releasing input pages; no parallel
  teardown: no measurable change.

## Working on this machine

* It is shared with other jobs. Check `scripts/load.sh` before measuring, compare only
  with interleaved runs (`scripts/abx.sh`, `scripts/cyc.sh`), and prefer user-mode
  cycles and instructions to wall time.
* The 20M set needs about 3 GB free. When the Windows host runs out of memory, `wsl`
  commands time out; wait and retry.
* `scripts/gate.sh` before keeping any change; `scripts/validate.sh REV` and
  `scripts/publish.sh BRANCH` before `main` moves.
