# Spec — TUI UX interaction overhaul (2026-05-22)

> **Status:** draft for review.
> **Sister docs:**
> - [Parent UX direction](2026-05-22-sid-ux-iteration.md) — the "galaxy cockpit" direction this builds on
> - [Existing overhaul plan](../plans/2026-05-22-sid-ux-overhaul.md) — substrate + CRUD landed via that plan
> - [Future-features backlog](2026-05-20-sid-future-features.md) — items intentionally deferred

## 1. Problem statement

The substrate from the UX overhaul plan landed: outer border, footer hints,
modal infrastructure, CRUD modals across every tab, animated background. In
real use the **interaction layer** still hurts:

| Symptom | What actually happens | Where |
|---|---|---|
| Popup menus only respond to Space/Enter; arrow keys are dead | `route_key_to_modal` has no `Up`/`Down`/`Left`/`Right` handlers on `Field::Choice` | `crates/sid-widgets/src/modal.rs:537-580` |
| Save/Cancel cannot be focused or selected | They are decorative labels, not focusable fields | `modal.rs:929-978` |
| Workspaces tab is pre-populated with repos I never registered | `startup_discover()` scans `~/vcs/` on every launch | `crates/sid/src/main.rs:391-396` |
| Pressing Enter on a workspace does nothing | Enter on leaf is a literal no-op; only umbrella toggles expand | `crates/sid-widgets/src/workspaces.rs:1905-1908` |
| No way to "open" a workspace as a working surface | `TabManager` has no `push`/`pop` API, six tabs fixed at startup | `crates/sid-core/src/tab.rs:93-211` |
| Network tab cannot search ports/processes via a discoverable affordance | `/` filter exists but is not advertised | `crates/sid-widgets/src/network.rs:783` |
| Interfaces tab is read-only and unsortable | `InterfacesSidebarState` has no drill-in; provider-sorted alphabetically | `crates/sid-widgets/src/network/interfaces_sidebar.rs:84` |
| Settings changes do not apply | Five of seven categories route to `{ /* handled by wire path */ }` — wire never landed | `crates/sid-widgets/src/settings.rs:643-652` |
| `Ctrl+1..6` and `Ctrl+,` do nothing | Bindings exist; most terminals do not deliver `Ctrl+digit` or `Ctrl+,` as distinct chords | `crates/sid-core/src/keybind.rs:240-268` |
| Slight UI hesitation when tab-switching | No frame-budget enforcement, hot-path allocations not bounded, FX renders every frame regardless of cost | `crates/sid/src/wire.rs::draw`, `crates/sid-fx/*` |

The user's brief, condensed: **make every popup keyboard-navigable, make
workspaces self-defined and open-as-tab, give Network real drill-in and
search, make Settings actually apply, give every Ctrl chord a working
fallback — and keep the cockpit fast.**

## 2. Design principles (binding)

1. **Substrate-first.** One modal substrate fix unlocks ~30 popups across
   the app. Fix the substrate before fixing individual modals.
2. **Self-defined state over auto-discovery.** Manual registration is the
   default. Discovery is an opt-in command, never a startup side effect.
3. **Dynamic tabs are first-class.** Workspace detail tabs are real tabs;
   they appear in the strip, have their own state, and close cleanly.
4. **Live apply with undo, not "Save and pray".** Settings changes
   persist on the keystroke that made them; a per-session undo ring lets
   the user back out cheaply.
5. **Keep the cockpit fast.** Frame budget is 8 ms at 120 Hz. Every PR in
   this overhaul carries a criterion benchmark for its hot path; a
   ≥10% regression fails the gate per CLAUDE.md.
6. **Adapter pattern preserved.** No new external-crate names land in
   `sid-widgets` or `sid-core`. Adapter trait method additions go on
   existing traits in `sid-core::adapters`.

## 3. Scope and branch sequencing

Five focused branches. Branch #1 unblocks the rest; #2–#5 can land in any
order after #1. We don't open GitHub PRs — work happens on local branches
or git worktrees and merges back to `main` when each is green.

| # | Branch | Crates touched | Depends on |
|---|---|---|---|
| 1 | `feat/modal-arrows-and-keybind-fallbacks` | `sid-widgets/modal`, `sid-core/keybind`, `sid-core/tab` | — |
| 2 | `feat/workspace-overview-self-defined` | `sid-core/workspace_discovery`, `sid-widgets/workspaces`, `sid` (main + wire) | #1 |
| 3 | `feat/workspace-detail-as-tab` | `sid-core/tab`, `sid-widgets/workspace_detail` (NEW), `sid` (wire) | #1, #2 |
| 4 | `feat/network-drill-in-and-sort` | `sid-widgets/network/*`, `sid-core/sys_probe`, `sid-core/adapters/sys` | #1 |
| 5 | `feat/settings-live-apply-undo` | `sid-widgets/settings*`, `sid` (wire) | #1 |

Each branch ships with its tests + a criterion benchmark for the path
it touches.

## 4. Architecture

```text
┌────────────────────────────────────────────────────────────────────┐
│  sid-core                                                           │
│  ├── tab.rs           TabManager + TabKind { Core, Detail }         │
│  │                    + push_detail / close_active / detail_count   │
│  ├── keybind.rs       cosmos_default + Alt fallbacks + tab.close    │
│  ├── adapters/sys     + default_route_iface_name() trait method     │
│  └── workspace_*      scan_workspace_root used only on-demand       │
├────────────────────────────────────────────────────────────────────┤
│  sid-widgets                                                        │
│  ├── modal.rs         route_key_to_modal handles arrows + L/R cycle │
│  │                    new ModalSpec methods: cycle_choice,          │
│  │                    flip_toggle, bump_u64                         │
│  ├── workspaces.rs    Enter on leaf → emit "workspaces.open_detail" │
│  ├── workspace_detail.rs   NEW — multi-repo dashboard widget        │
│  ├── network/*        interface sort + drill-in + filter hint       │
│  └── settings/*       sub-views emit typed Outcome → wire dispatch  │
├────────────────────────────────────────────────────────────────────┤
│  sid (binary)                                                       │
│  ├── wire.rs          tab.close action handler; open_detail wiring; │
│  │                    settings.outcome.* dispatch + undo ring       │
│  └── main.rs          drop unconditional startup_discover() call    │
└────────────────────────────────────────────────────────────────────┘
```

The change is structural in two crates:

- `sid-core::tab::Tab` grows a `kind: TabKind` discriminator. Adding a
  field is a `pub`-API change; downstream uses (the binary and Plan-1
  tests) are updated in the same commit.
- `sid-core::adapters::sys::SysProvider` grows
  `default_route_iface_name(&mut self) -> Result<Option<String>, SysError>`.
  Existing impls add a sensible default; the real impl in `sid-sysinfo`
  reads `/proc/net/route` on Linux, shells out to `route -n get default`
  on macOS.

## 5. Component design

### 5.1 Modal substrate (branch #1)

Replace `route_key_to_modal`'s match (`crates/sid-widgets/src/modal.rs:537-580`):

```rust
match (key.code, key.mods) {
    (KeyCode::Esc, _)                                  => Cancel,
    (KeyCode::Up, _) | (KeyCode::BackTab, _)           => { modal.cycle_focus_backward(); Consumed }
    (KeyCode::Down, _)                                 => { modal.cycle_focus_forward();  Consumed }
    (KeyCode::Tab, m) if !m.contains(KeyModifiers::SHIFT)
                                                       => { modal.cycle_focus_forward();  Consumed }
    (KeyCode::Tab, _)                                  => { modal.cycle_focus_backward(); Consumed }
    (KeyCode::Left, _)                                 => { modal.cycle_focused_value(-1); Consumed }
    (KeyCode::Right, _)                                => { modal.cycle_focused_value( 1); Consumed }
    (KeyCode::Backspace, _)                            => { modal.backspace();             Consumed }
    (KeyCode::Enter, _)                                => Submit,
    (KeyCode::Char(' '), _)
        if matches!(modal.fields.get(modal.focus), Some(Field::Toggle{..} | Field::Choice{..}))
                                                       => { modal.space_or_enter_on_field(); Consumed }
    (KeyCode::Char(c), m)
        if !m.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                                                       => { modal.type_char(c);            Consumed }
    _                                                  => Consumed,
}
```

`cycle_focused_value(dir)` is a new method that fans out by field kind:

| Field         | `dir=+1`              | `dir=-1`              |
|---------------|-----------------------|-----------------------|
| `Choice`      | next option (wrap)    | prev option (wrap)    |
| `Toggle`      | flip                  | flip                  |
| `U64`         | `value += step` clamp | `value -= step` clamp |
| `Text/Password/Picker/Display` | no-op | no-op |

Renderer changes (`crates/sid-widgets/src/modal.rs::render_field_value`):
the focused row of a `Choice`/`Toggle`/`U64` field shows a small `‹ ›`
hint to the right of its value so users see the cycle affordance.
Buttons stay decorative — labels read `[ Enter: Save ]` and `[ Esc: Cancel ]`.

Backwards compat note: the old Space-cycles-Choice behaviour is preserved
under the new code path so existing tests + muscle memory continue to work.

### 5.2 Global keybinds (branch #1)

In `cosmos_default()` (`crates/sid-core/src/keybind.rs:232`), add Alt-modifier
alternates and the new `tab.close` action:

```rust
for i in 1..=6 {
    let c = char::from_digit(i, 10).unwrap();
    bind(&mut m, KeyCode::Char(c), KeyModifiers::CONTROL, &format!("tabs.jump.{i}"));
    bind(&mut m, KeyCode::Char(c), KeyModifiers::ALT,     &format!("tabs.jump.{i}"));
}
bind(&mut m, KeyCode::Char(','), KeyModifiers::CONTROL, "app.open_settings");
bind(&mut m, KeyCode::Char(','), KeyModifiers::ALT,     "app.open_settings");
bind(&mut m, KeyCode::Char('w'), KeyModifiers::CONTROL, "tab.close");
bind(&mut m, KeyCode::Char('w'), KeyModifiers::ALT,     "tab.close");
```

`App::run_action` (`crates/sid-core/src/app.rs:422`) gains a `"tab.close"`
arm that calls `self.tabs.close_active()` and discards the bool (the
TabManager method is a no-op on `TabKind::Core`).

We do NOT auto-enable the kitty keyboard protocol. Users on
`kitty`/`ghostty`/`wezterm`/`foot`/`xterm` modifyOtherKeys=2 get the Ctrl
chord; everyone else uses Alt. Auto-enabling is tracked in the future
backlog.

### 5.3 TabManager dynamic-tab API (branch #1)

`Tab` (`crates/sid-core/src/tab.rs:60`) grows:

```rust
#[derive(Clone, Eq, PartialEq, Debug)]
pub enum TabKind {
    /// One of the six pinned cockpit tabs. Cannot be closed.
    Core,
    /// Dynamically opened, closable. Carries the index of the core
    /// tab that spawned it so `close_active` can snap focus back.
    Detail { parent_idx: usize },
}

pub struct Tab {
    pub id: TabId,
    pub title: String,
    pub layout: Layout,
    pub hotkey: Option<char>,
    pub kind: TabKind,
}
```

`TabManager` adds:

```rust
pub fn push_detail(&mut self, tab: Tab) -> Result<(), SidError>; // rejects Core
pub fn close_active(&mut self) -> bool;                          // false on Core
pub fn detail_count(&self) -> usize;
```

Invariant: the six `Core` tabs always occupy indices `0..6`; detail tabs
append at index `6..`. `close_active` removes by index and snaps
`active_idx` to the saved `parent_idx`. Property test enforces both.

### 5.4 Workspace overview cleanup (branch #2)

- Drop the unconditional `wire::startup_discover` call at
  `crates/sid/src/main.rs:391`. The function stays in the codebase but is
  now invoked only via a new command-palette action `workspaces.scan_now`
  (off by default — surfacing this is a follow-up; the function is
  exported and tested).
- Drop the `--skip-discovery` flag (kept as a no-op stub for one release
  cycle to avoid breaking muscle memory).
- `workspaces.rs:1905-1908`: when `Enter` is pressed and the selected
  workspace is `WorkspaceKind::Repo` or `WorkspaceKind::Umbrella`, emit
  the action `workspaces.open_detail` via `WidgetCtx`. Umbrella expansion
  moves to `Right`/`l` arrow (mirrors tree-style file pickers).
- Empty-state body when the workspace list is empty: a single line
  `no workspaces yet — press N to add one`.

### 5.5 `WorkspaceDetailWidget` (branch #3, NEW)

`crates/sid-widgets/src/workspace_detail.rs`:

```rust
pub struct WorkspaceDetailWidget {
    workspace: Workspace,
    sub_repos: Vec<RepoSummary>,
    selected: usize,
    right_pane: RightPane,                        // reused from workspaces.rs
    git_factory: Arc<dyn GitProvider>,
}

#[derive(Clone, Debug)]
pub struct RepoSummary {
    pub path: PathBuf,
    pub name: String,
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub dirty: u32,
    pub last_commit_age_secs: u64,
    pub ci_status: CiStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CiStatus { Pending, Pass, Fail, Unknown }
```

Layout: a six-column table at the top
(`repo · branch · ahead/behind · dirty · age · CI`), then the existing
`RightPane` enum (`Branches | Status | Log | Diff | Commit | Actions`)
scoped to the highlighted sub-repo at the bottom. Reuses the right-pane
substrate from `crates/sid-widgets/src/workspaces.rs` verbatim — same
border styling, same key handling.

Sub-repos are discovered on tab-open by calling
`scan_workspace_root(&workspace.path, 1)` (existing pure walker).
Results are owned by the widget; no store mutation. Scan happens off the
render thread (via `JobQueue`) so opening a workspace with 30 sub-repos
does not block the UI.

`Ctrl+W` (and `Alt+W`) close the tab via the global `tab.close` action.

`CiStatus` returns `Unknown` for every repo in v1 — wiring a real
fetcher (probably shelling out to `gh run list --json`) is tracked in
the backlog.

### 5.6 Network tab (branch #4)

**Sort.** In `InterfacesSidebarState::set_data`, sort by a computed
score before storing:

```rust
score(iface, default_route) =
    (Some(iface.name) == default_route) ? 0 : 100         // primary WAN at top
    + (iface.is_up ? 0 : 10)
    + (is_virtual(&iface.name) ? 5 : 0)                   // lo/docker0/tun*/veth*/br*
    + alphabetical_tiebreak(iface.name)
```

`default_route` comes from a new sys-probe call propagated through the
existing snapshot. `is_virtual` matches a fixed prefix list:
`lo`, `docker`, `br-`, `veth`, `tun`, `tap`, `virbr`, `vmnet`.

**Drill-in.** `Enter` on a focused interface opens an
`InterfaceDetailModal` (built with the substrate) using `Field::Display`
rows for each attribute. Greyed footer hint: `Edit (E) coming soon`.
The `E` chord is bound but its handler is a stub that pushes a toast
`"Interface editing not yet supported — see backlog"`.

**Search affordance.** The `/` chord stays. The Network tab's
`footer_hint()` now returns `[("/", "filter"), ("s", "sort"), ("K", "kill"), ("Enter", "detail")]`.
This is the discoverability fix the user asked for. No new code path —
just an updated hint list.

**Port accuracy note.** Today's `ListeningPort` only enumerates
listening TCP/UDP. Established connections, Unix sockets, and
sudo-required visibility live in the backlog (a separate adapter
expansion).

### 5.7 Settings live-apply (branch #5)

Resolve the TODO at `crates/sid-widgets/src/settings.rs:643-652`. The
contract is:

1. Each sub-view (`BehaviorTogglesView`, `WorkspaceRootsView`,
   `KeybindEditorView`, `QuickActionsView`, `DbPathView`, `ResetView`)
   already mutates its own state on key events. We add an
   `Outcome::<View>` return per sub-view that names the change that just
   happened in store-typed values.
2. `SettingsWidget::handle_event` matches on the outcome and emits
   `EventOutcome::Emit("settings.outcome.<view>", payload)`.
3. The binary's wire layer dispatches the emit via a new
   `dispatch_settings_outcome(sid_app, view, payload)` that calls the
   matching `Store::put_*` method.
4. Each successful put pushes a toast
   `"Saved <label>  (u: undo)"`. Pressing `u` while the toast is alive
   reverts the put, popping it off a per-session undo ring (max 10).
5. If `put_*` returns `Err`, push an error toast and leave the in-view
   state alone (so the user sees the value they tried to set).

The undo ring lives on `SidApp` (binary-local) as `Vec<UndoEntry>`;
entries are `{ key: &'static str, previous: SettingValue, label: String, expires_at: Epoch }`.
Expiry is 30 s — long enough to notice and undo, short enough that the
ring does not accumulate stale state.

## 6. Performance

The user noticed slight UI hesitations when tab-switching, especially
through every core tab. This section sets the floor and enforces it.

### 6.1 Frame budget

Target: **8 ms** per render at 120 Hz. Above that, the render loop
visibly hitches on a midrange machine. Hard ceiling: **16 ms** at 60 Hz —
anything slower than this is treated as a regression bug, not "slow but
fine".

### 6.2 Hot-path benches (each carried by its respective branch)

Add criterion benchmarks under `crates/sid-widgets/benches/` and
`crates/sid-core/benches/`. CLAUDE.md already mandates the 10%-regression
gate; these specific benches join the list.

| Bench | Branch | Budget |
|---|---|---|
| `bench_app_handle_event_noop` (sid-core) | #1 | ≤ 1 µs (already in CLAUDE.md plan, not committed) |
| `bench_tab_switch_render` (workspace, ssh, db, net, sys, settings) | #1 | ≤ 8 ms each |
| `bench_workspaces_visible_workspaces_for_n` (5, 50, 500 items) | #2 | ≤ 100 µs at n=500 |
| `bench_workspace_detail_open_with_5_subrepos` | #3 | ≤ 16 ms wall (first frame) |
| `bench_network_interface_sort_for_n` (5, 20, 100 ifaces) | #4 | ≤ 50 µs at n=100 |
| `bench_settings_outcome_dispatch` | #5 | ≤ 200 µs |

### 6.3 Known offenders to audit (branch #1 audit pass)

These are not all in scope to fix in #1 — but #1 includes a profiling
pass with `dhat-heap` to catch the worst cases, and fixes are tracked
either in the same branch (if the fix is small) or in the relevant
follow-up branch.

1. **`WorkspacesState::visible_workspaces` returns a fresh
   `Vec<&Workspace>` on every call (`workspaces.rs:1177-1187`).** The
   render path calls it potentially multiple times per frame. Memoize on
   the workspace list version.
2. **`NetworkWidget::render_into_frame` rebuilds every `Row` and every
   `Cell` from scratch every frame.** Profile first; the table widget
   may already be cheap enough. If allocations dominate, cache the
   per-row `String`s keyed on selection+sort state.
3. **`sid-fx` starfield + supernova render runs unconditionally.** It is
   the easiest cost to slash on slow terminals — gate behind a per-tab
   "draw fx" flag that respects the user's animation FPS setting (already
   in Settings; just plumb it).
4. **Postcard decode on `Widget::load_state` runs at app start, not on
   tab switch.** Not a tab-switch cost; flag for clarity only.

### 6.4 Profiling note

`dhat-heap` is wired in `Cargo.toml` already (see CLAUDE.md). Branch #1
includes one new doc in `docs/DEVELOPMENT.md` covering how to run a
`dhat` session against the cockpit and read the dump.

## 7. Data flow (one trace)

```text
key event
   │
   ▼ crossterm Event
app.handle_event()
   ├─ palette open? → palette dispatch, return
   ├─ modal_stack non-empty? → route_key_to_modal()         ◄── substrate updated
   ├─ global keybind hit?    → run_action()                 ◄── tab.close new
   └─ widget.handle_event(ev, ctx)
        │
        └─ widget may emit Action via WidgetCtx
                  │
                  ▼ wire.rs dispatch
                  ├─ "workspaces.open_detail" → build WorkspaceDetailWidget;
                  │                              tabs.push_detail(...)
                  ├─ "tab.close"              → tabs.close_active() (no-op on Core)
                  └─ "settings.outcome.<x>"   → Store::put_*() + toast + undo-push
```

## 8. Error handling

| Failure | Behaviour |
|---|---|
| Workspace path missing on Enter | Detail tab opens in "(path missing — press R to remove)" empty state |
| Sub-repo scan permission denied on subtree | Skip subtree, log WARN, render rest |
| `tab.close` invoked on `TabKind::Core` | No-op, toast `Cannot close pinned tab` |
| Settings undo with stale pointer | Toast `Undo no longer applicable`, drop entry |
| `default_route_iface_name()` returns Err | Sort falls back to alphabetical, log DEBUG |
| Modal Up/Down on empty `fields: vec![]` | No-op (existing `cycle_focus_*` guards) |
| Terminal cannot deliver Alt+digit either | User opens command palette (Ctrl+F) and picks `tabs.jump.N` from the registered actions |
| `put_setting` fails mid-apply | Error toast; in-view state stays at attempted value; undo entry **not** pushed |

## 9. Testing (per CLAUDE.md, in-same-commit)

Every branch lands its tests with the production code, no exceptions.

### Branch #1 — modal substrate + keybinds + TabManager

- Unit: exhaustive `route_key_to_modal` matrix — every `KeyCode` × every
  `Field` variant × focused-vs-not. ~30 tests.
- Unit: `cycle_focused_value(dir)` for each field variant; clamp
  behaviour on `U64::min`/`U64::max`; wrap behaviour on `Choice`.
- Property test: `cycle_choice` then `cycle_choice` in opposite direction
  is identity (when `options.len() > 1`).
- Property test: `bump_u64(+1)` then `bump_u64(-1)` is identity off
  boundaries.
- Snapshot (`insta`): render a 3-field modal with focus on each kind;
  assert `‹ ›` cycle hint appears beside Choice/Toggle/U64.
- Adversarial: synthetic event with `KeyCode::Char('1')` (no Ctrl) and
  `KeyCode::Char('1')` with `KeyModifiers::ALT` — only the latter fires
  `tabs.jump.1`.
- Unit: `TabManager::push_detail(Tab { kind: Core, .. })` returns
  `SidError::InvalidArgument`.
- Property test: arbitrary `push_detail`/`close_active` sequences keep
  `0 <= active_idx < tabs.len()` and `detail_count() == tabs.len() - 6`.
- Criterion: `bench_app_handle_event_noop`, `bench_tab_switch_render`.

### Branch #2 — workspace overview

- Unit: `Enter` on leaf workspace emits `workspaces.open_detail` with
  the correct path. `Enter` on umbrella toggles expand.
- Unit: empty workspace list renders the "press N to add one" hint.
- Adversarial: workspace registered at non-existent path — `Enter`
  still emits; binding side opens detail in "missing" state.
- Migration: existing users with auto-discovered workspaces in their
  store keep those records; the next launch just stops adding new ones.
- Criterion: `bench_workspaces_visible_workspaces_for_n(5, 50, 500)`.

### Branch #3 — workspace detail tab

- Unit: `WorkspaceDetailWidget::new` populates `sub_repos` from
  `scan_workspace_root(path, 1)` (mocked with a `tempfile::TempDir`
  containing fake `.git` markers).
- Unit: `tab.close` action while a detail tab is active drops the tab
  and snaps `active_idx` back to parent.
- Integration: open workspace → press `Ctrl+W` → assert active tab id
  is `"workspaces"` and `detail_count()` is 0.
- Adversarial: scan returns empty → widget renders "no sub-repos found"
  body, not a panic.
- `fail`-crate test: scan errors mid-walk → partial dashboard renders.
- Criterion: `bench_workspace_detail_open_with_5_subrepos`.

### Branch #4 — network drill-in + sort

- Unit: sort with `default_route = Some("wlan0")` puts `wlan0` first,
  `lo` last, regardless of input order.
- Unit: sort with `default_route = None` falls back to alphabetical UP
  physical first.
- Property test: sort is stable for arbitrary input orderings.
- Snapshot: 6-iface render with mixed kinds; assert order.
- Doc test: `NetworkWidget::footer_hint` returns a vec whose joined
  string contains `"/ filter"`.
- Unit: `Enter` on interface row opens `InterfaceDetailModal` with all
  `Field::Display` rows populated from the focused interface.
- Criterion: `bench_network_interface_sort_for_n(5, 20, 100)`.

### Branch #5 — settings live-apply

- Unit: a `BehaviorTogglesView` outcome dispatch calls
  `store.put_setting(key, value)`.
- Unit: `WorkspaceRootsView` outcome adds to the JSON-array setting.
- Unit: undo of a put restores the previous setting.
- Property test: random sequence of toggle/undo returns the store
  baseline.
- `fail`-crate test: `put_setting` failure → error toast, dirty
  in-view state preserved, no undo entry pushed.
- Criterion: `bench_settings_outcome_dispatch`.

## 10. Out of scope (deferred — tracked in `2026-05-20-sid-future-features.md`)

The following items are **deliberately not in v1 of this overhaul**. Each
is filed in the future-features doc; this section is the canonical
source of why they were deferred so a reviewer doesn't have to guess.

- **Network interface mutation.** DHCP toggle, static IP, MTU edit,
  service restart. Requires a new adapter (NetworkManager/iproute2),
  polkit/sudo integration, robust failure modes. The `E` chord exists
  and pushes a "coming soon" toast.
- **Real CI status fetcher** for `WorkspaceDetailWidget`. v1 stub
  returns `Unknown`. v2 will shell out to `gh run list --json` and
  cache per-repo.
- **"Sync all sub-repos to branch X"** actual implementation. The `b`
  modal exists in v1 as a stub that toasts "not yet implemented".
- **Tab detach** (`tab.detach`/`tab.attach` actions stay no-ops). Detach
  needs the IPC socket and a separate process model; it is its own plan.
- **Kitty-protocol auto-enable on startup.** Avoids the need for Alt
  fallbacks on supported terminals. Cheap to add but not strictly
  necessary now that fallbacks ship.
- **Workspace scan as a command-palette action** (the `workspaces.scan_now`
  affordance mentioned in §5.4). The function is exported and tested,
  but the UI affordance is a follow-up.
- **Established-connections / Unix-socket port visibility.** Adds a
  sys-probe surface and possible sudo escalation; not in v1.
- **Per-tab "draw fx" flag** for the starfield (§6.3 #3). Easiest perf
  win, but lives in its own branch once benches show it matters.
- **Windows terminal support.** Out of scope until a maintainer signs
  up to own it.

## 11. Risk and rollback

Branch #1 changes a public-facing struct (`Tab`) and the modal substrate
both. We do these together so downstream callers see the new shape in
one commit. Rollback is a single revert.

Branches #2–#5 each touch one or two crates and emit no breaking changes
to other crates' public API. Each can be reverted independently.

The biggest risk is on branch #3: dynamic tabs change the
`TabManager`'s invariants. The property test in #1 guards against
push/close sequences breaking those invariants; if it red-lights, work
stops on #3 until the test is satisfied.

---

## Appendix A — Mapping from user complaints to design

| User complaint | Address in branch | Code location |
|---|---|---|
| Arrow keys dead in session-restore popup | #1 | `modal.rs::route_key_to_modal` |
| Same in every other popup | #1 | same |
| Can't Tab to Save/Cancel | #1 (decision: not a thing) | n/a |
| Auto-populated workspaces I didn't add | #2 | `main.rs:391`, dropped |
| Can't enter a workspace | #2, #3 | `workspaces.rs:1905`, new emit |
| No workspace overview tab | #3 | `workspace_detail.rs`, NEW |
| Can't close that tab | #1, #3 | `tab.close` action |
| No port search | #4 | footer hint + `/` already works |
| No process search | #4 | footer hint + `/` already works |
| Ports inaccurate | (deferred) | sys-probe expansion |
| Interfaces are read-only | #4 (read-only detail) + backlog (write) | `InterfaceDetailModal` |
| No "primary WAN first" sort | #4 | `interfaces_sidebar.rs::set_data` |
| Settings won't apply | #5 | `settings.rs:643-652` resolution |
| Ctrl+1..6 dead | #1 | Alt fallback |
| Ctrl+, dead | #1 | Alt fallback |
| Ctrl+W to close tab | #1 (binding) + #3 (semantics) | `keybind.rs`, `tab.rs` |
| UI hesitations on tab switch | §6 perf budget + criterion in every branch | benches dir |
