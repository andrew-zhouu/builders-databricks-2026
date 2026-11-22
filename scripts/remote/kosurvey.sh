#!/usr/bin/env bash
# Runs ON the remote machine (under the lock): wall time of the current build
# (which must have the SS_KO knock-out switches) with each part knocked out in
# turn, against the whole: what each part costs. Timing only: a knocked-out
# run gives wrong answers.
# Usage: kosurvey.sh DATASET ROUNDS BASE_SETTING KO...
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
name="$1"; rounds="$2"; base="$3"; shift 3
variants="$base"
for k in "$@"; do variants="$variants $base,SS_KO=$k"; done
bash scripts/abx.sh "$name" "$rounds" $variants 2>&1 | tail -$(( $# + 3 ))
