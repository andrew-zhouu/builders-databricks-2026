#!/usr/bin/env bash
# Runs ON the remote machine: build the Docker image of an exported revision
# (remote.sh export REV DIR) exactly as the scorer would, and run it with the
# scorer's flags.
#
#   image.sh diag DIR             build, run on the dev set, show the diagnostic log (exit 1 expected)
#   image.sh gate DIR [SETS...]   build without cache, then with the image's own binary: fixtures,
#                                 dev answer, and each named set against its reference answer
#                                 (default: m5 m20 m60), all inside the container with scorer flags
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
mode="$1"; src="$2"; shift 2
ST="$PROJ/official/starter-duckdb/x/starter-duckdb"
tag="shopping-sessions:$(basename "$src")"
W="$PROJ/out/image-$(basename "$src")"; rm -rf "$W"; mkdir -p "$W"
cache=""; [ "$mode" = gate ] && cache="--no-cache"
# (This machine is x86-64 and has no buildx: the classic builder makes the same image.)
(cd "$src" && docker build $cache -t "$tag" .) > "$W/build.log" 2>&1 || { echo "BUILD FAILED"; tail -15 "$W/build.log"; exit 1; }
echo "built $tag: $(docker image inspect "$tag" --format '{{.Size}} bytes, env {{json .Config.Env}}, cmd {{json .Config.Cmd}}, user {{.Config.User}}')"
run() { # run DATA OUT -> exit code; log in OUT.log
  IMAGE="$tag" CPUS=8 MEM=24g PLATFORM=linux/amd64 sh "$ST/scripts/docker_run.sh" "$1" "$2" > "$2.log" 2>&1
}
case "$mode" in
  diag)
    run "$PROJ/data/dev" "$W/out"; echo "exit code: $? (1 is the intended result); log $(wc -c < "$W/out.log") bytes, $(wc -l < "$W/out.log") lines"
    sed -n '/== diag: probes/,/== diag: run 1/p; /== diag: summary/,$p' "$W/out.log" | cut -c1-230 ;;
  gate)
    rc=0
    # The image's binary, for the checks that drive it directly.
    id=$(docker create "$tag"); docker cp "$id:/sessions" "$W/sessions" >/dev/null; docker rm "$id" >/dev/null
    # AVX-512 code only in the functions behind the run-time processor test.
    if bash scripts/remote/widesyms.sh "$W/sessions" > "$W/widesyms.log" 2>&1; then
      echo "ok   AVX-512 only behind the run-time test ($(head -1 "$W/widesyms.log"))"
    else
      echo "FAIL: AVX-512 instructions outside the run-time-tested functions"; cat "$W/widesyms.log"; rc=1
    fi
    bash tools/fixtures.sh "$W/sessions" > "$W/fixtures.log" 2>&1 || rc=1
    tail -1 "$W/fixtures.log"
    # The same with the wide parser off: the path of every other processor.
    SS_SIMD=0 bash tools/fixtures.sh "$W/sessions" > "$W/fixtures0.log" 2>&1 || rc=1
    echo "wide parser off: $(tail -1 "$W/fixtures0.log")"
    for name in dev m5 m20; do
      [ "$name" = dev ] && want="official/dev-answer/x/dev-answer" || want="expected/$name.slow"
      rm -rf "$W/base-$name"
      if SS_SIMD=0 NCPU=8 "$W/sessions" "data/$name" "$W/base-$name" 2> "$W/base-$name.log" \
         && .venv/bin/python -I tools/cmp.py "$want" "$W/base-$name" > "$W/base-$name.cmp" 2>&1; then
        echo "ok   $name with the wide parser off: $(grep -c identical "$W/base-$name.cmp") tables identical"
      else echo "FAIL $name with the wide parser off"; head -8 "$W/base-$name.cmp"; rc=1; fi
      rm -rf "$W/base-$name"
    done
    [ $# -gt 0 ] || set -- m5 m20 m60
    for name in dev "$@"; do
      [ "$name" = dev ] && want="official/dev-answer/x/dev-answer" || want="expected/$name.slow"
      s=$(date +%s%N)
      if ! run "$PROJ/data/$name" "$W/$name"; then echo "FAIL $name: exit code"; tail -5 "$W/$name.log"; rc=1; continue; fi
      ms=$(( ($(date +%s%N) - s) / 1000000 ))
      if .venv/bin/python -I tools/cmp.py "$want" "$W/$name" > "$W/$name.cmp" 2>&1; then
        echo "ok   $name in the container ($ms ms including docker): $(grep -c identical "$W/$name.cmp") tables identical"
      else echo "FAIL $name in the container"; head -12 "$W/$name.cmp"; rc=1; fi
      sudo rm -rf "$W/$name" 2>/dev/null || rm -rf "$W/$name" 2>/dev/null || true
    done
    [ $rc = 0 ] && echo "IMAGE-GATE-OK $(basename "$src")" || echo "IMAGE-GATE-FAILED"
    exit $rc ;;
esac
