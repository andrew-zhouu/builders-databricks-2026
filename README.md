# Shopping Sessions

Entry for the Databricks **Shopping Sessions** track of the Builders Buildathon: clean
messy click-stream CSVs, join linked user IDs into people, build sessions, attribute
them to ad clicks, and write two exact daily Parquet tables as fast as possible.

The pipeline is one statically linked Rust binary with no dependencies, shipped in an
empty (`scratch`) image of under 1 MB. `rules.md` from the track page is the
specification; every rule is implemented literally in `fast/src/slow.rs` and
`reference/reference.py`, and the fast path is tested against both.

## Layout

| Path | What it is |
| --- | --- |
| `Dockerfile` | Builds `fast/` for x86-64 (musl, static) and copies the binary into `scratch`. |
| `fast/src/` | The pipeline (see below). |
| `reference/reference.py` | Independent, literal Python implementation of the rules: the test oracle. |
| `tools/fuzz.py` | Adversarial dataset generator (bad timestamps, quoting, odd IDs, duplicate copies that disagree, ...). |
| `tools/cmp.py` | Byte-exact, order-insensitive comparison of two outputs (CSV or Parquet). |
| `scripts/` | Build, test and benchmark helpers. `scripts/gate.sh` is the full correctness gate. |

## How it works

All input files are memory-mapped and cut into 1 MB chunks that worker threads pull
from a shared queue. Phases are separated by barriers; within a phase threads only
share lock-free structures.

1. **Links** (`parse.rs`, `table.rs`). Each user ID gets a slot in one global
   open-addressing table (CAS insert). A lock-free union-find over slots joins linked
   IDs; a union always points the byte-wise larger root at the smaller, so a set's root
   is the person's name. A final pass points every slot straight at its root. Key and
   parent share one 16-byte slot, so "user to person" is a single cache miss.
2. **Parse** (`parse.rs`). A line is first tried on an AVX2 fast path: comma, quote
   and newline bitmasks for two 64-byte windows; branch-free checks that the field
   count is right and that every quote is the first or last byte of a field; IDs packed
   with `pext`; timestamps parsed with a few 128-bit multiply-adds and a table of valid
   dates. The fast path only accepts lines it can decide completely. Everything else
   (malformed or long lines, unusual IDs, timestamp shapes it cannot judge) goes to a
   scalar parser that follows the rules step by step. Table probes are prefetched a few
   lines ahead.
   * Events are radix-partitioned by event ID, **invalid ones too**: dedup comes before
     validation, so an invalid earliest copy must still beat a later valid one.
   * Clicks need no dedup (copies are identical, and a repeated click changes neither
     the credited click nor the set of assisting campaigns), so valid clicks go
     straight to their person's partition.
3. **Dedup** (`pass2.rs`). Per event partition, a small hash table keeps the copy with
   the smallest `ingest_ts`; survivors that are valid are scattered to person
   partitions.
4. **Sessions and attribution** (`pass3.rs`). Per person partition, a 4-pass LSD radix
   sort groups events by person in time order. One sweep per person builds sessions,
   assigns purchases, finds the credited click with a two-pointer window (ties to the
   smallest `click_id`) and counts assists. People with very many sessions and clicks
   (shared devices, whales) count assists per campaign by merging 7-day intervals, so
   nothing is quadratic.
5. **Output** (`parquet.rs`). Rows are rendered into Parquet PLAIN pages as they are
   produced; each thread writes one `user_daily` file, and the per-campaign counters
   are merged into one `campaign_daily` file.

IDs are handled exactly, not hashed: an ID of up to nine characters from `[0-9a-z]`
packs injectively into an integer whose order is the byte order of the ID; any other
ID is interned in a dictionary. If an input does not fit the packed layouts at all (for
example a window longer than about 17 days), the binary falls back to the plain
single-threaded implementation in `slow.rs`.

The build targets AVX2/BMI2 and deliberately not AVX-512: 512-bit instructions lower
the frequency licence of Cascade Lake cores, and `scripts/gate.sh` fails if any appear
in the binary.

## Build and run

```sh
docker buildx build --platform linux/amd64 --load -t shopping-sessions:latest .
docker run --rm --network none --read-only --user 1000:1000 \
  -v "$PWD/dev":/data:ro -v "$PWD/out":/output \
  -e NCPU=8 shopping-sessions:latest
```

The binary reads `DATA_DIR` (default `/data`), writes `OUTPUT_DIR` (default `/output`)
and uses `NCPU` threads. `TIMING=1` prints phase times and a per-section CPU profile
to stderr.

## Testing

```sh
scripts/setup_toolchain.sh        # project-local Rust and Python tools (no root needed)
scripts/fetch_official.sh         # rules, starters with the 36 fixtures, dev answer, generator
scripts/build_generator.sh dev:1:1000000:250000
scripts/gate.sh                   # build, fixtures, dev answer, generated seeds, fuzzing
scripts/scale.sh m20 20 20000000 5000000   # 20M events: fast path vs the plain implementation
```

`gate.sh` checks, in order: the 36 official fixtures; the 1M-event dev set against
the published answer; six generated seeds against the Python reference; and a batch of
fuzzed datasets, each run four ways (SIMD multi-threaded, scalar-only, single-threaded,
and the plain implementation), all of which must equal the reference byte for byte.
