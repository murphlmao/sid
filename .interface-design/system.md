# sid design system (round-F design review)

Direction: a calm instrument panel in deep space. Murphy at his battlestation,
keyboard-first, hopping between a shell, a query, and system state — the UI is
orientation and engagement, never decoration.

## Tokens
`crates/sid/src/ui/theme.rs` is the single source: bg / surface / well / border /
fg / fg_strong / muted / faint / accent / success / warning / danger / selection
(+ ansi[16] for the terminal). No raw hex in UI code except the theme-agnostic
modal scrim `rgba(0x000000a8)` and the warning-badge's near-black label.

## Depth
Borders + surface shifts only. No shadows. Hairline `border` separates regions;
`surface` raises chrome/cards/modals; `well` recesses inputs/editors/terminals;
`selection` fills the active row. Sidebars share `bg` with the canvas.

## Structure rules
- ONE top chrome bar: wordmark · tabs · (right) scope chips · warning badge.
- Reading surfaces (SSH home, Settings, config lists) are centered columns capped
  at `max_w(880px)` — a label's action never lives a screen-width away. Data
  tables (processes, ports, results) stay full-width.
- One list per fact. Never render the same collection twice on one screen.
- Section headers: `text_xs` UPPERCASE `muted`, optionally `· count`.
- Rows: `px_3 py_2`, `rounded_md`, hover = `selection` fill; primary action
  inline, everything else in the right-click menu.
- `accent` means "engage" (connect, run, active marker). Orientation badges
  (origin, counts) are `faint`/`muted`. One accent, used sparingly.

## Typography
Content `text_sm`; metadata/hints `text_xs` muted; monospace for addresses,
paths, and data cells. Terminal font: CaskaydiaCove Nerd Font Mono @ 14px,
cell height = font ascent+descent (kitty geometry).

## Interaction
Every actionable row: hover fill, cursor_pointer, right-click menu. Modals close
on Esc and MUST refocus `AppState::root_focus`. Empty states say what to do
next, in muted text, without a box.
