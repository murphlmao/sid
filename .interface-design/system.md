# sid design system — v2 "friendly" (2026-11-07)

Direction: a calm cockpit in deep space — and a friendly one. Murphy at his
battlestation, keyboard-first, hopping between a shell, a query and system state. v1 was
an instrument panel and read clinical (12px UPPERCASE labels, 24px controls, hairlines
on near-black, no depth, no motion). v2 keeps the galaxy palettes, the semantic tokens,
the keyboard and the discipline, and spends them on warmth: bigger reading type, real
headings, generous controls, soft depth, rounded objects, a little motion, sentence-case
copy that tells you what to do. Rationale, before/after and the per-tab rollout:
`docs/design/2026-11-07-friendly-ui-v2.md`.

All measurements are design px at 100% zoom, emitted through `sid_ui::scaled` (rems), so
app zoom (`UiScale`, ctrl +/-) still scales everything. "Bigger" is real role sizes and
density, **not** a default zoom bump — the terminal grid and data tables stay
instrument-sized.

## Tokens
`crates/sid-ui/src/theme.rs` is the single source of colour: bg / surface / well /
border / fg / fg_strong / muted / faint / accent / success / warning / danger /
selection / shadow (+ ansi[16] for the terminal). No raw hex in UI code except
`sid_ui::bridge::SCRIM` (the theme-agnostic modal scrim). Four palettes: cosmos, void,
dusk, cosmos-light. In light palettes cards (`surface`) are *lighter* than the canvas.

`crates/sid-ui/src/metrics.rs` is the single source of measurement: `Space` (4px grid:
4/8/12/16/20/24/32), `Radius` (Xs 4 · Sm 6 · Md 8 · Lg 12 · Xl 16 · Full), control
heights, row densities, and the semantic gaps — `PAGE_GUTTER` 20, `SECTION_GAP` 16,
`CARD_PADDING` 16, `STACK_GAP` 12, `CONTROL_GAP` 8, `ICON_GAP` 8.

## Depth
Soft, theme-aware shadows on things above the canvas, plus a softened hairline so dark
palettes keep an edge. `crates/sid-ui/src/elevation.rs` names the rungs:

| Rung | Fill | Border | Shadow | For |
|---|---|---|---|---|
| `Bg` | `bg` | – | – | canvas, sidebars |
| `Surface` | `surface` | `border` | small | cards, panels |
| `Well` | `well` | `border` | – | fields, editors, terminals (a recess) |
| `Floating` | raised surface | `border` | medium | menus, popovers, the command palette, tooltips |
| `Overlay` | `surface` | `border` | large | modals, over the scrim |

Shadow ink is `Theme::shadow` (black on dark palettes, tinted indigo on cosmos-light);
alphas live in `elevation.rs`. Never hand-type a `shadow(..)` — name a rung.

## Structure rules
- ONE top chrome bar: wordmark · tabs · (right) scope switcher · warning badge.
- **One Display heading per screen** (22px semibold), with the screen's primary action
  (icon + label) on the same line, right-aligned.
- **Wide screens fill with surfaces** — card grids, side-by-side panes, multi-column card
  layouts. Never a narrow reading column beside a void (Murphy vetoed the 880px cap);
  data tables are full-width.
- Content inset `PAGE_GUTTER`; `SECTION_GAP` between cards; `CARD_PADDING` inside them.
- One list per fact. Never render the same collection twice on one screen.
- Card / panel headers: `Heading` role, sentence case, count as a pill — no UPPERCASE,
  no `· n`.
- Rows: `Row` — 44px comfortable (default), 36px `compact()` for sidebars; hover =
  `selection` fill; selected = `selection` + a 3px accent marker on the leading edge;
  primary action inline, everything else in the right-click `ContextMenu`.
- Nested data (host groups, schemas, repo trees) is a `Tree` (32px rows, chevrons,
  arrow-key navigation, lazy children), not hand-rolled carets.
- `accent` means "engage": primary buttons, selected markers, checkbox/switch on, focus
  halo, active tab, links. Never a wash over large areas. Orientation badges (origin,
  counts) are `faint`/`muted`.

## Typography
`crates/sid-ui/src/typography.rs` is the single source: **five rungs (13 / 14 / 15 / 18 /
22), three weights (400 / 500 / 600), one mono family**. Call sites name a role, never a
measurement — `tests/hygiene.rs` fails the build on `text_xs()` / `text_sm()` /
`text_size(..)` / `font_weight(..)` / `font_family(..)` anywhere else.

| Role | Helper | Size | Weight | Ink | For |
|---|---|---|---|---|---|
| Display | `.text_display(&t)` | 22 | semibold | `fg_strong` | the screen's page heading (one per screen) |
| Title | `.text_title(&t)` | 18 | semibold | `fg_strong` | modal titles, detail-pane headings |
| Heading | `.text_heading(&t)` | 15 | semibold | `fg_strong` | card/panel/group headers, emphasised row titles |
| Body | `.text_body(&t)` | 15 | normal | `fg` | rows, values, fields, copy |
| Control | `.text_control(&t)` | 15 | medium | `fg` | button, tab and segment labels |
| Label | `.text_label(&t)` | 13 | medium | `muted` | field labels, small-control labels (sentence case) |
| Meta | `.text_meta(&t)` | 13 | normal | `muted` | hints, counts, timestamps, badge words |
| Mono | `.text_mono(&t)` | 14 | normal | `fg` | data cells, paths, hosts, ports |
| MonoMeta | `.text_mono_meta(&t)` | 13 | normal | `muted` | ids, hashes, dim paths |

- **A role is absolute** — size, weight, family and ink. Only the ink is meant to be
  overridden (`.text_meta(&t).text_color(rgb(t.danger))`).
- **Every component owns its type.** Nothing inherits a size from an ancestor.
- **A data table sits on one rung** (Mono 14). Columns differ by ink, never by size.
- **No BOLD (700).** Semibold (600) is for Display/Title/Heading only; Medium (500) for
  Control and Label. Noto Sans ships a true SemiBold — no synthetic weights.
- 14 is the mono rung only: DejaVu Sans Mono is ~20% wider per glyph, so tables keep
  their columns.
- `section_label` / `hint_text` remain aliases of Label / Meta.

The **terminal grid** is outside the scale: `sid-term` paints a PTY at CaskaydiaCove
Nerd Font Mono @ 14px with kitty cell geometry. Instrument metrics, not UI type.

## Controls and states
- Heights: **Sm 32 · Md 36 (default) · Lg 44** for `Button`, `IconButton`, `TextInput`,
  `SearchInput`, `SegmentedControl`. Minimum hit target 32. Radius `Md` (8). The one
  exception is **Xs 24**, for controls inside a `FillTable` cell (a 32px instrument row
  has a 24px content box); `ConfirmButton` defaults to it.
- Labels use the `Control` role (Md/Lg) or `Label` (Sm); primary actions are icon +
  label; icon-only buttons must carry a tooltip (type-enforced).
- Hover: one step toward the light ink (`hover_of`). Pressed: one step toward the dark
  ink (`pressed_of`). Disabled: fill toward `bg`, ink `faint`, hairline kept, out of
  tab order. Loading: spinner in the leading slot, colours unchanged.
- **Focus**: accent hairline + 3px accent halo at 30% (`StyledExt::focus_ring`, shown on
  focus-visible) — one spelling, costs no layout.
- `Checkbox` (18px box, `Radius::Xs`, accent when on), `Switch` (36x20 pill, accent
  track when on, sliding thumb), `Radio` — all keyboard-operable (Space/Enter).

## Motion
`crates/sid-ui/src/motion.rs`: `FAST` 120ms · `BASE` 160ms · `SLOW` 200ms, ease-out
cubic, **entrances only** (`Enter::Fade`, `Enter::Rise` — an 8px settle via relative
offset, never a layout shift). Modal = Rise/SLOW, Toast = Rise/BASE, tab content =
Fade/BASE, switch thumb = FAST. **Reduced motion is honoured**: `SID_REDUCED_MOTION=1`
or `motion::set_reduced` (Settings switch) renders the final state with no animation
element; skeleton pulses stop too.

## Copy voice
- Sentence case everywhere. Components own the casing of their UI-vocabulary slots
  (`copy::sentence_case` in Button, Card title, EmptyState headline, Modal title, Toast
  title, ContextMenu items); data (aliases, paths, theme names) is never re-cased.
- Empty states: what is missing (headline) · what to do (one line) · the button to do
  it. Filtered-empty: `Nothing matches "q"` + `Clear filter`.
- Errors: what happened (title) · what to try (message) · the action (Retry). Raw error
  text goes below in MonoMeta, never as the headline.
- Verbs on buttons, `…` when a dialog follows, no `OK`. No emoji anywhere (hygiene).

## Interaction
Every actionable row: hover fill, cursor_pointer, right-click `ContextMenu`. Modals close
on Esc and MUST refocus `AppState::root_focus`; Tab is trapped inside a modal. Empty
states are `EmptyState` — icon, headline, one line, a primary button — never a bare
muted sentence.
