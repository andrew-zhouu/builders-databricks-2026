#!/usr/bin/env bash
# Runs ON the remote machine: what does output still dirty at exit cost after
# the container has exited? For an image and a data set: the program's own
# time, the container's (StartedAt..FinishedAt), the docker command's, and the
# dirty page cache right after the command returns. Extra arguments are passed
# to docker create (for example -e SS_SYNC=1).
# Usage: tdcheck.sh IMAGE_TAG DATASET [ROUNDS] [docker create args...]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
tag="$1"; name="$2"; rounds="${3:-3}"; shift 3 2>/dev/null || shift $#
cat "data/$name"/*/*.csv > /dev/null
out=/tmp/ss-tdc-out; scratch=/tmp/ss-tdc-scratch
for r in $(seq "$rounds"); do
  sudo rm -rf "$out" "$scratch"; mkdir -p "$out" "$scratch"; chmod 777 "$out" "$scratch"
  sync; sleep 1
  cid=$(docker create --platform linux/amd64 --network none --read-only --tmpfs /tmp:rw,size=64m,mode=1777 \
    --cap-drop ALL --security-opt no-new-privileges --pids-limit 4096 --user 1000:1000 --cpus 8 --memory 24g --memory-swap 24g \
    -v "$PROJ/data/$name":/data:ro -v "$out":/output -v "$scratch":/scratch \
    -e DATA_DIR=/data -e OUTPUT_DIR=/output -e SCRATCH_DIR=/scratch -e HOME=/scratch -e NCPU=8 -e MEM_GB=24 \
    -e SS_TIMING=1 "$@" "$tag")
  s=$(date +%s%N); docker start -a "$cid" > /tmp/ss-tdc.log 2>&1; e=$(date +%s%N)
  dirty=$(awk '/^Dirty:|^Writeback:/ { s += $2 } END { print int(s / 1024) }' /proc/meminfo)
  stamps=$(docker inspect "$cid" --format '{{.State.StartedAt}} {{.State.FinishedAt}}')
  docker rm "$cid" > /dev/null
  done_ms=$(grep -E 'ms\] done' /tmp/ss-tdc.log | sed 's/^\[ *\([0-9.]*\) ms.*/\1/')
  python3 -I - "$stamps" "$(( (e - s) / 1000000 ))" "${done_ms:-0}" "$dirty" "$(du -sm "$out" | cut -f1)" "$(ls "$out"/user_daily | wc -l)" <<'EOF'
import sys, datetime
a, b = sys.argv[1].split()
def t(x):
    d, frac = x.rstrip("Z").split(".")
    return datetime.datetime.strptime(d, "%Y-%m-%dT%H:%M:%S").timestamp() + float("0." + frac)
print(f"program {float(sys.argv[3]):7.1f}  container {1000*(t(b)-t(a)):7.1f}  docker start -a {int(sys.argv[2]):5d}  dirty after {sys.argv[4]:>4} MB  output {sys.argv[5]} MB in {sys.argv[6]} files")
EOF
done
sudo rm -rf "$out" "$scratch"
