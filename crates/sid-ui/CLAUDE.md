# sid-ui (design system)

The component crate: tokens, type roles, depth helpers, ~30 components, the gpui-component
theme bridge, and the component gallery. The only crate besides `sid` that may name GPUI.

## Surface
- `theme.rs` (`Theme`, `theme::active(cx)`, four palettes: cosmos/void/dusk/cosmos-light),
  `typography.rs` (ten roles: `.text_display/title/heading/body/control/label/meta/mono/cell(&t)`),
  `metrics.rs` (`Space`, `Radius` + `.corners()`, `Density`, semantic gaps), `motion.rs`
  (`motion::enter`, durations, reduced motion), `elevation.rs` (rungs incl. shadows),
  `copy.rs` (`sentence_case`), `styled.rs` (`StyledExt`: `clamp_one_line`, `focus_ring`),
  `scale.rs` (`UiScale`, `scaled`). Design law: `.interface-design/system.md` (v2).
- Components re-exported in `lib.rs`: `Button`/`IconButton`, `Badge`, `Card::panel`, `Toolbar`,
  `Modal`, `List`/`Row`, `table::FillTable`, `input::*`, `StatusBar`, `Meter`/`StatCluster`, `toast`,
  `Tree`/`TreeState`, `ContextMenu`/`MenuItem`, `Checkbox`, `Switch`, `Skeleton`, `EmptyState`, ...
- `bridge.rs` — projects the sid palette onto gpui-component's theme; `bridge::SCRIM` is the only
  allowed raw colour; `bridge::sync` is called before first paint.
- `gallery.rs` — every widget/variant/state; mounted by `SID_GALLERY=1`.

## Invariants
- Semantic tokens only; depth by a named `Elevation` rung (soft theme-aware shadows since v2,
  never a hand-typed `.shadow*` — hygiene enforces it in `crates/sid`); `accent` = "engage".
- Measurements come from `metrics` (spacing, radius, control heights 24/32/36/44, row
  densities); components own the sentence casing of their UI-vocabulary slots.
- Call sites name a type ROLE, never raw size/weight/family.
- Anything wrapping a gpui-component widget must go through `bridge`.
- Every new component appears in the gallery.

## Test
`nice -n 10 cargo test -p sid-ui -j 3` — `tests/hygiene.rs` is the load-bearing one: bans raw
hex, raw `text_xs()/text_sm()/text_size()/font_weight()/font_family()`, the literal `.truncate(`,
emoji, and `gpui_component::` in `crates/sid/src` outside `main.rs`. Visual check:
`SID_GALLERY=1 scripts/sid-cap.sh --out target/caps/gallery.png` (`just gallery`).

## Landmines
- `FillTable` measures in prepaint; widths land next frame via `Window::on_next_frame`
  (`Window::refresh` is a no-op mid-draw). `Column.width` is `Pixels`-only.
- A gpui-component widget not threaded through the bridge renders in the library's stock theme
  (looks like a missing style). `sid_ui::Kbd` draws its own chip for this reason.
- Hygiene allowlists must have no dead entries (tests enforce it) — delete entries when fixed.

## UI interaction tests
Assert behaviour (focus, keys, clicks, geometry) instead of eyeballing PNGs. `gpui-kit` is a
DEV-dependency here only (`gpui_kit::test::TestWindowExt`: `find/click/press/input/hover/within`
over a real headless window; no compositor). It cannot see pixels or colour — `sid-cap.sh` stays
the tool for ink/contrast/themes. Run: `just ui-test` (files: `crates/sid-ui/tests/*.rs`).
- Make a component findable: `.id(..).test_support()` BEFORE `.track_focus(..)`/`.tab_index(..)`
  (`sid_ui::TestSupportExt`; no-op in shipping builds). Library-backed widgets (Button, inputs)
  are already findable by the id you give them; `TextInput`/`SearchInput` take `.test_id("x")`.
- Use `tests/common/mod.rs`: `open_window(cx, size, |window, cx| entity)` (init + theme + bridge +
  `Root`) and `press(handle, "shift-tab", cx)` (real keymap, then settles deferred work).
- Bare `Tab` is bound by the app shell, not sid-ui. Inside a `Modal`/`TextInput` Tab works; for a
  loose button call `window.focus_next(cx)`.
- Import kit types explicitly (never `use gpui_kit::*`: its `test` macro shadows `#[test]`).
- `lib.rs` has `extern crate gpui as gpui_kit;` — gpui's derive macros emit `gpui_kit::` paths
  once the dev-dep exists; keep it.
- Write each test RED first: break the behaviour, watch it fail, restore.

```rust
mod common;
use gpui_kit::{AppContext as _, TestAppContext, test::TestWindowExt as _, px, size};
#[gpui_kit::test]
fn click_counts(cx: &mut TestAppContext) {
    let (handle, probe) = common::open_window(cx, size(px(400.), px(300.)), |_, cx| cx.new(|_| Probe::default()));
    cx.update_window(handle.into(), |_, w, cx| w.click("live", cx)).unwrap();
    assert_eq!(probe.read_with(cx, |p, _| p.clicks), 1);
}
```
