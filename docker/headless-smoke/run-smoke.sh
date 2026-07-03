#!/usr/bin/env bash
# run-smoke.sh — launch `sid` under a headless Xvfb X server + Lavapipe
# (software Vulkan) and assert: it starts, opens a window, survives N
# seconds, and terminates without an abnormal (crash) exit. This is a
# launch-survives smoke, deliberately NOT pixel/golden-image diffing — see
# docs/design/2026-07-02-testing-strategy.md.
#
# Runs as the container's ENTRYPOINT (docker/headless-smoke/Dockerfile).
# Env overrides: SMOKE_WINDOW_TIMEOUT_SECS (default 15), SMOKE_SURVIVE_SECS
# (default 5), SMOKE_TERM_GRACE_SECS (default 5).

set -uo pipefail

WINDOW_TIMEOUT="${SMOKE_WINDOW_TIMEOUT_SECS:-15}"
SURVIVE_SECS="${SMOKE_SURVIVE_SECS:-5}"
TERM_GRACE_SECS="${SMOKE_TERM_GRACE_SECS:-5}"

Xvfb :99 -screen 0 1280x800x24 -nolisten tcp &
XVFB_PID=$!
export DISPLAY=:99

# Hermetic XDG dirs — same pattern as scripts/sid-shot.sh's default (non
# --real) mode: a fresh demo-seeded store, never a real one.
XDG_ROOT="$(mktemp -d)"
mkdir -p "$XDG_ROOT/data" "$XDG_ROOT/state" "$XDG_ROOT/config"
export XDG_DATA_HOME="$XDG_ROOT/data"
export XDG_STATE_HOME="$XDG_ROOT/state"
export XDG_CONFIG_HOME="$XDG_ROOT/config"

cleanup() {
    kill "$XVFB_PID" >/dev/null 2>&1 || true
}
trap cleanup EXIT

# Give Xvfb a moment to bind before anything tries to connect to :99.
for _ in $(seq 1 20); do
    if xdotool getdisplaygeometry >/dev/null 2>&1; then
        break
    fi
    sleep 0.25
done

echo "run-smoke: launching sid (DISPLAY=$DISPLAY)..." >&2
/usr/local/bin/sid >/tmp/sid.stdout.log 2>/tmp/sid.stderr.log &
APP_PID=$!

FOUND=0
elapsed=0
while [[ "$elapsed" -lt "$WINDOW_TIMEOUT" ]]; do
    if ! kill -0 "$APP_PID" 2>/dev/null; then
        echo "run-smoke: sid (pid $APP_PID) exited before a window appeared" >&2
        echo "run-smoke: --- stdout ---" >&2
        cat /tmp/sid.stdout.log >&2
        echo "run-smoke: --- stderr ---" >&2
        cat /tmp/sid.stderr.log >&2
        exit 1
    fi
    if [[ "$(xdotool search --onlyvisible --pid "$APP_PID" '' 2>/dev/null | wc -l)" -gt 0 ]]; then
        FOUND=1
        break
    fi
    sleep 0.5
    elapsed=$((elapsed + 1))
done

if [[ "$FOUND" -ne 1 ]]; then
    echo "run-smoke: no visible window for pid $APP_PID within ${WINDOW_TIMEOUT}s" >&2
    echo "run-smoke: --- stdout ---" >&2
    cat /tmp/sid.stdout.log >&2
    echo "run-smoke: --- stderr ---" >&2
    cat /tmp/sid.stderr.log >&2
    kill "$APP_PID" >/dev/null 2>&1 || true
    exit 1
fi
echo "run-smoke: window found after ${elapsed}s (poll interval 0.5s)" >&2

echo "run-smoke: surviving ${SURVIVE_SECS}s..." >&2
sleep "$SURVIVE_SECS"
if ! kill -0 "$APP_PID" 2>/dev/null; then
    echo "run-smoke: sid died during the ${SURVIVE_SECS}s survive window" >&2
    echo "run-smoke: --- stdout ---" >&2
    cat /tmp/sid.stdout.log >&2
    echo "run-smoke: --- stderr ---" >&2
    cat /tmp/sid.stderr.log >&2
    exit 1
fi
echo "run-smoke: still alive after ${SURVIVE_SECS}s — sending SIGTERM" >&2

kill -TERM "$APP_PID"
term_elapsed=0
while kill -0 "$APP_PID" 2>/dev/null; do
    if [[ "$term_elapsed" -ge "$TERM_GRACE_SECS" ]]; then
        echo "run-smoke: sid did not exit within ${TERM_GRACE_SECS}s of SIGTERM — SIGKILL" >&2
        kill -KILL "$APP_PID" >/dev/null 2>&1 || true
        wait "$APP_PID" 2>/dev/null
        echo "run-smoke: FAIL — required SIGKILL (not a clean exit)" >&2
        exit 1
    fi
    sleep 0.5
    term_elapsed=$((term_elapsed + 1))
done
wait "$APP_PID" 2>/dev/null
EXIT=$?
echo "run-smoke: sid exited with code $EXIT after SIGTERM (${term_elapsed}s later)" >&2

# 0 = graceful shutdown; 143 = 128+SIGTERM, the expected code for a process
# with no custom SIGTERM handler (gpui installs none) that we killed
# ourselves — both count as clean. Anything else (segfault=139, abort=134,
# a Rust panic's default 101, ...) is a real crash.
if [[ "$EXIT" -eq 0 || "$EXIT" -eq 143 ]]; then
    echo "run-smoke: PASS" >&2
    exit 0
fi
echo "run-smoke: FAIL — abnormal exit code $EXIT" >&2
echo "run-smoke: --- stdout ---" >&2
cat /tmp/sid.stdout.log >&2
echo "run-smoke: --- stderr ---" >&2
cat /tmp/sid.stderr.log >&2
exit 1
