#!/usr/bin/env bash
# A second working tree for a parallel line of work: a git worktree next to
# this one on a new branch, with its own directory on the remote machine that
# shares the data sets, the reference answers and the toolchain with the main
# one (symbolic links) and has its own sources, build and output.
# Usage: newtree.sh NAME [START_POINT]      -> ../db-NAME, branch exp-NAME, remote ~/db_NAME
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
cd "$PROJ"
name="$1"; start="${2:-HEAD}"
tree="$(dirname "$PROJ")/db-$name"
. "$PROJ/.local/remote.env"
main_dir="$REMOTE_DIR"
new_dir="$(dirname "$REMOTE_DIR")/db_$name"
git worktree add -q -b "exp-$name" "$tree" "$start"
mkdir -p "$tree/.local"
sed "s#^REMOTE_DIR=.*#REMOTE_DIR=$new_dir#" "$PROJ/.local/remote.env" > "$tree/.local/remote.env"
# The official material (generator, rules, fixtures) is not in git.
[ -d "$PROJ/official" ] && ln -s "$PROJ/official" "$tree/official"
bash scripts/remote.sh quick "mkdir -p $new_dir/out $new_dir/bin && cd $new_dir && for d in data expected .toolchain .venv official; do [ -e \$d ] || ln -s $main_dir/\$d \$d; done && cp -n $main_dir/bin/scored $main_dir/bin/oracle $main_dir/bin/tbl $main_dir/bin/c8 bin/ 2>/dev/null; ls -la $new_dir | head -20"
(cd "$tree" && bash scripts/remote.sh push > /dev/null && echo "pushed $tree to $new_dir")
echo "tree $tree on branch exp-$name; remote $new_dir"
