#!/usr/bin/env bash
# Where does the time go? Runs the binary under several switch settings,
# interleaved, and prints per variant the MEDIAN over the rounds of: wall ms,
# user-mode ms, kernel ms, page faults, and kernel ms per group of sections.
# Usage: sysvar.sh DATASET ROUNDS VARIANT...
#   a variant is comma-separated env assignments; X=1 is the default build;
#   SSBIN=name runs bin/name (see snap.sh).
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
BIN="${BIN:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
name="$1"; rounds="$2"; shift 2
out="/tmp/ss-sysvar-$$"; res="/tmp/ss-sysvar-$$.res"; : > "$res"
for r in $(seq "$rounds"); do
  for v in "$@"; do
    envs="${v//,/ }"; bin="$BIN"
    for kv in $envs; do case "$kv" in SSBIN=*) bin="$PROJ/bin/${kv#SSBIN=}";; esac; done
    rm -rf "$out"; mkdir -p "$out"
    env $envs NCPU="${NCPU:-8}" SS_TIMING=1 "$bin" "$PROJ/data/$name" "$out" 2>"$out.log"
    awk -v v="$v" '
      /parse events/ {e=$5} /parse clicks/ {c=$5} /dedup/ {d+=$5} /sessions (gather|sort|people)/ {s+=$5} /^ *links/ {l=$4}
      /process: user/ { n=split($0, a, /[ ,]+/); for(i=1;i<=n;i++){ if(a[i]=="process:"){u=a[i+2]; y=a[i+5]; f=a[i+7]} } }
      /\] done;/ { w=$0; sub(/^\[ */,"",w); sub(/ ms.*/,"",w) }
      END {print v, w, u, y, f, e, c, d, s, l}' "$out.log" >> "$res"
  done
done
rm -rf "$out" "$out.log"
printf '%-36s %6s %6s %6s %7s | %s\n' variant wall user sys faults "kernel ms in: events clicks dedup sessions links"
for v in "$@"; do
  awk -v v="$v" '$1==v' "$res" | {
    declare -a col
    med() { sort -n | awk '{a[NR]=$1} END{print a[int((NR+1)/2)]}'; }
    rows=$(cat)
    for i in 2 3 4 5 6 7 8 9 10; do col[$i]=$(echo "$rows" | awk -v i="$i" '{print $i}' | med); done
    printf '%-36s %6.0f %6.0f %6.0f %7.0f | %6.0f %6.0f %6.0f %6.0f %6.0f\n' "$v" "${col[2]}" "${col[3]}" "${col[4]}" "${col[5]}" "${col[6]}" "${col[7]}" "${col[8]}" "${col[9]}" "${col[10]}"
  }
done
rm -f "$res"
echo "(medians of $rounds interleaved rounds; load $(cut -d' ' -f1 /proc/loadavg))"
