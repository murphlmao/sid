# UI overhaul plan — `sid-ui` component crate

**Date:** 2026-07-26
**Status:** proposal, awaiting go/no-go
**Trigger (verbatim user verdict):** *"all of the buttons and the user interfaces look and
feel like absolute shit. if we need to make like a separate gpui components crate to get
better uis, please do that. you can also research to see if there are better ui components
we can use here. for example, the systems screen doesnt really look great, nor does the
sftp/ssh page feel like its really intuitive and/or works well at all"*

Evidence: full-tab captures at 2000x1200 (`scripts/sid-cap.sh --size 2000x1200`), all six
primary tabs, cosmos theme, hermetic demo store. Referenced below as `[cap:<tab>]`.

---

## 0. The one-paragraph diagnosis

sid does not have a styling problem. It has a **missing-component-layer** problem with two
compounding root causes. First, there is **no shared widget vocabulary at all**: 113
interactive sites across 17 files, and not one shared button/badge/card/field constructor
between them — every affordance is an ad-hoc `div()` style chain retyped inline, so quality
is whatever each call site remembered to type, and the floor is "text with a hover fill."
Second, sid pays for `gpui-component` 0.5.1 in its dependency tree but **uses 3 of its ~50
widget modules and bridges its own 13-token galaxy palette into it through a single bit**
(`ThemeMode::Light | Dark`) — so the few library widgets that are used render in the
library's stock shadcn colors, foreign to everything around them. The dead-black right half
of the System and Network tables is a third, purely mechanical bug: **every one of the 30
declared table columns is a hard-coded pixel width**, and `gpui-component` 0.5.1's `Table`
has no fill/flex column concept whatsoever.

---

## 1. Per-screen critique, worst first

### 1.1 System — worst screen in the app `[cap:ui-system.png]`

The screenshot is a catalogue of every failure mode at once.

- **67% of the table is dead black space.** `systems_tab.rs:211-216` declares
  `CPU% 70 + Mem 90 + PID 80 + Name 220 + User 120 + kill 72 = 652px` of fixed columns in a
  2000px window. Nothing fills the remainder, so the process table is a narrow ribbon
  pinned to the left edge of a black field. The `Name` column is simultaneously *too
  narrow* — "Isolated Web Co" is truncated mid-word at 220px while 1348px of nothing sits
  to its right. This is the single most damaging visual defect in sid.
- **The `kill` action is bare accent text floating in the void.** `systems_tab.rs:376-390`
  renders it as a `div()` with `text_color` + `hover(bg)` and no border, no fill, no
  padding box. It reads as a stray word, not a destructive action — and it is a
  *destructive* action rendered with less affordance than a hyperlink. It also sits in a
  72px column at x≈620, i.e. adjacent to nothing, anchored to nothing.
- **The filter is a hairline rectangle; `refresh` is an unstyled red word.** No label, no
  icon, no search affordance, no button chrome (`systems_tab.rs:468-483` — eight chained
  style calls typed inline for what should be `Button::new("refresh")`).
- **CPU/Memory/Swap meters have no container.** Three thin unframed bars with labels
  floating on the window background at the very top of the canvas, separated from
  everything by nothing but whitespace. There is no card, no section header, no border —
  they read as debug output, not as an instrument cluster. The per-core mini-bar strip
  underneath is a row of ~20 tiny red ticks with no axis, no label, and no explanation of
  what it is.
- **The `COMMON` config section is structurally orphaned.** It is a centered 880px reading
  column (`max_w(880px)` per the design system) placed directly beneath a **full-width**
  table. The result: a block of content that starts at x≈560 with no left-edge alignment to
  anything above it, appearing to float in the middle of the page. Its `pin` actions are
  red text at the far right of that invisible 880px boundary — ~800px from the filename
  they act on, with no row fill, no rule, and no visual tie to their row. The design
  system's own rule ("a label's action never lives a screen-width away") is violated by its
  own layout rule.
- **No hierarchy.** Chrome bar, meter strip, filter row, table, and config list all sit on
  the same `bg` value with hairline `border` separators. Nothing is raised, nothing is
  recessed, so the eye has no reading order.

### 1.2 SSH / SFTP home — the "doesn't feel intuitive" screen `[cap:ui-ssh.png]`

- **A 2000px window renders ~880px of content and 1120px of nothing**, split evenly as
  dead margins left and right. Two saved connections occupy the top 200px; the remaining
  1000px of vertical space is empty black. The screen communicates "this app has nothing in
  it."
- **Every row action is bare text or a naked glyph**, right-aligned in a cluster:
  `global`  `»`  `✎`  `folder`  `×`. Five different affordance styles in 150px, none of
  them a button, two of them undecodable Unicode. `»` and `✎` are not labeled and not
  tooltipped; `folder` is a verb rendered as a noun; `×` is 11px of clickable area for a
  delete.
- **The quick-connect row is a broad input with a red square glyph button** (`↵`) at its
  right. A red filled square is sid's most emphatic affordance and it is spent on "submit
  this text field," while the actual primary action of the entire tab (connect to a host)
  has no button at all — you double-click a row, or right-click, and the screen tells you so
  in 12px muted prose: *"saved connections below · double-click a name to rename ·
  right-click for more."* **The tab's primary verb is documented in a hint string instead
  of being a control.** That is the whole of the user's "doesn't feel intuitive."
- **Connection state is a 6px hollow circle** with no legend. Nothing distinguishes
  disconnected from unknown.
- **`+ Add connection`** is the only real button on the screen and it is the *least*
  important action on it.
- SFTP itself is unreachable from this screen — no browse affordance, no split hint. The
  tab is named "SSH / SFTP" and exposes one of the two.

### 1.3 Network `[cap:ui-network.png]`

- **Same fixed-width table disease, worse**: `64+72+120+80+240+72 = 648px` of 2000
  (`network_tab.rs:374-380`), so 68% dead space. The `Addr` column truncates
  `fd7a:115c:a1e0::1a0…` at 120px — an IPv6 address, unreadable — with 1350px of unused
  width to its right. Truncating data while wasting two-thirds of the viewport is the
  clearest possible statement that no one is minding the layout.
- 30 of 37 rows show `—  —  —` for PID/Process/action. The table is mostly em-dashes;
  the sub-view has no empty-ish state or column-collapse for that case.
- `refresh` is once again unstyled red text. The `Ports/Services/Interfaces/Docker/
  Kubernetes` segmented control is, ironically, the **best-looking control in the app** —
  bordered, padded, with an active fill. It proves the codebase *can* do this and simply
  does not, anywhere else.
- Four separate `TableDelegate` impls in this one file (ports/services/docker/kube),
  each with its own hand-written header, cell, and sort boilerplate.

### 1.4 Workspaces `[cap:ui-workspaces.png]`

- A 300px sidebar and then **1700px of pure black** with the words `select a workspace`
  centered in it. The empty state is technically per-spec ("muted text, without a box") but
  at this window size it reads as a broken render.
- The one workspace row truncates its own path mid-string (`/tmp/sid-cap.lr7gnG/xdg/data/
  sid/demo-…`) inside a 300px sidebar while the entire canvas is free.
- `not a git repo` and `3 hosts · 0 connections` are the same muted text weight as
  everything else — status is indistinguishable from metadata.
- `+ add` is a real button; the refresh next to it is a bare `⟳` glyph. Two adjacent
  actions, two unrelated visual languages.

### 1.5 Database `[cap:ui-database.png]`

Not separately critiqued in depth — it is the tab closest to acceptable because
`gpui-component`'s `Input` (SQL editor) and `Table` (results) carry it. But that is also
the tab where the **theme mismatch** is most obvious: library widgets in stock shadcn
grays sitting inside cosmos chrome.

### 1.6 Settings `[cap:ui-settings.png]`

The strongest screen — theme rows are real cards with borders, the scope/side/keyring
toggles are real segmented controls. Two defects:

- **Accent is spent on inert data.** Every keybinding value (`Ctrl+K`, `Ctrl+1`, …, 16 of
  them) is rendered in accent red. The design system says accent means "engage"; here it
  marks a static reference table, so the eye is dragged to the least actionable content on
  the screen, and 16 red items train the user to ignore red. These should be `Kbd`-style
  muted chips.
- Same centered-880px-in-2000px emptiness as SSH home.

---

## 2. Component inventory and duplication findings

### 2.1 What widget patterns exist today

**Shared, reusable, cross-tab: essentially none.** The complete set of shared UI primitives
in `crates/sid/src/ui/`:

| Primitive | Where | Reusable? |
|---|---|---|
| `TextInput` | `ui/text_input.rs`, **946 lines** | yes — the only real shared widget |
| `theme::Theme` tokens | `ui/theme.rs`, 306 lines | yes (13 tokens + 16 ANSI) |
| everything else | inline `div()` chains | **no** |

The ten free helper functions that look like components are all **file-private to one tab**:

```
ui/settings_tab.rs:96   section_header()      ui/systems_tab.rs:1022  cpu_card()
ui/settings_tab.rs:122  keyboard_section()    ui/systems_tab.rs:1058  mem_card()
ui/settings_tab.rs:166  storage_section()     ui/systems_tab.rs:1087  horizontal_bar()
ui/session.rs:1629      message_pane()        ui/systems_tab.rs:1099  vertical_core_bar()
ui/session.rs:1642      status_line()         ui/network_tab.rs:2376  graceful_absence_notice()
```

`section_header` in `settings_tab.rs` is the design system's mandated section-header
pattern — and the System, Network, SSH, and Workspaces tabs each reimplement it inline
rather than importing it, because it is a private fn in another module.

**There is no `Button` anywhere in the codebase.** Not a shared one, not a private one.

### 2.2 Quantified duplication

| Metric | Count | Note |
|---|---|---|
| Interactive sites (`.on_click` / `on_mouse_down`) | **113** | across 17 files |
| Shared button/affordance constructors | **0** | every one is inline |
| `rounded_md()` literal repeats | **74** | the row/chip corner spec, retyped |
| `px_3()` literal repeats | **41** | the design system's row padding, retyped |
| `hover(...)` handlers hand-written | **41** | across 9 files |
| `TableDelegate` impls | **7** | 4 in `network_tab.rs` alone |
| `Column::new` declarations | **30** | **100% fixed `px()` — zero flexible** |
| Ad-hoc Unicode glyphs used as icons | **~130 occurrences, 18 distinct** | `⟳`×19, `→`×46, `▸`×9, `↑`×9, `▾`×8, `✎`×7, `●`×7, `✗`×6, `○`×4, `×`×4, `✦`×3, `»`/`«`×3, `✓`×2 |
| Icons used from `gpui-component-assets` (99 available, already a declared dep) | **0** | |

The canonical example, `systems_tab.rs:469-483` — a refresh button, hand-built, eight
chained style calls, duplicated in spirit in `network_tab.rs` and `workspaces_tab.rs`:

```rust
div().id("systems-refresh").px_2().py_1().rounded_md().text_sm()
    .cursor_pointer().text_color(rgb(theme.accent))
    .hover(|s| s.bg(rgb(theme.selection)))
    .child(refresh_label)
    .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| { this.refresh_systems(cx); }))
```

Note what it does *not* have: a border, a surface fill, a focus ring, a disabled state, an
active/pressed state, a tooltip, or a keyboard affordance. That is the ceiling of every
affordance in sid today.

### 2.3 `gpui-component` — bought, barely used

Pinned: `gpui = "0.2"` → **0.2.2**; `gpui-component = "0.5.1"` + `gpui-component-assets =
"0.5.1"` (Apache-2.0). `gpui-component` 0.5.1 exports **~50 widget modules**:

```
accordion alert animation avatar badge breadcrumb button chart checkbox clipboard
collapsible color_picker description_list dialog divider dock form group_box highlighter
history input kbd label link list menu notification plot popover progress radio resizable
scroll select setting sheet sidebar skeleton slider spinner switch tab table tag text
theme tooltip tree calendar date_picker + VirtualList, Root, StyledExt, IconName, TitleBar
```

**sid imports exactly three of them**, plus theming/root plumbing:

| Import | Files |
|---|---|
| `table::{Column, ColumnSort, Table, TableDelegate, TableState}` | 4 (7 delegates) |
| `input::{Input, InputEvent, InputState, Position}` | 2 (`config_editor`, `db_tab` SQL) |
| `menu::{ContextMenuExt, PopupMenu, PopupMenuItem}` | 2 |
| `Root`, `Theme`, `ThemeMode` | `main.rs`, `db_tab.rs`, `settings_tab.rs`, `theme.rs` |

Bypassed while sitting in the lockfile: `button`, `badge`, `tag`, `kbd`, `label`, `tooltip`,
`dialog`, `sheet`, `notification`, `switch`, `checkbox`, `radio`, `select`, `progress`,
`group_box`, `sidebar`, `tab`, `skeleton`, `divider`, `breadcrumb`, `form`, `resizable`,
`link`, `spinner`, `description_list`, `tree`.

**Three concrete consequences:**

1. **`ui/text_input.rs` is 946 lines reimplementing `gpui_component::input::Input`** — a
   widget already compiled into the binary and already used in two other files. Grapheme
   segmentation, word motion, masking, clipboard, and 24 keybindings, all hand-maintained,
   all duplicated.
2. **The theme bridge is one bit.** The *only* thing sid tells `gpui-component` about its
   palette is `Light` vs `Dark` (`ui/theme.rs:204`, `component_mode()` → three
   `Theme::change(mode, ..)` call sites). `gpui-component` 0.5.1 exposes a `ThemeColor`
   struct with **150+ named semantic tokens** and a `Theme::global_mut(cx).apply_config(..)`
   entry point that accepts a plain constructible Rust struct. sid uses none of it. Every
   library widget therefore renders in stock shadcn dark, not cosmos — which is precisely
   why the process-table header reads as a washed-out gray foreign object in
   `[cap:ui-system.png]`. **This is a one-file fix with app-wide effect.**
3. **`gpui-component-assets` is wired and unused.** `main.rs:91` calls
   `.with_assets(gpui_component_assets::Assets)` — 99 Lucide monochrome SVG icons, zero
   emoji, exactly the "monochrome glyphs only" house rule — and not one is referenced.
   Meanwhile 18 distinct ad-hoc Unicode glyphs stand in for icons across the UI.

### 2.4 The fill-width problem is partly upstream

Verified in the vendored crate source: `gpui-component-0.5.1/src/table/column.rs:28` —
`pub width: Pixels`. There is **no** `Length`/`DefiniteLength`/flex/grow option, and no
fill logic in `table/mod.rs`. `TableDelegate::render_last_empty_col` (delegate.rs:157)
only lets you *paint* the leftover strip, not distribute it.

So fill-width cannot be had by configuration. **But it is cheap to implement:**
`Column.width` is a public mutable field, the delegate owns its `Vec<Column>` and hands
them out via `fn column(&self, ix) -> &Column`, `resize_cols` already mutates widths at
runtime (state.rs:469-495), and `visible_columns_changed` exists as a hook. A wrapper in
`sid-ui` that stores per-column `Fixed(px) | Min(px) | Grow(weight)` intent and
redistributes the delegate's `Column.width` values against the measured viewport width
solves it for all 7 tables at once. **~120 lines, one place.**

---

## 3. Ecosystem survey (mid-2026)

### 3.1 `longbridge/gpui-component` — Apache-2.0

- **Latest crates.io: 0.5.1 (2026-02-05)**, which pins `gpui = "0.2.2"` from crates.io.
  This is the last self-contained, registry-only combination — **exactly what sid pins.**
- `main` has since moved to git `gpui` + the new `gpui_platform` split (upstream Zed change
  ~2026-02-22; see gpui-component issue #2064 / PR #2066, where the split silently left
  Linux with no rendering backend). The unreleased 0.5.2 on `main` requires git gpui.
- **Implication for sid: stay on 0.5.1.** It is a stable, frozen, fully-registry pin with
  no upgrade pressure. Adopting more of its widgets adds **zero new dependencies** and zero
  version risk, because the crate is already compiled in. Deferring the git-`gpui`
  migration is a separate future decision and is *not* coupled to this work.
- Maturity: 12.2k stars, 731 forks, 112 open issues, last push 2026-07-24 (active).
  Production use: Longbridge Pro; also DBFlux, pgui, zedis, GitComet.
- Theming: global `Theme` + `ActiveTheme` trait (`cx.theme()`), backed by `ThemeColor`
  (150+ fields, semantic groups `primary`/`secondary`/`danger`/`success`/`warning` each
  with `_hover`/`_active`/`_foreground`, plus scoped `button_*`/`list_*`/`table_*`/
  `sidebar_*`). Runtime custom palette via `apply_config(ThemeConfig)`; JSON themes and
  live-reload also supported but unnecessary here. **Every widget reads through
  `cx.theme()`, so replacing the active `ThemeColor` re-skins all of them — sid's cosmos
  tokens can drive the whole library.** This is the intended path, not a hack.
- Assets: icons live in the separate `gpui-component-assets` crate (Lucide monochrome SVG,
  `rust-embed`). `IconName` is generated from that crate at build time; glyphs are
  swappable by supplying a custom `AssetSource` at the same paths. **No emoji in any
  default component** — verified clean against the house rule.
- Footprint: **no default features**. sid currently opts into `tree-sitter-languages`,
  which is 33 grammars — a real compile-time cost sid already chose and could trim to the
  handful of languages the SQL editor actually needs. `markdown` + `html5ever` are
  unconditional. WebView is a wholly separate crate (`gpui-wry`) and is not pulled in.
- Integration: `gpui_component::init(cx)` must run before use, and each window's root must
  be `Root::new(..)`. **sid already does both** (`main.rs:91-153`, `db_tab.rs:1198-1200`).
  Mixing library widgets with hand-rolled `gpui` elements is safe and normal — `Theme` is a
  passive global that widgets choose to read; hand-rolled elements are unaffected.

### 3.2 Zed's own `crates/ui` — GPL-3.0-or-later, **unpublished**

Not usable: `publish = false`, and it depends on the unpublished GPL crates `theme`,
`component`, `menu`, `icons`, `ui_macros`, `gpui_util`. Patterns worth cribbing (and only
patterns — consistent with CLAUDE.md's "crib view logic, don't carry over scaffolding"):

- `ButtonLike` / `ButtonCommon` layering — one styled shell (`id`, `style`, `size`,
  `disabled`, `selected`, `tooltip`, `focus_handle`, `on_click`, `on_right_click`) that
  `Button`/`IconButton`/`ToggleButton` all decorate. `ButtonStyle` variants: `Filled`,
  `Tinted`, `Outlined`, `OutlinedGhost`, `Subtle`, `Transparent`. **This is the exact shape
  `sid-ui::Button` should take.**
- Small orthogonal traits: `Clickable`, `Disableable`, `Toggleable` (+ a `ToggleState`
  enum `Unselected`/`Indeterminate`/`Selected`).
- `StyledExt` extension trait with `h_flex()`/`v_flex()` and spacing shortcuts.
- `ElevationIndex`: a 5-level surface ladder (`Background` → `Surface` → `EditorSurface` →
  `ElevatedSurface` → `ModalSurface`). sid's `bg`/`surface`/`well` tokens are already this
  idea; naming the ladder explicitly is what the System tab is missing.
- A `Component` trait + `inventory`-based auto-registered preview gallery. Worth copying
  the *idea* (a `sid-ui` gallery screen) later; not worth the `inventory` dep now.

### 3.3 Other GPUI component ecosystems

| Project | State | Verdict |
|---|---|---|
| `fluix` (lipish) | gpui 0.2, ~46 components, v0.1.x, "APIs may change" | too immature |
| `adabraka-ui` (Augani) | 85+ components, requires nightly, ships `adabraka-gpui` — **a fork of gpui itself** | disqualifying: forked framework + nightly |
| `declarative-gpui` | `ui!` macro, Tailwind-ish tokens | different paradigm, not a widget set |
| `guise` | Mantine-inspired, tokens + components | small, unproven |
| `Base GPUI` | headless Base-UI-style APIs | interesting, immature |
| `gpui-storybook` | preview harness | possible later nicety |
| narrow libs | `gpui-form`, `gpui-symbols`, `gpui-router`, `plotters-gpui`, … | single-purpose, not needed |

Discovery index: `zed-industries/awesome-gpui`.

**Conclusion: `gpui-component` 0.5.1 is the only credible library, and sid already has it.**
There is no "better component library" to switch to. The problem was never library choice.

---

## 4. Options analysis

### Option A — adopt `gpui-component` widgets broadly, no new crate

Replace inline `div()` chains with `Button`, `Badge`, `Tooltip`, `Switch`, `Kbd` etc.
directly in each tab module; drive the whole library from cosmos via one `ThemeColor` map.

- **Pro:** no new crate; zero new deps; fastest path to real affordances; kills the
  946-line `text_input.rs`; instantly fixes theme coherence.
- **Con:** ~50 library types named directly across 9 tab files — sid's design language
  becomes "whatever `gpui-component` defaults to," and per-tab call sites drift again.
  Does **not** fix fill-width tables (upstream can't). No home for sid-specific
  compounds (`MeterCard`, `EmptyState`, `ScopeChip`, the `Fixed|Min|Grow` column model).
  Every future library-version change touches nine files.

### Option B — bespoke `sid-ui` crate, hand-rolled from raw `gpui`

Build Button/Input/Table/etc. from scratch against sid tokens; drop `gpui-component`.

- **Pro:** total control; one design language; no upstream coupling.
- **Con:** hand-rolling a virtualized sortable `Table`, a full text `Input`, popovers,
  modals, and focus management is **months** of work and is exactly the tar pit
  `text_input.rs` (946 lines, one widget) already demonstrates. Violates
  minimal-footprint by re-solving a solved problem. Throws away a 12k-star Apache-2.0
  dependency already in the tree.

### Option C — hybrid: thin `sid-ui` crate that skins/wraps `gpui-component` where it's good, hand-rolls where it isn't  **[RECOMMENDED]**

A new **frontend** crate `crates/sid-ui`:

- Owns the semantic tokens (`theme.rs` moves here) and owns the **single** cosmos →
  `gpui_component::ThemeColor` mapping, so the whole library renders in cosmos/void/dusk/
  cosmos-light.
- Re-exports thin sid-flavoured wrappers over library widgets where the library is good
  (Button, IconButton, Input, Badge/Tag, Kbd, Tooltip, Switch, Checkbox, Select, Dialog/
  Sheet, Notification, Divider, Skeleton, Sidebar, Tab).
- Hand-rolls the small handful the library lacks or gets wrong: the **fill-width column
  model** over `Table`, `Card`/`Section`, `Meter`, `StatCluster`, `EmptyState`,
  `StatusBar`, `ScopeChip`, `ActionCell`.
- Is the **only** crate besides `sid` that may name `gpui`/`gpui-component`; tab modules
  then import from `sid_ui` and stop naming the library at all.

- **Pro:** one place defines "what a button looks like in sid"; one place bridges the
  theme; one place absorbs the eventual git-`gpui`/0.5.2 migration; sid-specific compounds
  get a home; footprint unchanged (zero new third-party deps). Aligns exactly with the
  adapter-pattern rule sid already applies to every OS/library seam — this is that rule,
  applied to the rendering surface.
- **Con:** one more crate; a wrapper layer to keep honest (mitigation: wrappers stay thin,
  and re-export the library type when no sid opinion is needed rather than mirroring its
  whole API).

---

## 5. Recommendation

**Adopt Option C.** Create `crates/sid-ui` as a frontend crate.

Rationale:

1. **The diagnosis says component layer, not library.** 113 affordances, 0 shared
   constructors, 74 retyped `rounded_md()`. Any option that leaves styling decisions at
   the call site regresses within a month. Only a crate boundary makes "buttons look like
   this" enforceable.
2. **The single highest-leverage fix in the entire audit is one function.** Mapping cosmos's
   13 tokens onto `ThemeColor`'s 150+ fields re-skins every `Table`, `Input`, and
   `PopupMenu` already on screen, and pre-skins the ~26 widgets not yet adopted. That
   function needs an owner; `sid-ui` is it.
3. **It is the only option that can fix fill-width tables.** Upstream `Column` is
   `Pixels`-only; the `Fixed|Min|Grow` redistribution wrapper (~120 lines) fixes System,
   Network x4, Workspaces, and DB results simultaneously. That single change reclaims 67%
   of the System tab and 68% of the Network tab.
4. **It is the minimal-footprint answer.** Zero new third-party dependencies. It uses more
   of a dependency sid already ships and deletes ~950 lines of hand-rolled input code.
5. **It matches CLAUDE.md verbatim.** GPUI is named only in frontend crates; a `sid-ui`
   frontend crate is explicitly allowed. It also gives the eventual `gpui-component` 0.5.2
   / git-`gpui` migration exactly one blast radius instead of nine files.
6. **The galaxy aesthetic is preserved, not replaced.** cosmos/void/dusk/cosmos-light stay
   the source of truth; the library is skinned *to* them. `.interface-design/system.md`
   stays the design law — `sid-ui` is its enforcement mechanism, with two amendments
   (below).

### Two amendments to `.interface-design/system.md`

- **The `max_w(880px)` centered-column rule needs a wide-window clause.** At 2000px it
  produces 56% dead margin (SSH home, Settings) and, when mixed with a full-width table on
  the same screen, structurally orphans the column (System's `COMMON` block). Proposal:
  reading columns cap at 880px but **left-align to the content gutter** rather than
  centering when the screen also contains full-width content; and wide screens get a
  two-column reading layout instead of one narrow ribbon.
- **"Borders + surface shifts only, no shadows" is under-specifying depth.** Keep the
  no-shadow rule, but name the ladder explicitly (Zed's `ElevationIndex` idea) and require
  data/meter clusters to sit on a `surface` card with a section header — the System tab's
  unframed meters are what "hairlines only" degrades into.

---

## 6. Component checklist for `sid-ui`

`W` = thin wrapper over `gpui-component`; `H` = hand-rolled over raw `gpui`;
`R` = re-export as-is.

### Foundation
- [ ] `tokens` — `theme.rs` moved in; cosmos/void/dusk/cosmos-light; `Theme` global
- [ ] `theme_bridge` — **cosmos → `gpui_component::ThemeColor` (the keystone)**; one fn,
      round-tripped in tests for all four palettes
- [ ] `Elevation` — `Bg | Surface | Well | Overlay` ladder mapped to tokens `H`
- [ ] `StyledExt` — `h_flex()`/`v_flex()`, `row_padding()`, `hairline()`, `hover_fill()` `H`
- [ ] `Icon` — named registry over the 99 bundled Lucide monochrome SVGs; **retires the 18
      ad-hoc Unicode glyphs**; no emoji, enforced by a unit test `W`

### Controls
- [ ] `Button` — `Primary | Secondary | Ghost | Danger` x `Sm | Md`; disabled, loading,
      pressed, focus ring, optional leading icon `W`
- [ ] `IconButton` — square, tooltip **required** by the type (kills naked `»` / `✎` / `⟳`) `W`
- [ ] `SegmentedControl` — promote the Network sub-view control (best control in the app) `W`
- [ ] `TextInput` / `SearchInput` — over `gpui_component::input::Input`; leading search
      icon, placeholder, clear affordance; **deletes `ui/text_input.rs` (946 lines)** `W`
- [ ] `Switch`, `Checkbox`, `Radio` `R`
- [ ] `Select` / `Dropdown` `W`
- [ ] `Kbd` — for the 16 keybinding values now painted accent-red `W`

### Data
- [ ] `Table` — wrapper adding **`ColumnWidth::{Fixed, Min, Grow}` + viewport
      redistribution**; sort indicators; stripe; sticky header; `ActionCell` slot `H+W`
- [ ] `ActionCell` — right-anchored, real `Button`s, destructive variant with two-step
      confirm (replaces bare `kill` text) `H`
- [ ] `List` / `Row` — the `px_3 py_2 rounded_md` + hover-fill + context-menu row,
      once (replaces 41 hand-written `hover()` sites) `H`
- [ ] `Tree` — SSH host tree, DB schema tree, workspace tree `W`

### Structure
- [ ] `Card` / `Section` — `surface` fill, hairline border, `text_xs` uppercase muted
      header + optional count and header actions (replaces 4 inline reimplementations of
      `settings_tab::section_header`) `H`
- [ ] `Meter` / `StatCluster` — labeled bar + value + framing; the System overview `H`
- [ ] `Badge` / `Pill` / `ScopeChip` — origin (`global`/`workspace`), counts, states `W+H`
- [ ] `StatusDot` — connection state with a legend, replacing the unlabeled 6px circle `H`
- [ ] `EmptyState` — headline + one-line "what to do next" + **a primary action button**
      (Workspaces' `select a workspace`, SSH's zero-host case) `H`
- [ ] `Toolbar` — the filter + refresh + count row, once (System / Network / Workspaces) `H`
- [ ] `StatusBar` `W`

### Overlays
- [ ] `Modal` / `Dialog` — scrim, Esc-closes, refocuses `root_focus` (host form, DB conn
      form, password prompt, config editor) `W`
- [ ] `Tooltip` — required by `IconButton` `R`
- [ ] `ContextMenu` `R`
- [ ] `Toast` / `Notification` — replaces inline `✗ {e}` error text `W`

### Hygiene gates
- [ ] no-emoji test over `sid-ui` + `sid` string literals
- [ ] no-raw-hex test (only the modal scrim exempt, per the design system)
- [ ] `Tooltip`-or-label required on every icon-only control (type-enforced)
- [ ] a `sid-ui` gallery screen (dev-only) rendering every component in all four themes,
      captured by `sid-cap.sh` — the observation gate for rendering work

---

## 7. Migration order, sized in gate-green commits

Each step is independently shippable and observation-gated by a `sid-cap.sh` capture in all
four themes. Critical-path logic (store/scope/secrets) is untouched throughout — this is a
view-layer change only.

**Phase 0 — foundation (2 commits)**

1. `feat(sid-ui): crate skeleton + tokens + theme bridge`
   Create `crates/sid-ui`; move `ui/theme.rs` in; add the cosmos → `ThemeColor` mapping and
   call it from the three existing `Theme::change` sites; add `StyledExt`, `Elevation`,
   `Icon`. Tests: all four palettes round-trip; no-emoji; no-raw-hex.
   *Visible outcome on its own: every `gpui-component` widget already on screen (System's
   process table, Network's four tables, the DB SQL editor and results grid, all popup
   menus) stops rendering in shadcn gray and starts rendering in cosmos.* Highest
   value-per-line commit in the plan.
2. `feat(sid-ui): Button, IconButton, Badge, Kbd, Card/Section, Toolbar, EmptyState`
   Plus the dev-only gallery screen. No tab touched yet; gate = gallery capture.

**Phase 1 — System tab (3 commits) — worst screen first**

3. `feat(sid-ui): Table fill-width column model (Fixed | Min | Grow)`
   Wrapper + redistribution; migrate `ProcessesDelegate` as the first consumer.
   *Gate: `[cap:ui-system.png]` at 2000px shows zero dead space and an untruncated `Name`.*
4. `refactor(sid): System tab onto sid-ui`
   Meters into a `StatCluster` card; filter+refresh into `Toolbar` with a real `Button`;
   `kill` into `ActionCell` (destructive, two-step); `COMMON` into a `Card`/`Section` with
   `pin` as row-anchored `IconButton`s; fix the centered-880-under-full-width-table
   orphaning.
5. `chore(sid): retire ad-hoc glyphs on System; adopt bundled icons`

**Phase 2 — SSH/SFTP home (3 commits) — the "not intuitive" screen**

6. `refactor(sid): SSH home rows onto sid-ui List/Row + ActionCell`
   Five mixed affordances (`global` `»` `✎` `folder` `×`) become a `ScopeChip` + labeled,
   tooltipped `IconButton`s. `StatusDot` with a legend.
7. `feat(sid): SSH home primary action — a real Connect button`
   **Promote connect out of the hint string into a control** (per-row primary + quick-connect
   primary). Retire the "double-click to rename · right-click for more" prose. Surface an
   explicit SFTP/browse affordance so the tab's name matches its contents.
8. `refactor(sid): SSH home layout at wide widths`
   Apply the amended reading-column rule; real `EmptyState` with a primary action.
   *Gate: `[cap:ui-ssh.png]` at 2000px — primary action visible without prose, no 1120px void.*

**Phase 3 — remaining tabs (4 commits)**

9. `refactor(sid): Network onto sid-ui` — 4 delegates onto the fill-width model; `Toolbar`;
   `ActionCell`; keep the segmented control, now from `sid-ui`.
10. `refactor(sid): Workspaces onto sid-ui` — fleet table fill-width; `EmptyState` with
    action; `Badge` for git status; sidebar row via `List/Row`.
11. `refactor(sid): Database + Settings onto sid-ui` — results table fill-width; keybindings
    from accent-red text to `Kbd` chips; theme rows and toggles re-pointed at `sid-ui`.
12. `refactor(sid): modals + toasts onto sid-ui` — host form, DB conn form, password prompt,
    config editor onto `Modal`; inline `✗ {e}` onto `Toast`.

**Phase 4 — deletion and gate (2 commits)**

13. `refactor(sid): delete ui/text_input.rs; SearchInput/TextInput from sid-ui`
    ~950 lines removed; the 24 hand-registered keybindings go with it.
14. `chore(sid): UI overhaul gate` — full six-tab capture set in all four themes; hygiene
    tests green; update `.interface-design/system.md` with the two amendments; audit that no
    tab module names `gpui_component` any more.

**Sizing note.** Commits 1 and 3 are the disproportionate wins: commit 1 fixes theme
coherence app-wide, commit 3 reclaims two-thirds of the two densest screens. If the plan
must be cut short, ship 1–4 and stop — that alone answers "the systems screen doesn't look
great."

---

## Appendix — evidence

Captures (`scripts/sid-cap.sh --tab <t> --size 2000x1200`, cosmos, hermetic store):

```
scratchpad/ui-ssh.png         SSH / SFTP home
scratchpad/ui-database.png    Database
scratchpad/ui-network.png     Network (Ports sub-view)
scratchpad/ui-workspaces.png  Workspaces
scratchpad/ui-system.png      System
scratchpad/ui-settings.png    Settings
```

Key source references:

```
crates/sid/src/ui/theme.rs:204            component_mode() — the one-bit theme bridge
crates/sid/src/main.rs:91                 with_assets(gpui_component_assets::Assets) — 0 icons used
crates/sid/src/ui/systems_tab.rs:211-216  652px of fixed columns in a 2000px window
crates/sid/src/ui/systems_tab.rs:376-390  the bare-text `kill` action
crates/sid/src/ui/systems_tab.rs:469-483  the hand-built refresh "button"
crates/sid/src/ui/network_tab.rs:374-380  648px of fixed columns; Addr truncated at 120px
crates/sid/src/ui/text_input.rs           946 lines duplicating gpui_component::input::Input
crates/sid/src/ui/settings_tab.rs:96      section_header() — private, reimplemented in 4 tabs
<cargo>/gpui-component-0.5.1/src/table/column.rs:28   pub width: Pixels — no flex, upstream
```
