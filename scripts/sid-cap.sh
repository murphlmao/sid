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
# Shares repo-root discovery, hermetic XDG setup, launch/poll-for-window,
# and the cleanup/--keep/print-path plumbing with scripts/sid-shot.sh via
# scripts/lib/sid-app.sh — see that file for what's shared vs. kept here
# (sway detection, all input injection, and the fullscreen+grim capture).
#
# Usage:
#   scripts/sid-cap.sh --out shot.png                        # SSH tab, default size
#   scripts/sid-cap.sh --tab system --out sys.png            # any primary tab
#   scripts/sid-cap.sh --tab database --click 300,200 --out after-click.png
#   scripts/sid-cap.sh --tab ssh --dclick 300,300 --out connected.png
#   scripts/sid-cap.sh --tab network --type "postgres" --out filtered.png
#   scripts/sid-cap.sh --tab ssh --drag 483,600,300,600 --out narrower-sidebar.png
#   scripts/sid-cap.sh --key ctrl+2 --out database.png       # chords work first, now
#   scripts/sid-cap.sh --tab ssh --wait 8 --out slow.png     # extra settle time
#   scripts/sid-cap.sh --tree                                # dump the window tree (debug)
#
# Flags:
#   --tab  ssh|database|network|workspaces|system|settings   (SID_START_TAB; default ssh)
#   --theme NAME       SID_THEME override (cosmos|void|dusk|cosmos-light)
#   --out  PATH        where the PNG goes (required unless --tree)
#   --size WxH         virtual output size (default 1920x1080)
#   --click X,Y        move pointer + left-click (repeatable, in order)
#   --dclick X,Y       DOUBLE-click — two clicks inside gpui's 400ms window, emitted
#                      as ONE pointer-driver command so nothing can stretch the gap.
#                      This is the only way to reach a `click_count() >= 2` handler
#                      (SSH card = connect, DB/Workspaces rows = open/rename); two
#                      `--click`s can never do it, see the note below.
#   --rclick X,Y       move pointer + right-click — opens context menus (repeatable)
#   --drag X1,Y1,X2,Y2[,STEPS]
#                      press at X1,Y1, glide through STEPS interpolated moves
#                      (default 16), release at X2,Y2 — drag-resizable dividers
#                      (SFTP sidebar) and draggable boxes (DB diagram). Pacing is
#                      deliberately brisk; see DRAG below before slowing it down.
#   --key  KEYS        key chord, e.g. "Return", "ctrl+tab", "ctrl+shift+t" (repeatable).
#                      NEEDS KEYBOARD FOCUS — the harness grabs it for you; see below.
#   --type TEXT        wtype literal text (repeatable, needs wtype)
#   --sleep SECS       pause between actions (repeatable) — e.g. wait out an SSH connect
#   --wait SECS        settle time after launch/actions before capture (default 3)
#   --no-build         skip the `cargo build -p sid` this script does by default, and
#                      instead just WARN if target/debug/sid is older than a source file
#   --real             use the REAL store (default: hermetic demo-seeded XDG)
#   --xdg DIR          use a PREPARED hermetic XDG data dir (copied fresh per run) —
#                      e.g. one with a saved test host + pinned known_hosts
#   --env KEY=VALUE    extra env var for the sid process (repeatable) — e.g. passing
#                      a host SSH_AUTH_SOCK through so a live-ssh capture can agent-auth
#   --keep             leave the compositor + app running (debug; prints env)
#   --tree             print swaymsg -t get_tree instead of capturing
#
# Actions execute in command-line order (click/key/type interleave correctly).
#
# BUILD FIRST, BY DEFAULT. `cargo clippy` / `cargo check` do NOT produce a binary, so
# a capture taken after a check-only loop used to silently photograph the PREVIOUS
# build. This script now runs `cargo build -p sid` itself before launching anything;
# `--no-build` opts out and downgrades to a loud staleness warning.
#
# WHY `--click` AND `--dclick` ARE DIFFERENT ACTIONS. gpui's Linux backend derives
# `click_count` from the wall-clock gap between two button-DOWN events: under 400ms
# (`DOUBLE_CLICK_INTERVAL`, gpui-0.2.2 platform/linux/platform.rs:38) and within 5px,
# same button. The 0.4s settle this script leaves between actions is exactly at that
# boundary, so `--click X,Y --click X,Y` reliably reads as two single clicks. `--dclick`
# hands the whole gesture to the pointer driver, which paces the two presses ~100ms
# apart and never moves in between.
#
# DRAG, AND WHY IT IS IN A HURRY. `--drag` hands the whole press/move…/release
# sequence to the pointer driver, which paces it from `scripts/cap-input/vptr.py`
# (tunable per run with SID_CAP_DRAG_STEP_MS / SID_CAP_DRAG_SETTLE_MS /
# SID_CAP_DRAG_RELEASE_MS). Do not make it slower without re-measuring: a drag that
# lingers on the grab point before pressing stops being applied partway through, and
# a half-finished resize looks entirely plausible in a screenshot. The measurements
# are in vptr.py next to the constants.
#
# KNOWN, NOT FIXED (app side, 2026-08-09): after a `--drag` on the SFTP divider the
# app's drag state stays armed — the synthetic mouse-up does not reach
# `SshSession::on_sidebar_drag_up`, even though the press and every move land. The
# drag's own result is correct and stable, but a LATER pointer action in the same run
# will re-drag the divider to wherever that pointer goes. Until that is fixed app-side,
# put `--drag` last, or expect to re-drag.
#
# `--key`/`--type` NEED A CLICK FIRST — the harness now does it for you. gpui never
# receives a `wl_keyboard::enter` in this compositor (no input devices exist when the
# window is focused, so the keyboard is bound too late), and it drops every keystroke
# until it does; a pointer BUTTON is what shakes the enter loose. So before the first
# key action, if no pointer action has run yet, the script clicks one inert pixel (2,2,
# the chrome bar's left padding). Set SID_CAP_FOCUS_CLICK=X,Y to move that click, or
# =none to disable it. Full measurements in focus_keyboard() below — this is the
# "--key chord injection no-ops entirely" note in the 2026-07-27 resume doc, and it was
# never a wtype bug.

set -uo pipefail

source "$(dirname -- "${BASH_SOURCE[0]}")/lib/sid-app.sh"

die() { echo "sid-cap: $*" >&2; exit 1; }

TAB="ssh"
THEME=""
OUT=""
SIZE="1920x1080"
WAIT=3
REAL=0
KEEP=0
TREE=0
BUILD=1
XDG_SRC=""
# Ordered action list: each entry is
#   "click:X,Y" | "dclick:X,Y" | "rclick:X,Y" | "drag:X1,Y1,X2,Y2[,STEPS]"
#   | "key:KEYS" | "type:TEXT" | "sleep:SECS".
ACTIONS=()
# Extra "KEY=VALUE" env vars forwarded into the sid process (see --env above).
EXTRA_ENV=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --tab)   TAB="$2"; shift 2 ;;
        --theme) THEME="$2"; shift 2 ;;
        --out)   OUT="$2"; shift 2 ;;
        --size)  SIZE="$2"; shift 2 ;;
        --click) ACTIONS+=("click:$2"); shift 2 ;;
        --dclick) ACTIONS+=("dclick:$2"); shift 2 ;;
        --rclick) ACTIONS+=("rclick:$2"); shift 2 ;;
        --drag)  ACTIONS+=("drag:$2"); shift 2 ;;
        --key)   ACTIONS+=("key:$2"); shift 2 ;;
        --type)  ACTIONS+=("type:$2"); shift 2 ;;
        --sleep) ACTIONS+=("sleep:$2"); shift 2 ;;
        --wait)  WAIT="$2"; shift 2 ;;
        --no-build) BUILD=0; shift ;;
        --real)  REAL=1; shift ;;
        --xdg)   XDG_SRC="$2"; shift 2 ;;
        --env)   EXTRA_ENV+=("$2"); shift 2 ;;
        --keep)  KEEP=1; shift ;;
        --tree)  TREE=1; shift ;;
        *) die "unknown argument: $1 (see the header of this script)" ;;
    esac
done

command -v sway >/dev/null 2>&1 || die "sway is not installed — it provides the private headless compositor this harness runs sid inside. Install: sudo pacman -S sway   (wtype too, for --type/--key)"
command -v grim >/dev/null 2>&1 || die "grim is not installed (sudo pacman -S grim)"
[[ "$TREE" -eq 1 || -n "$OUT" ]] || die "--out PATH is required (or --tree)"

sid_app_locate_repo
SID_BIN="$REPO_ROOT/target/debug/sid"

# ---- 0. never photograph a stale binary ---------------------------------------------
# `cargo clippy`/`cargo check` type-check without linking, so an edit-then-clippy loop
# leaves target/debug/sid at whatever the last real `cargo build` produced. Several
# agents burned a cycle "verifying" a change against the previous build. Default is to
# build; --no-build keeps the old behaviour but shouts if the binary looks stale.
newer_sources() {
    # Sources that would go into `sid` and are newer than the binary. Printing at most
    # a few keeps the warning readable; `head` closing the pipe early is fine.
    find "$REPO_ROOT/crates" "$REPO_ROOT/Cargo.toml" "$REPO_ROOT/Cargo.lock" \
        -type f \( -name '*.rs' -o -name 'Cargo.toml' -o -name 'Cargo.lock' \) \
        -newer "$SID_BIN" -print 2>/dev/null | head -5
}

if [[ "$BUILD" -eq 1 ]]; then
    command -v cargo >/dev/null 2>&1 \
        || die "cargo is not on PATH — pass --no-build to capture the existing $SID_BIN as-is"
    echo "sid-cap: building sid (cargo build -p sid; --no-build to skip)..." >&2
    cargo build -p sid --manifest-path "$REPO_ROOT/Cargo.toml" >&2 \
        || die "cargo build -p sid FAILED — refusing to capture, the binary on disk is stale"
fi

[[ -x "$SID_BIN" ]] || die "$SID_BIN not built — run: cargo build -p sid"

if [[ "$BUILD" -eq 0 ]]; then
    STALE="$(newer_sources)"
    if [[ -n "$STALE" ]]; then
        {
            echo "sid-cap: ############################################################"
            echo "sid-cap: # WARNING: STALE BINARY. target/debug/sid is OLDER than:"
            while IFS= read -r f; do echo "sid-cap: #   ${f#"$REPO_ROOT"/}"; done <<<"$STALE"
            echo "sid-cap: # The capture below shows the PREVIOUS build, not your edit."
            echo "sid-cap: # (cargo clippy/check never link a binary.) Drop --no-build,"
            echo "sid-cap: # or run: cargo build -p sid"
            echo "sid-cap: ############################################################"
        } >&2
    fi
fi

CAP_DIR="$(mktemp -d -t sid-cap.XXXXXX)"
SWAY_PID=""
APP_PID=""

cleanup() {
    if [[ "$KEEP" -eq 1 ]]; then
        echo "sid-cap: --keep: compositor pid $SWAY_PID, app pid $APP_PID, dir $CAP_DIR" >&2
        echo "sid-cap: --keep: SWAYSOCK=$SWAYSOCK WAYLAND_DISPLAY=$(cat "$CAP_DIR/display" 2>/dev/null)" >&2
        return
    fi
    sid_app_kill_if_set "${HOLDER_PID:-}" "${VPTR_PID:-}" "$APP_PID"
    if [[ -n "${SWAYSOCK:-}" ]]; then swaymsg -s "$SWAYSOCK" exit >/dev/null 2>&1; fi
    sid_app_kill_if_set "$SWAY_PID"
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
sid_app_export_tab "$TAB"
declare -a APP_ENV=("WAYLAND_DISPLAY=$NESTED_DISPLAY")
[[ -n "$THEME" ]] && APP_ENV+=("SID_THEME=$THEME")
[[ -n "${SID_PERF:-}" ]] && APP_ENV+=("SID_PERF=1")
# sid_app_setup_xdg exports XDG_{DATA,STATE,CONFIG}_HOME (a prepared --xdg store is
# copied in first so the source is never mutated); env below inherits them.
sid_app_setup_xdg "$REAL" "$CAP_DIR/xdg" "$XDG_SRC"
if [[ ${#EXTRA_ENV[@]} -gt 0 ]]; then
    APP_ENV+=("${EXTRA_ENV[@]}")
fi
env "${APP_ENV[@]}" "$SID_BIN" >"$CAP_DIR/sid.log" 2>&1 &
APP_PID=$!

# Wait for the window (app_id "sid" — set in crates/sid/src/main.rs), then
# fullscreen it so the capture is exactly the virtual output's size.
sid_cap_detect_window() {
    swaymsg -s "$SWAYSOCK" -t get_tree | python3 -c '
import json, sys
def walk(n):
    if n.get("app_id") == "sid": return True
    return any(walk(c) for c in n.get("nodes", []) + n.get("floating_nodes", []))
sys.exit(0 if walk(json.load(sys.stdin)) else 1)
' 2>/dev/null
}

sid_app_wait_for_window "$APP_PID" 15 0.25 "$CAP_DIR/sid.log" sid_cap_detect_window
case $? in
    1) die "sid exited before opening a window (log above)" ;;
    2) die "no sid window appeared in the nested compositor within 15s (log above)" ;;
esac
swaymsg -s "$SWAYSOCK" '[app_id="sid"] fullscreen enable' >/dev/null

if [[ "$TREE" -eq 1 ]]; then
    swaymsg -s "$SWAYSOCK" -t get_tree
    exit 0
fi

sleep "$WAIT"

# ---- 3. scripted input, in order ---------------------------------------------------
# See the header: a persistent pointer driver for clicks, a keyboard-capability
# holder around the whole phase for reliable typing.
VPTR_PID=""
HOLDER_PID=""
FOCUSED=0
# How many commands have been handed to the pointer driver — ptr_cmd waits for that
# many "ok" lines back before returning.
PTR_SENT=0
PTR_FIFO="$CAP_DIR/ptr-cmd"
# A dead pointer driver must fail the write loudly (EPIPE -> die), not kill the
# whole script with an unhandled SIGPIPE.
trap '' PIPE

ensure_vptr() {
    [[ -n "$VPTR_PID" ]] && return 0
    local venv="$HOME/.cache/sid-cap/venv"
    local gen="$HOME/.cache/sid-cap/gen"
    if [[ ! -x "$venv/bin/python" ]]; then
        echo "sid-cap: bootstrapping click support (one-time venv + pywayland)..." >&2
        python3 -m venv "$venv" && "$venv/bin/pip" -q install pywayland \
            || die "click bootstrap failed (need python3 + network once)"
    fi
    # The scanner's output uses package-relative imports (`from ..wayland import`),
    # so it must be generated INTO pywayland.protocol inside the venv.
    local proto_dir
    proto_dir="$("$venv/bin/python" -c 'import pywayland.protocol, os; print(os.path.dirname(pywayland.protocol.__file__))')"
    if [[ ! -d "$proto_dir/wlr_virtual_pointer_unstable_v1" ]]; then
        "$venv/bin/python" -m pywayland.scanner -i \
            /usr/share/wayland/wayland.xml \
            "$SCRIPT_DIR/cap-input/wlr-virtual-pointer-unstable-v1.xml" \
            -o "$proto_dir" || die "pywayland protocol generation failed"
    fi
    mkfifo "$PTR_FIFO"
    WAYLAND_DISPLAY="$NESTED_DISPLAY" \
        "$venv/bin/python" "$SCRIPT_DIR/cap-input/vptr.py" \
        < "$PTR_FIFO" > "$CAP_DIR/vptr.log" 2>&1 &
    VPTR_PID=$!
    exec 4> "$PTR_FIFO"
    sleep 1
    kill -0 "$VPTR_PID" 2>/dev/null || { cat "$CAP_DIR/vptr.log" >&2; die "pointer driver failed to start (log above)"; }
}

# Send one pointer-driver command and WAIT for its "ok <cmd>" acknowledgement.
#
# A multi-event gesture (dclick, drag) runs for a second or more inside the driver,
# while the FIFO write here returns immediately — so without this the next action, or
# the capture itself, could fire into the middle of a drag. The driver prints one
# `ok …` line per completed command, so counting those lines is an exact barrier;
# no guessed sleep can be.
ptr_cmd() {
    ensure_vptr
    echo "$*" >&4 || { cat "$CAP_DIR/vptr.log" >&2; die "pointer driver died (log above)"; }
    PTR_SENT=$((PTR_SENT + 1))
    local i acks
    for ((i = 0; i < 400; i++)); do
        acks="$(grep -c '^ok ' "$CAP_DIR/vptr.log" 2>/dev/null)"
        [[ "${acks:-0}" -ge "$PTR_SENT" ]] && return 0
        kill -0 "$VPTR_PID" 2>/dev/null || { cat "$CAP_DIR/vptr.log" >&2; die "pointer driver died mid-gesture (log above)"; }
        sleep 0.05
    done
    echo "sid-cap: warning: pointer driver never acknowledged '$*' (20s)" >&2
}

# THE `--key` FOOTGUN, measured rather than guessed (2026-08-09):
#
#   `--key ctrl+2` as the FIRST action  -> nothing happens (still on the SSH tab)
#   `--click 960,700 --key ctrl+2`      -> Database tab, every time
#
# What it is NOT: sway focus. `swaymsg -t get_tree` already reports the sid container
# `"focused": true` the moment it maps, and adding an explicit `[app_id="sid"] focus`
# changes nothing (tested). A bare pointer `move` doesn't help either (tested).
# What it IS: a wl_keyboard ENTER that never arrives. This compositor starts with no
# input devices at all (headless + WLR_LIBINPUT_NO_DEVICES=1), so the seat has no
# keyboard capability when sid's window is focused; gpui only binds wl_keyboard once
# wtype attaches a virtual one, i.e. strictly after that focus happened, and nothing
# re-sends `enter` to the late-bound resource. gpui keys off that event alone
# (`keyboard_focused_window`, wayland/client.rs `wl_keyboard::Event::Enter`) and drops
# every keystroke while it is `None`. A pointer BUTTON makes the compositor redo the
# focus handoff, which finally emits the enter — which is why a leading `--click`
# "fixes" `--key`, and why a move does not.
#
# So: before the first key action, if no pointer action has run yet, click one inert
# pixel. (2,2) is the top chrome bar's left padding — a plain `div` with no id and no
# handler, identical on every tab, so the click lands on nothing. Override with
# SID_CAP_FOCUS_CLICK=X,Y, or SID_CAP_FOCUS_CLICK=none to opt out entirely and take the
# no-op back. Any `--click`/`--dclick`/`--rclick`/`--drag` earlier in the action list
# has already done the job, and this is skipped (so it can never dismiss a menu a
# script just opened).
focus_keyboard() {
    [[ "$FOCUSED" -eq 1 ]] && return 0
    FOCUSED=1
    local spot="${SID_CAP_FOCUS_CLICK:-2,2}"
    if [[ "$spot" == "none" ]]; then
        echo "sid-cap: SID_CAP_FOCUS_CLICK=none — skipping the focus click; a --key before any click will NO-OP" >&2
        return 0
    fi
    [[ "$spot" =~ ^[0-9]+,[0-9]+$ ]] || die "SID_CAP_FOCUS_CLICK wants X,Y or 'none', got '$spot'"
    echo "sid-cap: focusing the window with an inert click at $spot so --key/--type dispatch" >&2
    ptr_cmd "click ${spot%,*} ${spot#*,} ${SIZE/x/ }"
    sleep 0.3
}

ensure_kbd_holder() {
    [[ -n "$HOLDER_PID" ]] && return 0
    command -v wtype >/dev/null 2>&1 || die "--key/--type need wtype (sudo pacman -S wtype)"
    # Sleeps far longer than any run; killed in stop_input below. Keeps the seat's
    # keyboard capability stably on so gpui binds once and never drops keys.
    WAYLAND_DISPLAY="$NESTED_DISPLAY" wtype -s 600000 -k F20 &
    HOLDER_PID=$!
    sleep 1
}

stop_input() {
    [[ -n "$HOLDER_PID" ]] && kill "$HOLDER_PID" >/dev/null 2>&1
    if [[ -n "$VPTR_PID" ]]; then
        echo "quit" >&4 2>/dev/null
        exec 4>&- 2>/dev/null
        kill "$VPTR_PID" >/dev/null 2>&1
    fi
    HOLDER_PID=""; VPTR_PID=""
}

for action in "${ACTIONS[@]+"${ACTIONS[@]}"}"; do
    kind="${action%%:*}"; arg="${action#*:}"
    case "$kind" in
        click|dclick|rclick)
            x="${arg%,*}"; y="${arg#*,}"
            [[ "$x" =~ ^-?[0-9]+$ && "$y" =~ ^-?[0-9]+$ ]] \
                || die "--$kind wants X,Y (integers), got '$arg'"
            # A pointer press also hands the window keyboard focus, so a later
            # --key needs no separate focus step.
            FOCUSED=1
            ptr_cmd "$kind $x $y ${SIZE/x/ }"
            ;;
        drag)
            IFS=',' read -ra d <<<"$arg"
            [[ ${#d[@]} -eq 4 || ${#d[@]} -eq 5 ]] \
                || die "--drag wants X1,Y1,X2,Y2[,STEPS], got '$arg'"
            for n in "${d[@]}"; do
                [[ "$n" =~ ^-?[0-9]+$ ]] || die "--drag wants integers, got '$arg'"
            done
            FOCUSED=1
            # press + N interpolated moves + release all run inside the driver;
            # ptr_cmd blocks on its ack, so the whole gesture is over before the
            # next action (or the capture) happens.
            ptr_cmd "drag ${d[0]} ${d[1]} ${d[2]} ${d[3]} ${SIZE/x/ } ${d[4]:-}"
            ;;
        key)
            focus_keyboard
            ensure_kbd_holder
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
            focus_keyboard
            ensure_kbd_holder
            WAYLAND_DISPLAY="$NESTED_DISPLAY" wtype -d 50 "$arg"
            ;;
        sleep)
            sleep "$arg"
            ;;
    esac
    sleep 0.4
done
[[ ${#ACTIONS[@]} -gt 0 ]] && sleep 1
stop_input

# ---- 4. capture --------------------------------------------------------------------
WAYLAND_DISPLAY="$NESTED_DISPLAY" grim -o HEADLESS-1 "$OUT" || die "grim capture failed"
sid_app_emit_result "sid-cap" "$OUT"
