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
    quit             destroy + exit
Prints "ok <cmd>" to stdout after each command is flushed.
"""
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
        time.sleep(0.05)
        do_button(1)
        display.flush()
        time.sleep(0.05)
        do_button(0)
    elif cmd == "rclick":
        x, y, w, h = parts[1:5]
        do_move(x, y, w, h)
        display.flush()
        time.sleep(0.05)
        do_button(1, BTN_RIGHT)
        display.flush()
        time.sleep(0.05)
        do_button(0, BTN_RIGHT)
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
