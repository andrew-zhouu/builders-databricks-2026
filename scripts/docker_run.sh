#!/usr/bin/env bash
# Build the image and run it the way the scorer does (same flags as the official
# starter's scripts/docker_run.sh), then diff against an expected answer.
# Usage: docker_run.sh DATASET EXPECTED_DIR [CPUS] [MEM]
set -u
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
IMAGE="${IMAGE:-shopping-sessions:latest}"
name="$1"; expected="$2"; CPUS="${3:-8}"; MEM="${4:-24g}"
cd "$PROJ"
if [ -z "${NOBUILD:-}" ]; then
  docker buildx build --platform linux/amd64 --load -t "$IMAGE" . 2>&1 | tail -4
fi
docker image ls "$IMAGE" --format 'image {{.Repository}}:{{.Tag}} size {{.Size}}'
DATA="$PROJ/data/$name"
OUT="$PROJ/out/docker-$name"; rm -rf "$OUT"; mkdir -p "$OUT"
SCRATCH=$(mktemp -d)
chmod 777 "$OUT" "$SCRATCH"
s=$(date +%s%N)
docker run --rm --platform linux/amd64 \
  --network none --read-only --tmpfs /tmp:rw,size=64m,mode=1777 \
  --cap-drop ALL --security-opt no-new-privileges \
  --pids-limit 4096 --cpus "$CPUS" --memory "$MEM" --memory-swap "$MEM" \
  --user 1000:1000 \
  -v "$DATA":/data:ro -v "$OUT":/output -v "$SCRATCH":/scratch \
  -e DATA_DIR=/data -e OUTPUT_DIR=/output -e SCRATCH_DIR=/scratch -e HOME=/scratch \
  -e NCPU="${CPUS%.*}" -e MEM_GB="${MEM%g}" ${TIMING:+-e SS_TIMING=1} \
  "$IMAGE"
rc=$?
e=$(date +%s%N)
echo "docker run exit $rc, wall $(( (e - s) / 1000000 )) ms (includes container create/remove)"
rm -rf "$SCRATCH" 2>/dev/null
ls "$OUT"
"$PROJ/.venv/bin/python" -I "$PROJ/tools/cmp.py" "$expected" "$OUT"
