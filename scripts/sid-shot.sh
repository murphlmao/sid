#!/usr/bin/env bash
# sid-shot.sh — launch sid, screenshot a given tab, clean up.
#
# Usage:
#   scripts/sid-shot.sh [--tab ssh|database|network|workspaces|system] [--real]
#                        [--keep] [--out PATH] [--wait SECS]
#
# Hermetic by default: runs against a throwaway XDG_DATA_HOME/XDG_STATE_HOME/
# XDG_CONFIG_HOME (a fresh `mktemp -d`), so the app boots on its own demo seed and
# never touches the real store. Pass --real to use the live environment instead.
#
# The window opens on a TEMPORARY HEADLESS OUTPUT (hyprctl output create
# headless + a `workspace … silent` windowrule on class `sid`) and is captured
# from there — nothing ever flashes onto the user's visible workspace.
#
# LIMITATION: this captures via the running session's screencopy, so a LOCKED
# session yields hyprlock's surface (by design — a Wayland security property).
# For lock-proof / fully-detached captures use scripts/sid-cap.sh (a private
# headless sway compositor; needs `sway` installed).
#
# Requires a live Wayland session: hyprctl (Hyprland), grim, jq.
#
# Prints the screenshot path as the last line of stdout; everything else (build
# output, progress) goes to stderr.

set -uo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." >/dev/null 2>&1 && pwd)"

TAB="ssh"
REAL=0
KEEP=0
OUT=""
WAIT_SECS=3
POLL_TIMEOUT=15

usage() {
    cat <<'EOF' >&2
Usage: scripts/sid-shot.sh [--tab ssh|database|network|workspaces|system] [--real] [--keep] [--out PATH] [--wait SECS]
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --tab)
            TAB="${2:-}"
            shift 2
            ;;
        --real)
            REAL=1
            shift
            ;;
        --keep)
            KEEP=1
            shift
            ;;
        --out)
            OUT="${2:-}"
            shift 2
            ;;
        --wait)
            WAIT_SECS="${2:-}"
            shift 2
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            echo "sid-shot: unknown argument: $1" >&2
            usage
            exit 1
            ;;
    esac
done

case "$TAB" in
    ssh | database | network | workspaces | system) ;;
    *)
        echo "sid-shot: invalid --tab '$TAB' (want ssh|database|network|workspaces|system)" >&2
        exit 1
        ;;
esac

for bin in hyprctl grim jq cargo; do
    if ! command -v "$bin" >/dev/null 2>&1; then
        echo "sid-shot: required tool '$bin' not found on PATH" >&2
        exit 1
    fi
done

if [[ -z "$OUT" ]]; then
    OUT="/tmp/sid-shot-${TAB}-$(date +%s).png"
fi

TMP_XDG=""
APP_PID=""
HEADLESS_OUT=""
RULE_SET=0

cleanup() {
    if [[ -n "$APP_PID" ]] && [[ "$KEEP" -eq 0 ]]; then
        kill "$APP_PID" >/dev/null 2>&1 || true
        wait "$APP_PID" 2>/dev/null || true
    fi
    if [[ "$RULE_SET" -eq 1 ]]; then
        hyprctl keyword windowrulev2 "unset, class:^(sid)\$" >/dev/null 2>&1 || true
    fi
    if [[ -n "$HEADLESS_OUT" ]] && [[ "$KEEP" -eq 0 ]]; then
        hyprctl output remove "$HEADLESS_OUT" >/dev/null 2>&1 || true
    fi
    if [[ -n "$TMP_XDG" ]] && [[ "$KEEP" -eq 0 ]]; then
        rm -rf -- "$TMP_XDG"
    fi
}
trap cleanup EXIT

# A private virtual monitor + a silent windowrule keep the capture run
# completely off the user's visible workspaces. The workspace number just
# needs to be one nothing else is using — pid-derived is unique enough.
HEADLESS_OUT="sid-shot-$$"
CAP_WS=$((RANDOM % 1000 + 9000))
if hyprctl output create headless "$HEADLESS_OUT" >/dev/null 2>&1; then
    hyprctl keyword monitor "$HEADLESS_OUT,1920x1080,auto,1" >/dev/null 2>&1 || true
    hyprctl keyword windowrulev2 "workspace $CAP_WS silent, class:^(sid)\$" >/dev/null 2>&1 && RULE_SET=1
    hyprctl dispatch moveworkspacetomonitor "$CAP_WS" "$HEADLESS_OUT" >/dev/null 2>&1 || true
else
    echo "sid-shot: could not create a headless output — falling back to on-screen capture" >&2
    HEADLESS_OUT=""
fi

echo "sid-shot: building sid (cargo build -p sid)…" >&2
(cd "$REPO_ROOT" && cargo build -p sid) >&2
build_status=$?
if [[ $build_status -ne 0 ]]; then
    echo "sid-shot: cargo build -p sid failed (exit $build_status)" >&2
    exit "$build_status"
fi

BIN="$REPO_ROOT/target/debug/sid"
if [[ ! -x "$BIN" ]]; then
    echo "sid-shot: expected binary not found or not executable: $BIN" >&2
    exit 1
fi

export SID_START_TAB="$TAB"
if [[ "$REAL" -eq 0 ]]; then
    TMP_XDG="$(mktemp -d /tmp/sid-shot-xdg.XXXXXX)"
    mkdir -p "$TMP_XDG/data" "$TMP_XDG/state" "$TMP_XDG/config"
    export XDG_DATA_HOME="$TMP_XDG/data"
    export XDG_STATE_HOME="$TMP_XDG/state"
    export XDG_CONFIG_HOME="$TMP_XDG/config"
    echo "sid-shot: hermetic run — XDG home = $TMP_XDG" >&2
else
    echo "sid-shot: --real — using the live environment" >&2
fi

"$BIN" &
APP_PID=$!
echo "sid-shot: launched pid $APP_PID (tab=$TAB)" >&2

GEOM=""
SECONDS=0
while [[ $SECONDS -lt $POLL_TIMEOUT ]]; do
    if ! kill -0 "$APP_PID" 2>/dev/null; then
        echo "sid-shot: pid $APP_PID exited before its window appeared" >&2
        exit 1
    fi
    GEOM="$(hyprctl clients -j | jq -c --argjson pid "$APP_PID" '[.[] | select(.pid == $pid)][0] // empty')"
    if [[ -n "$GEOM" ]]; then
        break
    fi
    sleep 0.3
done

if [[ -z "$GEOM" ]]; then
    echo "sid-shot: no window for pid $APP_PID appeared within ${POLL_TIMEOUT}s" >&2
    echo "sid-shot: current hyprctl window classes:" >&2
    hyprctl clients -j | jq '.[].class' >&2
    exit 1
fi

# Workspaces are created lazily — now that the window exists, (re-)pin its
# workspace to the headless output so the geometry below is off-screen.
if [[ -n "$HEADLESS_OUT" ]]; then
    hyprctl dispatch moveworkspacetomonitor "$CAP_WS" "$HEADLESS_OUT" >/dev/null 2>&1 || true
    sleep 0.3
    GEOM="$(hyprctl clients -j | jq -c --argjson pid "$APP_PID" '[.[] | select(.pid == $pid)][0] // empty')"
fi

X="$(jq -r '.at[0]' <<<"$GEOM")"
Y="$(jq -r '.at[1]' <<<"$GEOM")"
W="$(jq -r '.size[0]' <<<"$GEOM")"
H="$(jq -r '.size[1]' <<<"$GEOM")"

echo "sid-shot: window at ${X},${Y} ${W}x${H} — settling ${WAIT_SECS}s before capture" >&2
sleep "$WAIT_SECS"

grim -g "${X},${Y} ${W}x${H}" "$OUT"
echo "sid-shot: wrote $OUT" >&2

echo "$OUT"
