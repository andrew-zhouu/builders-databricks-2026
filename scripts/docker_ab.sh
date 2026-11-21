#!/usr/bin/env bash
# Interleaved A/B INSIDE the container, with the scorer's flags (CPU quota,
# memory limit, read-only root, uid 1000): the binary's own time to "done",
# and its user and kernel CPU time, per variant.
#
# Usage: docker_ab.sh IMAGE DATASET ROUNDS VARIANT...
#   VARIANT = comma-separated env assignments passed into the container (X=1 = none).
#   CPUS (default 8), MEM (default 12g).
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
image="$1"; name="$2"; rounds="$3"; shift 3
CPUS="${CPUS:-8}"; MEM="${MEM:-12g}"
OUT=$(mktemp -d); SCRATCH=$(mktemp -d); chmod 777 "$OUT" "$SCRATCH"
res=$(mktemp)
cat "$PROJ"/data/"$name"/*/*.csv > /dev/null
for r in $(seq "$rounds"); do
  for v in "$@"; do
    envs=""; for kv in ${v//,/ }; do envs="$envs -e $kv"; done
    s=$(date +%s%N)
    docker run --rm --platform linux/amd64 --network none --read-only --tmpfs /tmp:rw,size=64m,mode=1777 \
      --cap-drop ALL --security-opt no-new-privileges --pids-limit 4096 \
      --cpus "$CPUS" --memory "$MEM" --memory-swap "$MEM" --user 1000:1000 \
      -v "$PROJ/data/$name":/data:ro -v "$OUT":/output -v "$SCRATCH":/scratch \
      -e DATA_DIR=/data -e OUTPUT_DIR=/output -e SCRATCH_DIR=/scratch -e HOME=/scratch \
      -e NCPU="$CPUS" -e MEM_GB="${MEM%g}" -e SS_TIMING=1 $envs "$image" 2>"$res.log" >/dev/null
    e=$(date +%s%N)
    awk -v v="$v" -v w="$(( (e - s) / 1000000 ))" '
      /process: user/ { n=split($0, a, /[ ,]+/); for(i=1;i<=n;i++) if (a[i]=="process:") {u=a[i+2]; y=a[i+5]; f=a[i+7]} }
      /\] done;/ { d=$0; sub(/^\[ */,"",d); sub(/ ms.*/,"",d) }
      END {print v, d, u, y, f, w}' "$res.log" >> "$res"
  done
done
printf '%-30s %14s %14s %14s %12s %16s\n' variant "to done ms" "user ms" "kernel ms" "faults" "docker run ms"
for v in "$@"; do
  awk -v v="$v" '$1==v' "$res" > "$res.v"
  stat() { awk -v c="$1" '{print $c}' "$res.v" | sort -n | awk '{a[NR]=$1} END{printf "%.0f/%.0f", a[1], a[int((NR+1)/2)]}'; }
  printf '%-30s %14s %14s %14s %12s %16s\n' "$v" "$(stat 2)" "$(stat 3)" "$(stat 4)" "$(stat 5)" "$(stat 6)"
done
docker run --rm --user 1000:1000 -v "$OUT":/output -v "$SCRATCH":/scratch --entrypoint /sessions "$image" --version >/dev/null 2>&1 || true
rm -rf "$OUT" "$SCRATCH" "$res" "$res.v" "$res.log" 2>/dev/null
echo "(min/median of $rounds interleaved rounds; --cpus $CPUS --memory $MEM; load $(cut -d' ' -f1 /proc/loadavg))"
