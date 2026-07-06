#!/usr/bin/env bash
# sid-cap.sh — Playwright-style capture harness for the sid GPUI binary.
#
# Runs sid inside its OWN headless sway compositor (a private WAYLAND_DISPLAY
# on the GPU's render node) — completely decoupled from the user's seat
# session. Unlike scripts/sid-shot.sh (which grim-captures the PHYSICAL
# output of the running Hyprland session), this works:
#   - while the session is LOCKED (wlr screencopy shows hyprlock's surface on
#     every real output — a security property, not a bug; the nested sway has
#     no session lock, so its screencopy always sees real window content),
#   - regardless of which workspace/monitor the user is on (nothing ever
#     appears on their screen),
#   - in parallel (each invocation gets its own compositor + hermetic store).
#
# Input injection goes through the nested compositor too: `swaymsg seat`
# drives the pointer (move/click), `wtype` (optional) types text — the
# Wayland-native equivalent of Playwright's page.click()/page.type().
#
# Requirements: sway, grim (pacman -S sway; wtype optional for --type).
#
# Usage:
#   scripts/sid-cap.sh --out shot.png                        # SSH tab, default size
#   scripts/sid-cap.sh --tab system --out sys.png            # any primary tab
#   scripts/sid-cap.sh --tab database --click 300,200 --out after-click.png
#   scripts/sid-cap.sh --tab network --type "postgres" --out filtered.png
#   scripts/sid-cap.sh --tab ssh --wait 8 --out slow.png     # extra settle time
#   scripts/sid-cap.sh --tree                                # dump the window tree (debug)
#
# Flags:
#   --tab  ssh|database|network|workspaces|system|settings   (SID_START_TAB; default ssh)
#   --theme NAME       SID_THEME override (cosmos|void|dusk|cosmos-light)
#   --out  PATH        where the PNG goes (required unless --tree)
#   --size WxH         virtual output size (default 1920x1080)
#   --click X,Y        move pointer + left-click (repeatable, in order)
#   --key  KEYS        key chord, e.g. "Return", "ctrl+tab", "ctrl+shift+t" (repeatable)
#   --type TEXT        wtype literal text (repeatable, needs wtype)
#   --wait SECS        settle time after launch/actions before capture (default 3)
#   --real             use the REAL store (default: hermetic demo-seeded XDG)
#   --keep             leave the compositor + app running (debug; prints env)
#   --tree             print swaymsg -t get_tree instead of capturing
#
# Actions execute in command-line order (click/key/type interleave correctly).

set -uo pipefail

die() { echo "sid-cap: $*" >&2; exit 1; }

TAB="ssh"
THEME=""
OUT=""
SIZE="1920x1080"
WAIT=3
REAL=0
KEEP=0
TREE=0
# Ordered action list: each entry is "click:X,Y" | "key:KEYS" | "type:TEXT".
ACTIONS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --tab)   TAB="$2"; shift 2 ;;
        --theme) THEME="$2"; shift 2 ;;
        --out)   OUT="$2"; shift 2 ;;
        --size)  SIZE="$2"; shift 2 ;;
        --click) ACTIONS+=("click:$2"); shift 2 ;;
        --key)   ACTIONS+=("key:$2"); shift 2 ;;
        --type)  ACTIONS+=("type:$2"); shift 2 ;;
        --wait)  WAIT="$2"; shift 2 ;;
        --real)  REAL=1; shift ;;
        --keep)  KEEP=1; shift ;;
        --tree)  TREE=1; shift ;;
        *) die "unknown argument: $1 (see the header of this script)" ;;
    esac
done

command -v sway >/dev/null 2>&1 || die "sway is not installed — it provides the private headless compositor this harness runs sid inside. Install: sudo pacman -S sway   (wtype too, for --type/--key)"
command -v grim >/dev/null 2>&1 || die "grim is not installed (sudo pacman -S grim)"
[[ "$TREE" -eq 1 || -n "$OUT" ]] || die "--out PATH is required (or --tree)"

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." >/dev/null 2>&1 && pwd)"
SID_BIN="$REPO_ROOT/target/debug/sid"
[[ -x "$SID_BIN" ]] || die "$SID_BIN not built — run: cargo build -p sid"

CAP_DIR="$(mktemp -d -t sid-cap.XXXXXX)"
SWAY_PID=""
APP_PID=""

cleanup() {
    if [[ "$KEEP" -eq 1 ]]; then
        echo "sid-cap: --keep: compositor pid $SWAY_PID, app pid $APP_PID, dir $CAP_DIR" >&2
        echo "sid-cap: --keep: SWAYSOCK=$SWAYSOCK WAYLAND_DISPLAY=$(cat "$CAP_DIR/display" 2>/dev/null)" >&2
        return
    fi
    [[ -n "$APP_PID" ]] && kill "$APP_PID" >/dev/null 2>&1
    if [[ -n "${SWAYSOCK:-}" ]]; then swaymsg -s "$SWAYSOCK" exit >/dev/null 2>&1; fi
    [[ -n "$SWAY_PID" ]] && kill "$SWAY_PID" >/dev/null 2>&1
    rm -rf "$CAP_DIR"
}
trap cleanup EXIT

# ---- 1. a private headless sway ---------------------------------------------------
# The config publishes the nested compositor's WAYLAND_DISPLAY to a file (the
# only reliable discovery mechanism — sway allocates the name at startup and
# only its exec'd children inherit it) and pins the virtual output's size.
SWAY_CFG="$CAP_DIR/sway.cfg"
cat > "$SWAY_CFG" <<EOF
output HEADLESS-1 resolution ${SIZE/x/ }
output HEADLESS-1 bg #000000 solid_color
default_border none
exec sh -c 'echo "\$WAYLAND_DISPLAY" > $CAP_DIR/display'
EOF
# `resolution W H` wants a space; the substitution above turns 1920x1080 into
# "1920 1080". sway also accepts WxH — keep the space form for older sways.

WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 \
    sway -c "$SWAY_CFG" >"$CAP_DIR/sway.log" 2>&1 &
SWAY_PID=$!

# sway's IPC socket contains its pid — poll for it, then for the display file.
SWAYSOCK=""
for _ in $(seq 1 40); do
    SWAYSOCK="$(ls "${XDG_RUNTIME_DIR:-/run/user/$UID}"/sway-ipc.*."$SWAY_PID".sock 2>/dev/null | head -1)"
    [[ -n "$SWAYSOCK" && -s "$CAP_DIR/display" ]] && break
    kill -0 "$SWAY_PID" 2>/dev/null || { cat "$CAP_DIR/sway.log" >&2; die "headless sway died at startup (log above)"; }
    sleep 0.25
done
[[ -n "$SWAYSOCK" && -s "$CAP_DIR/display" ]] || die "headless sway came up but IPC socket / WAYLAND_DISPLAY never appeared"
export SWAYSOCK
NESTED_DISPLAY="$(cat "$CAP_DIR/display")"

# ---- 2. sid, hermetic by default ---------------------------------------------------
declare -a APP_ENV=("WAYLAND_DISPLAY=$NESTED_DISPLAY" "SID_START_TAB=$TAB")
[[ -n "$THEME" ]] && APP_ENV+=("SID_THEME=$THEME")
[[ -n "${SID_PERF:-}" ]] && APP_ENV+=("SID_PERF=1")
if [[ "$REAL" -ne 1 ]]; then
    mkdir -p "$CAP_DIR/xdg/data" "$CAP_DIR/xdg/state" "$CAP_DIR/xdg/config"
    APP_ENV+=("XDG_DATA_HOME=$CAP_DIR/xdg/data" "XDG_STATE_HOME=$CAP_DIR/xdg/state" "XDG_CONFIG_HOME=$CAP_DIR/xdg/config")
fi
env "${APP_ENV[@]}" "$SID_BIN" >"$CAP_DIR/sid.log" 2>&1 &
APP_PID=$!

# Wait for the window (app_id "sid" — set in crates/sid/src/main.rs), then
# fullscreen it so the capture is exactly the virtual output's size.
FOUND=0
for _ in $(seq 1 60); do
    kill -0 "$APP_PID" 2>/dev/null || { cat "$CAP_DIR/sid.log" >&2; die "sid exited before opening a window (log above)"; }
    if swaymsg -s "$SWAYSOCK" -t get_tree | python3 -c '
import json, sys
def walk(n):
    if n.get("app_id") == "sid": return True
    return any(walk(c) for c in n.get("nodes", []) + n.get("floating_nodes", []))
sys.exit(0 if walk(json.load(sys.stdin)) else 1)
' 2>/dev/null; then FOUND=1; break; fi
    sleep 0.25
done
[[ "$FOUND" -eq 1 ]] || { cat "$CAP_DIR/sid.log" >&2; die "no sid window appeared in the nested compositor within 15s (log above)"; }
swaymsg -s "$SWAYSOCK" '[app_id="sid"] fullscreen enable' >/dev/null

if [[ "$TREE" -eq 1 ]]; then
    swaymsg -s "$SWAYSOCK" -t get_tree
    exit 0
fi

sleep "$WAIT"

# ---- 3. scripted input, in order ---------------------------------------------------
for action in "${ACTIONS[@]+"${ACTIONS[@]}"}"; do
    kind="${action%%:*}"; arg="${action#*:}"
    case "$kind" in
        click)
            x="${arg%,*}"; y="${arg#*,}"
            swaymsg -s "$SWAYSOCK" seat seat0 cursor set "$x" "$y" >/dev/null
            swaymsg -s "$SWAYSOCK" seat seat0 cursor press button1 >/dev/null
            swaymsg -s "$SWAYSOCK" seat seat0 cursor release button1 >/dev/null
            ;;
        key)
            command -v wtype >/dev/null 2>&1 || die "--key needs wtype (sudo pacman -S wtype)"
            # "ctrl+shift+tab" -> wtype -M ctrl -M shift -k Tab -m shift -m ctrl
            # (modifiers pressed in order, released in reverse).
            IFS='+' read -ra parts <<<"$arg"
            keyname="${parts[-1]}"
            mods=("${parts[@]:0:${#parts[@]}-1}")
            wt_args=()
            for m in "${mods[@]+"${mods[@]}"}"; do wt_args+=(-M "$m"); done
            wt_args+=(-k "$keyname")
            for ((i=${#mods[@]}-1; i>=0; i--)); do wt_args+=(-m "${mods[$i]}"); done
            WAYLAND_DISPLAY="$NESTED_DISPLAY" wtype "${wt_args[@]}"
            ;;
        type)
            command -v wtype >/dev/null 2>&1 || die "--type needs wtype (sudo pacman -S wtype)"
            WAYLAND_DISPLAY="$NESTED_DISPLAY" wtype "$arg"
            ;;
    esac
    sleep 0.4
done
[[ ${#ACTIONS[@]} -gt 0 ]] && sleep 1

# ---- 4. capture --------------------------------------------------------------------
WAYLAND_DISPLAY="$NESTED_DISPLAY" grim -o HEADLESS-1 "$OUT" || die "grim capture failed"
echo "sid-cap: wrote $OUT" >&2
echo "$OUT"
