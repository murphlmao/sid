# sid design system (round-F design review)

Direction: a calm instrument panel in deep space. Murphy at his battlestation,
keyboard-first, hopping between a shell, a query, and system state — the UI is
orientation and engagement, never decoration.

## Tokens
`crates/sid-ui/src/theme.rs` is the single source: bg / surface / well / border /
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
`crates/sid-ui/src/typography.rs` is the single source: **three sizes, two
weights, one mono family**. Call sites name a role, never a measurement —
`tests/hygiene.rs` fails the build on `text_xs()` / `text_sm()` /
`text_size(..)` / `font_weight(..)` / `font_family(..)` anywhere else.

| Role        | Helper              | Size | Weight | Ink         | For |
|-------------|---------------------|------|--------|-------------|-----|
| Title       | `.text_title(&t)`   | 16px | medium | `fg_strong` | screen / panel / modal headings |
| Body        | `.text_body(&t)`    | 14px | normal | `fg`        | rows, values, fields, buttons, copy |
| Label       | `.text_label(&t)`   | 12px | medium | `muted`     | UPPERCASE section headers (`· count` optional) |
| Meta        | `.text_meta(&t)`    | 12px | normal | `muted`     | hints, counts, timestamps, badge words, status strips |
| Mono        | `.text_mono(&t)`    | 14px | normal | `fg`        | paths, addresses, hosts, ports, data cells |
| MonoMeta    | `.text_mono_meta(&t)` | 12px | normal | `muted`   | dim paths, ids, hashes in a row's tail |

- **A role is absolute.** It sets size *and* weight *and* family *and* ink, so
  a Meta inside a Title is still 12px normal. Only the ink is meant to be
  overridden — `.text_meta(&t).text_color(rgb(t.danger))` is a dim error hint.
- **Every component owns its type.** A `Badge`, `Card`, `Row`, `Toolbar` or
  table header that sets no size renders at whatever its ancestor happened to
  choose; each of them now states a role.
- **A data table sits on one rung.** Columns differ by ink, never by size —
  a grid whose cells disagree about size reads as a rendering bug.
- **No BOLD.** 700 at 12-16px on a dark panel blooms into a colour change, and
  colour already carries meaning here. Medium (500) is the whole emphasis
  budget, spent on Title and Label.
- **`px`, not `rems`.** `text_xs`/`text_sm` were `rems(0.75)`/`rems(0.875)`
  against a 16px `rem_size` sid never changes — 12px and 14px in disguise.
  gpui pixels are logical, so a px scale is still HiDPI-correct.

The **terminal grid** is outside the scale: `sid-term` paints a PTY at
CaskaydiaCove Nerd Font Mono @ 14px, cell height = font ascent+descent (kitty
geometry). That is instrument metrics, not UI type.

## Interaction
Every actionable row: hover fill, cursor_pointer, right-click menu. Modals close
on Esc and MUST refocus `AppState::root_focus`. Empty states say what to do
next, in muted text, without a box.
