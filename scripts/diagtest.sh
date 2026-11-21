#!/usr/bin/env bash
# Build the image of the `diag` branch from a clean export and run it the way
# the scorer does on the dev set. Expected: its log, then exit code 1.
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
cd "$PROJ"
ST="$PROJ/official/starter-duckdb/x/starter-duckdb"
W="$PROJ/out/diagtest"; rm -rf "$W"; mkdir -p "$W/src"
git archive diag | tar -x -C "$W/src"
(cd "$W/src" && docker buildx build --platform linux/amd64 --load -t shopping-sessions:diag .) >"$W/build.log" 2>&1 || { tail -5 "$W/build.log"; exit 1; }
docker image inspect shopping-sessions:diag --format 'image env {{json .Config.Env}} cmd {{json .Config.Cmd}}'
IMAGE=shopping-sessions:diag CPUS=8 MEM=12g PLATFORM=linux/amd64 sh "$ST/scripts/docker_run.sh" "$PROJ/data/dev" "$W/out" >"$W/run.log" 2>&1
echo "exit code: $? (1 is the intended result)"
grep -cE 'diag:' "$W/run.log" | sed 's/^/diag lines: /'
grep -E '^== diag|model name|cache index[23]|transparent_hugepage/enabled|cpu.max|memory.max|perf counters|mounts: .* /data |sections: cpu' "$W/run.log" | cut -c1-170
wc -c < "$W/run.log" | sed 's/^/log bytes: /'
