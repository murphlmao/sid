---
name: sid-new-component
description: Add a new component to the sid-ui design-system crate - file, exports, gallery entry, hygiene rules, and captures. Use when a tab needs a widget that does not exist in sid-ui, or when a repeated hand-rolled UI pattern in crates/sid should be promoted.
---

# sid-new-component

First check `crates/sid-ui/src/lib.rs` exports - an existing component (`Card::panel`,
`Toolbar`, `List`, `FillTable`, `InlineNotice`, `SegmentedControl`, `Modal`, ...) may already do it.

## Steps

1. **Create** `crates/sid-ui/src/<name>.rs`: a builder struct implementing `RenderOnce`
   (stateless) or an entity with `Render` (stateful). Read a close sibling first
   (`badge.rs` for simple, `toolbar.rs` / `card.rs` for composites).
2. **Style by tokens and roles only**: colours from `theme::active(cx)` (bg/surface/well/
   border/fg/muted/accent/...), never hex; text via type roles (`.text_body(&t)` etc. from
   `typography.rs`), never `text_xs()/text_sm()/text_size()/font_weight()/font_family()`;
   depth by border/surface shift, no shadows; one-line text = `min_w(0)` +
   `.clamp_one_line()` (+ `flex_none()` on the sibling); never the literal `.truncate(`;
   focus via `StyledExt::focus_ring`; scale via `UiScale`/`scaled`. No emoji in source.
3. **Wrapping a gpui-component widget?** Thread it through `bridge.rs` or it renders in the
   library's stock theme.
4. **Export**: `pub mod <name>;` and `pub use <name>::{..};` in `crates/sid-ui/src/lib.rs`
   (match the alphabetical neighbours).
5. **Gallery**: show every variant and state in `crates/sid-ui/src/gallery.rs` (extend
   `buttons` / `chips` / `structure` or add a section fn and call it in the `render` columns).
6. **Tests**: pure logic (paint/state computation) as unit tests in the module, like
   `radio_paint` in `radio.rs`. Pixels are verified by capture, not unit tests.
7. **Verify**: `nice -n 10 cargo test -p sid-ui -j 3` (`tests/hygiene.rs` enforces rule 3);
   `just gallery cosmos` and `just gallery cosmos-light`, Read both PNGs.
8. **Use it** from `crates/sid` through the `sid_ui` re-export - never `gpui_component::`
   (hygiene bans it outside `crates/sid/src/main.rs`).
9. Gate (`sid-gate` skill), commit `feat(sid-ui): <component>`, push.

## Gotchas
- `h_flex()` centres the cross axis; a `flex_1` table beside a sized sibling gets zero height.
- Do not remove or widen hygiene allowlists to get green - fix the call site.
