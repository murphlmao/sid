# Plan — sid UX overhaul (2026-05-22)

> Companion to [`specs/2026-05-22-sid-ux-iteration.md`](../specs/2026-05-22-sid-ux-iteration.md). Read the spec first.

Six phases. Each phase ships independently and leaves the TUI in a usable
state. Phases are ordered by **user-visible payoff per hour of work** —
Phase 1 is the most impactful (fixes the empty Settings page and adds the
outer shell); Phase 6 is the most polish-y (animated background).

If you're tight on time and want to ship the maximum perceived improvement
in one sitting, **Phase 1 + Phase 4 + Phase 6.1** gives you: populated
Settings, in-TUI workspace creation, and the starfield. That's roughly two
days of focused work.

## Phase 1 — Outer shell, footer, Settings wiring (CRITICAL FIX)

**Why first:** The empty Settings page is a bug, not a feature gap. Plan 7
shipped every Settings sub-view; the binary just doesn't construct them.
Same root cause makes Database show "no connections" forever even when
`sid db add` has been run — the widget is built with `vec![]` and never
re-hydrated from store.

**Tasks:**

1. **Fix `wire::build_app` to read from store and pass real data to widgets.**
   - `WorkspacesWidget::new(workspaces)` — already takes a vec; called correctly.
   - `DatabaseWidget::new(...)` — call `store.list_db_connections()` and pass.
   - `SystemWidget::with_state(SystemState { pinned_configs, quick_actions, ... })`
     constructed from store reads.
   - `SettingsWidget::with_categories(vec![Theme(...), Keybinds(...), ...])` —
     build all 7 categories using `ThemeRegistry`, store reads, and the existing
     `Keybind*View::new()` constructors.
2. **Outer border + 1-cell padding inside.** Wrap the entire `frame.area()`
   in a double-line `Block` titled `✦ sid — <active-tab>`. Inner content
   shifts to `frame.area().inner(Margin { vertical: 1, horizontal: 2 })`.
3. **Footer hint strip with capital-letter actions.** Today's footer is
   global-only. Add a per-tab line above the global line, populated by
   `Widget::footer_hint(&self) -> Vec<HintAction>` (new trait method, default
   empty) returning `[(char, label)]` pairs.

**Definition of done:**
- Settings shows all 7 categories on launch with the first selected.
- Database shows registered connections.
- System pinned configs / quick-actions are populated from store.
- A `cosmos`-themed outer border is visible.
- Per-tab footer shows context-specific letters (e.g., `[ N: new ] [ E: edit ]`).
- `cargo test --workspace` green; one new insta snapshot per tab demonstrating
  the bordered shell + footer.

**Commits:** 4 — one per widget hydration, one for shell border, one for
footer hints, one rolling all the insta snapshot updates.

**Risk:** Low. No new dependencies. No new traits beyond `footer_hint` (one
optional method with a default impl). Existing tests should mostly stay green
after a one-line snapshot regen per affected test.

## Phase 2 — Modal/dialog system

**Why second:** every subsequent CRUD task in Phases 3–5 depends on having
a modal infrastructure. Build the substrate once.

**Tasks:**

1. **`sid-widgets/src/modal.rs`** — `ModalSpec`, `Field` enum (Text /
   Password / Picker / Toggle / Choice), `ModalState` (cursor, field values,
   validation errors), `render_modal(frame, area, theme, modal)`.
2. **Modal stack on `App`.** `App.modals: Vec<ModalSpec>` push/pop. `wire::draw`
   renders the topmost modal AFTER the body, BEFORE the footer. Background dims.
3. **Routing.** When `App.modals` is non-empty, key events route to the modal
   first. `Esc` pops. `Enter` submits and pops on success.
4. **Test harness:** `crates/sid-widgets/tests/modal.rs` with insta snapshots
   of: empty form, partial input, validation error, password masked.

**Definition of done:**
- A demo modal renders cleanly with all field types.
- `Esc` / `Enter` work.
- Insta snapshot proves the dim background.
- Animation (when later added) pauses while a modal is open.

**Commits:** 3 — modal renderer + state, modal routing in App, demo + snapshots.

## Phase 3 — In-TUI CRUD: Workspaces

**Tasks:**

1. **Workspace data model update.** Two kinds in store: `Workspace` (created
   by user, has a name and maybe an umbrella relationship) and
   `DetectedRepo` (auto-discovered, can be promoted). Already in store; just
   surface the distinction in `WorkspacesWidget`.
2. **Modals:**
   - `N` — New workspace: name + path picker + kind (Umbrella / Repo).
   - `A` — Add repo to workspace: path picker; if path is already detected,
     promote it.
   - `R` — Confirm-modal: remove workspace (does NOT delete files).
3. **Tree rendering.** Umbrella expands/collapses with `Enter`. Detected
   repos appear under a "── auto-detected ──" separator at the bottom.

**Definition of done:**
- `N` opens a working modal that calls `store.upsert_workspace(...)`.
- `A` opens a working modal that promotes auto-detected repos into a workspace.
- `R` opens a confirm modal and calls `store.remove_workspace(...)`.
- Two new integration tests round-trip these through the store.

**Commits:** 3.

## Phase 4 — In-TUI CRUD: SSH (the big one)

**Tasks:**

1. **Modals for hosts:**
   - `N` — Add Host: alias, host, user, port, identity-file picker.
   - `E` — Edit Host: same form pre-filled.
   - `Del` — Confirm-Remove.
2. **Generate-key wizard (`G`).** Three modals chained:
   - Algorithm choice (Ed25519 default).
   - Output path + passphrase (skip-able).
   - "Copy to remote?" — runs `ssh-copy-id` against the focused host. Falls
     back to "here is your public key" display + clipboard.
3. **Setup-remote-auth wizard (`S`).** Pre-flight: pick identity, run
   `ssh-copy-id`, verify, persist `identity_file` on host record.
4. **Key manager drawer (`K`).** A modal-style side panel listing every key
   in `~/.ssh/`. Per-key: fingerprint, algo, comment, which hosts use it,
   delete, regenerate.
5. **Debug drawer (`X`).** Sub-options: show known_hosts for selected host;
   remove known_hosts entry (fixes "host key has changed"); show identity
   diagnostics; test connection verbose; clear cached agent identities.
6. **SFTP persist (`F`).** When connected via SFTP, "persist this path" adds
   `last_sftp_path` to the host record; subsequent `F` lands in the same
   directory.

**Definition of done:**
- All 6 affordances work end-to-end with a real SSH server in a manual test.
- Unit tests cover the wizards' state machines.
- Integration test: round-trip Add Host through TUI events, verify in store.

**Commits:** 6 (one per affordance), plus 1 for the test harness.

**Risk:** Medium-high. SSH key gen shells out (`ssh-keygen`); we need to
audit that we never log passphrases. `ssh-copy-id` is also a subprocess.
Both gated behind the existing `TerminalSpawner` abstraction so they remain
testable.

## Phase 5 — In-TUI CRUD: Database, System polish

**Tasks:**

1. **Database:** Add Connection modal (PG / SQLite branch). Edit modal.
   Confirm-delete. Test-connection button.
2. **System pinned configs:** `N` add, `E` edit label, `D` remove.
3. **System quick-actions:** `N` add, `E` edit, `D` remove. Includes a
   keybind chord field. After CRUD, call
   `wire::rehydrate_global_quick_actions` (already exists) so the palette
   reflects changes.
4. **System services pane:** action menu on `Enter` (Start / Stop / Restart
   / Journal tail). Modal for journal tail with follow toggle.

**Definition of done:**
- Three new modal flows tested.
- Quick-actions CRUD verifiably re-registers actions in the palette.

**Commits:** 4.

## Phase 6 — Animated background

Substantial new code; touches the render loop. Sub-phased so 6.1 (starfield)
can ship before 6.2 (supernovae).

### Phase 6.1 — Starfield

**Tasks:**

1. **New crate `sid-fx`** with:
   - `pub struct FxState { stars: Vec<Star>, rng: StdRng, tick: u64 }`
   - `pub fn render_starfield(buffer: &mut Buffer, area: Rect, state: &FxState, cfg: &AnimationConfig, theme: &Theme)`
   - `pub fn tick(state: &mut FxState, cfg: &AnimationConfig)` — advances
     twinkle phase, may add/remove stars to match density.
2. **Wire it in `wire::draw`** as the first paint step.
3. **Event loop tick.** `runtime::spawn_event_pump` emits a `Tick` event at
   the configured FPS. The render loop redraws on Tick OR Key. Skip Tick if
   any modal is open.
4. **Animation sub-view** in Settings (`crates/sid-widgets/src/settings/animation.rs`)
   exposing the AnimationConfig fields. Persists via
   `Store::set_setting(SETTING_ANIMATION, postcard)`.
5. **Test mode determinism.** `FxState::with_seed(u64)` constructor. Tests
   pass a fixed seed; production uses `OsRng::seed()`. Insta snapshots that
   include starfield use a seed.

**Definition of done:**
- Stars visible behind the body of every tab.
- Slider in Settings → Animation actually adjusts density.
- `animation.enabled = false` setting renders zero stars and skips ticks
  entirely.
- Insta snapshot proves starfield is deterministic under a fixed seed.

**Commits:** 5 (crate, render fn, runtime tick, settings sub-view, tests).

### Phase 6.2 — Supernovae

**Tasks:**

1. **`SupernovaQueue`** in FxState. Each entry has spawn time, age, location,
   colour palette.
2. **Idle trigger.** Every `supernova_idle_secs`, spawn one at a random
   off-content location (avoids overlapping selected rows).
3. **Event-driven triggers.** A new `EventBus::publish(SidEvent::Celebrate)`
   channel that widgets fire on:
   - Commit success (Workspaces).
   - Connection established (SSH).
   - Kill confirmed (Network).
   - Workspace added (Workspaces).
   Rate-limited to 1 per 30s.
4. **Render.** 5-frame animation: tiny → bloom → fade. Glyphs from the
   configured glyph set.

**Definition of done:**
- Manual test: open Workspaces, add a workspace via modal, see a supernova.
- `animation.supernova_idle_secs = 0` disables idle ones.
- `animation.supernova_on_event = false` disables celebrations.

**Commits:** 3.

## Total estimate

- Phase 1: ~6 hours · highest payoff.
- Phase 2: ~6 hours · enables 3/4/5.
- Phase 3: ~4 hours.
- Phase 4: ~12 hours · the big one.
- Phase 5: ~6 hours.
- Phase 6: ~8 hours total (6.1 = ~5, 6.2 = ~3).

**Grand total:** ~42 hours of focused work. Ship Phase 1 first (turns a
broken-feeling app into a functional one in one sitting). Then phase by
phase as the user prioritises.

## What ships if we only get Phase 1

Even Phase 1 alone transforms the experience:
- Settings tab populated, navigable, functional theme picker.
- Database tab shows real connections.
- System tab shows real pinned configs + quick-actions.
- Workspaces tab still has the same shape but renders inside the new bordered shell.
- Footer prompts you with capital-letter next-actions.

Roughly: 80% of the "this app is unfinished" feeling comes from Phase 1.
