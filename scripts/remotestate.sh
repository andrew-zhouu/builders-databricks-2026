#!/usr/bin/env bash
# What is on GitHub, and how does it relate to the local branches and to the
# scored commit? Fetches; changes nothing else.
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
cd "$PROJ"
GCM_INTERACTIVE=Never GIT_TERMINAL_PROMPT=0 git fetch -q origin '+refs/heads/*:refs/remotes/origin/*' '+refs/pull/*/head:refs/remotes/origin/pr/*' --tags 2>&1 | tail -2
echo "--- remote refs"; git ls-remote origin | awk '{print substr($1,1,12), $2}'
echo "--- local branches"; git branch -vv | cut -c1-110
scored=$(git rev-parse scored-0.85)
for ref in origin/main origin/dev origin/diag main dev; do
  sha=$(git rev-parse -q --verify "$ref") || continue
  rel="does NOT contain the scored commit"; git merge-base --is-ancestor "$scored" "$sha" && rel="contains the scored commit"
  printf '%-12s %s  %s\n' "$ref" "$(git rev-parse --short "$sha")" "$rel"
done
echo "--- origin/main tip"
git log -1 --format='commit %H%nparents %P%nauthor %an%ncommitter %cn%ndate %cI%nsubject %s' origin/main
echo "--- trees: origin/main vs candidates"
for ref in origin/main f1459cc 9fb62dd 5aa33bc dev; do printf '%-12s tree %s  fast/ %s\n' "$ref" "$(git rev-parse --short "$ref^{tree}")" "$(git rev-parse --short "$ref:fast")"; done
echo "--- commits on origin/main not on local main"; git log --oneline main..origin/main | head
echo "--- commits on dev not on origin/main"; git log --oneline origin/main..dev | head
