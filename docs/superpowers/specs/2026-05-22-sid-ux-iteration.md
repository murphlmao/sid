# Spec — sid UX iteration (2026-05-22)

> **Status:** draft for review. Sister docs:
> [plan](../plans/2026-05-22-sid-ux-overhaul.md) ·
> [mockups](../mockups/)

## 1. Problem statement

The current TUI **renders** the six tabs but doesn't yet **inhabit** them. Three
discrete failure modes show up in real use:

| Symptom | Concrete | Root cause |
|---|---|---|
| Settings tab is empty | `(no categories)` body | `wire::build_app` calls `SettingsWidget::new()` (the legacy zero-arg ctor) instead of `SettingsWidget::with_categories(...)`. The 7 sub-views Plan 7 shipped never reach the binary. |
| All CRUD lives on the CLI | `sid workspace add`, `sid ssh add`, `sid db add`, `sid system pin`, `sid settings set` | The widgets expose typed mutators (and the TUI tests use them) but the keybindings and modal dialogs that would let a user type them in the running TUI aren't wired. |
| Visual hierarchy is thin | No outer border, no animated background, sparse separators inside tabs | The render layer is correct but minimal: just headers + body + footer, no chrome. |

The user's ask, condensed: **make every CLI feature reachable from inside the
TUI, add a clear bordered shell with an animated cosmic background, and make
the layouts inside each tab visually scan-able instead of "list of words on
near-black."**

## 2. Design principles

These bind the rest of the spec.

1. **CLI/TUI parity.** Every `sid <subcommand>` that mutates state has an
   equivalent in-TUI affordance — a modal, a sub-view, or a wizard. The CLI
   stays for batching and scripting; the TUI is the *primary* surface.
2. **Discoverability without clutter.** Every screen tells you the next two
   actions. We don't add a fourth.
3. **One galaxy, one rhythm.** The animated background is *one* visual layer
   shared across all tabs. Per-tab content is overlaid as if floating in
   front of it. No tab gets its own background.
4. **Configurable, not opinionated.** Density, animation FPS, and supernova
   rate are all settings. Power users running on a 12-year-old laptop should
   be able to set everything to zero and still get the structural design.
5. **Modal-first CRUD.** Add/edit/delete uses a modal overlay, never an
   inline form. Modals dim the background, focus the eye, and read clearly
   over the starfield.
6. **One keystroke promise per screen.** Every tab promises a single
   capital-letter action at the bottom: `N`ew, `E`dit, `D`elete, `?` help.

## 3. Direction options

The user asked for "a couple plans." Below are three directions; my
recommendation is **A** because it matches the user's explicit ask and
preserves the "fast cockpit" ambition. **B** and **C** are foils, useful as
fallbacks if **A** is too ambitious or too noisy.

### A. Galaxy cockpit (recommended)

> The TUI sits in a bordered window. Behind every tab a slow starfield
> twinkles; once in a while a supernova quietly blooms in a corner.
> Each tab has a left-rail picker, a right detail pane, and a footer
> hint strip. Every list has a `N` ew action and `?` opens contextual
> help. Modals dim the background and the supernovae pause while a
> modal is open.

- Pros: Matches the project's stated aesthetic. High polish. Distinctive.
- Cons: Animation overhead (mitigated by FPS clamp + opt-out). More render
  code to test (insta snapshots become motion-sensitive — handled by
  freezing the RNG seed in test mode).

### B. Compact cockpit

> Same layouts as A, no background animation. Borders are thin, single-line,
> dim. No supernovae, no idle motion. The footer keeps the keybind hint
> strip. Per-tab "next-action" capital-letter hints are kept.

- Pros: Lowest CPU cost. Zero risk on slow terminals. Easiest to test.
- Cons: Falls flat against the project's "galaxy aesthetic" stated in
  README/CLAUDE.md.

### C. Power-user dense

> Three-pane layouts everywhere (left picker, middle detail, right "now").
> Bottom always shows a 4-line activity log. Status icons in the title bar
> (DB connections active, kill jobs running, etc.). No animation.

- Pros: Maximises information density. Closer to `btop` / `k9s`.
- Cons: User explicitly said "calm cockpit, btop is beautiful but busy"
  (see `README.md` Design philosophy). Direction C is closer to btop than
  to sid's stated direction.

The rest of this spec assumes **A**.

## 4. Outer shell

```
╔════════════════════════════════════════════════════════════════════════════╗
║  ✦  sid — galaxy cockpit                                    Workspaces  ●  ║
║     ────────────────────────────────────────────────────────────────────   ║
║     Workspaces · SSH · Database · Network · System · Settings              ║
║                                                                            ║
║    ┌─ left rail ────┐  ┌─ detail ──────────────────────────────────────┐   ║
║    │  ● umbrella    │  │                                                │   ║
║    │  ○ sub-repo a  │  │   <tab body — varies per tab>                  │   ║
║    │  ○ sub-repo b  │  │                                                │   ║
║    │  ─────────     │  │                                                │   ║
║    │  ＋ new        │  │                                                │   ║
║    └─────────────────┘ └────────────────────────────────────────────────┘   ║
║                                                                            ║
║    [ Tab cycle ] [ N: new ] [ E: edit ] [ ? help ]   Ctrl+Q quit            ║
║                                                                            ║
║   ·     ·    ✦       ·            ·     ✦         ·        ·       ✦      ║  ← starfield layer
║                          ✦                                                 ║
║      ✦       ·               ·            ✦        ·          ·           ║
╚════════════════════════════════════════════════════════════════════════════╝
```

- **Outer border:** double-line (`╔ ═ ╗`) in `theme.border` with a 1-line
  padding strip on every side, so the inner content has breathing room.
- **Top strip:** `✦  sid — <title of active tab>` left; active-tab marker
  `●` right. Beneath: the six tab names with bullet separators.
- **Body region:** lives inside another single-line border whose title is the
  active tab name. This is the per-tab content area.
- **Footer hint strip:** *up to* four capital-letter action hints + global
  hints (`Ctrl+Q`, `Ctrl+F`, etc).
- **Background:** a starfield layer (see §6) renders behind everything. The
  outer border and any solid-coloured surfaces sit on top of it. Empty cells
  inside the body fall through to the starfield, giving the illusion that
  the tab content floats in space.

The shell is **identical across tabs.** Only the body region changes.

## 5. Per-tab layouts

Each section below has the goal, the layout sketch, and the in-TUI affordances
(keystrokes that don't yet exist).

### 5.1 Workspaces

**Goal:** show the user-created **workspaces** (umbrella containers) and the
repos inside them. Adding a workspace is a one-keystroke modal. Adding a repo
to a selected workspace is a one-keystroke modal.

**Data model clarification.** Today the store holds two kinds of records:
1. Discovered repos (auto-scanned from `~/vcs/`).
2. Explicitly-added workspaces (via `sid workspace add`).

The UI doesn't distinguish them, which is confusing. New layout:

```
┌─ Workspaces ─────────────────────────────┐  ┌─ ~/vcs/sid (selected) ───────┐
│  ● ~/vcs                  (umbrella)     │  │  branch:  main               │
│    ├─ ○ sid               (clean)        │  │  ahead:   8 commits          │
│    ├─ ○ dotfiles          (1↑)           │  │  status:  modified 3, ?      │
│    └─ ○ scratch           (dirty)        │  │                              │
│  ● ~/work                                │  │  recent commits:             │
│    └─ ○ acme-monorepo     (clean)        │  │   3da0503  feat(sysinfo)…    │
│                                          │  │   540e976  test(sysinfo)…    │
│  ─── auto-detected (not yet workspaces) ─│  │                              │
│  · ~/code/old-experiment                 │  │  [ S: status ] [ L: log ]    │
│  · /tmp/cloned                           │  │  [ B: branches ] [ C: commit]│
│                                          │  │  [ D: diff ] [ E: editor ]   │
│  [ N: new workspace ] [ A: add repo ]    │  └──────────────────────────────┘
│  [ R: remove ] [ ? help ]                │
└──────────────────────────────────────────┘
```

**New affordances:**
- `N` — opens "New workspace" modal: name + path picker + kind (Umbrella / Repo).
- `A` — opens "Add repo to selected workspace" modal: filesystem path picker
  with autocomplete; if the chosen path is already an auto-detected repo, it's
  *promoted* into the umbrella.
- `R` — confirm-modal: remove workspace (does NOT delete files).
- `Enter` on an auto-detected repo: same as `A` (one-tap promote).

### 5.2 SSH

**Goal:** host list with detail panel. Easy "add host," "generate key,"
"set up remote auth," and a debug drawer for stale connections.

```
┌─ Hosts ──────────────────────────────┐ ┌─ my-prod-server ─────────────────┐
│  ● my-prod-server                    │ │  host:      prod.example.com     │
│  ○ staging-bastion                   │ │  user:      root                  │
│  ○ github.com                  (cfg) │ │  port:      22                    │
│  ─────  (5 from ~/.ssh/config)  ─────│ │  identity:  ~/.ssh/id_ed25519     │
│  ○ gitlab.com                  (cfg) │ │  state:     connected · 00:14:32  │
│                                      │ │                                   │
│                                      │ │  [ C: connect ] [ F: sftp ]       │
│  [ N: new ] [ G: gen key ]           │ │  [ K: keys ] [ X: debug ]         │
│  [ S: setup remote auth ]            │ │  [ E: edit host ] [ Del: remove ] │
│  [ ? help ]                          │ │                                   │
└──────────────────────────────────────┘ └───────────────────────────────────┘
```

**New affordances:**

- `N` — **Add Host** modal. Fields: alias, host, user, port, identity-file
  picker (with "browse ~/.ssh"), known-good auth methods (key / password /
  agent).
- `G` — **Generate SSH key** wizard. 3 steps:
  1. Algorithm (Ed25519 *default*, RSA-4096, ECDSA-256)
  2. Output path (defaults to `~/.ssh/id_<alg>_<alias>`) + passphrase prompt
  3. "Copy public key to remote?" — if yes, runs `ssh-copy-id` against the
     selected host. Shows the public key for manual copy as a fallback.
- `S` — **Setup remote auth** wizard for an existing host:
  1. Pick an identity (current or generate new)
  2. Confirm: SSH in, append to `~/.ssh/authorized_keys`, verify by
     re-connecting with the key.
  3. On success, persist `identity_file` in the host record.
- `K` — **Key manager** drawer (modal panel from the right):
  - List of every key in `~/.ssh/` with fingerprint, algorithm, comment.
  - For each: which hosts use it, "regenerate" (with safety confirm), "show
    public key" (clipboard), "delete" (with safety confirm).
- `X` — **Debug drawer** (modal panel from the right):
  - "Show known_hosts for this host" — shows the matching lines.
  - "Remove known_hosts entry" — fixes "host key has changed" warnings.
  - "Show identity file diagnostics" — permissions check, key type sanity.
  - "Test connection (verbose)" — runs `ssh -vv` and shows the trace.
  - "Clear cached agent identities" — `ssh-add -D`.
- `F` — opens SFTP session pane (already partly wired). Adds:
  - "Persist this SFTP session" — saves the current path so re-connect lands
    in the same dir. Stored in the host record (`last_sftp_path: Option<String>`).

### 5.3 Database

**Goal:** in-TUI parity with `sid db add/remove/list/query`.

```
┌─ Connections ─────────────┐  ┌─ SQL editor ─────────────────────────────┐
│  ● prod-db    (postgres)  │  │                                          │
│  ○ analytics  (sqlite)    │  │  SELECT id, name FROM users WHERE…       │
│  ○ scratch    (sqlite)    │  │  ▌                                       │
│                           │  └──────────────────────────────────────────┘
│  [ N: new ] [ E: edit ]   │  ┌─ Results · page 1/4 ─────────────────────┐
│  [ D: delete ]            │  │  id   name      created                  │
│  [ T: test ]              │  │  101  alice     2026-01-01               │
│                           │  └──────────────────────────────────────────┘
└───────────────────────────┘  [ Tab cycles right ] [ Ctrl+R run ] [ Ctrl+E csv ]
```

**New affordances:**

- `N` — **Add Connection** modal:
  - Kind: Postgres / SQLite
  - If PG: host, port, user, db name, **password (stored via SecretStore)**
  - If SQLite: file path picker (`:memory:` allowed)
  - "Test connection" button before save.
- `E` — edit selected connection (same form, pre-filled).
- `D` — confirm-delete (and forget the secret).
- `T` — test connection without saving.

### 5.4 Network

Already structurally fine. Improvements:
- Visible borders between the three sub-panels.
- One-line header per sub-panel showing column names.
- Footer adds `K` for the kill-confirm modal (already partly wired).
- `R` for refresh-now (current behaviour is timer-based).

### 5.5 System

**Goal:** in-TUI CRUD for pinned configs and quick-actions.

```
┌─ System ─────────────────────────────────────────────────────────────┐
│  ● Pinned configs · ○ Services · ○ Quick actions    (Tab cycles)     │
│  ──────────────────────────────────────────────────────────────────  │
│  > ~/.zshrc                                                          │
│    ~/.config/nvim/init.lua                                           │
│    /etc/hosts                                                        │
│                                                                      │
│  [ N: new pin ] [ E: edit ] [ D: remove ] [ Enter: open in editor ]  │
└──────────────────────────────────────────────────────────────────────┘
```

- **Pinned configs** pane:
  - `N` — modal: path picker + label (defaults to basename).
  - `E` — modal: edit label.
  - `D` — confirm-modal: remove pin.
  - `Enter` — `TerminalSpawner::spawn(EDITOR, path)`. (Already wired.)
- **Services** pane:
  - `Enter` on a unit: action menu (Start / Stop / Restart / Journal tail).
  - `J` — journal tail modal.
  - `/` — filter.
- **Quick actions** pane:
  - `N` — modal: id, label, command (multi-line), scope (Global / Workspace), keybind chord.
  - `E` — edit.
  - `D` — remove.
  - `Enter` — run the command via `TerminalSpawner`.

### 5.6 Settings

**Goal:** the populated Settings tab the user expected. Fix the wiring bug
*and* add CRUD on every sub-view that's currently read-only-display-only.

```
┌─ Settings ──────────────────────────────────────────────────────────────┐
│ Categories (Tab/Shift-Tab) │ Theme                                      │
│ ─────────────────────────────────────────────────────────────────────── │
│ > ● Theme                  │ > cosmos                                   │
│   ○ Keybinds               │   cosmos-light                             │
│   ○ Behavior               │   solarized-dark                           │
│   ○ Workspace roots        │   ──── live preview ────                   │
│   ○ Quick actions          │   ┌─ sample ─┐                             │
│   ○ Animation              │   │ ✦ hello  │                             │
│   ○ DB path                │   └──────────┘                             │
│   ○ Reset                  │                                            │
│                            │   [ Enter: apply ] [ N: import theme ]     │
└─────────────────────────────────────────────────────────────────────────┘
```

**New sub-views & affordances:**

- **Theme:** apply on Enter (already implemented). Add `N` = import a `.toml`
  theme from a file picker.
- **Keybinds:** show every action with its current chord; Enter to capture
  a new chord; conflict-confirm modal (already in widget state, just needs
  the event wiring).
- **Behavior:** toggle list. `Space` flips a toggle. Includes new toggle
  for `animation.enabled`.
- **Workspace roots:** the directories scanned at startup. `N` to add a root.
- **Quick actions:** same data as System tab's quick-actions pane. CRUD via
  modals.
- **Animation (NEW):** see §6. Sliders for density, FPS, supernova rate; glyph
  set picker; reset-to-defaults button.
- **DB path:** edit override (writes to `sid.toml`).
- **Reset:** confirm-modal then factory reset of selected category.

## 6. Animated background

### 6.1 What renders

Two independent layers, both behind the foreground content:

1. **Starfield.** N stars at random positions, each with a brightness ∈ [0, 1]
   and a twinkle phase. Each frame, brightness drifts via a sine of phase
   plus a bit of noise. Stars never move position once spawned (no parallax).
   Glyphs: `·`, `✦`, `·`, `*` mixed by per-star type.
2. **Supernovae.** Occasional 5-frame animation: a bright cluster of 5-9
   glyphs flashes red→pink→fade at a random screen location. Triggered by:
   - **Idle timer:** every `supernova_rate_secs` (default 90s).
   - **Event:** on commit success, on connection established, on kill
     confirmed. The "celebration" pattern.

### 6.2 Configuration

A new `Animation` sub-view in Settings exposes:

| Setting | Type | Default | Notes |
|---|---|---|---|
| `animation.enabled` | bool | true | master switch |
| `animation.density` | u8 0–100 | 30 | stars per 80×24 |
| `animation.fps` | u8 1–30 | 8 | upper bound on render ticks |
| `animation.supernova_idle_secs` | u32 0–3600 | 90 | 0 disables |
| `animation.supernova_on_event` | bool | true | celebration mode |
| `animation.glyph_set` | enum | `Cosmos` | `Cosmos / Minimal / ASCII` |
| `animation.parallax_factor` | u8 0–10 | 0 | reserved for v2 |

Stored in the `settings` redb table as one postcard-encoded
`AnimationConfig` blob keyed under `setting.animation`. Load on startup;
re-apply when the sub-view writes via the existing `Store::set_setting`
path.

### 6.3 Performance

- Frame budget: at 8 FPS we draw the background once every 125ms. With 30
  stars at 80×24, each frame computes ~30 cells. Worst case at 30 FPS / 200
  stars on a 200×60 terminal: ~6000 cells/s. Well within ratatui's draw budget.
- Event loop: the tokio event pump wakes on either a key event OR a 1/FPS
  tick. The widget render fn re-paints the *entire* frame each tick. To
  avoid burning CPU when the background is paused (modal open), skip the
  tick.
- Test mode: `SID_ANIMATION_SEED=42` env var freezes the RNG so insta
  snapshots stay deterministic. The default starfield rng is `thread_rng`
  but tests pass an explicit `StdRng` via a test-only constructor.

### 6.4 Where it lives in code

- New crate `sid-fx` (small, ~300 LoC) for the starfield + supernova
  renderer. Pure Rust, no terminal coupling — takes a `&mut Buffer` and a
  `&Theme` and writes cells.
- `wire::draw` calls `sid_fx::render_starfield(buffer, theme, &state)` as
  the *first* draw step, before the widget renderers run. The widget
  renderers overwrite cells they need to; empty cells fall through to the
  starfield.
- `SidApp` gains a `fx_state: Option<sid_fx::FxState>` field (None when
  disabled or in non-rendering tests). State is mutable so the run loop
  ticks it.

## 7. Modal/dialog system

A shared `Modal` widget lives in `sid-widgets/src/modal.rs`. Every per-tab
form (Add Host, Add Workspace, Add Connection, Confirm Delete, etc.) is a
`ModalSpec` rendered through it.

```rust
pub struct ModalSpec {
    pub title: String,
    pub fields: Vec<Field>,        // text, password, picker, toggle, choice
    pub primary: ButtonSpec,       // "Save"
    pub secondary: Option<ButtonSpec>, // "Cancel"
    pub help_hint: Option<String>, // "Tab moves between fields"
}
```

- Centered overlay, ~60% width × auto height.
- Background dims by writing a dim layer over the existing buffer.
- Starfield motion *pauses* while a modal is open.
- `Esc` cancels. `Enter` submits.
- The modal stack lives in `App` as `Vec<ModalSpec>` (so we can chain wizards).

## 8. Status bar & toasts

Add a 1-line status bar between body and footer:

```
[ disk: 23.4 GB free · 1 job running · ssh: 1 active · network: refreshing ]
```

Toasts (already partly built for kill outcomes) render in the lower-right
corner with a 3-second fade. Used for: "host added," "key generated,"
"connection failed," etc.

## 9. Tab-level keystroke promise

Each tab footer:

```
[ N: new ] [ E: edit ] [ D: delete ] [ ? help ] · Tab: focus · Ctrl+F: palette · Ctrl+Q: quit
```

Capital letters are the per-tab actions (vary). Lowercase + modifier are
global. `?` opens a tab-specific help modal with a complete chord table.

## 10. Out of scope (for this iteration)

- Multi-pane window managers (Hyprland-style splits). Already deferred to v2.
- Actually rendering vt100 PTY output inside the SSH tab — Plan 3 carved
  this out as future work.
- Light theme polish — cosmos-light works but isn't the primary surface.
- Mouse support — `crossterm` exposes it; not a priority.

## 11. Open questions for the user

1. **Animation intensity.** Default density 30 / FPS 8 is conservative. Are
   you happier with denser (50 stars, 15 FPS) by default? I default low to
   protect older terminals.
2. **Modal vs full-page forms.** I default to modals. If you'd rather have
   "Add Host" replace the entire body region with a form pane, that's a
   simpler implementation but a less polished feel.
3. **Workspace data model.** Should removing a workspace cascade-delete its
   sub-repo records, or just untangle the umbrella relationship and leave
   the repos auto-detected? I default to the latter (no data loss).
4. **SSH key generation.** Should `G` write to `~/.ssh/` directly, or to a
   scratch dir until the user confirms? I default to writing directly with
   `0600` perms and an undo prompt for 5 seconds.
5. **Supernova celebrations.** Should they trigger on every commit, or only
   on commits to non-default branches (less noise on long sessions)? I
   default to "every commit, max 1 per 30s."

A "yes / pick A / pick B" reply to each gets us straight to the plan.
