#!/usr/bin/env bash
# Runs ON the remote machine: build the image of an exported revision (with the
# build cache) and time it in the container with the scorer's flags, twice per
# data set: program, container (StartedAt..FinishedAt) and docker command.
# Usage: imgtime.sh EXPORT_DIR [DATASET...]      (default: m20 m60)
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
src="$1"; shift
[ $# -gt 0 ] || set -- m20 m60
tag="shopping-sessions:$(basename "$src")"
(cd "$src" && docker build -q -t "$tag" . > /dev/null) || { echo "build failed"; exit 1; }
for name in "$@"; do
  echo "== $tag on $name"
  bash scripts/remote/teardown.sh "$tag" "$name" | head -2
done
