#!/usr/bin/env bash
# move_to X Y — converge the cursor onto a target via relative ydotool moves
# with hyprctl read-back (robust to any abs-axis mapping / pointer accel).
set -u
export YDOTOOL_SOCKET="${XDG_RUNTIME_DIR}/.ydotool_socket"

move_to() {
    local tx=$1 ty=$2 i cur cx cy dx dy
    for i in $(seq 1 8); do
        cur=$(hyprctl cursorpos)
        cx=${cur%%,*}; cy=${cur##*, }
        dx=$((tx - cx)); dy=$((ty - cy))
        if [ "${dx#-}" -le 2 ] && [ "${dy#-}" -le 2 ]; then return 0; fi
        ydotool mousemove -x "$dx" -y "$dy"
        sleep 0.15
    done
    cur=$(hyprctl cursorpos)
    echo "move_to: settled at $cur (target $tx,$ty)" >&2
}

click_at() {
    move_to "$1" "$2"
    sleep 0.1
    ydotool click 0xC0
    sleep 0.2
}

"$@"
