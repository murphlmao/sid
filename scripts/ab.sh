#!/usr/bin/env bash
# ab.sh — interleaved A/B of two sid binaries on the quiescent Network tab.
# Arms alternate every round so a drifting box load hits both equally.
set -uo pipefail
S="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
A="$1"; B="$2"; ROUNDS="${3:-3}"; TAG="${4:-ab}"
for r in $(seq 1 "$ROUNDS"); do
    for arm in A B; do
        bin="$A"; [[ "$arm" == "B" ]] && bin="$B"
        out="$S/$TAG-$arm-$r"
        line=$(bash "$S/perfcap.sh" --bin "$bin" --out "$out" --tab network --mode hover \
            --hover 960,320,1000 --notches 400 --gap 12 --window 8 --park 960,320 2>&1 \
            | grep -E "^(-- scroll|\*\*)" | tr '\n' ' ')
        echo "$arm r$r  $line"
    done
done
