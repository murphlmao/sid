---
name: sid-capture
description: Verify sid UI changes visually with scripts/sid-cap.sh - a hermetic headless sway that screenshots the real GPUI app and can click/type/scroll/drag first. Use for any UI change (before/after captures), reproducing layout bugs, or checking themes/zoom. Never uses sid-shot.sh or the live session.
---

# sid-capture

`scripts/sid-cap.sh` runs `sid` in its own private headless sway compositor with a hermetic
demo store. Nothing appears on Murphy's screen; it works while the session is locked and is
parallel-safe. Needs `sway`, `grim`; `wtype` for `--type`/`--key`. It builds `sid` itself
(`--no-build` to skip when already built).

## Basics

```bash
mkdir -p target/caps          # gitignored; never write captures under /tmp
scripts/sid-cap.sh --tab ssh --theme cosmos --out target/caps/ssh.png
# same via just:  just cap ssh cosmos
```
Then **Read the PNG** with the Read tool and actually look at it. Capture before and after
every UI change, in at least `cosmos` and `cosmos-light` (full set: cosmos, void, dusk, cosmos-light).

Flags: `--tab ssh|database|network|workspaces|system|settings`, `--theme`, `--size WxH`
(default 1920x1080), `--click X,Y` (repeatable), `--dclick X,Y` (true double-click: SSH card =
connect, DB/Workspaces rows = open), `--rclick X,Y` (context menu), `--scroll X,Y,N`,
`--type TEXT`, `--key chord`, `--drag X1,Y1,X2,Y2[,STEPS]`, `--env K=V`, `--wait N`,
`--out PATH`, `--no-build`, `--tree` (window tree, debug). Env: `SID_UI_SCALE=1.25` zooms
(same lever as ctrl +/-); `SID_GALLERY=1` (via `--env`) shows the component gallery.

## Caveats (each cost someone time)

- **`--key` needs a prior `--click`.** Pointer motion alone does not hand keyboard focus on a
  headless seat. Order: `--click X,Y --key ctrl+k`.
- **`--key shift+slash` does not reach gpui's `?` binding through wtype.** Use `--type "?"`
  (after a `--click`) to open the shortcut cheat sheet.
- **`--drag` goes last.** Its synthetic mouse-up does not clear the app's drag state, so a
  later pointer action would re-drag.
- Two `--click`s can never be a double-click (gpui's 400 ms window); use `--dclick`.
- `--scroll X,Y,N` is the only way to reach the foot of a long tab (Settings, System).
- The Database diagram pop-out is a separate window and cannot be grim-captured.
- Do not use `scripts/sid-shot.sh` (broken on this Hyprland), and never `cargo run` the GUI
  on the live session. If a live window is truly needed:
  `hyprctl dispatch exec "[workspace 4 silent] <cmd>"` - workspace 4 only.
- Coordinates are in output pixels at the chosen `--size`; capture once, read the PNG, pick targets.

## Per-tab recipes

| Check | Command |
|:--|:--|
| SSH home, folders, quick-connect | `--tab ssh` ; type `user@host`: click the filter/quick-connect field, then `--type user@host` |
| SSH session | `--tab ssh --dclick <card x,y>` then `--wait 6` (needs a reachable host; demo hosts will show the failure state) |
| Database query + results | `--tab database --dclick <connection row>`; click the Run button (find coordinates from a first capture) |
| Network sub-tabs | `--tab network`, click a sub-tab pill; kill-by-pid is two-click |
| Workspaces list/detail | `--tab workspaces --dclick <row>` |
| System tables | `--tab system`; long tables: `--scroll 960,600,10` |
| Settings sections / keymap | `--tab settings`, click the 220px rail items; narrow layout: `--size 800x900` |
| Zoom | `--env SID_UI_SCALE=1.5` (clamped 50-200%) |
| Component gallery | `just gallery cosmos-light` |
| Palette | `--click 960,500 --key ctrl+k` |

## Reading the result against the design law
Check `.interface-design/system.md`: tokens not hex, borders not shadows, one accent used
sparingly, three type sizes, no clipped/overflowing text, empty states say what to do next.
For a thorough pass spawn the `ui-reviewer` agent.
