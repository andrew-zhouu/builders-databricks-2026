# Handoff: where the work stands

Last updated 2026-11-21, about 11:45 UTC (paused on request; resume from here).

## Checkpoint of the EC2 round (read this first)

Everything below this section predates the round; where they disagree, this wins.

### Commits

| What | Commit | Status |
| --- | --- | --- |
| Officially scored, **0.85 s** | `f1459cc` (tag `scored-0.85`) | Untouched. Still the only thing to submit for a score. |
| GitHub `main` | `3201a41` | Untouched this round. |
| `dev`, first version of the new architecture ("dedup by exception") | `81c1f75` | Fixtures, dev answer, 60 fuzz seeds x 8 modes, six seeds, 5M / 20M / 20M / 60M against the plain implementation: all pass. Same speed as scored. |
| `dev`, staged event parser | `80002d6` | Fixtures, dev answer, 40 fuzz seeds x 8 modes pass. The state just before it (staged parser + faster settle, without the leaner decode loop and the cursor writers) passed the full check: 150 fuzz seeds x 8 modes, six seeds fast and slow, 5M / 20M / 20M / 60M. |
| `dev` tip: the above + staged click parser | this commit | Fixtures, dev answer, 40 fuzz seeds x 8 modes pass (`scripts/remote/cycle.sh`). **Not** yet through the full check or `scripts/validate.sh` (Docker). |

Nothing on `dev` is a submission candidate yet: the best measured gain is 2.5%, and no
`dev` commit of this round has been through the Docker gate.

### Latest A/B against the scored binary

EC2 m6i.2xlarge (Ice Lake, 4 cores x 2 threads), 20M events, 8 threads, wall ms,
min / median of 5 interleaved rounds (`scripts/abx.sh m20 5 SSBIN=scored X=1`):

| Build | scored | build |
| --- | --- | --- |
| `81c1f75` | 559 / 562 | 565 / 567 |
| `80002d6` | 562 / 564 | 549 / 550 |
| `dev` tip | 559 / 564 | 546 / 549 |

No 60M A/B of the new builds yet (scored: 1703 / 1709 ms). CPU ms by section at the
`dev` tip, 20M: parse events 2028 (96.9 ns/row), parse clicks 418, dedup gather 26 +
scan 132 + settle 334, sessions gather 287 + sort 130 + people 624, links 91; kernel
about 500 of 4200.

### What was learned

* **Everything is instruction-bound, with no hot spot.** Exact counts per event row
  (valgrind on the dev set, `scripts/remote/cgasm.sh`): 867 in all; event split 155,
  event decode 217, event emit 60, dedup + sessions 236, clicks 78, pair sort 18. The
  time profile is equally flat. 8 threads on 4 cores run at about 1.3 instructions per
  cycle each, so wall time follows the instruction count.
* The leader's 0.36 s corresponds to roughly half our instructions per line. Hand
  estimates say that is reachable only with assembly-level care in every loop (the
  compiler emits about 1.5x what a careful hand count predicts: flag plumbing,
  reloaded constants, spills) or with 512-bit masks.
* What helped: validation as integers OR-ed together instead of booleans (decode 242
  -> 217), write pointers instead of fill counts in the partition writers, no calls
  inside the hot loops, F1 timestamps through a day-of-month table, radix grouping and
  O(1) file lookup in settle (380 -> 338 ms).
* What did not: staging by itself (same count as the fused loop); the staged click
  parser (81 against 79.5 ns/row, kept because it is not slower in the A/B); a second
  input prefetch per line; input prefetch distance 512 to 4096 (none at all costs 18%
  of the parse); trace partitions of 1.2 MB instead of 2.4 MB (neutral; 0.6 MB is
  worse); the settle prefetch distance, 4 to 128 lines (no effect at all, so settle is
  not waiting for memory: find out where its 240 ns per re-read line go with
  `scripts/remote/cgsrc.sh sessions::worker pass2.rs`, written but not yet run).
* Dedup by exception and the old partition-then-hash dedup cost the same. The
  generator scatters the copies of an event over random files, so nothing local helps.
* Probes on EC2 (`scripts/remote/probe.sh dev`), for comparison with the scorer once
  the diagnostic image is submitted: clock 3.30 GHz scalar, 3.22 with 512-bit
  instructions in between; first touch of 256 MB 35 ms with huge pages, 93 ms without;
  random read 108 ns (125 ns on 4K pages); copy 6.9 GB/s; map-and-touch of the input
  45 ns/page.

### Next steps, in order

1. **Diagnostic image.** The probes are in (`fast/src/probe.rs`, cargo feature
   `probe`), and `scripts/mkdiag.sh` now builds with them, but the `diag` branch has
   NOT been rebuilt: it is still the old `a55f0bc`. To do: `scripts/mkdiag.sh dev`,
   `scripts/diagtest.sh` on EC2, push `diag`, then one submission of its SHA. It tells
   us the scorer's CPU, whether 512-bit instructions slow its clock, huge pages, page
   fault and memory costs, and whether the input is in the page cache.
2. Keep cutting instructions with `cgasm.sh` / `cgshow.sh`: decode (217), split (155),
   emit (60, a bucketed branch-free table probe would save about 5), clicks.
3. Settle without re-reading lines: 16-byte traces holding the exact ID key, person,
   timestamp and ingest delta (design in the session notes; estimated 3 to 4%).
4. 512-bit separator masks behind a run-time switch, if the diagnostic says the clock
   penalty is small (about 30 instructions per event line).
5. Before any candidate: `scripts/remote/fullcheck.sh`, a 60M A/B, `scripts/validate.sh`.

### EC2 box

Address, user and key are in `.local/remote.env` (untracked). Nothing is running there;
it was left idle with everything in place.

* `~/db`: the working tree (rsync of this one, no `.git` history of its own beyond the
  first clone), toolchain in `~/db/.toolchain`, scored binary `~/db/bin/scored`.
* Data: `~/db/data/{dev,m5,m20,n20,m60,s2_200000..s7_200000}`; reference answers
  `~/db/expected/*.slow`; job logs `~/db/out/logs`.
* From WSL: `scripts/remote.sh push` (sync sources), then
  `scripts/remote.sh run 'bash scripts/remote/cycle.sh 40 900'` (build, fixtures, dev,
  40 fuzz seeds from 900, A/B against scored, section profile);
  `scripts/remote.sh tmux NAME 'bash scripts/remote/fullcheck.sh'` and
  `scripts/remote.sh log NAME` for the long check; `scripts/remote/status.sh` shows
  load and jobs. Other tools in `scripts/remote/`: `cgasm.sh`, `cgshow.sh`, `cgfn.sh`
  (instructions per line of assembly), `lineprof.sh` (time by source line), `knobs.sh`,
  `sweep.sh`, `probe.sh`.
* From PowerShell, a command with `|` or `$` inside nested quotes gets mangled: put it
  in a script under `scripts/remote/` and run that.

---

The sections below are from before the EC2 round. `bench/NOTES.md` has the
measurements behind them.

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
