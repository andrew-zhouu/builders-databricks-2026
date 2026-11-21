#!/usr/bin/env bash
# Runs ON the remote machine: time an image inside the container with the
# scorer's flags, several times, with the program's own timeline. Shows how the
# container's wall time (docker's own start/finish stamps, which is what a
# scorer would use) relates to the program's.
# Usage: dockertime.sh IMAGE_TAG DATASET [RUNS] [EXTRA_DOCKER_ARGS...]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
tag="$1"; name="$2"; runs="${3:-3}"; shift 3 2>/dev/null || shift $#
out=/tmp/ss-dt-out; scratch=/tmp/ss-dt-scratch
cat "data/$name"/*/*.csv > /dev/null
for i in $(seq "$runs"); do
  sudo rm -rf "$out" "$scratch"; mkdir -p "$out" "$scratch"; chmod 777 "$out" "$scratch"
  cid=$(docker create --platform linux/amd64 --network none --read-only --tmpfs /tmp:rw,size=64m,mode=1777 \
    --cap-drop ALL --security-opt no-new-privileges --pids-limit 4096 --cpus 8 --memory 24g --memory-swap 24g \
    --user 1000:1000 -v "$PROJ/data/$name":/data:ro -v "$out":/output -v "$scratch":/scratch \
    -e DATA_DIR=/data -e OUTPUT_DIR=/output -e SCRATCH_DIR=/scratch -e HOME=/scratch -e NCPU=8 -e MEM_GB=24 \
    -e SS_TIMING=1 "$@" "$tag")
  s=$(date +%s%N); docker start -a "$cid" > /tmp/ss-dt.log 2>&1; rc=$?; e=$(date +%s%N)
  stamps=$(docker inspect "$cid" --format '{{.State.StartedAt}} {{.State.FinishedAt}}')
  docker rm "$cid" > /dev/null
  python3 -I - "$stamps" "$(( (e - s) / 1000000 ))" "$rc" <<'EOF'
import sys, datetime
a, b = sys.argv[1].split()
def t(x):
    d, frac = x.rstrip("Z").split(".")
    return datetime.datetime.strptime(d, "%Y-%m-%dT%H:%M:%S").timestamp() + float("0." + frac)
print(f"container {1000*(t(b)-t(a)):7.1f} ms (docker's StartedAt..FinishedAt); docker start -a took {sys.argv[2]} ms; exit code {sys.argv[3]}")
EOF
  grep -E '^\[ *[0-9.]+ ms\]' /tmp/ss-dt.log | tr '\n' ' ' | sed 's/ms\] /ms /g; s/  */ /g' | cut -c1-330; echo
done
grep -E 'sections: cpu|wall' /tmp/ss-dt.log | cut -c1-120
sudo rm -rf "$out" "$scratch"
