#!/usr/bin/env bash
# Runs ON the remote machine: where does the time between the program's last
# line and the end of `docker run` go? One line per variant: the container's own
# wall time (StartedAt..FinishedAt), the wall time of the docker command, and
# the program's "done" stamp.
# Usage: teardown.sh IMAGE_TAG DATASET
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
tag="$1"; name="$2"
cat "data/$name"/*/*.csv > /dev/null
one() { # label outdir extra-docker-args...
  local label="$1" out="$2"; shift 2
  local scratch=/tmp/ss-td-scratch
  sudo rm -rf "$out" "$scratch"; mkdir -p "$out" "$scratch"; chmod 777 "$out" "$scratch"
  local cid; cid=$(docker create --platform linux/amd64 --network none --read-only --tmpfs /tmp:rw,size=64m,mode=1777 \
    --cap-drop ALL --security-opt no-new-privileges --pids-limit 4096 --user 1000:1000 \
    -v "$PROJ/data/$name":/data:ro -v "$out":/output -v "$scratch":/scratch \
    -e DATA_DIR=/data -e OUTPUT_DIR=/output -e SCRATCH_DIR=/scratch -e HOME=/scratch -e NCPU=8 -e MEM_GB=24 \
    -e SS_TIMING=1 "$@" "$tag")
  local s e; s=$(date +%s%N); docker start -a "$cid" > /tmp/ss-td.log 2>&1; e=$(date +%s%N)
  local stamps; stamps=$(docker inspect "$cid" --format '{{.State.StartedAt}} {{.State.FinishedAt}}')
  local s2 e2; s2=$(date +%s%N); docker rm "$cid" > /dev/null; e2=$(date +%s%N)
  local done_ms; done_ms=$(grep -E 'ms\] done' /tmp/ss-td.log | sed 's/^\[ *\([0-9.]*\) ms.*/\1/')
  python3 -I - "$stamps" "$(( (e - s) / 1000000 ))" "$(( (e2 - s2) / 1000000 ))" "${done_ms:-0}" "$label" "$(du -sm "$out" | cut -f1)" <<'EOF'
import sys, datetime
a, b = sys.argv[1].split()
def t(x):
    d, frac = x.rstrip("Z").split(".")
    return datetime.datetime.strptime(d, "%Y-%m-%dT%H:%M:%S").timestamp() + float("0." + frac)
print(f"{sys.argv[5]:34s} program done {float(sys.argv[4]):7.1f}  container {1000*(t(b)-t(a)):7.1f}  docker start -a {int(sys.argv[2]):5d}  docker rm {int(sys.argv[3]):4d}  output {sys.argv[6]} MB")
EOF
  sudo rm -rf "$out" "$scratch"
}
L="--cpus 8 --memory 24g --memory-swap 24g"
one "scorer flags"                 /tmp/ss-td-out $L
one "scorer flags (again)"         /tmp/ss-td-out $L
one "no parallel teardown"         /tmp/ss-td-out $L -e SS_NOZAP=1
one "no memory limit"              /tmp/ss-td-out --cpus 8
one "no cpu or memory limit"       /tmp/ss-td-out
one "output on tmpfs"              /dev/shm/ss-td-out $L
