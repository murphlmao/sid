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
