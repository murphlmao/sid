# sid design system v2 — bigger, easier, friendlier (2026-11-07)

Murphy, verbatim: *"id like the ui to feel a little bigger and easier to use and more
friendly, because right now it handles itself very clinically, and im not sure i super
love it. if that means maybe we switch to another ui framework, so be it, but see what we
can do with the tools we have here."*

**Decision: stay on GPUI.** Nothing in the captures below is a framework limit. The
clinical feel is sid's own design law (`.interface-design/system.md` v1): three sizes,
no bold, no shadows, hairlines on near-black, 24px controls, UPPERCASE 12px labels, and
motion explicitly excluded. GPUI renders soft shadows, real font weights, rounded
corners, opacity and timed animations; v1 simply chose not to use them. v2 changes the
law, then the components (`crates/sid-ui`), and most screens follow for free because
they already call components and type roles. What the components cannot reach is in the
**Rollout guide** (§4), which the next wave executes tab by tab.

Before captures: `target/caps/before/{ssh,database,network,workspaces,system,settings,gallery}-{cosmos,cosmos-light}.png`.
After captures: `target/caps/after/…` (same names).

---

## 1. Why it reads clinical — per screen

Measured on the 1920x1080 captures above (cosmos and cosmos-light).

**Everywhere (the law, not the screens)**
- The only heading on most screens is a 12px UPPERCASE muted label (`CONNECTIONS · 2`,
  `QUERY`, `RESULTS`, `PORTS · 37`, `SYSTEM`, `WORKSPACE`). Nothing on any screen is
  bigger than 16px, and nothing is heavier than 500. The eye has no landing point; every
  screen reads like a form's fine print.
- UPPERCASE + letter-dense + muted is the typographic voice of a warning label. Combined
  with lowercase everything else (`connect`, `add a connection`, `no connection
  selected`) the copy reads terse and machine-like rather than relaxed.
- Controls are small: `ButtonSize::Sm` is 24px with a 12px label, `Md` 32px. Every row
  action, every card action and every panel-header action is the 24px size, so most of
  the app is 24px targets. Row padding is 8px vertical — rows are ~38px of mostly text.
- Depth is a 1px hairline on a fill 3% brighter than the canvas (cosmos `surface`
  `#13131f` on `bg` `#0b0b14`). Cards read as outlines, not objects. Panels butt against
  each other with 8px gaps, so the screen is a lattice of hairlines.
- Corners are 6px (library radius) / `rounded_md` (6px) / `rounded_lg` (8px) — near
  square at these sizes.
- Nothing moves. A modal, a menu and a toast pop into existence in one frame, which
  reads as abrupt rather than responsive.
- cosmos-light: `surface` (`#eaeaf2`) is *darker* than the canvas (`#f4f4f8`), so every
  card in the light theme reads as a grey recess — the opposite of the paper-on-desk
  convention every light UI uses.

**SSH / SFTP** (`ssh_home.rs`)
- Two 340px host cards and ~1.5 million px² of empty canvas: the grid fills one row and
  the panel stretches to the bottom of the window around nothing.
- The status legend (`not connected · connecting · connected · connection failed`) is an
  always-on strip of four dots above the cards — it reads as a debug key, not content.
- Card anatomy is dense and small: 14px alias, 12px mono `user@host:port`, a 24px
  `connect` button and two 24px icon buttons, all inside 12px padding.
- The panel header is the 12px `CONNECTIONS · 2` with a 300px filter and two 24px
  buttons, one of them a red `connect` — the first red thing on screen is a button
  labelled with a verb that does nothing until the field has text.

**Database** (`db_tab.rs`)
- Six panels, six UPPERCASE 12px headers, 8px gutters; the sidebar is three stacked
  hairline boxes. The schema empty state is a bare 12px line (`no schema loaded — select
  a connection`) inside a 560px-tall box; HISTORY's is the same.
- `explain` and `run` are disabled with 24px boxes and faint ink — they disappear.
- The good part: the results EmptyState (icon, headline, guidance, primary button) is
  already the right shape; it is just small (14px headline, 12px guidance).

**Network** (`network_tab.rs`)
- The data table is fine as an instrument (32px rows, mono 14px) — this is the one screen
  where density is correct. What reads clinical is the chrome around it: 12px `PORTS ·
  37`, a 24px filter, a 24px `refresh`, a column of identical 24px `kill` chips down the
  right edge, and a segmented sub-nav whose selected chip is a barely-lighter grey.

**Workspaces** (`workspaces_tab.rs`)
- Left list rows lead with a `no git` badge before the name, then a truncated mono path,
  then 12px counts. The detail pane's header is the bare word `WORKSPACE` over 1600px of
  empty panel with a small centred empty state.

**System** (`systems_tab.rs`)
- `SYSTEM` header over one 12px line of facts; meters are 4px bars with 12px labels; 16
  core bars render as a row of 6px ticks. The processes table is a correct instrument,
  but the header strip above it repeats the 12px/24px pattern.

**Settings** (`settings_tab.rs`)
- The 880px reading column (`CONTENT_MAX_W`) leaves a 700px void to its right at 1920
  wide — the exact layout Murphy vetoed on SSH home.
- Theme rows are bordered boxes inside a bordered card (double hairlines); `APPEARANCE ·
  4` counts the themes, which is not information anyone wants.

**App chrome** (`app.rs`)
- Top bar: 14px tab labels — fine — but the scope switcher is a 12px segmented control
  in the far corner, and the status bar is a single cyan word (`keyring`) at the bottom
  left. Neither reads as part of the same object as the tab strip.

---

## 2. The v2 system — exact values

All sizes are design pixels at 100% zoom, emitted through `sid_ui::scaled` (rems), so app
zoom (`UiScale`, ctrl +/-) still scales everything uniformly.

### 2.1 On "bigger": roles and density, not a zoom bump

The default `UiScale` stays **100%**. Zoom multiplies *everything* by the same factor —
the terminal grid, the data tables, the gutters — so a default of 110% would make the
dense instruments (processes, ports, results, the PTY) worse in exactly proportion to
how much it helped the chrome, and would fit less on screen. v2 changes *proportions*:
reading text, headings, controls and rows get bigger; data tables and the terminal stay
instrument-sized. Zoom remains the user's lever on top.

### 2.2 Type roles (`crates/sid-ui/src/typography.rs`)

| Role | Helper | Size | Weight | Ink | For |
|---|---|---|---|---|---|
| Display | `.text_display(&t)` | **22** | Semibold 600 | `fg_strong` | one per screen: the page heading |
| Title | `.text_title(&t)` | **18** | Semibold 600 | `fg_strong` | modal titles, detail-pane headings |
| Heading | `.text_heading(&t)` | **15** | Semibold 600 | `fg_strong` | card/panel headers, group headers, emphasised row titles |
| Body | `.text_body(&t)` | **15** | Normal 400 | `fg` | rows, values, fields, copy |
| Control | `.text_control(&t)` | **15** | Medium 500 | `fg` | button labels (Md/Lg), tab labels, segment labels |
| Label | `.text_label(&t)` | **13** | Medium 500 | `muted` | field labels, small-control labels — **sentence case** |
| Meta | `.text_meta(&t)` | **13** | Normal 400 | `muted` | hints, counts, timestamps, badge words |
| Mono | `.text_mono(&t)` | **14** | Normal 400 | `fg` | data cells, paths, hosts, ports |
| MonoMeta | `.text_mono_meta(&t)` | **13** | Normal 400 | `muted` | ids, hashes, dim paths |

- **Five rungs: 13 / 14 / 15 / 18 / 22.** Body moves 14 → 15 and Meta 12 → 13: the
  "a little bigger" Murphy asked for, everywhere text is read. 14 is the mono rung only:
  DejaVu Sans Mono is ~20% wider per glyph than Noto Sans, so a 15px data cell would cost
  a table a column; tables stay where they were.
- **Weights 400 / 500 / 600.** Semibold is allowed for Display/Title/Heading. Noto Sans
  (the system UI face on this box) ships a true SemiBold, so it renders as a weight, not
  as a smeared synthetic bold. 700 stays banned — it still blooms on dark at these sizes.
- **Label is sentence case.** No UPPERCASE anywhere in v2. Card headers move off Label
  entirely, onto Heading.
- **Old names stay.** `section_label` / `hint_text` remain aliases (Label / Meta). No
  call site in `crates/sid` has to change for the roles.

### 2.3 Spacing (`crates/sid-ui/src/metrics.rs`)

A 4px grid. `Space::{X1=4, X2=8, X3=12, X4=16, X5=20, X6=24, X8=32}`. Semantic tokens:

| Token | v1 | **v2** | Use |
|---|---|---|---|
| `PAGE_GUTTER` | 12 | **20** | a tab's content inset from the window edge |
| `SECTION_GAP` | 8–12 | **16** | between cards/panels on a screen |
| `CARD_PADDING` | 12 | **16** | inside a card |
| `STACK_GAP` | 8 | **12** | between rows/fields inside a card |
| `CONTROL_GAP` | 4–8 | **8** | between adjacent controls |
| `ICON_GAP` | 6 | **8** | icon ↔ label inside a control |

### 2.4 Density and hit targets

| Thing | v1 | **v2** |
|---|---|---|
| Button / IconButton — Xs (new: inside a FillTable cell only) | – | **24** |
| Button / IconButton / field — Sm | 24 | **32** |
| — Md (default) | 32 | **36** |
| — Lg (new: page primary, empty-state action) | – | **44** |
| List `Row` min height (comfortable, default) | ~38 (content) | **44** |
| `Row` compact (`.compact()`: sidebars, menus-in-lists) | – | **36** |
| `Tree` row | – | **32** |
| Panel header strip | 40 | **52** |
| Table row (FillTable, library) | 32 | 32 (instrument — unchanged) |
| Row padding | 12 / 8 | **12 / 6** (+ min height) |
| Minimum hit target | 24 | **32** (except Xs, in-table) |

### 2.5 Radius (`metrics::Radius`)

| Token | px | For |
|---|---|---|
| `Xs` | 4 | checkbox box, kbd |
| `Sm` | 6 | badges, tooltips, chips inside rows |
| `Md` | 8 | buttons, fields, rows, segmented controls, menu items |
| `Lg` | 12 | cards, panels, toasts, popovers, menus, grid cards |
| `Xl` | 16 | modals |
| `Full` | ∞ | switch, count pills, status pills |

The library bridge moves from `radius 6 / radius_lg 8` to **8 / 12**.

### 2.6 Elevation (`crates/sid-ui/src/elevation.rs`) — amends "no shadows"

v1's "borders + surface shifts only" is retired. On a near-black canvas a 3% fill step
plus a hairline is the clinical look; light themes cannot separate a white card from an
off-white desk without one. v2 uses **soft, theme-aware shadows on things that sit
above the canvas**, keeps the hairline (softened) so dark palettes still have an edge,
and lifts surfaces a step so cards read as objects:

| Rung | Fill | Border | Shadow (dark palettes / cosmos-light) | For |
|---|---|---|---|---|
| `Bg` | `bg` | – | – | canvas, sidebars |
| `Surface` | `surface` | `border` | `0 1 2 /.30` + `0 2 8 /.18` · light `0 1 2 /.06` + `0 2 8 /.05` | cards, panels |
| `Well` | `well` | `border` | – (recess) | fields, editors, terminals |
| `Floating` (new) | raised surface (dark) · `surface` (light) | `border` | `0 2 4 /.30` + `0 8 24 /.45` · light `/.06` + `/.12` | menus, popovers, palettes, tooltips |
| `Overlay` | `surface` | `border` | `0 4 8 /.30` + `0 16 48 /.60` · light `/.06` + `/.20` | modals (over the scrim) |

Shadow ink is a new palette token, `Theme::shadow` (`#000000` on dark palettes, a
tinted `#1d1a33` on cosmos-light so the shadow is part of the palette, not grey
smudge). The library's own `shadow` flag goes **on**, so `PopupMenu`/popover chrome
gets the library's shadow too.

### 2.7 Colour warmth (exact palette deltas, `theme.rs`)

The four palettes and their identities stay. Surfaces lift one step so cards are
objects; borders soften toward the surface (shadows now carry separation); neutrals in
cosmos tilt from blue-grey toward violet (the nebula, not the instrument bezel); `muted`
gets brighter and warmer so 13px meta stays comfortably legible.

| Palette | Token | v1 | **v2** |
|---|---|---|---|
| cosmos | `surface` | `#13131f` | **`#181626`** |
| | `border` | `#1f1f2e` | **`#29263d`** |
| | `selection` | `#1c1c2c` | **`#242138`** |
| | `muted` | `#8a8a9a` | **`#9893ab`** |
| | `faint` | `#4a4a60` | **`#554f6c`** |
| | `fg` | `#e6e6f0` | **`#e9e6f3`** |
| void | `surface` / `border` / `selection` | `#0a0a0a` / `#222222` / `#161616` | **`#101010` / `#262626` / `#1c1c1c`** (stays monochrome) |
| dusk | `surface` / `border` / `selection` / `muted` | `#1c1812` / `#2a221a` / `#241d14` / `#9a8c74` | **`#211b14` / `#32291e` / `#2b231a` / `#a6977c`** |
| cosmos-light | `bg` | `#f4f4f8` | **`#f1f0f6`** |
| | `surface` | `#eaeaf2` | **`#fdfcff`** (cards are paper on the desk) |
| | `well` | `#e6e6ee` | **`#ebeaf2`** |
| | `border` | `#d0d0dc` | **`#dcdae6`** |
| | `selection` | `#dedee8` | **`#e7e5f0`** |
| all | `shadow` (new) | – | `#000000` dark · `#1d1a33` light |

**Accent, more generously — for affordances only.** The accent still means "engage", and
in cosmos it is red, so it is never a wash over large areas. v2 spends it on: primary
buttons; the selected-row marker (a 3px accent pill on the leading edge of a selected
`Row`/`Tree` row); checkbox and switch "on"; the focus halo; the active tab underline;
links and text actions.

### 2.8 Focus ring (`styled::focus_ring`)

v1: an accent hairline. v2: the accent hairline **plus a 3px accent halo at 30% alpha**
(a spread shadow, so it costs no layout), shown on `focus-visible` for hand-rolled
elements. The library's own ring token stays mapped to accent by the bridge.

### 2.9 States

| State | Rule |
|---|---|
| Hover | fill one step toward the palette's light ink (`hover_of`, 12%); ghost → `selection` |
| Pressed | 14% toward the dark ink (`pressed_of`) |
| Selected | `selection` fill + 3px accent marker on the leading edge |
| Focus | accent hairline + 3px accent halo (30%) |
| Disabled | fill 80% toward `bg`, ink `faint`, hairline kept, no pointer, out of tab order |
| Loading | spinner replaces the leading icon; colours unchanged |

### 2.10 Motion (`crates/sid-ui/src/motion.rs`) — removes "deliberately not on the list"

| Token | ms | Easing | Used by |
|---|---|---|---|
| `FAST` | 120 | ease-out cubic | tooltips, small reveals, switch thumb |
| `BASE` | 160 | ease-out cubic | toasts, popovers, tab-content fade |
| `SLOW` | 200 | ease-out cubic | modals |

- Entrances only (gpui drops an element the frame it leaves the tree; exit animation
  would need retained state for no real gain).
- Kinds: `Enter::Fade` (opacity 0 → 1) and `Enter::Rise` (opacity + 8px upward settle,
  via relative offset so layout never moves).
- **Reduced motion is honoured**: `SID_REDUCED_MOTION=1`, or `motion::set_reduced(cx,
  true)` (the Settings toggle — rollout §4.6). Reduced = final state, no animation
  element at all. Skeleton pulses and the switch slide also stop.
- Modal (`Rise`, SLOW), Toast (`Rise`, BASE) and Switch animate inside `sid-ui`. Tab
  content and library popovers adopt `motion::enter` in the rollout.

### 2.11 Copy voice

- **Sentence case everywhere** — headers, buttons, menu items, modal titles, empty-state
  headlines. Components that own a UI-vocabulary slot apply `copy::sentence_case`
  themselves (Button labels, Card titles, EmptyState headlines, Modal titles, Toast
  titles, ContextMenu items), so `"add connection"` renders `Add connection` without a
  call-site change. Data (aliases, paths, theme names) is never touched.
- Section headers drop the `· count` middot; a count renders as a small pill after the
  title.
- **Empty states say what is missing, what to do, and offer the button**: `No saved
  connections` / `Add one, or type user@host above and press Enter.` / `[+ Add
  connection]`. A filtered-empty state says `Nothing matches "foo"` and offers `Clear
  filter`.
- **Errors say what happened, then what to try, then offer the action**: `Couldn't
  connect to vps-1` / `The host refused the key. Check the key path, or try a password.`
  / `[Retry]`. The raw error goes underneath in MonoMeta, never as the headline.
- Buttons are verbs (`Connect`, `Add host`); `…` when the button opens a dialog
  (`Edit…`). No `OK`.
- Primary actions carry **icon + label**. Icon-only buttons keep their mandatory tooltip.
- No emoji, ever (hygiene-enforced); glyphs come from `Icon`.

### 2.12 New and changed components (`crates/sid-ui`)

Changed: `Button`/`IconButton` (Sm/Md/Lg = 32/36/44, Control role labels, sentence case,
8px radius), `TextInput`/`SearchInput` (32/36/44 heights), `Card` (12px radius, 16px
padding, shadow, Heading-role header + count pill, 52px panel header), `Row` (44px
min, compact 36, accent marker when selected), `Modal` (16px radius, overlay shadow,
rise-in), `Toast` (an *inline* strip, so 8px radius and no shadow; Heading title,
Body message, rise-in keyed by its message), `EmptyState` (v2 shape: 22px icon on a
48px soft disc, Title headline, Body guidance, the caller's Lg primary button, optional
secondary; `compact()` for small panels), `Badge` (6px corner, pills stay pills),
`SegmentedControl` (track = the button of its size; a white chip on the light track),
`GridCard` (12px radius, 16px padding, Surface shadow, 16px grid gap), `Toolbar`
(same 52px strip as a panel header), `focus_ring` (halo).

New: `Tree` (expand/collapse, keyboard nav, selection, lazy children), `ContextMenu`
(sid-styled builder over `PopupMenu`, icons, danger items, headings), `Checkbox`,
`Switch`, `Skeleton` (line / block / rows, pulsing), `CountPill`, `motion`, `metrics`,
`copy`. `Tipped` already covers tooltips; it gains `tip_with_key`.

---

## 3. What stays

The deep-space palettes and semantic tokens; keyboard-first; one list per fact; fill
wide screens with surfaces (grids, panes, cards) — never a narrow column beside a void;
named type roles with the hygiene ratchet; no emoji; no raw hex outside `theme.rs` and
`bridge::SCRIM`; the terminal grid outside the type scale.

---

## 4. Rollout guide — per tab

For the next wave. Each tab is one agent, one branch. Do the items in order; capture
before/after with `scripts/sid-cap.sh` at 1920x1080 and 1280x800 in `cosmos` and
`cosmos-light`. Components referenced are all in `sid_ui`. The components already moved
the sizes, radii, shadows and sentence case; what is left is what only the screen knows.

### 4.0 Rules that apply to every tab
1. **One Display heading per screen**, top-left of the content, with the screen's primary
   action (Lg or Md primary button, icon + label) on the same line, right-aligned. Use
   `.text_display(&t)`. Example: `SSH hosts            [+ Add connection]`.
2. Outer padding `metrics::PAGE_GUTTER` (20); gaps between panels
   `metrics::SECTION_GAP` (16) — replace `p_3()`/`gap_2()` at the tab root.
3. Replace ad-hoc empty text (`div().text_meta(..).child("no … yet")`) with `EmptyState`
   (headline + one line + action). Inside small panels use `EmptyState::compact()`.
4. Replace `error_line(e)` under a panel with `Toast::danger(..).title("Couldn't …")`
   + the raw error as the message; offer a Retry action where one exists.
5. Hand-rolled chevron disclosure → `Tree`. Hand-rolled glyph toggles → `Checkbox` /
   `Switch`. `PopupMenuItem` chains → `ContextMenu` (`ContextMenu::new().item(..)
   .danger_item(..).apply(menu)`); put destructive items last, after a separator, as
   `danger_item`.
6. Rewrite lowercase copy in strings the components do *not* own (guidance lines,
   tooltips, placeholders, status words) to sentence case. Delete any
   `.to_uppercase()` and hard-coded UPPERCASE (`"FILES"`).
7. Row lists → `Row` (44px). Sidebars of rows → `Row::compact()` (36px).
8. Anything inside a `FillTable` cell is `ButtonSize::Xs` (24px): a table row is a
   32px instrument row with 4px cell padding, so its content box is 24px and a 32px
   control pokes out of it. `ConfirmButton` (kill/stop) already defaults to Xs.
9. Wrap a tab's content root in `motion::enter(("tab-content", tab_ix), Enter::Fade,
   BASE, cx, el)` so switching tabs settles instead of snapping.

### 4.1 SSH / SFTP — `ssh_home.rs`, `session.rs`, `host_form.rs`, `password_prompt.rs`
- **Home header**: replace the `Card::panel("connections")` wrapper (L630) with a page
  header row: `Display "SSH hosts"` + count pill, then the quick-connect field (Md,
  `FieldWidth::Fill`, max 480) and `[+ Add connection]` (primary, Md, icon Add). Drop the
  red header `connect` button (L988): Enter in the field connects; the field's trailing
  hint says so.
- **Status legend** (`StatusLegend`, above the grid): remove from the default view; put
  it in the status dot tooltips (`tip("Connected")`) — the dots are self-describing once
  they have tooltips.
- **Card grid** (`CardGrid`, L615): cards become 12px-radius raised surfaces with 16px
  padding (automatic). Inside `GridCard` (L1407): alias in `Heading`, `user@host:port`
  in `Mono`, the scope chip as a pill top-right, and the actions row as
  `Button "Connect"` (Md, primary, icon Terminal) + IconButtons Md for SFTP/Edit.
- **Folders** (hand-rolled caret L1194–1241, `collapsed_folders`): folder group headers
  become `Heading` + count pill with a ChevronDown/Right IconButton; keep the grid layout
  inside each folder (a `Tree` is wrong here — the cards are the content).
- **Context menu** (`row_context_menu` L819): move to `ContextMenu`; labels already
  sentence-case via the component; `Delete` as `danger_item` after a separator; `Edit…`,
  `Assign folder…` keep the ellipsis.
- **Empty states** (L731, L747): `EmptyState::new("No saved connections")
  .guidance("Add one, or type user@host above and press Enter.")
  .action(Button "Add connection" Lg primary icon Add)`; filtered: `Nothing matches
  "{query}"` + `Clear filter`.
- **Session** (`session.rs`): the SFTP sidebar's hand-rolled glyph buttons (`↑`, `⟳`,
  `☑/☐ hidden`, `«`, `»`, `✕ close` — L1943–2083, L2659) become `IconButton`s
  (ArrowUp, Refresh, ChevronsLeft/Right, Close) and a `Checkbox` labelled `Show hidden`.
  `"FILES"` (L2007) → `Heading "Files"`. The file list → `Tree` with lazy children
  (directories load on expand — `TreeEvent::LoadChildren`), replacing the
  navigate-into-directory model only if Murphy agrees (decision D2 below); otherwise keep
  rows and just adopt `Row::compact()`.
- **Host form / password prompt**: titles already sentence-case through `Modal`; rewrite
  the keyring hint (`host_form.rs` L333, `password_prompt.rs` L127) to "There's no system
  keyring, so sid will keep this password for this session only." Radio marks
  (`host_form.rs` L833) → `Radio` (exists).

### 4.2 Database — `db_tab.rs`, `db_conn_form.rs`, `db_diagram.rs`
- Page header: `Display "Databases"` + `[+ Add connection]` primary; the sidebar's own
  `+` IconButton (L2533) goes.
- **Schema** (`schema_tree_rows` L1011, toggles L2350–2368): replace with `Tree` — schema
  → table → column, lazy table children (`LoadChildren` fires the column probe), column
  type in the row meta (`MonoMeta`), `Icon::Table`/`Icon::Columns` leading icons.
  Right-click → `ContextMenu` (Select top 100, Copy name, Show in diagram).
- **Connection folders** (`group_connections` L2473, chevron L2606): `Tree` with folder
  nodes and connection leaves; the leaf's StatusDot as leading, read-only/scope badges
  as meta.
- Ad-hoc empties → `EmptyState::compact()`: L2213 "Pick a connection to see its schema",
  L2419 "Queries you run show up here", L2497 "No connections yet".
- Headers: `Card::panel("query")` etc. render sentence-case Heading automatically; drop
  the `· {name}` from the title format (L1598, L1653) and put the connection name in the
  panel's actions as a Badge.
- Errors L1280 / L2202 → `Toast::danger` with title `Query failed` / `Couldn't load
  the schema`, raw error as message, Retry action.
- Results-grid cell button (`db-cell-view`, L934, `.small()`) → `.size(ButtonSize::Xs)`
  — it sits in a 32px table row.
- `run` (L1609) stays the one primary; `explain` secondary; both Md in the query header.
- `db_conn_form.rs`: the kind picker stays a `SegmentedControl`; radio marks
  (L654–661) → `Radio`.

### 4.3 Network — `network_tab.rs`
- Page header: `Display "Network"`; the sub-nav `SegmentedControl` (L1367) moves onto
  the header line at Md size (it is the page's primary navigation).
- Panel title (L1336) is the sub-tab label → automatic Heading. `refresh` (L1412) →
  IconButton Md with tooltip `Refresh (F5)` in the panel header.
- `kill` ConfirmButtons (L668) stay `Sm` (32px now) — the table is an instrument.
- Empty states (L1448–1719): already `EmptyState`; rewrite headlines/guidance to
  sentence case with a next step, e.g. `Docker isn't installed` / `Install Docker and
  start the daemon to see containers here.` + `[Open docs]` only if a link action exists.
- Hidden-interfaces disclosure (L1688–1716) → `Tree` group node or a `Switch` labelled
  `Show hidden interfaces`.
- `caveat_line("… needs root")` (L1469) → `InlineNotice` with an `Unlock with sudo`
  action if privfs supports it.

### 4.4 Workspaces — `workspaces_tab.rs`
- Page header: `Display "Workspaces"` + `[+ Add workspace]` primary; remove the panel's
  small `add` (L1425) and refresh (L1415 → IconButton Md in the header).
- List rows (L1665): name first (`Heading`), path second (`MonoMeta`), counts as meta;
  the `no git` badge moves to the row's meta slot, after the name. `Row::compact()` is
  wrong here — keep 44px.
- Detail pane: when a workspace is selected the pane header is `Title` with the name and
  the branch badge; the sub-tab segmented control (Overview/Branches/Status/Log) sits on
  that line.
- Scope items (`Card::section("Scope items")`, L1934): remove the 880px cap (L1935);
  hosts and connections become a two-column card grid at wide widths.
- Repos (fleet, L2355) → `Tree` (repo → branches) if it nests; otherwise keep the table.
- Ad-hoc empties L1924, L2117, L2235, L2304 → `EmptyState::compact()` with sentence-case
  copy ("This workspace has no hosts or connections of its own", "No commits yet",
  "Working tree is clean").
- Log errors rendered as danger text cells → a `Toast::danger` above the log.

### 4.5 System — `systems_tab.rs`
- Page header: `Display "System"` + host facts line in `Meta` under it (user · OS ·
  kernel · uptime · load), replacing the `SYSTEM` StatCluster title (L1360/L1409).
- Meters (`Meter`): the component change makes them 8px tall with 12px radius ends; the
  tab should lay CPU / Memory / Swap as three cards in a row (`Card::new()` each, Heading
  title, big value in `Title`), not one card.
- Sub-nav (L686) → page header line, Md.
- Config files: hand-rolled list rows in `config_list_card` (L1282+) → `Row`; pinned /
  common as two `Card::panel`s side by side at wide widths. Pin input `max_w(420)`
  (L1057) is fine.
- `error_line` (L755, L763, L1042) → `Toast::danger` with titles `Couldn't read
  processes`, `Couldn't stop that process`, `Couldn't pin that file`.
- Ad-hoc "no system overview yet" (L1364) → `Skeleton::rows(3)` while probing.

### 4.6 Settings — `settings_tab.rs`
- **Remove `CONTENT_MAX_W`** (L57/L178): at wide widths lay the section cards in a
  two-column grid (Appearance + Behaviour | Keyboard + Storage) instead of an 880px
  column beside a void.
- Page header: `Display "Settings"`; the rail (`RAIL_W`) keeps its accent marker.
- Appearance: theme rows become selectable cards (swatch strip + name) in a 2x2 grid;
  drop the `· 4` count (`Card::title` with no `.count`).
- **Add a `Reduce motion` `Switch`** under Appearance wired to `motion::set_reduced` and
  persisted (`Settings` store field → entity version bump + migration test, per the
  postcard rule). Add `Interface size` (the existing zoom) as a segmented control beside
  it so "bigger" is discoverable.
- Behaviour: the three SegmentedControls (L541–565) stay; boolean settings (if any land)
  use `Switch`.
- Caveats (L635, L831–833) → `InlineNotice` with sentence-case copy.

### 4.7 App chrome — `app.rs`, `command_palette.rs`, `config_editor.rs`
- **Top bar** (`chrome_tab` L1495, strip L1482–1600): bar height 48; tab labels in
  `Control` role; active tab = `fg_strong` + 2px accent underline with rounded ends
  (exists, keep) + a `selection` pill behind the label on hover. The wordmark gets 16px
  of room.
- **Scope switcher** (`SegmentedControl::new("scope-switcher")` L1524): Md size, with a
  Folder icon on the workspace segment, so the most important state in the app is not a
  12px chip.
- **SSH session tabs** (L1935/L1954): IconButtons are Sm → now 32px automatically; tab
  strip height (`TAB_STRIP_H`, L1551/L1975) should become 40.
- **Status bar**: 32px tall, items in `Meta`, the secrets word gets an icon (ShieldCheck /
  Warning) so it does not read as a stray cyan word.
- **Startup** (`main.rs`): call `sid_ui::motion::init(cx)` explicitly beside
  `theme::install` (which calls it today so `SID_REDUCED_MOTION` already works).
- **Cheat sheet** (L1415–1445): `Modal` instead of the hand-built overlay; `Kbd` rows.
- **Command palette** (`command_palette.rs`): use `Elevation::Floating` + `Radius::Lg`,
  `motion::enter(.., Enter::Rise, FAST)`, result rows as `Row::compact()` with icon +
  label + Kbd; empty → `EmptyState::compact()` ("No matches"). Keep the hand-built
  overlay host (it is not a modal form).
- **Config editor** (`config_editor.rs`): title row `Title`; the root-unlock panel is
  already an EmptyState — rewrite to "This file needs root" / "Unlock with your sudo
  password to view and edit it. The password is used for this file only and never
  saved." + `[Unlock with sudo]` Lg.

### 4.8 Decisions for Murphy
- **D1 — Page headings.** v2 adds a 22px Display heading per screen. It costs ~56px of
  height; it is the single biggest "friendly" lever after sizes. OK to roll out?
- **D2 — SFTP as a tree.** The SFTP sidebar could become an expandable `Tree` (lazy
  directories) instead of navigate-into-folder. Different interaction model; Murphy's
  call.
- **D3 — Reduce motion default.** Motion is on by default with a Settings switch. Default
  off instead?
- **D4 — cosmos-light cards.** v2 makes light-theme cards white on a grey desk (the
  universal light-UI convention) — the inverse of v1. Confirm.
- **D5 — Red primary.** In cosmos the accent (and so every primary button) is red. v2
  keeps that; a friendlier option is a violet/indigo accent for cosmos with red kept for
  danger only. Palette identity question, not done.
