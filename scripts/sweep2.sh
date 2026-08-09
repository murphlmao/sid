#!/usr/bin/env bash
# sweep2.sh — price one table cell, on the quiescent Network tab.
#
# The System tab was unusable as a measurement surface: its 2s sysinfo probe
# burns 0.6-2.5G instructions a tick (vs ~50M for a frame) and its cost tracks
# the box's process count, which four other agents are churning. The Network tab
# polls nothing — measured idle: 0 frames, 6.8M instructions in 8s — so
# instructions/frame needs no baseline subtraction at all.
#
# Frames come from pointer motion (each row crossing flips the hover state and
# costs one full frame), not the wheel: it works on a 35-row table and does not
# run out of scroll.
set -uo pipefail
S="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
BIN="$1"; TAG="${2:-net}"
run() { # height hover_y0 hover_y1
    local h="$1" y0="$2" y1="$3"
    bash "$S/perfcap.sh" --bin "$BIN" --out "$S/$TAG-$h" --tab network --mode hover \
        --hover "960,$y0,$y1" --notches 400 --gap 12 --window 8 --size "1920x$h" \
        --park "960,$y0" 2>&1 | grep -E "^(==|-- (idle|scroll)|\*\*)"
    bash "$S/perfcap.sh" --bin "$BIN" --out "$S/$TAG-cells-$h" --tab network --mode hover \
        --hover "960,$y0,$y1" --notches 150 --gap 12 --window 5 --size "1920x$h" \
        --park "960,$y0" --env GPUI_MEASUREMENTS=1 >/dev/null 2>&1
    echo "   cells/frame @${h}px: $(grep -o 'last render [0-9]* cells' "$S/$TAG-cells-$h/sid.log" | awk '{print $3}' | sort -n | uniq -c | sort -rn | head -3 | tr '\n' ' ')"
}
run 1080 320 1000
run 800 320 760
run 620 300 590
