#!/usr/bin/env python3
"""Persistent virtual-pointer driver for a wlroots compositor.

Attaches a zwlr_virtual_pointer_v1 to the seat and KEEPS IT ALIVE, which
flips the seat's advertised capabilities to include `pointer` — required
before gpui (and any strictly capability-gated client) will bind wl_pointer
and receive any pointer events at all.

Reads commands from stdin, one per line:
    move X Y W H     absolute motion (extents W H = output size)
    press            left button down
    release          left button up
    click X Y W H    move + press + release (with small frame gaps)
    rclick X Y W H   move + right-press + right-release — for context menus
                     (Workspaces tab's row menu; sid-cap.sh's --rclick)
    dclick X Y W H   TWO left clicks inside gpui's double-click window
                     (sid-cap.sh's --dclick; see DOUBLE_CLICK_* below)
    drag X1 Y1 X2 Y2 W H [STEPS]
                     press at X1,Y1 · STEPS interpolated moves · release at
                     X2,Y2 (sid-cap.sh's --drag) — divider/box drags
    quit             destroy + exit
Prints "ok <cmd>" to stdout after each command is flushed.

Why the multi-event gestures live HERE and not in the shell: gpui's Linux
backend counts a double click by the wall-clock gap between two BUTTON-DOWN
events (`DOUBLE_CLICK_INTERVAL` = 400ms, `DOUBLE_CLICK_DISTANCE` = 5px, same
button — gpui-0.2.2 `platform/linux/platform.rs:38` and the press arm of
`wayland/client.rs`). One command per gesture keeps the timing in a single
`time.sleep` chain instead of paying a FIFO write + shell round trip between
the halves; the caller's inter-action pause can then stay as slow as it likes.
"""
import os
import sys
import time

from pywayland.client import Display  # noqa: E402
from pywayland.protocol.wayland import WlSeat  # noqa: E402

# Generated into pywayland.protocol by sid-cap.sh's ensure_vptr (the scanner's
# output uses package-relative imports, so it must live inside pywayland).
from pywayland.protocol.wlr_virtual_pointer_unstable_v1 import (  # noqa: E402
    ZwlrVirtualPointerManagerV1,
)

BTN_LEFT = 0x110
BTN_RIGHT = 0x111

# Gap between the press and release of one click, and between the two presses
# of a `dclick`. The second must stay comfortably under gpui's 400ms
# DOUBLE_CLICK_INTERVAL *including* compositor latency, and comfortably above a
# single frame so the two downs are never coalesced.
CLICK_HOLD_S = 0.04
DCLICK_GAP_S = 0.06

# Drag defaults: enough intermediate motion events that a gpui drag handler
# sees a real movement stream (some only start dragging after the pointer has
# moved past a threshold), slow enough that each one lands in its own frame.
#
# A DRAG MUST BE BRISK — this is measured, not taste. Dragging sid's SFTP
# divider from x=483 to x=300 (a 480px -> 297px resize):
#
#   settle  60ms · step 10ms  -> lands exactly on 297     (correct)
#   settle 400ms · step 10ms  -> stops at 470             (13px of 183)
#   settle 400ms · step 60ms  -> stops at 445 / 468 / 388 (varies per run)
#   settle  80ms · step 20ms  -> stops at 378
#
# The failure tracks WALL TIME spent hovering the grab point before the press,
# not the number of motion events: linger on the handle and the target stops
# applying moves partway through, leaving a half-finished drag that still
# looks plausible in a screenshot. (A hover-triggered tooltip is the obvious
# suspect — sid's divider has one — but whatever the mechanism, the cure is to
# not dawdle.) So: press almost immediately, move fast, release immediately.
# Override per-run with SID_CAP_DRAG_STEP_MS / SID_CAP_DRAG_SETTLE_MS.
DRAG_STEPS = 16
DRAG_STEP_S = int(os.environ.get("SID_CAP_DRAG_STEP_MS", "10")) / 1000.0
# Settling either side of the press/release — long enough that the press and
# the release each land in their own frame, short enough not to trip the
# lingering-hover failure above.
DRAG_SETTLE_S = int(os.environ.get("SID_CAP_DRAG_SETTLE_MS", "60")) / 1000.0
# Hold at the destination before letting go. Separate from the settle above
# because the two are tuned against opposite failures: hovering too long
# BEFORE the press loses the drag, while releasing too soon AFTER the last
# motion loses the mouse-up (the target stays armed and the next pointer
# action drags it somewhere absurd).
DRAG_RELEASE_S = int(os.environ.get("SID_CAP_DRAG_RELEASE_MS", "250")) / 1000.0


def now_ms():
    return int(time.monotonic() * 1000) & 0xFFFFFFFF


seat = None
mgr = None


def registry_handler(registry, id_, interface, version):
    global seat, mgr
    if interface == "wl_seat" and seat is None:
        seat = registry.bind(id_, WlSeat, min(version, 7))
    elif interface == "zwlr_virtual_pointer_manager_v1":
        mgr = registry.bind(id_, ZwlrVirtualPointerManagerV1, min(version, 2))


display = Display()
display.connect()
registry = display.get_registry()
registry.dispatcher["global"] = registry_handler
display.dispatch(block=True)
display.roundtrip()

if mgr is None:
    print("FATAL: no zwlr_virtual_pointer_manager_v1", flush=True)
    sys.exit(1)
if seat is None:
    print("FATAL: no wl_seat", flush=True)
    sys.exit(1)

ptr = mgr.create_virtual_pointer(seat)
display.roundtrip()
print("ready", flush=True)


def do_move(x, y, w, h):
    ptr.motion_absolute(now_ms(), int(x), int(y), int(w), int(h))
    ptr.frame()


def do_button(state, button=BTN_LEFT):
    ptr.button(now_ms(), button, state)
    ptr.frame()


# Wheel geometry. A "notch" is one physical detent; the discrete count travels
# alongside the continuous value because clients may honour either.
AXIS_VERTICAL = 0
AXIS_SOURCE_WHEEL = 0
NOTCH = 15.0


def do_notch(sign):
    ptr.axis_source(AXIS_SOURCE_WHEEL)
    ptr.axis_discrete(now_ms(), AXIS_VERTICAL, NOTCH * sign, 1 * sign)
    ptr.frame()


def do_click(button=BTN_LEFT):
    """One press/release pair, each in its own flushed frame."""
    do_button(1, button)
    display.flush()
    time.sleep(CLICK_HOLD_S)
    do_button(0, button)
    display.flush()


def do_drag(x1, y1, x2, y2, w, h, steps=DRAG_STEPS):
    x1, y1, x2, y2 = int(x1), int(y1), int(x2), int(y2)
    steps = max(1, int(steps))
    do_move(x1, y1, w, h)
    display.flush()
    time.sleep(DRAG_SETTLE_S)
    do_button(1)
    display.flush()
    time.sleep(DRAG_SETTLE_S)
    for i in range(1, steps + 1):
        do_move(
            x1 + (x2 - x1) * i // steps,
            y1 + (y2 - y1) * i // steps,
            w,
            h,
        )
        # roundtrip, not flush: it blocks until the compositor has processed
        # everything queued, so a long gesture can never outrun it.
        display.roundtrip()
        time.sleep(DRAG_STEP_S)
    time.sleep(DRAG_RELEASE_S)
    do_button(0)
    display.roundtrip()


for line in sys.stdin:
    parts = line.strip().split()
    if not parts:
        continue
    cmd = parts[0]
    if cmd == "move":
        x, y, w, h = parts[1:5]
        do_move(x, y, w, h)
    elif cmd == "press":
        do_button(1)
    elif cmd == "release":
        do_button(0)
    elif cmd == "click":
        x, y, w, h = parts[1:5]
        do_move(x, y, w, h)
        display.flush()
        time.sleep(CLICK_HOLD_S)
        do_click()
    elif cmd == "rclick":
        x, y, w, h = parts[1:5]
        do_move(x, y, w, h)
        display.flush()
        time.sleep(CLICK_HOLD_S)
        do_click(BTN_RIGHT)
    elif cmd == "dclick":
        # The pointer must NOT move between the two clicks: gpui drops the
        # count back to 1 if the second down lands >5px from the first.
        x, y, w, h = parts[1:5]
        do_move(x, y, w, h)
        display.flush()
        time.sleep(CLICK_HOLD_S)
        do_click()
        time.sleep(DCLICK_GAP_S)
        do_click()
    elif cmd == "drag":
        x1, y1, x2, y2, w, h = parts[1:7]
        steps = parts[7] if len(parts) > 7 else DRAG_STEPS
        do_drag(x1, y1, x2, y2, w, h, steps)
    elif cmd in ("scroll", "scrollup"):
        # scroll N [GAP_MS] — N wheel notches, down for `scroll`, up for
        # `scrollup`. Each notch is its own flushed frame, because a burst
        # coalesced into one frame measures nothing useful.
        n = int(parts[1])
        gap = float(parts[2]) / 1000.0 if len(parts) > 2 else 0.025
        sign = 1 if cmd == "scroll" else -1
        for _ in range(n):
            do_notch(sign)
            display.flush()
            display.dispatch(block=False)
            time.sleep(gap)
    elif cmd == "hover":
        # hover X Y0 Y1 N MS W H — ping-pong the pointer down and up a column
        # of rows. Every row crossing flips the table's hover state and costs
        # one full frame, so this is a frame generator that needs neither
        # scrollable content nor a wheel.
        x, y0, y1, n, ms, w, h = (int(v) for v in parts[1:8])
        step = max(1, abs(y1 - y0) // 12)
        y, dy = y0, step
        for _ in range(n):
            y += dy
            if y >= y1 or y <= y0:
                dy = -dy
            do_move(x, y, w, h)
            display.flush()
            display.dispatch(block=False)
            time.sleep(ms / 1000.0)
    elif cmd == "quit":
        break
    display.flush()
    # drain any events (none expected; keeps the connection healthy)
    display.dispatch(block=False)
    print(f"ok {cmd}", flush=True)

ptr.destroy()
display.flush()
display.disconnect()
print("bye", flush=True)
