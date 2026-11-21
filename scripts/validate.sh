#!/usr/bin/env bash
# Validate one revision exactly as the scorer will build it.
#
#   1. export the revision (git archive) into an empty directory: nothing
#      uncommitted or ignored can leak into the build;
#   2. build the Docker image from that export with no cache;
#   3. check the image against the competition's container rules;
#   4. extract the binary from the image and run every correctness check on it
#      (fixtures two ways, dev answer two ways, generated seeds, fuzzing, 5M and
#      20M scale checks);
#   5. run the image itself with the scorer's flags on the fixtures and dev set.
#
# Usage: validate.sh [REV] [FUZZ_COUNT]        exit 0 only if everything passed
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
cd "$PROJ"
rev="${1:-main}"; fuzz_count="${2:-30}"
sha=$(git rev-parse "$rev") || exit 1
short=$(git rev-parse --short "$sha")
IMAGE="shopping-sessions:$short"
W="$PROJ/out/validate-$short"; rm -rf "$W"; mkdir -p "$W/src"
PY="$PROJ/.venv/bin/python"
ST="$PROJ/official/starter-duckdb/x/starter-duckdb"
ANS="$PROJ/official/dev-answer/x/dev-answer"
fails=0
ok()   { echo "PASS  $*"; }
bad()  { echo "FAIL  $*"; fails=$((fails + 1)); }
step() { if "${@:2}" >"$W/$1.log" 2>&1; then ok "$1: $(tail -1 "$W/$1.log")"; else bad "$1 (see $W/$1.log)"; tail -5 "$W/$1.log"; fi; }

echo "== revision $sha ($(git log -1 --format=%s "$sha"))"
git archive "$sha" | tar -x -C "$W/src"

# ---- 1. repository requirements
[ -f "$W/src/Dockerfile" ] && ok "Dockerfile at repository root" || bad "no Dockerfile at root"
size=$(du -sb "$W/src" | cut -f1); files=$(find "$W/src" -type f | wc -l)
[ "$size" -lt 50000000 ] && ok "repository export is $size bytes in $files files (limit 50 MB)" || bad "repository too large: $size"
big=$(find "$W/src" -type f -size +1M | wc -l)
[ "$big" = 0 ] && ok "no file over 1 MB (no data or binaries checked in)" || bad "$big large files"
if bash "$PROJ/scripts/secretscan.sh" "$W/src" >"$W/secrets.log" 2>&1; then ok "no addresses, keys or tokens in the tree"; else bad "possible secret in tree"; cat "$W/secrets.log"; fi
if grep -rIlE '/home/[a-z]+/|/mnt/c/Users' "$W/src" >/dev/null; then bad "machine-specific path in tree"; else ok "no machine-specific paths in the tree"; fi

# ---- 2. clean Docker build
s=$(date +%s)
if (cd "$W/src" && docker buildx build --no-cache --platform linux/amd64 --load -t "$IMAGE" .) >"$W/docker-build.log" 2>&1; then
  ok "docker build from the clean export, no cache: $(( $(date +%s) - s )) s (limit 600 s)"
else
  bad "docker build"; tail -20 "$W/docker-build.log"; echo "== $fails FAILED"; exit 1
fi

# ---- 3. image requirements
read -r arch os bytes user cmd entry <<<"$(docker image inspect "$IMAGE" --format '{{.Architecture}} {{.Os}} {{.Size}} {{.Config.User}} {{json .Config.Cmd}} {{json .Config.Entrypoint}}')"
[ "$arch/$os" = "amd64/linux" ] && ok "image platform linux/amd64" || bad "image platform $os/$arch"
[ "$bytes" -lt 8000000000 ] && ok "image size $bytes bytes (limit 8 GB)" || bad "image too large"
[ "$cmd" = '["/sessions"]' ] && [ "$entry" = "null" ] && ok "CMD $cmd, no entrypoint, user ${user:-root}: runs with no arguments" || bad "CMD/entrypoint: $cmd / $entry"

# ---- 4. the image's binary
cid=$(docker create "$IMAGE"); docker cp "$cid:/sessions" "$W/sessions" >/dev/null; docker rm "$cid" >/dev/null
export BIN="$W/sessions"
OBJDUMP=$(find "$RUSTUP_HOME/toolchains" -name llvm-objdump | head -1)
READOBJ=$(dirname "$OBJDUMP")/llvm-readobj
"$OBJDUMP" -d --no-show-raw-insn "$BIN" >"$W/disasm.txt"
wide=$(grep -cE '%zmm|%k[0-7]|vmovdq[au](8|16|32|64)|vpternlog|vpmovm2|vpcompress|vpexpand' "$W/disasm.txt")
ymm=$(grep -c '%ymm' "$W/disasm.txt")
[ "$wide" = 0 ] && ok "no AVX-512 (zmm, mask-register or EVEX-only) instructions; $ymm AVX/AVX2 ymm instructions" || bad "$wide AVX-512 instructions"
dyn=$("$READOBJ" --needed-libs "$BIN" 2>/dev/null | grep -c '\.so')
interp=$("$READOBJ" --program-headers "$BIN" 2>/dev/null | grep -c PT_INTERP)
[ "$dyn" = 0 ] && [ "$interp" = 0 ] && ok "binary is fully static (no interpreter, no shared libraries), $(stat -c %s "$BIN") bytes" || bad "binary is dynamic"

step fixtures-bytes   bash "$PROJ/tools/fixtures.sh" "$BIN"
step fixtures-official "$PY" "$ST/scripts/run_fixtures.py" "$ST/fixtures" -- "$BIN"
dev_out="$W/dev"; mkdir -p "$dev_out"
"$BIN" "$PROJ/data/dev" "$dev_out" 2>"$W/dev-run.log" || bad "dev run exit code"
step dev-bytes        "$PY" -I "$PROJ/tools/cmp.py" "$ANS" "$dev_out"
step dev-official     "$PY" "$ST/scripts/compare.py" "$dev_out" "$ANS"
step seeds            bash "$PROJ/scripts/seeds.sh" 200000 50000 2 3 4 5 6 7
step fuzz             bash "$PROJ/scripts/fuzz.sh" "${FUZZ_FIRST:-7000}" "$fuzz_count" 3000
for name in m5 m20; do
  [ -d "$PROJ/expected/$name.slow/user_daily" ] || { echo "SKIP  scale-$name: no expected answer"; continue; }
  avail=$(awk '/MemAvailable/ {print int($2/1024)}' /proc/meminfo)
  need=3000; [ "$name" = m20 ] && need=6000
  if [ "$avail" -lt "$need" ]; then echo "SKIP  scale-$name: only $avail MB available"; continue; fi
  o="$W/$name"; mkdir -p "$o"
  NCPU=8 "$BIN" "$PROJ/data/$name" "$o" 2>"$W/$name-run.log" || bad "$name run exit code"
  step "scale-$name" "$PY" -I "$PROJ/tools/cmp.py" "$PROJ/expected/$name.slow" "$o"
  rm -rf "$o"
done

# ---- 5. the image, run the way the scorer runs it
export IMAGE CPUS=8 MEM=12g PLATFORM=linux/amd64
step docker-fixtures  "$PY" "$ST/scripts/run_fixtures.py" "$ST/fixtures" -- sh "$ST/scripts/docker_run.sh"
dout="$W/docker-dev"; rm -rf "$dout"
s=$(date +%s%N)
if sh "$ST/scripts/docker_run.sh" "$PROJ/data/dev" "$dout" >"$W/docker-dev-run.log" 2>&1; then
  ok "docker run on dev with scorer flags: exit 0, $(( ($(date +%s%N) - s) / 1000000 )) ms including container create/remove"
else
  bad "docker run on dev"; tail -5 "$W/docker-dev-run.log"
fi
step docker-dev-bytes    "$PY" -I "$PROJ/tools/cmp.py" "$ANS" "$dout"
step docker-dev-official "$PY" "$ST/scripts/compare.py" "$dout" "$ANS"
ls -l "$dout/user_daily" "$dout/campaign_daily" 2>/dev/null | awk '$1 ~ /^-/ {print "      output file", $NF, $5, "bytes, mode", $1}' | head -12

echo "== $sha: $([ "$fails" = 0 ] && echo ALL CHECKS PASSED || echo "$fails CHECK(S) FAILED")"
[ "$fails" = 0 ]
