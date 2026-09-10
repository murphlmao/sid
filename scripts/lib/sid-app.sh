# scripts/lib/sid-app.sh — shared bits of the sid capture harnesses.
#
# Sourced by scripts/sid-cap.sh (private headless sway) and scripts/sid-shot.sh
# (the user's live Hyprland session). Both launch the sid binary against a
# hermetic XDG store, poll for its window to appear, settle, capture, and
# clean up on --keep — this file holds exactly that overlap. Everything
# compositor-specific (how a window is detected, how it's driven, how the
# PNG is actually cropped) stays in the two scripts.
#
# Not meant to be executed directly.

# sid_app_locate_repo — sets (not exports) SCRIPT_DIR (the scripts/ dir) and
# REPO_ROOT (the repo root), derived from THIS file's own location
# (scripts/lib/sid-app.sh), not the caller's. Call with no args.
sid_app_locate_repo() {
    local lib_dir
    lib_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
    SCRIPT_DIR="$(cd -- "$lib_dir/.." >/dev/null 2>&1 && pwd)"
    REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." >/dev/null 2>&1 && pwd)"
}

# sid_app_export_tab TAB — export SID_START_TAB for the child process.
sid_app_export_tab() {
    export SID_START_TAB="$1"
}

# sid_app_setup_xdg REAL BASE_DIR [XDG_SRC]
#
# REAL != 1: creates BASE_DIR/{data,state,config}, optionally seeds
# BASE_DIR/data from XDG_SRC (copied — the source is never mutated), and
# exports XDG_DATA_HOME/XDG_STATE_HOME/XDG_CONFIG_HOME to point at them, so
# the app boots on a hermetic demo-seeded store.
# REAL == 1: does nothing — the app inherits whatever XDG_* the caller
# already has (the live/real store).
sid_app_setup_xdg() {
    local real="$1" base="$2" xdg_src="${3:-}"
    [[ "$real" -eq 1 ]] && return 0
    mkdir -p "$base/data" "$base/state" "$base/config"
    if [[ -n "$xdg_src" ]]; then
        cp -r "$xdg_src"/. "$base/data/"
    fi
    export XDG_DATA_HOME="$base/data" XDG_STATE_HOME="$base/state" XDG_CONFIG_HOME="$base/config"
}

# sid_app_wait_for_window PID TIMEOUT_SECS INTERVAL LOGFILE DETECT_FN [ARGS...]
#
# Polls DETECT_FN (called as `DETECT_FN PID [ARGS...]`) until it exits 0, PID
# dies, or TIMEOUT_SECS elapse. DETECT_FN may stash whatever it found (e.g. a
# GEOM json blob) in the caller's own variables as a side effect — this just
# runs it and times the loop.
#
# Returns: 0 found, 1 PID died (LOGFILE dumped to stderr if non-empty),
# 2 timed out (LOGFILE dumped to stderr if non-empty).
sid_app_wait_for_window() {
    local pid="$1" timeout="$2" interval="$3" logfile="$4" detect_fn="$5"
    shift 5
    local start=$SECONDS
    while (( SECONDS - start < timeout )); do
        if ! kill -0 "$pid" 2>/dev/null; then
            [[ -n "$logfile" ]] && cat "$logfile" >&2
            return 1
        fi
        "$detect_fn" "$pid" "$@" && return 0
        sleep "$interval"
    done
    [[ -n "$logfile" ]] && cat "$logfile" >&2
    return 2
}

# sid_app_kill_if_set PID... — kill each non-empty pid, ignoring errors.
# Safe to call with empty/unset args (nothing to kill yet).
sid_app_kill_if_set() {
    local p
    for p in "$@"; do
        [[ -n "$p" ]] && kill "$p" >/dev/null 2>&1
    done
}

# sid_app_rm_unless_keep KEEP DIR — rm -rf DIR, unless KEEP=1 or DIR is empty.
sid_app_rm_unless_keep() {
    local keep="$1" dir="$2"
    [[ "$keep" -eq 1 || -z "$dir" ]] && return 0
    rm -rf -- "$dir"
}

# sid_app_emit_result PREFIX OUT — the "print the PNG path last" contract:
# a note to stderr, then OUT as the only thing on stdout.
sid_app_emit_result() {
    local prefix="$1" out="$2"
    echo "$prefix: wrote $out" >&2
    echo "$out"
}
