# Handoff: where the work stands

Last updated 2026-11-22, 08:50 UTC. Deadline: 2026-11-22 16:00 UTC (11:00 ET).

## 22 November, 08:50 UTC: development stopped here; notes for a new implementation

This line of work is now the validated fallback. Nothing of it runs on the EC2 box.

- **Submit / fallback:** candidate v5 = `37023b2caeeac3b0c363538e87a09cd46cbd2b8c` (tag
  `cand-v5`), full gate passed (`CANDIDATE-OK`), not yet scored officially. Official best so
  far: v4 = `ca5f5f91958909da54b1acff50c570b957e139bc` (tag `cand-v4`), 0.6745 s.
- v5 against v4: a 4-lane 256-bit parser instance and a start-up timing that picks the
  fastest instance in steady state. On EC2 (Ice Lake) at 20M: AVX2 565 ms, 256-bit 510 ms,
  512-bit without VBMI2 454 ms, with VBMI2 439 ms. What the scorer picks is unknown.
  Official is about 1.067 x (EC2 program time + 0.07 s) for the AVX2 path.

### The data (measured, scorer version 5)

- Lines are fully shuffled: the same user follows itself on 0.1% of lines; ingest order is
  random. File sizes are very uneven (20M: 87 event files, the largest 344 MB).
- Files are not shards: 99% of event rows belong to users spread over several files, and
  34,393 of 35,834 repeated event IDs (dev) have their copies in different files. So
  de-duplication has to be global, and no file can be finished on its own.
- One distinct ID in eight is a letter plus 32 hex digits (33 bytes). IDs come out of a
  keyed bijection: they cannot be turned back into indices (direct placement is dead).
- 20M: 667,450 people; 234,692 have no click, 296,860 have 1 to 8. 4,767 people have so
  many sessions and clicks that their (session, click in window) pairs number 62.6 million
  of 78.6 million in all: assists need a way that is not per pair for them.

### Rules that cost us bugs

- IDs compare as exact bytes, never normalised. A short ID may contain NUL: a hash must
  cover the length. Ties between clicks at one time go to the smallest `click_id` in byte
  order, across the regular and the long forms.
- De-duplicate by `event_id`, keeping the smallest `ingest_ts`; a copy may name another
  user, so de-duplicating inside a person's partition alone is not exact.
- Everything else as in the rules file; the 36 fixtures, `tools/cmp.py` (strings as bytes)
  and the fuzzers on branch `exp-tests` (`FUZZ_IDS=long`, `mutate.py`, `tssweep.py`) check
  them. `scripts/cand.sh` is the whole gate (about 65 minutes).

### Where v5's time goes (EC2, 20M, 256-bit instance, 516 ms wall; by knocking parts out)

| part | ms of wall |
| --- | --- |
| links, union-find, resolve | about 37 |
| event files: split, decode, user lookup, trace out | about 230 |
| click files | about 70 |
| de-duplication (pass 2) | about 43 |
| sessions: gather and sort | about 40 |
| sessions: walk (click attribution 40, output rows 21, purchases 7, names 2) | 78 |
| writing `user_daily` | 7 |
| long-ID resolution in the parser | 13 |
| end of run | about 15 |

CPU: 3.5 s user, 0.48 s system. The kernel's share is page faults on the mapped input and
zeroing about 460 MB of fresh pages (7 to 9% of CPU).

### Tried and measured, no gain (do not repeat)

- Session walk: a vector compare for people with up to 8 clicks (511/515 ms against
  510/514: noise), and assists by one sweep over clicks and sessions in place of the
  per-session scan (516/519: none). The cost of click attribution is not in those loops;
  it was not found before work stopped.
- Memory: pre-faulting the pool from a helper thread, `MADV_POPULATE_READ` per chunk
  (1.4% slower), `MADV_SEQUENTIAL` on the input, thread pinning, a smaller long-ID table.
- Per-campaign counter cache misses (at most 1.3%), 32-byte branch alignment (slower).
- Earlier in the week: a hash-table de-duplication in place of partitions and a second
  pipeline layout, within 4% of each other.
- 512-bit code on the scorer (Cascade Lake class, no VBMI2): our probe measured it at
  105% or more of the AVX2 parser in short bursts; v5 times it in steady state instead.

## Scorer version 5 (22 November, about 02:20 UTC): read this before anything else

The organisers changed the generator and re-scored every entry ("Scorer version
shopping-sessions-5"). Our scored commit `42b0f35` went from 0.62 s to **3.6747 s**; the
leader from 0.27 s to 0.7257 s. **Every time and every data set mentioned further down
is from version 4** and only of historical interest.

What changed (rules line on IDs, `generator/src/text.rs`, `user.rs`, `plan.rs`):

* One in eight distinct `user_id`, `event_id` and `click_id` values is the prefix letter
  and **32 lowercase hexadecimal digits** (33 bytes); the rest are as before (prefix and
  1 to 8 base-36 characters). Lines are about 8% longer on average.
* IDs go through a keyed 6-round Feistel network with a 256-bit key: the number behind
  an ID cannot be recovered. The index experiment (`exp-index`, `exp-wix`: decoding the
  event ID to the event's place among all events, placement instead of dedup, session
  intervals) is **dead**, and it is very likely what the old leaders were doing.
* The final: at least the 30 fastest entries are retimed 9 times in interleaved rounds
  across 3 fresh final data sets on one idle machine; the mean of the per-set medians
  is the score. A scored repository may now be made private.

Why we were 5.9 times slower: an ID over 9 bytes was "irregular": the line left the fast
path (23% of event and click lines) and long user and click IDs were interned behind one
mutex.

What was done (branch `exp-v5`, from the wide-parser branch without any index code):

* `ids.rs`: three kinds of key besides the regular one. Long user IDs (`u` + 32 hex) sit
  in a lock-free table of 128-bit values (`LongIds`), key = its slot; click IDs that are
  not regular are keyed by the address of their bytes in the mapped input (they are
  only ever compared, on a tie in `ts`); the mutex dictionary remains for everything
  else (upper case and the like, about 1,000 IDs in 20M events). `KeySpace::less` orders
  long against long by value and regular against long by the first eight characters.
  Event IDs of 33 bytes have a hash of their own (`hash33`).
* `parse.rs`: the line splitters use three 64-byte windows (an ID and its comma; the
  next field or two; the rest), so lines up to about 190 bytes stay on the fast path;
  the decode stage hashes 33-byte event IDs and flags 33-byte user IDs, and a small
  loop between decode and emit (`stage_long`, `click_long`) checks those and finds
  their keys. The wide (AVX-512) instance is **off unless `SS_SIMD=1`**
  (`WIDE_DEFAULT`) until its decode does the same.
* Data on the EC2 box is version 5 under the old names (`dev`, `m5`, `m20`, `n20`,
  `m60`, the 200K seeds); the old ones are in `data-v4`, `expected-v4`, `official-v4`.
  `scripts/refetch.sh` downloads the official materials again and shows what changed;
  `scripts/remote/v5data.sh` rebuilt everything. The plain implementation and the
  Python reference needed no change (both reproduce the new official dev answer).

| What | Commit | Status |
| --- | --- | --- |
| Scored fallback (version 5: **3.6747 s**) | `42b0f35` on `main` | Correct on version 5, slow. |
| **Candidate v1: long IDs on the base (AVX2) path** | `21a5b428e7d0843b04c81f24e762dc7580ad9a04` (`exp-v5`) | New 20M set on EC2: **569 / 571 ms against 3171 / 3249** for the scored build. Gate `v1` (`scripts/remote.sh log v1 40`). |
| **Candidate v1** passed the full gate (`CANDIDATE-OK`), tag `cand-v1` | as above | Given to the user for official scoring at 03:05 UTC. |
| **Candidate v2: v1 + the wide (AVX-512) parser with long IDs, on by default again; NUL fix** | `296b0dd3f2e79d4d9e1f8302bfa18618e63cacfc` (`exp-v5`) | One interleaved run on EC2 (Ice Lake), v2 / v2 with `SS_SIMD=0` (= v1's path): 20M **444 / 450 against 570 / 574**; 60M **1414 / 1417 against 1787 / 1792**. Gate `v2` started 03:12 UTC. Only faster where the processor has AVX-512 VBMI2; elsewhere it runs v1's path. |
| Diagnostic image for version 5 (fails on purpose, shows a log) | `80cc95beefbfd09c487cf3b76958c4283c0676f3` (`diag-v5`) | Prints the scorer's processor (model, the flags the wide path needs) and times the practice data with the wide parser asked for and refused, back to back. One submission answers whether v2 or v1 is the commit for the final. |

What is in v2 beyond v1: the wide decode keeps long IDs on the fast path (`id_trace8`
mirrors `id_trace_at`: `hash33` on eight lanes, the NUL rule; 33-byte users flagged for
`stage_long`; click IDs as references), the batch splitters use the three windows of
the line splitters, a wide emit (`SS_WEMIT=0` turns it off; worth 1 to 2 ms at 20M),
and a short event ID with a NUL byte is no longer hashed like the same ID without it
(found by the tests fork; the generator writes no NUL; the fault dates from c1).

**04:00 UTC.** v2 passed its full gate (`CANDIDATE-OK`, tag `cand-v2`) and went to the
user at 03:41 together with v1 and the diagnostic commit; the tests fork ran 2,558
adversarial runs on it (long-ID shapes and near misses, the three windows, whole
batches of long IDs, NUL-tailed IDs, both parser instances and both self-checks)
without a failure. `exp-v5` has since moved to `7236be4` = v2 + the SIMD fork's fast
path for link lines with long IDs (`link_fast`, checked by `SS_SPLIT_CHECK=1`) and the
long-ID loops taking their list from the flags at once: 20M 443 / 446 wide and
565 / 569 base, 60M 1396 / 1402 and 1772 / 1778, i.e. 1.5% better than v2 on both
paths. Not gated yet: the base of a v3, to be gated when something larger joins it.

**04:25 UTC: v2 scored 0.6919 s officially** (our entry in the final as of then; the
leader's 0.7257 s was the best visible before). On EC2 v2 takes 0.444 s with the wide
parser and 0.570 s without; with the container's 0.07 s and the usual 1.08 to 1.15
between EC2 and the scorer, 0.69 is what the *base* path gives. So the scorer's
processor very probably has no AVX-512 VBMI2.

**Candidate v3**, `cb4dae1df7cca24cfd127506fc1beadb11d4f5b9` (`exp-v5`), gate `v3`
started 04:45 UTC:

* a third parser instance for processors with AVX-512 F / BW / VL / DQ / CD but no
  VBMI2 (Skylake-X, Cascade Lake), by the SIMD fork: the wide decode and batch
  splitters with other instructions where the VBMI2 instance uses VBMI2 or VPOPCNTDQ,
  and with every multiplication done on 256-bit halves (inline asm), so that it holds
  the milder of the two reduced clock levels of those processors. `parse::wide_kind()`
  chooses: VBMI2 instance where possible, else this one (`WIDE5_DEFAULT`), else the
  base path; `SS_SIMD=0/1/2` force one. On EC2, forced: 20M 453 / 456 against 440 / 444
  (VBMI2) and 562 / 566 (base); 60M 1433 / 1438, 1397 / 1400, 1768 / 1769. **It has
  never run on a processor of the kind it is for**: v3's official score against v2's
  is the measurement (a worse score does not replace the best one).
* the links fast path and the long-stage list (above), the end-of-run memory release
  handed out in pieces, compressed stores through a register (the store to memory is
  very slow on AMD Zen 4, which would take the VBMI2 instance).
* `scripts/remote/fullcheck.sh` and `image.sh gate` now run the third instance as well
  (fuzz, seeds, scale, both self-checks, `mulcount.sh`: no 512-bit multiplication).
* Diagnostic image of the same sources: `aaaef3824bef68859beff77645647d1b5e76afc3`
  (`diag-v5b`): processor model and flags, and the practice data three times with each
  instance.

**05:20 UTC: v3 passed its full gate** (`CANDIDATE-OK`, tag `cand-v3`) and went to the
user; the tests fork ran 6,373 runs on it without a failure, 2,758 of them with the
third instance forced (among them a sweep of timestamps and prices in which one
millisecond or cent changes the answer, since that instance's multiplications were
rewritten by hand). `scripts/remote/icxonly.sh` (now part of `image.sh gate`) confirms
that instructions newer than Cascade Lake occur only in the VBMI2 instance's batch
splitters. Not done: a run under an emulator of such a processor (Intel SDE needs a
licence accepted by hand; QEMU has no AVX-512).

How to read v3's official score against v2's 0.6919 s: about 0.60 to 0.62 = the
scorer has AVX-512 without VBMI2 and the third instance pays; about 0.68 to 0.69 = no
AVX-512 there at all (v3 is then 1.5% better than v2); clearly worse = the third
instance loses on that processor: set `WIDE5_DEFAULT` to false, gate, submit that.

**Candidate v4**, `ca5f5f91958909da54b1acff50c570b957e139bc` (`exp-v5`), gate `v4`
started 05:50 UTC: v3 plus **the parser instance chosen by timing** (`main::calibrate`).
A processor's flags say which instances it can run, not which is fastest there (clock
reductions for 512-bit code on Skylake-X / Cascade Lake, slow single instructions on
AMD), and none of that can be measured on our box. So thread 0, right after loading
the lookups and while the other threads read the links, parses the first 512 KB of the
largest events file once unclocked and then three times each with the ordinary
instance and with the wide instance the processor offers, into writers of its own
(traces thrown away), and takes the wide one only if its best time is under 95% of the
ordinary one's. Not when `SS_SIMD`, a self-check or `SS_SCALAR` is set, `SS_CALIBRATE=0`,
or the largest events file is under 4 MB (then: the rule by flags). `SS_TIMING=1`
prints what was measured. On EC2: 614..634 us against 424 us, instance 1 every time;
wall time unchanged (20M 437 / 442 against 439 / 443 with `SS_CALIBRATE=0`; 60M
1388 / 1392 against 1387 / 1394). This makes v3's bet on the third instance safe: it is
only taken where it measures faster. Diagnostic image of v4:
`575ddf8add4781538b57bd865f5e35b310d01fce` (`diag-v5c`).

**06:10 UTC: v4 passed its full gate** (`CANDIDATE-OK`, tag `cand-v4`) and went to the
user as the commit to enter; the tests fork ran 710 runs aimed at the calibration
(one-file sets whose first 512 KB is a whale, all long IDs, all malformed, the longest
lines, link storms; files of exactly 4 MB minus one byte and 4 MB; 1, 2, 3, 4, 8
threads) without a failure. Both forks have finished and reported.

**The scorers' processors are known after all** (official starter README, which the
earlier notes had lost): "Google Cloud n2 Intel Xeon machines, Cascade Lake or newer,
with AVX-512 available". So every scorer runs one of the two wide instances, and v2's
0.6919 s, taken on the base path, came from a Cascade Lake machine.

After v4, measured on EC2 and not adopted: each thread pinned to a processor of its own
(no difference); each 1 MB chunk of input mapped in one call before it is parsed
(1.4% slower; it would only pay where the kernel maps one page per fault, which could
not be reproduced here: kernel 7.0 maps a whole large folio per fault whatever
`fault_around_bytes` says); `-C llvm-args=-x86-branches-within-32B-boundaries`
(padding against Cascade Lake's JCC erratum: 0.3 to 0.8% slower here, where there is
nothing to mitigate; our hot loops have few branches, so no gain is to be expected
there either). `scripts/remote/faultaround.sh`, `flagab.sh` repeat these.

Competitor repositories (the user asked for a look at Team 67's `express-lane`, MIT
licence, carrying a request to other teams' tools not to use it; the user overrode
that): read its build files and system layer only. Nothing was taken from it. What it
showed: built for `target-cpu=cascadelake` with a re-exec fallback on a fault; all
input mapped into one region; a start-up probe for whether a page fault maps the
neighbouring pages, switching to reads if not (the idea behind the chunk experiment
above); threads pinned by hyperthread siblings; parallel release of memory at exit as
ours.

**08:00 UTC. v4 scored 0.6745 s** (the entry in the final as of then); **a probe of
v4** (`09bce89`, branch `signal-v4`: the same build, sleeping a whole number of seconds
at its end that says what the calibration found; `scripts/mksignal.sh`) **scored
6.6902 s**: 6 = a processor without VBMI2 on which the 512-bit instance took 105% or
more of the ordinary one's time, so v4 ran the base path there, as v2 did.

Why, most likely (the SIMD fork's point): v4 timed base and wide in turn on 512 KB,
0.3 to 0.6 ms a run. On Cascade Lake the first 512-bit instructions run at a fraction
of their speed while the processor changes its clock, and the change lapses a few
milliseconds after the last one: every timed wide run sat in a change-over. Even on
this box the wide instance measures 56% of base when timed in a row and 68% in turn.

**Candidate v5**, `37023b2caeeac3b0c363538e87a09cd46cbd2b8c` (`exp-v5`), gate `v5`
started 08:00 UTC:

* calibration: base three times in a row first; then each wide instance the processor
  offers, three times in a row: the four-lane one first, a 512-bit one last and after
  five unclocked runs; the fastest wins if it beats base by 5% (instance 2 counts 10%
  more than it took: one core at the lower clock here, all of them in the run);
* **parser instance 3** (SIMD fork; `parse/wide_quad.rs`, `parse/splitw_quad.rs`): the
  wide decode and batch splitters with four lanes in 256-bit registers, EVEX
  encodings, every multiplication on 128-bit halves: nothing in it lowers the clock
  on Cascade Lake, so its gain over base on this box carries over. 20M 510 / 514
  against 565 / 568 base (454 / 456 instance 2, 439 / 442 instance 1); 60M 1605 against
  1758 (1430, 1389). `scripts/remote/quadcheck.sh` fails on a 512-bit register or a
  256-bit multiplication in it; the gate runs `SS_SIMD=3` wherever it runs 2.
* Probe of the 512-bit instance under the new timing, with the user:
  `28246edb1e0d251fa4432b049437ba74e0c02f80` (`signal-v5`; scale: 1 to 17 s = its time
  as a share of base in steps of 5% from "under 55%", 18 = a VBMI2 machine, 19 =
  nothing tried). `SS_SIGNAL=3` (`mksignal.sh REV 3`) reports instance 3 instead.

Measured and left alone after v3: marking the input mappings sequential (no change in
the 13 ms the release of memory takes at the end: it is work per page, done by all
threads, and still 8 ms better than leaving it to the exit); the campaign counters of
the session pass (all their cache misses together are 1.3% of the run: one cell for
everything saves 5 ms); the base decode by source line (flat: timestamps 40 of its
257 instructions per line, nothing else over 15).

Tried since v2 without gain (all removed): a table of long IDs a quarter the size; a
helper thread faulting the partition pool in ahead of the writers (by
`MADV_POPULATE_WRITE`: system time halved, wall time 3% worse; by one write fault per
huge page: nothing either way); my own versions of the links fast path and the list
loop (the SIMD fork's are in).

Known and open:

* **The scorer's processor is unknown**, and it decides most of what is left: on a
  processor with AVX-512 VBMI2 v2 is 22% faster than v1; on Cascade Lake / Skylake-X
  (AVX-512 without VBMI2) both run the base path. The diagnostic image answers it. The
  SIMD fork is building a second wide instance without VBMI / VBMI2 / VPOPCNTDQ,
  chosen only by `SS_SIMD=2`: whether 512-bit code wins there (it lowers the clock)
  can only be measured on such a machine, i.e. by a diagnostic run on the scorer.
* Accepted, not a fault to fix tonight: two different event IDs made to have the same
  64-bit hash, on events of one person at one millisecond, count as one event (equal
  fingerprints with equal person and time are taken for copies without reading the
  lines; so since c1). It takes input built against our hash functions
  (`tools/collide.py` on `exp-tests` makes such pairs); generated data cannot do it.
* Where the time is now (sampled, wide path, new 20M): the session and dedup passes
  24%, batch split 9%, decode 9%, emit and partition writes 9%, **the kernel zeroing
  fresh pages 7 to 9%** (about 630 MB: 463 MB of partition blocks, the user table, the
  long table), `stage_long` 2%, clicks 5%. Fresh memory costs about 0.5 ms of CPU per
  MB with eight threads at it. Tried without gain: a four times smaller table of long
  IDs. A trace cannot be less than 16 bytes (fingerprint, position, person, time).
* Never push sources to `~/db` on the box while a gate runs there (its later steps
  rebuild from that directory): use a secondary tree (`scripts/newtree.sh`).

## State before scorer version 5 (historical)

Everything below the horizontal rule predates this section's first line; where they
disagree, the later text wins.

### What to submit

| What | Commit | Status |
| --- | --- | --- |
| Officially scored, **0.85 s** | `f1459cc` (tag `scored-0.85`) | Untouched fallback. |
| GitHub `main` | `3201a41` | Untouched. Submit by SHA, never with the commit field empty. |
| Candidate c1: staged parser, old 77 MB output | `1c50711` (tag `cand-c1`) | Full gate passed. 20M: 523 / 527 ms against 560 / 563 scored. |
| Candidate c2: c1 + dictionary-encoded output (25 MB) | `c4b7fcf` (tag `cand-c2`) | Full gate passed. |
| **Candidate c3: trace-only parser, pass 2 routes events** | `69557e4` (tag `cand-c3`) | Full gate passed. 20M: 501 / 503 against 555 / 561; 60M: 1556 / 1557 against 1704 / 1710. |
| **Candidate c4: c3 + merged pass 2, faster writer** | `864640b` (tag `cand-c4`) | Full gate passed. 20M: 490 / 492 against 560 / 562; 60M: 1522 / 1526 against 1704 / 1705. |
| **Candidate c5: c4 + same-day check on the text, user table in cache-line buckets** | `4b2bdb3` (tag `cand-c5`) | Full gate passed. One interleaved run, scored / c4 / c5: 20M 561/564, 488/491, 488/489; 60M 1702/1705, 1520/1522, 1503/1509; a second 60M set (seed 77) 1671/1677 scored, 1438/1442 c5. Also identical to the plain implementation on four more large sets (`scripts/remote/moreseeds.sh`: a window holding 29 February 2028 at 5M and 20M, seed 31 at 20M, seed 77 at 60M) and to the Python reference on the leap-day seed at 200K. **The one to submit.** |
| Officially scored, **0.63 s and 0.62 s** | `42b0f35` on `main` (a squash of `dev`; same build inputs as c5) | **The best scored so far, and the fallback.** |
| Candidate c6: c5 + user_daily in 512 KB files written during the session pass, writeback of each started at once | `bd38f17` (tag `cand-c6`) | Full gate passed. Not scored. |
| Candidate c7: c6 + smaller output (rows without revenue first, person indexes as runs, campaign_daily dictionary-encoded) | `01708cd` (tag `cand-c7`) | Full gate passed. **Officially 0.64 s**: no better than c5. |
| Candidate c8: c7 + writer that keeps its buffers between files | `1dd7fb8` (tag `cand-c8`) | Full gate passed. Superseded by c9 (same, without early writeback). |
| **Candidate c9: c8 with early writeback off** | `5c5d8d1` | One interleaved run, c5 / c8 / c9: 20M 484/488, 480/483, 482/484; 60M 1507/1509, 1486/1487, 1487/1488. Gate started 01:40 UTC on the 22nd (`scripts/remote.sh log c9`). The one to submit once it says `CANDIDATE-OK`; c5 stays the choice for the final unless c9 scores at least as well. |
| Diagnostic image (fails on purpose, shows a log) | `2a2ea40` (branch `diag4`) | Not yet submitted. The single most informative submission: scorer CPU, clock under 512-bit code, huge pages, page-fault and memory costs, `/output` write speed, CPU-quota throttling. |

"Full gate" = `scripts/cand.sh NAME`: on EC2, `fullcheck.sh` (fixtures, dev answer, 150
fuzz seeds x 8 modes, six seeds fast and slow, 5M / 20M / 20M / 60M against the plain
implementation), then `image.sh gate` (no-cache Docker build of a clean export, no
512-bit instructions, fixtures and dev / 5M / 20M / 60M inside the container with the
scorer's flags). All times: EC2 m6i.2xlarge, 8 threads, wall ms, min / median of
interleaved rounds (`scripts/abx.sh`).

### Architecture now

1. Links -> user table (union-find), as before.
2. Parse, in batches of 64 lines and three loops (split, decode, emit). An event line
   leaves only a two-word *trace* in a partition chosen by a hash of its event ID:
   `fingerprint | line position`, and `valid | purchase | 4 hash bits | person | ts`
   (zero if the line failed validation). Purchases also leave price and category with
   their person; clicks go to their person as before.
3. Pass 2, per trace partition: a two-bit table finds repeated fingerprints. A trace
   that is alone is sent to its person's partition at once. Repeats whose second words
   are equal (same person, time, valid, not a purchase) are settled without reading
   anything: one is sent, the rest dropped. The remaining 6% of repeat groups have
   their lines read again and are settled by the literal rule. Events are written into
   the trace blocks already read, so they cost no new memory.
4. Pass 3, per person partition (8192 people): 2-pass radix by (slot, person), session
   walk, attribution, rows. Output is dictionary-encoded Parquet.

### What was learned this round (measured)

* **Output size shows up outside the program.** With 77 MB of plain Parquet (223 MB at
  60M), `docker run` took 0.1 s (20M) to 0.87 s (60M) longer than the container's own
  StartedAt..FinishedAt on EC2; with `/output` on tmpfs or with the 3x smaller
  dictionary-encoded output it does not (`scripts/remote/teardown.sh`). Whether the
  scorer's clock includes that is unknown; c1 against c2/c3 on the scorer would tell.
* **Fresh memory costs about 0.7 ms of kernel time per MB** (page zeroing under load):
  8 more bytes per trace cost as much as they saved until the event stream moved into
  recycled trace blocks. Ordinary pages for the pool are worse than huge pages.
* **Dedup by exception is sensitive to the duplicate rate** (2 to 4% of events per
  seed; the 60M set has more than the 20M one). Re-reading lines cost 140 to 250 ns
  each; settling repeats from the traces removed 94% of the re-reads and took 60M from
  -1% to -10% against scored.
* The container adds a fixed ~70 ms (StartedAt..FinishedAt minus the program) on EC2.
* The parser is bound by instruction throughput: a second hyperthread on the same core
  adds only 24% (`scripts/remote/pbench.sh`). 512-bit masks would save ~6% of the
  parser's instructions but lower the clock ~3% here: not worth it on this machine.
* 12, 13 or 14 person bits per partition: no difference (13 kept). Trace partition
  size: no difference. Settle prefetch distance: no difference.
* Tools that proved their worth: `scripts/remote/cgasm.sh` / `cgshow.sh` / `cgsrc.sh`
  (exact instructions per row by function, line, or instruction, via valgrind),
  `cgcache.sh` (simulated cache misses), `sweep.sh` (profile sections per env value),
  `keep.sh` (named binaries for A/B), `imgtime.sh` (container and docker times).
* Beware: `remote.sh push` must not preserve file times (fixed), or cargo on EC2 skips
  files edited before its last build.

### 22 November, 01:15 UTC: three lines of work in parallel

After the third official score the brief changed to "independent approaches, in
parallel". `scripts/newtree.sh NAME` makes a git worktree `../db-NAME` (branch
`exp-NAME`) with its own directory on the EC2 box (`~/db_NAME`, sharing data, references
and toolchain by symbolic links); `scripts/remote.sh run` takes a machine-wide lock so
that nobody disturbs a measurement (`quick` for reading logs).

* **`exp-index` (shelved, pushed as a record, f6b84eb).** Event IDs are `e` + base 36
  of a bijective scramble of a counter that runs through each user's events in time
  order. Decoding it (the key is found from six sample IDs in about a millisecond) lets
  pass 2 place each event at its index (dedup without fingerprints) and walk the slots,
  handing each person's events to pass 3 as session intervals `[first, last, count]`;
  pass 3 then sorts 3.5M intervals instead of 19.5M events and merges them. Exact for
  any input (IDs that are not canonical take the old road and become intervals of one).
  Measured at 20M: pass 3 gather 205 -> 28 ms CPU, people 523 -> 415, interval sort
  +55; pass 2 unchanged in total; **parsing +200 to 250 ms**, because the exact decode
  is about 105 instructions per line as compiled against 12 for the hash, in its own
  loop or inside `decode`. Wall 484 to 489 ms against 483. A decode without the
  canonical checks would still cost about half of that. Not worth its risk.
* **`exp-simd` (running).** A parser instance for Ice Lake and newer chosen at run
  time, and a decode stage vectorised across lines.
* **`exp-tests` (running).** `tools/mutate.py`, `scripts/remote/idxfuzz.sh`,
  `fuzzidx.sh`: mutations of generated data aimed at dedup, session and link edge
  cases, each run compared with the plain implementation and the Python reference.
  No failure in c8/c9 so far (about 450 runs).

### What the third official score says (this corrects the section below)

c7 (early writeback, 18.5 MB of output) scored **0.64 s**; c5 (no writeback, 25 MB)
scored 0.63 s and 0.62 s. So neither the output still unwritten at exit nor the output's
size is in the official clock, and the two-point model below was a coincidence: the
first score (0.85 s) was simply taken on a slower machine. The organisers say practice
times vary by up to 20% between scoring machines; 0.85 / 0.631 s (our container time
for that build) is 1.35, and 0.62 to 0.64 over 0.557 s is 1.11 to 1.15. A 1.2x spread is
what one generation of Intel core (Cascade Lake against Ice Lake, both "n2") gives at
equal clock, so there is no sign that our code suits one of them worse than usual.

What follows from it:

* Official score = machine factor x (program time + about 0.07 s). Only program time
  is ours, and EC2 A/B results carry over as ratios. Differences under about 3% cannot
  be seen in a single official score.
* Early writeback is off by default from c9 on (`SS_WRITEBACK=1` turns it on): it buys
  nothing there, and disk activity during the run can only cost.
* Kept from c6 to c8, because they save CPU time whatever the clock includes: output
  in 512 KB files as the rows come (row buffers stop growing through fresh memory),
  rows without revenue first and person indexes as runs (fewer bytes through the page
  cache), the writer's buffers kept between files.
* For the final (60M, median of 3, the ten fastest retimed 7 times on one idle
  machine) the commit to choose is the one with the lowest program time that has an
  official score: c5 until c9 has one that is not worse.

### What the second official score seemed to say (superseded by the section above)

0.85 s (scored build, 77 MB of output) and 0.63 s (c5, 25 MB) both fit

    score = program time on our EC2 box + about 0.07 s + about 2.85 ms per MB of output

to within 2% (561 ms and 488 ms of program). Two consequences:

* The scorer's CPU is about as fast as our m6i (solving for a slower CPU makes the fixed
  part negative). EC2 program times translate one to one.
* Output bytes cost time after the program has ended. On EC2 the reason is visible
  (`scripts/remote/tdcheck.sh`): when the container exits, Docker unmounts its overlay
  root, the kernel then writes out everything dirty on that file system, and `/output`
  is on it; the dirty page count is zero the moment `docker start -a` returns. 2.85 ms
  per MB is a disk writing 350 MB/s.

Hence c6 (the output leaves in small files while pass 3 is still running, and a helper
thread calls `sync_file_range(SYNC_FILE_RANGE_WRITE)` on each: on EC2 the gap between
container exit and the docker command's return drops to its fixed 62 ms at any output
size) and c7 (fewer bytes). Starting writeback from the worker threads themselves
stalled a run once (journal commit), hence the helper thread. musl's allocator has one
lock: an allocation per run-length run made eight threads queue for it (+200 ms).

Measured directly (`scripts/remote/dirtycheck.sh`): when the process exits, c5 still has
all of its output unwritten (24.5 MB at 20M, 72.6 MB at 60M); c6 and c7 have 0 to 0.3 MB.

More from the same night:

* A thread that writes many files must keep its buffers: musl gives large blocks
  back with an unmap each time, which interrupts every thread (0.5% of all CPU time
  in map/unmap calls at 60M before c8), and the standard hash map costs 0.4%.
* No gain, not kept: flushing after every partition near the end of pass 3 (nothing
  is left unwritten anyway); releasing the input mappings in equal byte shares with
  thread 0 exempt; a branch-free "candidate or not" in pass 2 (under two threads per
  core a misprediction is cheaper than the extra instructions; simulated mispredictions
  are 3.8 per event line, `scripts/remote/cgbranch.sh`, and none of the top sites is
  cheap to remove for that reason).

Not yet known: whether early writeback takes effect on the scorer (c6 should score
about 0.57 if so). If c6 stays near 0.63, bytes are what counts there: next would be
Huffman-coding the user_id dictionary (half the output) as GZIP pages.

### Evening of 21 November: what the search for something bigger found

The leaderboard's best was 0.30 s against our 0.85 s. Read against the official page
(the clock runs from container start to container exit; practice is about 20M events;
**the final is 60M new events, median of 3, the ten fastest retimed 7 times on an idle
machine**), that is a program about three times faster than c4 on this machine.
Nothing found here closes a gap of that size:

* The profile is flat. By instructions per event line (802 in all): decode 216, passes
  2 and 3 together 173, split 155, emit 65, clicks 79, pair sort 24, links 10. No source
  line of passes 2 and 3 takes more than 9; the parser's listing shows no waste beyond
  a dozen constants rematerialised per line (16 registers).
* What trusting the generator instead of the rules would save (no quote pairing, no ID
  byte checks) is about 27 instructions per line, 3%: not worth giving up exactness.
* Profile-guided optimisation (instrumented run on dev + 5M + 20M, `-C profile-use`):
  488 / 490 against 489 / 492. Nothing. Not kept.
* Sampling by instruction (`scripts/remote/hotip.sh`, `hotasm.sh`: the instructions
  that hold up retirement, with their inlined source lines) found cache misses nobody
  had asked for:
  * user-table lookups that ran into the next cache line (home slot at the end of its
    line, or a third probe). Fixed: probes start at a line boundary, four slots to a
    line, one SIMD compare. 20M -0.6%, 60M -1.1%. A denser table is much worse (load
    0.58: +6%), a sparser one no better.
  * the per-session campaign counters (672 KB and 168 KB tables per thread). Deferring
    the updates through a 16-entry ring with a prefetch: user time -1.4%, wall time
    unchanged at both sizes. Not kept. Same for prefetching the start of the next 4 KB
    block of a chain.
* Kernel side (`scripts/remote/kprof.sh`): 12% of CPU time; page zeroing alone 7.4%,
  the rest fault handling. All of it is first use of memory; traces are 335 MB of the
  650 MB at 20M and cannot shrink (16 bytes carry the fingerprint, the line position,
  the person and the time).
* Considered and not started, with the reason:
  * a splitter that works on 64-byte blocks of the whole input instead of two windows
    per line: its quote logic costs as much per block as ours per line, and flattening
    two position arrays eats the rest; perhaps -40 instructions (5%), a rewrite of the
    parser's core;
  * decoding event IDs to the generator's dense index (the scramble is a bijection, so
    this is exact for any input) to place events directly and build sessions as
    mergeable intervals before the person pass: perhaps -10% in all, more than a day
    of work touching dedup, sessions and attribution at once. Too much risk for the
    final run, which gives no score at all if it is wrong.

### Where the time goes now (20M, CPU ms of about 3760)

parse events 1952 (52%; split 155 + decode 217 + emit ~45 instructions per line), people
523, parse clicks 405, pass 2 scan and routing 265, pass 3 gather 205, pair sort 135,
links 92, write 82, resolve 33, group 30, re-read 21.

### Next, in order of expected value

1. Get the diagnostic and c1 / c3 scored (needs the user). If output or teardown
   matters on the scorer: gzip the user_id dictionary (the bulk of the 25 MB), start
   writeback early.
2. Small parser items (F1 same-day check by direct text compare is in the working
   tree), pair sort for small lists (in the working tree), click parser diet
   (campaign lookup by hash of the raw bytes), people loop.
3. Finer event sub-partitions out of pass 2 so that pass 3's radix runs in L1.

### EC2 box

Address, user and key are in `.local/remote.env` (untracked).

* `~/db`: working tree (rsync of this one), toolchain `~/db/.toolchain`, scored binary
  `~/db/bin/scored`, kept variants `~/db/bin/*`, exported candidates `~/img/*`.
* Data: `~/db/data/{dev,m5,m20,n20,m60,s2_200000..s7_200000}`; references
  `~/db/expected/*.slow`; job logs `~/db/out/logs`.
* From WSL: `scripts/remote.sh push`, then
  `scripts/remote.sh run 'bash scripts/remote/cycle.sh 40 900'` (build, fixtures, dev,
  40 fuzz seeds from 900, A/B against scored, section profile).
  `scripts/cand.sh NAME` for the full gate of the committed HEAD (clean tree needed).
* From PowerShell, never put `|` or `$` inside nested quotes: write a script under
  `scripts/remote/` instead.

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
