#!/usr/bin/env bash
# How many symbolic links each published branch's tree holds (there must be none).
# Usage: links.sh [BRANCH...]
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
cd "$PROJ"
git fetch -q origin
[ $# -gt 0 ] || set -- main dev exp-split exp-simd exp-tests exp-index exp-wix
for b in "$@"; do
  echo "$b: $(git ls-tree -r "origin/$b" | grep -c '^120000' || true) links ($(git rev-parse --short "origin/$b"))"
done
