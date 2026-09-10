#!/usr/bin/env bash
# sid-shot.sh — launch sid, screenshot a given tab, clean up.
#
# Usage:
#   scripts/sid-shot.sh [--tab ssh|database|network|workspaces|system|settings] [--real]
#                        [--keep] [--out PATH] [--wait SECS]
#
# Hermetic by default: runs against a throwaway XDG_DATA_HOME/XDG_STATE_HOME/
# XDG_CONFIG_HOME (a fresh `mktemp -d`), so the app boots on its own demo seed and
# never touches the real store. Pass --real to use the live environment instead.
#
# The window opens on a TEMPORARY HEADLESS OUTPUT (hyprctl output create
# headless + a `workspace … silent` windowrule on class `sid`) and is captured
# from there — nothing ever flashes onto the user's visible workspace. Before
# capturing, the script CONFIRMS via `hyprctl clients -j` that the launched
# window actually landed on that headless output (matched by pid); if it
# never does within the poll timeout, this exits non-zero with a one-line
# reason instead of capturing whatever happens to be at that geometry. After
# `grim`, the PNG itself is sanity-checked (non-empty, dimensions matching the
# headless output's mode) before its path is printed — a wrong-output or
# short-lived capture fails loudly rather than silently naming the wrong PNG.
#
# LIMITATION: this captures via the running session's screencopy, so a LOCKED
# session yields hyprlock's surface (by design — a Wayland security property).
# For lock-proof / fully-detached captures use scripts/sid-cap.sh (a private
# headless sway compositor; needs `sway` installed).
#
# LIMITATION: some Hyprland builds run a Lua config parser that rejects
# `hyprctl keyword`/legacy two-arg `hyprctl dispatch` calls outright — hyprctl
# still exits 0, printing a rejection string instead of erroring, so this was
# silently swallowed by the existing `|| true`s and the sid window never
# actually moved off-screen. On such a build the landed-on-headless-output
# check below correctly fails closed (a one-line reason, no capture) instead
# of grim-cropping whatever the real screen shows at that geometry.
#
# Shares repo-root discovery, hermetic XDG setup, launch/poll-for-window, and
# the cleanup/--keep/print-path plumbing with scripts/sid-cap.sh via
# scripts/lib/sid-app.sh — see that file for what's shared vs. kept here
# (the hyprctl headless-output dance, the landed-on-output verification, and
# the PNG sanity check).
#
# Requires a live Wayland session: hyprctl (Hyprland), grim, jq.
#
# Prints the screenshot path as the last line of stdout; everything else (build
# output, progress) goes to stderr.

set -uo pipefail

source "$(dirname -- "${BASH_SOURCE[0]}")/lib/sid-app.sh"
sid_app_locate_repo

TAB="ssh"
REAL=0
KEEP=0
OUT=""
WAIT_SECS=3
POLL_TIMEOUT=15

usage() {
    cat <<'EOF' >&2
Usage: scripts/sid-shot.sh [--tab ssh|database|network|workspaces|system|settings] [--real] [--keep] [--out PATH] [--wait SECS]
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
    ssh | database | network | workspaces | system | settings) ;;
    *)
        echo "sid-shot: invalid --tab '$TAB' (want ssh|database|network|workspaces|system|settings)" >&2
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
        sid_app_kill_if_set "$APP_PID"
        wait "$APP_PID" 2>/dev/null || true
    fi
    if [[ "$RULE_SET" -eq 1 ]]; then
        hyprctl keyword windowrulev2 "unset, class:^(sid)\$" >/dev/null 2>&1 || true
    fi
    if [[ -n "$HEADLESS_OUT" ]] && [[ "$KEEP" -eq 0 ]]; then
        hyprctl output remove "$HEADLESS_OUT" >/dev/null 2>&1 || true
    fi
    sid_app_rm_unless_keep "$KEEP" "$TMP_XDG"
}
trap cleanup EXIT

# A private virtual monitor + a silent windowrule keep the capture run
# completely off the user's visible workspaces. The workspace number just
# needs to be one nothing else is using — pid-derived is unique enough.
HEADLESS_OUT="sid-shot-$$"
HEADLESS_MON=""
CAP_WS=$((RANDOM % 1000 + 9000))
if hyprctl output create headless "$HEADLESS_OUT" >/dev/null 2>&1; then
    HEADLESS_MON="$(hyprctl monitors -j | jq -r --arg name "$HEADLESS_OUT" '.[] | select(.name==$name) | .id')"
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

sid_app_export_tab "$TAB"
if [[ "$REAL" -eq 0 ]]; then
    TMP_XDG="$(mktemp -d /tmp/sid-shot-xdg.XXXXXX)"
    sid_app_setup_xdg 0 "$TMP_XDG"
    echo "sid-shot: hermetic run — XDG home = $TMP_XDG" >&2
else
    echo "sid-shot: --real — using the live environment" >&2
fi

"$BIN" &
APP_PID=$!
echo "sid-shot: launched pid $APP_PID (tab=$TAB)" >&2

# GEOM is a side-effect output of the detect functions below: the matching
# `hyprctl clients -j` entry as of the last poll tick.
GEOM=""

sid_shot_detect_pid() {
    local pid="$1"
    GEOM="$(hyprctl clients -j | jq -c --argjson pid "$pid" '[.[] | select(.pid == $pid)][0] // empty')"
    [[ -n "$GEOM" ]]
}

sid_app_wait_for_window "$APP_PID" "$POLL_TIMEOUT" 0.3 "" sid_shot_detect_pid
rc=$?
if [[ $rc -eq 1 ]]; then
    echo "sid-shot: pid $APP_PID exited before its window appeared" >&2
    exit 1
elif [[ $rc -eq 2 ]]; then
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

    # CONFIRM it actually landed there before trusting its geometry: grim -g
    # crops whatever is at X,Y on the compositor's global canvas, so a window
    # that never made it off the real screen still yields a correctly-sized
    # PNG of the WRONG content (the bug this closes). Re-issue the move each
    # tick — the workspace can still be settling — and require BOTH class
    # "sid" and the headless output's monitor id before proceeding.
    sid_shot_landed_on_headless() {
        local pid="$1" want_mon="$2"
        hyprctl dispatch moveworkspacetomonitor "$CAP_WS" "$HEADLESS_OUT" >/dev/null 2>&1 || true
        GEOM="$(hyprctl clients -j | jq -c --argjson pid "$pid" '[.[] | select(.pid == $pid and .class == "sid")][0] // empty' 2>/dev/null)"
        [[ -n "$GEOM" ]] || return 1
        [[ "$(jq -r '.monitor' <<<"$GEOM")" == "$want_mon" ]]
    }

    sid_app_wait_for_window "$APP_PID" "$POLL_TIMEOUT" 0.3 "" sid_shot_landed_on_headless "$HEADLESS_MON"
    rc=$?
    if [[ $rc -ne 0 ]]; then
        mon="$(jq -r '.monitor // "?"' <<<"${GEOM:-{}}")"
        ws="$(jq -r '.workspace.name // "?"' <<<"${GEOM:-{}}")"
        echo "sid-shot: sid window (pid $APP_PID) never landed on headless output $HEADLESS_OUT (monitor id $HEADLESS_MON) within ${POLL_TIMEOUT}s — last seen monitor=$mon workspace=$ws — refusing to capture" >&2
        exit 1
    fi
fi

X="$(jq -r '.at[0]' <<<"$GEOM")"
Y="$(jq -r '.at[1]' <<<"$GEOM")"
W="$(jq -r '.size[0]' <<<"$GEOM")"
H="$(jq -r '.size[1]' <<<"$GEOM")"

echo "sid-shot: window at ${X},${Y} ${W}x${H} — settling ${WAIT_SECS}s before capture" >&2
sleep "$WAIT_SECS"

grim -g "${X},${Y} ${W}x${H}" "$OUT" || { echo "sid-shot: grim capture failed" >&2; exit 1; }

# Sanity-check the PNG before printing its path: non-empty, and its
# dimensions match the headless output's actual mode (or, with no headless
# output, the geometry we asked grim to crop) — catches a truncated or
# stale-content capture that grim otherwise "successfully" writes.
if [[ ! -s "$OUT" ]]; then
    echo "sid-shot: capture failed sanity check — $OUT is empty or missing" >&2
    exit 1
fi

EXPECT_W="$W"
EXPECT_H="$H"
if [[ -n "$HEADLESS_OUT" ]]; then
    read -r hw hh < <(hyprctl monitors -j | jq -r --arg name "$HEADLESS_OUT" '.[] | select(.name==$name) | "\(.width) \(.height)"')
    [[ -n "${hw:-}" && -n "${hh:-}" ]] && { EXPECT_W="$hw"; EXPECT_H="$hh"; }
fi

if command -v identify >/dev/null 2>&1; then
    DIMS="$(identify -format '%wx%h' "$OUT" 2>/dev/null)"
    if [[ "$DIMS" != "${EXPECT_W}x${EXPECT_H}" ]]; then
        echo "sid-shot: capture failed sanity check — $OUT is ${DIMS:-unreadable}, expected ${EXPECT_W}x${EXPECT_H}" >&2
        exit 1
    fi
elif command -v file >/dev/null 2>&1; then
    FILE_INFO="$(file -b "$OUT")"
    if [[ "$FILE_INFO" != *"${EXPECT_W} x ${EXPECT_H}"* ]]; then
        echo "sid-shot: capture failed sanity check — file(1) says '$FILE_INFO', expected ${EXPECT_W} x ${EXPECT_H}" >&2
        exit 1
    fi
fi

sid_app_emit_result "sid-shot" "$OUT"
