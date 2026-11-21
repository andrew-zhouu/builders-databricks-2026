#!/usr/bin/env bash
# One-time preparation of the (still unpublished) history for publication:
#  1. commit pending work on the development line;
#  2. remove a personal address that early versions of scripts/commit.sh carried;
#  3. put the TEAM.md commit right after the chosen candidate, so that
#       main = candidate + TEAM.md   (what gets submitted)
#       dev  = main + later development
# Usage: prepare_publish.sh "SUBJECT PREFIX OF CANDIDATE COMMIT"
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
cd "$PROJ"
subject="$1"
if git remote | grep -q .; then echo "refusing: a remote is configured"; exit 1; fi

git add -A
git commit -q -m "Faster hasher for the plain fallback; quiet-machine benchmark and publication scripts" || true

FILTER_BRANCH_SQUELCH_WARNING=1 git filter-branch -f --tree-filter '
  if [ -f scripts/commit.sh ]; then
    sed -i -e "s/[A-Za-z0-9._%+-]*@gmail\.com/team@users.noreply.github.com/g" \
           -e "s/GIT_AUTHOR_NAME:-[a-z]*}/GIT_AUTHOR_NAME:-team}/g" scripts/commit.sh
  fi' -- --all >/dev/null
git for-each-ref refs/original --format='%(refname)' | while read -r ref; do git update-ref -d "$ref"; done

left=$(for rev in $(git rev-list --all); do git grep -lI 'gmail\.com' "$rev" -- . 2>/dev/null || true; done | wc -l)
echo "files in history still holding the address: $left"
[ "$left" = 0 ]

C=$(git log --format=%H --fixed-strings --grep="$subject" main | tail -1)
[ -n "$C" ] || { echo "candidate not found"; exit 1; }
echo "candidate: $(git log -1 --format='%h %s' "$C")"

git checkout -q -B submission "$C"
cat > TEAM.md <<'EOF'
# Team

Name:
Members: andrew-zhouu
Engine: Rust (single static binary: hand-written AVX2 CSV parser, radix partitioning, custom Parquet writer)
EOF
git add TEAM.md
git commit -q -m "TEAM.md: members and engine"
T=$(git rev-parse HEAD)
git rebase -q --onto "$T" "$C" main
git branch -m main dev
git branch -m submission main
git checkout -q dev
echo "--- main"; git log --oneline main | head -4
echo "--- dev (ahead of main)"; git log --oneline main..dev
git status --short | head
