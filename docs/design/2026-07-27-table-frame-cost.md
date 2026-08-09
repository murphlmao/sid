# Table measure passes: priced, and why nothing shipped

Session goal: kill the `measure_item` passes gpui computes and discards (~26 % of the
table's cell builds). Result, in one line: **that target is worth 4.6 % and needs a
2 091-line vendor; deleting one wrapper `div` per cell is worth 11 % and needs
nothing. Nothing was committed** — the 11 % change lands in files a typography agent
is sweeping this session.

Worktree: `.claude/worktrees/agent-perfmeasure`, branch `perf-table-measure`, base
`3964342` (the brief's base; `main` has since advanced to `ca17742`, touching only
`sid-gpu`, `sid-privfs` and `scripts/` — nothing in the table render path, so these
numbers carry). **No commits.** The only working-tree change was a throwaway
`[patch.crates-io]` used for the §5/§6 measurements, reverted before finishing;
the worktree is now byte-identical to `3964342` (`git status` clean,
`cargo fmt --all --check` = 0).

---

## 0. The harness had to be rebuilt

The previous session's scratchpad (`perfcap.sh`, `perfstat.py`, `ab*.sh`, all raw
runs) was **wiped** when the session was interrupted. Rebuilt from
`scripts/sid-cap.sh` + `scripts/cap-input/vptr.py`:

* `vptr2.py` — `vptr.py` plus `scroll N MS` / `scrollup` (wlr virtual-pointer
  `axis_source` + `axis_discrete`) and `hover X Y0 Y1 N MS W H` (ping-pong motion).
* `perfcap.sh` — headless sway + release sid + `SID_PERF=1`, two fixed-length
  windows (idle, then driven), `perf stat` over each, PNG at the end.
* `sweep2.sh` — the cell-pricing sweep. `ab.sh` — interleaved A/B.

Five measurement traps, all paid for in wasted runs — **do not re-learn these**:

1. **Never measure on the System tab.** Its 2 s sysinfo probe costs **0.6–2.5 G
   instructions a tick**; a frame costs ~28 M. Measured idle over 8 s: System tab
   **5 259.6 M** instructions / 4 frames, Settings tab **6.5 M** / 0 frames — an
   800x difference. Worse, the probe's cost tracks the box's process count, which
   four concurrent agents churn, so it will not subtract out (idle-subtracted
   deltas came out *negative*).
2. **Measure on the Network tab.** It polls nothing: idle = **0 frames, 6.8–13.0 M
   instructions in 8 s**. instructions/frame then needs no baseline correction.
3. **Drive frames with pointer motion, not the wheel.** Each row crossing flips the
   hover state and costs one full frame; it needs no scrollable content and cannot
   run out of list. A chunked wheel loop that flipped direction drifted to the top of
   the list, where wheel-up produces no frames at all — 120 s for 71 frames.
4. **No `python3` (pyenv shim) inside a timing loop.** Under load 60 each call cost
   seconds and stretched an 8 s window to 120 s.
5. **`perf stat` needs `--no-scale` on this hybrid CPU.** It opens one PMU per core
   type and **scales each count by that PMU's own enabled-time fraction**, so summing
   `cpu_atom/instructions` + `cpu_core/instructions` multiplies whichever core type the
   task barely touched by 1/f. One real report:
   `1,735,517,144 cpu_atom (98.67%)` + `2,221,226,623 cpu_core (1.33%)` — the second
   number is a ~30 M raw sample scaled by 75x. Because the scheduler's P/E placement
   moves with load, the inflated sum swung 2.3x between identical runs (64.8 M vs
   28.2 M instr/frame). With `--no-scale` the same measurement is stable to **<1 %**
   round to round (28.1 / 28.2). Sanity check on the corrected figure: ~28 M cycles per
   frame at ~4 GHz is ~7 ms of CPU per frame, which matches the previous session's
   independent low-load wall-clock measurement of ~6 ms.

Everything below is release, 1920x1080 unless stated, box load 30–70 on 16 cores.

---

## 1. Baseline (Network tab, Ports table, 35 rows, 27 visible)

| height | cells/frame (`GPUI_MEASUREMENTS=1`) | instructions:u/frame |
|---|---|---|
| 1080 | **191** | 28.4 M |
| 800  | 135 | 21.5 M |
| 620  | 93  | 16.6 M |

Least-squares fit of instructions/frame against cells/frame (R^2 ~ 0.9998 — the three
points predict each other to 0.1 M):

```
instr/frame  =  5.3 M  +  121 K x cells
```

* **121 K instructions per table cell** (build + layout + prepaint + paint).
* **5.3 M fixed** — nav bar, tab strip, filter row, header, scrollbars, chrome.
* At 1080p the cells are **191 x 121 K = 23.0 M of 28.4 M = 81 % of the frame.**
  "The table *is* the frame" is now a measured statement, not an inference: there is
  nothing else worth attacking in a sid frame.
* 28 M cycles/frame ~ **7 ms of CPU per frame** on this machine.

System tab, 478 processes, same build: **178 cells/frame** (deterministic across 32
frames), wall p50 ~20–45 ms at load 60 (useless as a signal — see trap 1).

## 2. Where the 191 cells go

R = 27 visible rows. The model below is exact — it predicts both the baseline (191)
and the stubbed build (162) with no slack:

| calls | source | used for |
|---|---|---|
| 6R = 162 | the real cells, `UniformList::prepaint` -> `render_table_row` | the picture |
| R = 27 | **one per row**: `VirtualList::request_layout` -> `measure_item(0..1)` (`virtual_list.rs:365,304`) | the row's height only |
| 1 | `UniformList::request_layout` -> `measure_item` (`uniform_list.rs:262`) | **nothing** |
| 1 | `UniformList::prepaint` -> `measure_item` (`uniform_list.rs:338`) | the row's height only |

The two `UniformList` measure passes cost only **one** `render_td` each, not seven:
they lay the row out at `MinContent` width, so the row's horizontal `VirtualList`
resolves an empty visible column range and only its own `measure_item` fires.

**29 of 191 cells (15 %) are measurement.** All three measurements recover a height
that `render_table_row` has already pinned to the constant
`options.size.table_row_height()` (`state.rs:988`), and the width every one of them
computes is discarded outright: `ListHorizontalSizingBehavior` defaults to `FitList`
(`list.rs:129-135`), so `uniform_list.rs:339-343` takes `padded_bounds.size.width`
and throws `longest_item_size.width` away.

Naively that looks like 15 % of the frame. It is not: measure rows go through
`layout_as_root`, which is request_layout + taffy and *not* prepaint or paint
(`element.rs:619-626`, and `prepaint_as_root` at `:641-649` is the one that adds
prepaint). §5 measures the real figure: **4.6 %**.

## 3. Route (a) — inside sid's wrapper/delegates — is impossible

The brief asked whether a sizing-behaviour knob avoids the discarded pass. It does not:

* `uniform_list.rs:262` calls `measure_item` **before and outside** the
  `match self.sizing_behavior` at `:268`. `Infer` uses the result, `Auto` discards it,
  but **both pay for it**. There is no behaviour value that skips the call.
* The knob is unreachable anyway: `.with_sizing_behavior(ListSizingBehavior::Auto)` is
  applied inside `gpui-component`'s `TableState::render` (`state.rs:1389`), and the
  `UniformList` and per-row `VirtualList` are constructed there too
  (`state.rs:1325`, `state.rs:1041`). sid hands `gpui-component` a delegate; it never
  touches the list elements.
* A phase sentinel *is* possible from sid (an element whose `request_layout` sets a
  thread-local the delegate reads), but it can only catch the pass that runs in the
  window's request-layout phase — **1 cell of 191**, in exchange for a thread-local and
  a hack in every delegate. Comfortably below the noise floor. Not worth it.

## 4. Route (b) — vendoring — is smaller than feared, and blocked for a different reason

**Vendoring "just `uniform_list`/`virtual_list`" does not work**: `gpui-component`'s
table constructs both internally, so a copy under sid's control would simply never be
called. The correct unit is `gpui-component/src/table/` — and once sid owns that, it
owns the two render closures, and **all three measure passes can be stubbed without
vendoring `uniform_list` or `virtual_list` at all** (both call the table's own closure
with the range `0..1`; return a `div().h(row_height)` for that call — proven by the
experiment in §5).

Scope, verified:

| item | size |
|---|---|
| `gpui-component-0.5.1/src/table/{mod,state,column,delegate,loading}.rs` | **2 091 lines** |
| private deps to fix: `crate::actions::{Cancel,SelectDown,SelectUp}` (`pub(crate)`) | redefine + rebind the table's keyboard actions |
| private deps to fix: `crate::measure_enable()` (`pub(crate)`) | delete |
| private dep: `crate::virtual_list::virtual_list()` | use the public `h_virtual_list` |
| everything else it imports (`ActiveTheme`, `Icon`, `StyleSized`, `StyledExt`, `VirtualListScrollHandle`, `h_flex`, `v_flex`, `menu::{ContextMenuExt,PopupMenu}`, `scroll::{ScrollableMask,Scrollbar}`) | already public |

**The blocker is the delegate trait, not the size.** `TableDelegate`'s methods take
`cx: &mut Context<TableState<Self>>` (`delegate.rs:83-90`) — welded to upstream's
concrete `TableState`. A vendored table cannot reuse the trait, so the trait forks too,
and every `impl TableDelegate` in sid must be for the *vendored* trait. There are
**seven**, in five files:

```
crates/sid/src/ui/systems_tab.rs:432     ProcessesDelegate
crates/sid/src/ui/network_tab.rs:505     PortsDelegate
crates/sid/src/ui/network_tab.rs:792     ServicesDelegate
crates/sid/src/ui/network_tab.rs:990     DockerDelegate
crates/sid/src/ui/network_tab.rs:1152    KubePodsDelegate
crates/sid/src/ui/db_tab.rs:828          ResultDelegate
crates/sid/src/ui/workspaces_tab.rs:536  FleetDelegate
```

The good news: if the vendored module keeps the upstream names, **the edit to each tab
file is one `use` line** —
`use gpui_component::table::{Column, ColumnSort, TableDelegate, TableState};` becomes
the `sid_ui::table` equivalent. Four such lines
(`network_tab.rs:87`, `workspaces_tab.rs:41`, `systems_tab.rs:78`, `db_tab.rs:34`)
plus `sid-ui/src/table/{mod,columns,header}.rs`.

This session owns `crates/sid-ui/src/table/**` and may make surgical edits to
`systems_tab.rs` only; `network_tab.rs`, `db_tab.rs` and `workspaces_tab.rs` are being
edited concurrently by other agents. **A trait fork that touches all of them is not
safely doable here.** That is the blocking fact.

## 5. The prize, measured (throwaway patch, reverted)

To turn the 16 % estimate into a number, `gpui-component`'s two render closures were
patched **in a scratch copy outside the repo**, wired in with
`[patch.crates-io]`, measured, and reverted. The patch stubs exactly what a vendored
table would stub:

* `state.rs` uniform-list closure: `visible_range == 0..1` -> return
  `div().id("sid-measure-stub").w_full().h(options.size.table_row_height())`.
* `state.rs` virtual-list closure: `visible_range == 0..1` -> return
  `h_flex().w(col_width).h(row_height)`.

Mechanism check (`GPUI_MEASUREMENTS=1`, deterministic): **191 -> 162 cells/frame.**
Exactly the 29 predicted measure cells, and nothing else, disappeared.

Interleaved A/B, Network tab, 1920x1080, 8 s windows, four rounds each, arms
alternating (`ab.sh`). instructions:u/frame, `--no-scale`:

| round | A = stock | B = measure passes stubbed |
|---|---|---|
| 1 | 28.1 M | 26.9 M |
| 2 | 28.2 M | 26.9 M |
| 3 | 28.3 M | 26.9 M |
| 4 | 28.2 M | 26.9 M |
| **mean** | **28.20 M** | **26.90 M** |

**-1.30 M instructions/frame = -4.6 %.** No overlap between the arms; the spread
within each arm is under 0.1 M. Screenshot of the stubbed build's Network table is
pixel-identical to stock (fill-width, all rows, sort chevrons, hover, kill buttons).

**So the whole measure-pass prize is 4.6 % of a frame, not the ~16 % the cell count
suggests.** A measure cell costs 1.30 M / 29 = **45 K instructions**, roughly a third
of a rendered cell, because `measure_item` calls `layout_as_root`
(`element.rs:619-626`) — request_layout plus taffy, and then the element is dropped.
It never prepaints and never paints. Counting *cell builds* overstates the cost of a
pass that only ever builds and measures.

## 6. Pricing the other big item: one wrapper div per cell (ranked #2)

Same instrument, same throwaway-patch trick, one more line changed: drop the
`render_col_wrap` `h_flex` so each cell is **one flex node shallower**
(`render_col_wrap > render_cell > delegate div > text` becomes
`render_cell > delegate div > text`). Cell *count* is unchanged — 162 either way — so
this isolates node depth from cell work. Arm A here is the measure-stub build from
§5, so the two effects do not overlap.

| round | A = measure-stubbed | B = A minus one node per cell |
|---|---|---|
| 1 | 26.9 M | 23.9 M |
| 2 | 26.9 M | 24.0 M |
| 3 | 27.1 M | 24.0 M |
| **mean** | **26.97 M** | **23.97 M** |

**-3.0 M instructions/frame = -11 %,** from deleting one `div` per cell. That is
**2.4x the entire measure-pass prize**, for one line. Per flex node per frame:
3.0 M / 162 = **~18.5 K instructions**. Screenshot: pixel-identical (the wrapper only
carries the column-*selection* background, which sid does not use).

**This is the finding that matters.** The lever on a sid frame is *nodes per cell*,
and sid's delegates own one of those nodes themselves: every `render_td` returns
`div().px_2().text_mono(&theme).text_color(..).child(text)`. Removing **that** div
should be worth the same order (~11 %, *not measured* — same kind of node in the same
position, one level deeper), and it needs **no vendoring at all**:

* `px_2` moves to `Column::paddings`, which `render_cell` already applies
  (`state.rs:604-627`) and which sid declares in `FillColumns::new`.
* the font and colour move to `TableDelegate::render_tr`, which sid's delegates may
  override and which is called once per row instead of once per cell — so the cells
  that share the row's colour need no `div` at all and can return the `SharedString`
  directly. Only the columns whose colour differs from the row default keep one
  (2 of 6 on the System table).

That is a pure route-(a) change, in sid's own files. It was **not attempted here**
because it is exactly the text-style surface a typography agent is sweeping in
`systems_tab.rs` / `network_tab.rs` this session, and the brief forbids touching it.
It should be the next thing anybody does to the table.

## 7. Recommendation

1. **Do not vendor the table for the measure passes.** 2 091 lines of third-party code
   plus a forked delegate trait, for **4.6 %**. Cross it off the ranked list; the "26 %
   of cell builds" figure that made it look like the biggest remaining win counts
   builds, and a measure cell is a third of the price of a rendered one.
2. **The System tab's sysinfo probe is worth far more than anything in the table**
   (§8) and is a much smaller change.
3. If the table is attacked again, attack **cells**, not measure passes: they are
   **81 % of the frame** at 121 K instructions each. The lever with the best
   ratio is node count per cell (§6), and the ceiling is a custom row element that
   paints pre-shaped lines (the terminal grid's `SshSession::shaped_cache` is the
   existence proof).
4. **Commit the harness.** This is the second time it has been rebuilt from scratch
   after a scratchpad wipe; it costs about ninety minutes each time.
   `scripts/perfcap.sh` + a `scroll`/`hover` addition to `scripts/cap-input/vptr.py`
   would end that, and the five traps in §0 are worth more than the scripts.
   (`main` has since moved to `ca17742`, which added `dclick`/`drag` to `vptr.py` —
   still no `scroll` and no `hover`. Those two commands are what a perf run needs.)

## 8. What else the numbers say (not this session's lane)

* **The System tab's 2 s sysinfo probe is the biggest CPU item in the app by an order
  of magnitude**: 0.6–2.5 G instructions per tick against ~50 M for a frame — one tick
  costs 12–50 frames of CPU. It runs on `ssh_runtime()` (a background tokio thread,
  `systems_tab.rs:781-786`), so it does not block the UI thread directly, but on a
  contended box it starves it, which matches the previously observed "tick frames
  measured 40–300 ms while scroll frames stayed at 25 ms". Ranked item #5 deserves to
  be #1 for the System tab specifically. Fix direction: `refresh_processes_specifics`
  with a minimal `ProcessRefreshKind` instead of a full refresh, and/or a longer
  interval when the window is unfocused.
* **The scrollbar fade-out did not reproduce as a frame source.** `FADE_OUT_DELAY` 2 s,
  `FADE_OUT_DURATION` 3 s (`scrollbar.rs:32-33`), so the `request_animation_frame`
  burst is the *third* second after input stops; a 2 s tail window measured 0–2 frames.
  Worth re-measuring with a 2–4 s window before spending anything on it.
* `ScrollbarShow::Always` (theme-level) would skip the fade branch entirely
  (`scrollbar.rs:607-667`) — a one-line theme change if that burst is ever confirmed
  to matter. `Hover` would **not**: it falls through to the same fade branch.
