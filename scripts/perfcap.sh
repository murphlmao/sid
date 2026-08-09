#!/usr/bin/env bash
# perfcap.sh — frame-cost harness for sid (rebuilt; the earlier scratchpad copy
# was wiped when the session was interrupted).
#
# Design notes, all of them paid for in wasted runs:
#  * This box runs 4 other agents; load swings 20..70 on 16 cores. Wall-clock
#    frame time swings 10x with it, so the headline number is instructions:u
#    per frame, not milliseconds.
#  * The System tab re-probes sysinfo every 2s. Over a long window those ticks
#    dominate the instruction count (a 120s window read 596M instr/frame vs
#    ~80M for the render itself). So: two windows of the SAME length, one idle
#    and one scrolling, and report (I_scroll - I_idle) / (F_scroll - F_idle).
#    The ticks appear in both and cancel.
#  * Window length is fixed in wall time (not "until the driver acks"): under
#    load the python pointer driver takes 10x longer to deliver 120 notches,
#    which would make the two arms incomparable.
#  * One direction only, <=130 notches (409 rows, ~3 rows a notch): a chunked
#    loop that flipped direction drifted to the top of the list, where wheel-up
#    produces no frames at all.
#
# Usage:
#   perfcap.sh --bin /path/to/sid --out DIR [--tab system] [--window 8]
#              [--notches 130] [--gap 16] [--size 1920x1080] [--park X,Y]
set -uo pipefail

die() { echo "perfcap: $*" >&2; exit 1; }

BIN=""; OUTDIR=""; TAB="system"; NOTCHES=130; GAP=16; SIZE="1920x1080"
WINDOW=8; WAIT=6; PARK="960,500"; ENVS=(); MODE="scroll"; HOVER="960,300,900"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --bin) BIN="$2"; shift 2 ;;
        --out) OUTDIR="$2"; shift 2 ;;
        --tab) TAB="$2"; shift 2 ;;
        --notches) NOTCHES="$2"; shift 2 ;;
        --gap) GAP="$2"; shift 2 ;;
        --size) SIZE="$2"; shift 2 ;;
        --window) WINDOW="$2"; shift 2 ;;
        --wait) WAIT="$2"; shift 2 ;;
        --park) PARK="$2"; shift 2 ;;
        --env) ENVS+=("$2"); shift 2 ;;
        --mode) MODE="$2"; shift 2 ;;
        --hover) HOVER="$2"; shift 2 ;;
        *) die "unknown argument: $1" ;;
    esac
done

[[ -x "$BIN" ]] || die "--bin must be an executable sid binary"
[[ -n "$OUTDIR" ]] || die "--out DIR required"
mkdir -p "$OUTDIR"
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

CAP_DIR="$(mktemp -d -t perfcap.XXXXXX)"
SWAY_PID=""; APP_PID=""; VPTR_PID=""

cleanup() {
    [[ -n "$VPTR_PID" ]] && kill "$VPTR_PID" >/dev/null 2>&1
    [[ -n "$APP_PID" ]] && kill "$APP_PID" >/dev/null 2>&1
    if [[ -n "${SWAYSOCK:-}" ]]; then swaymsg -s "$SWAYSOCK" exit >/dev/null 2>&1; fi
    [[ -n "$SWAY_PID" ]] && kill "$SWAY_PID" >/dev/null 2>&1
    rm -rf "$CAP_DIR"
}
trap cleanup EXIT
trap '' PIPE

cat > "$CAP_DIR/sway.cfg" <<EOF
output HEADLESS-1 resolution ${SIZE/x/ }
output HEADLESS-1 bg #000000 solid_color
default_border none
exec sh -c 'echo "\$WAYLAND_DISPLAY" > $CAP_DIR/display'
EOF

WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 \
    sway -c "$CAP_DIR/sway.cfg" >"$CAP_DIR/sway.log" 2>&1 &
SWAY_PID=$!

SWAYSOCK=""
for _ in $(seq 1 60); do
    SWAYSOCK="$(ls "${XDG_RUNTIME_DIR:-/run/user/$UID}"/sway-ipc.*."$SWAY_PID".sock 2>/dev/null | head -1)"
    [[ -n "$SWAYSOCK" && -s "$CAP_DIR/display" ]] && break
    kill -0 "$SWAY_PID" 2>/dev/null || { cat "$CAP_DIR/sway.log" >&2; die "sway died"; }
    sleep 0.25
done
[[ -n "$SWAYSOCK" && -s "$CAP_DIR/display" ]] || die "sway IPC/display never appeared"
export SWAYSOCK
ND="$(cat "$CAP_DIR/display")"

mkdir -p "$CAP_DIR/xdg/data" "$CAP_DIR/xdg/state" "$CAP_DIR/xdg/config"
LOG="$OUTDIR/sid.log"; : > "$LOG"
env WAYLAND_DISPLAY="$ND" SID_START_TAB="$TAB" SID_PERF=1 \
    XDG_DATA_HOME="$CAP_DIR/xdg/data" XDG_STATE_HOME="$CAP_DIR/xdg/state" \
    XDG_CONFIG_HOME="$CAP_DIR/xdg/config" ${ENVS[@]+"${ENVS[@]}"} \
    "$BIN" >"$LOG" 2>&1 &
APP_PID=$!

FOUND=0
for _ in $(seq 1 120); do
    kill -0 "$APP_PID" 2>/dev/null || { tail -20 "$LOG" >&2; die "sid exited early"; }
    if swaymsg -s "$SWAYSOCK" -t get_tree 2>/dev/null | grep -q '"app_id": "sid"'; then FOUND=1; break; fi
    sleep 0.25
done
[[ "$FOUND" -eq 1 ]] || { tail -20 "$LOG" >&2; die "no sid window"; }
swaymsg -s "$SWAYSOCK" '[app_id="sid"] fullscreen enable' >/dev/null
sleep "$WAIT"

VENV="$HOME/.cache/sid-cap/venv"
[[ -x "$VENV/bin/python" ]] || die "no sid-cap venv (run scripts/sid-cap.sh once)"
mkfifo "$CAP_DIR/ptr"
VPTR_LOG="$OUTDIR/vptr.log"
WAYLAND_DISPLAY="$ND" "$VENV/bin/python" "$HERE/vptr2.py" \
    < "$CAP_DIR/ptr" > "$VPTR_LOG" 2>&1 &
VPTR_PID=$!
exec 4> "$CAP_DIR/ptr"
sleep 1
kill -0 "$VPTR_PID" 2>/dev/null || { cat "$VPTR_LOG" >&2; die "vptr failed"; }

PX="${PARK%,*}"; PY="${PARK#*,}"
echo "move $PX $PY ${SIZE/x/ }" >&4
sleep 1.5

count() { grep -c "sid-perf: frame" "$LOG" 2>/dev/null || echo 0; }

# ---- window 1: idle (the 2s sysinfo tick and nothing else) -------------------
I0=$(count)
perf stat --no-scale -e instructions:u,cycles:u -p "$APP_PID" -o "$OUTDIR/perf-idle.txt" \
    -- sleep "$WINDOW" >/dev/null 2>&1
I1=$(count)

# ---- window 2: the same length, scrolling ------------------------------------
S0=$(count)
if [[ "$MODE" == "hover" ]]; then
    HX="${HOVER%%,*}"; HREST="${HOVER#*,}"; HY0="${HREST%%,*}"; HY1="${HREST#*,}"
    echo "hover $HX $HY0 $HY1 $NOTCHES $GAP ${SIZE/x/ }" >&4 || { cat "$VPTR_LOG" >&2; die "vptr died"; }
else
    echo "scroll $NOTCHES $GAP" >&4 || { cat "$VPTR_LOG" >&2; die "vptr died"; }
fi
perf stat --no-scale -e instructions:u,cycles:u -p "$APP_PID" -o "$OUTDIR/perf-scroll.txt" \
    -- sleep "$WINDOW" >/dev/null 2>&1
S1=$(count)

grep "sid-perf: frame" "$LOG" | sed -n "$((I0+1)),${I1}p" > "$OUTDIR/idle.txt"
grep "sid-perf: frame" "$LOG" | sed -n "$((S0+1)),${S1}p" > "$OUTDIR/scroll.txt"

# ---- tail: how long does the app keep repainting after input stops? ----------
# (the scrollbar fade-out requests animation frames for ~1s). Only meaningful if
# the driver has finished delivering; note how many notches it got through.
while ! grep -qE "^ok (scroll|hover)" "$VPTR_LOG"; do sleep 0.2; done
T0=$(count); sleep 2; T1=$(count)

WAYLAND_DISPLAY="$ND" grim "$OUTDIR/final.png" >/dev/null 2>&1
echo "quit" >&4 2>/dev/null

python3 - "$OUTDIR" "$((I1-I0))" "$((S1-S0))" "$((T1-T0))" "$WINDOW" <<'PY'
import re, sys, os
out, fi, fs, ft, window = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4]), float(sys.argv[5])

def counters(p):
    if not os.path.exists(p):
        return 0, 0
    txt = open(p).read()
    ins = sum(int(m.replace(',', '')) for m in
              re.findall(r'^\s*([\d,]+)\s+\S*instructions\S*', txt, re.M))
    cyc = sum(int(m.replace(',', '')) for m in
              re.findall(r'^\s*([\d,]+)\s+\S*cycles\S*', txt, re.M))
    return ins, cyc

def pct(xs, p):
    if not xs: return float('nan')
    xs = sorted(xs); i = min(len(xs)-1, int(round((len(xs)-1)*p)))
    return xs[i]

pat = re.compile(r'build=([\d.]+)ms layout=([\d.]+)ms total=([\d.]+)ms')
def phases(p):
    rows = [tuple(float(x) for x in m.groups())
            for m in (pat.search(l) for l in open(p)) if m]
    return rows

ii, ic = counters(f'{out}/perf-idle.txt')
si, sc = counters(f'{out}/perf-scroll.txt')
print(f'== {out}   window={window:g}s  load={open("/proc/loadavg").read().split()[0]}')
print(f'-- idle   {fi:3d} frames  {ii/1e6:9.1f}M instr')
print(f'-- scroll {fs:3d} frames  {si/1e6:9.1f}M instr')
rows = phases(f'{out}/scroll.txt')
if rows:
    for i, name in enumerate(('build', 'layout', 'total')):
        xs = [r[i] for r in rows]
        print(f'   {name:6s} p50={pct(xs,0.5):7.3f} p95={pct(xs,0.95):7.3f} '
              f'max={max(xs):8.3f} mean={sum(xs)/len(xs):7.3f}')
d_f, d_i, d_c = fs - fi, si - ii, sc - ic
if d_f > 0:
    print(f'** render cost = {d_i/d_f/1e6:6.1f}M instr/frame   {d_c/d_f/1e6:6.1f}M cycles/frame'
          f'   (delta over {d_f} extra frames)')
print(f'-- post-input tail: {ft} frames in 2s')
PY
