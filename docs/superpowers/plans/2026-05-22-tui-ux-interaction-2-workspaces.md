# Branch #2 — Workspace overview cleanup (drop auto-discovery + open-detail emit)

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Workspaces tab show only what the user has explicitly registered, and have `Enter` on a leaf workspace emit an `workspaces.open_detail` action so branch #3's `WorkspaceDetailWidget` can pick it up.

**Architecture:** Three changes. (1) Remove the unconditional `startup_discover` call so launching sid no longer scans `~/vcs/`. (2) Change `WorkspacesWidget`'s key handler: `Enter` on a leaf emits `workspaces.open_detail`; umbrella expand moves to `Right`/`l` (matching tree-style file pickers). (3) Empty-state body when the workspace list is empty: a single line "no workspaces yet — press N to add one".

**Tech Stack:** Rust 2024 edition, sid-core::context::WidgetCtx for action emission, ratatui (rendering).

**Branch:** `feat/workspace-overview-self-defined`

**Depends on:** Branch #1 merged (`feat/modal-arrows-and-keybind-fallbacks`).

**Spec reference:** [`docs/superpowers/specs/2026-05-22-tui-ux-interaction-design.md`](../specs/2026-05-22-tui-ux-interaction-design.md) §§ 5.4, 6.3.

---

## File map

| File | Purpose | Action |
|---|---|---|
| `crates/sid/src/main.rs:390-397` | drop unconditional discovery call | Modify |
| `crates/sid/src/main.rs:33-43` | keep `--skip-discovery` flag as no-op stub | Modify (doc only) |
| `crates/sid-widgets/src/workspaces.rs:1895-1909` | Enter emits action, Right toggles expand | Modify |
| `crates/sid-widgets/src/workspaces.rs:1400ish` | empty-state rendering | Modify |
| `crates/sid-core/src/widget.rs` | confirm `WidgetCtx::emit` exists (it does — `emit_action`) | Read-only |
| `crates/sid-widgets/tests/workspaces_actions.rs` | tests for Enter-emits + Right-expand | Modify |
| `crates/sid-widgets/tests/workspaces_render.rs` | snapshot for empty-state | Modify |
| `crates/sid-widgets/benches/visible_workspaces.rs` | criterion bench | Create |
| `crates/sid-widgets/Cargo.toml` | bench entry | Modify |

---

## Task 1 — Drop `startup_discover` from `main.rs`, keep `--skip-discovery` as no-op

**Files:**
- Modify: `crates/sid/src/main.rs:33-43, 390-397`

`★ Insight ─────────────────────────────────────`
The function `wire::startup_discover` is kept exported in `wire.rs` because branch #2's follow-up (a command-palette `workspaces.scan_now` action — tracked in the backlog) will call it on demand. We just stop calling it at launch.
`─────────────────────────────────────────────────`

- [ ] **Step 1.1: Modify `Cli` doc and remove the unconditional discovery call**

Edit `crates/sid/src/main.rs` around lines 33-43:

```rust
    /// Skip workspace discovery scan on startup.
    ///
    /// **No-op in this release.** sid no longer scans `~/vcs/` at startup;
    /// the flag is preserved for one release cycle so muscle memory doesn't
    /// trigger a CLI error. Use the `workspaces.scan_now` command-palette
    /// action (when implemented) to scan on demand.
    #[arg(long)]
    skip_discovery: bool,
```

Edit `crates/sid/src/main.rs` around lines 390-397:

```rust
    // Startup workspace discovery is disabled. Workspaces are exclusively
    // user-registered (via `sid workspace add` or the in-TUI N modal).
    // `cli.skip_discovery` is preserved as a no-op for one release cycle
    // and is otherwise ignored.
    let _ = cli.skip_discovery;
```

- [ ] **Step 1.2: Verify the binary still compiles**

```bash
cargo build -p sid
```

Expected: success.

- [ ] **Step 1.3: Confirm launching sid against a fresh store no longer pre-populates workspaces**

```bash
mkdir -p /tmp/sid-test-empty
cargo run -p sid -- --db /tmp/sid-test-empty/sid.redb workspace list
```

Expected: empty list (no "found N repos in ~/vcs/" log line). Even if `~/vcs/` exists locally.

- [ ] **Step 1.4: Commit Task 1**

```bash
git add crates/sid/src/main.rs
git commit -m "feat(sid): drop unconditional startup_discover scan of ~/vcs/

The Workspaces tab is now exclusively populated by user-registered
entries (via sid workspace add or the in-TUI N modal). startup_discover
remains in wire.rs as a pub function; a follow-up branch will surface
it through a workspaces.scan_now command-palette action.

--skip-discovery flag preserved as a no-op stub for one release."
```

---

## Task 2 — Empty-state render hint when workspace list is empty

**Files:**
- Modify: `crates/sid-widgets/src/workspaces.rs` — the tree-rendering function
- Test: `crates/sid-widgets/tests/workspaces_render.rs`

- [ ] **Step 2.1: Find the tree-render function**

Search:

```bash
grep -n "fn render_into_frame\|fn render_tree\|render_workspaces_tree\|visible_workspaces().is_empty()" crates/sid-widgets/src/workspaces.rs | head -10
```

Read the function. The tree pane renders the list of visible workspaces inside a bordered Block titled `" Workspaces "`. We need to insert an empty-state line when `visible_workspaces()` returns an empty slice.

- [ ] **Step 2.2: Add failing test for empty-state hint**

Append to `crates/sid-widgets/tests/workspaces_render.rs`:

```rust
use sid_widgets::workspaces::render_to_string;
use sid_widgets::WorkspacesWidget;

#[test]
fn empty_workspaces_renders_press_n_hint() {
    let w = WorkspacesWidget::new(vec![], None);
    let s = render_to_string(&w, 100, 30);
    assert!(
        s.contains("no workspaces yet") && s.contains("press N to add one"),
        "expected empty-state hint in:\n{s}",
    );
}
```

> Note: if `sid_widgets::workspaces::render_to_string` doesn't exist, add a small helper modelled on `sid_widgets::network::render_to_string` (which already exists at `crates/sid-widgets/src/network.rs:900`).

- [ ] **Step 2.3: Run test, verify failure**

```bash
cargo test -p sid-widgets --test workspaces_render empty_workspaces_renders_press_n_hint
```

Expected: FAIL — string not present.

- [ ] **Step 2.4: Insert the empty-state hint in the tree render**

In the tree-rendering function (around the `Paragraph::new(...)` that draws the workspace rows), branch on `state.visible_count() == 0`:

```rust
let rows: Vec<Line> = if self.state.visible_count() == 0 {
    vec![
        Line::from(""),
        Line::from(Span::styled(
            "  no workspaces yet — press N to add one",
            Style::default().fg(theme.muted.into()),
        )),
    ]
} else {
    /* existing row build loop */
};
```

- [ ] **Step 2.5: Run test, verify PASS**

```bash
cargo test -p sid-widgets --test workspaces_render empty_workspaces_renders_press_n_hint
```

Expected: PASS.

- [ ] **Step 2.6: Refresh any insta snapshots that touched the workspace tree**

```bash
cargo insta test -p sid-widgets --review
```

Accept diffs that only show the empty-state line where it should appear.

- [ ] **Step 2.7: Commit Task 2**

```bash
git add crates/sid-widgets/src/workspaces.rs crates/sid-widgets/tests/workspaces_render.rs crates/sid-widgets/tests/snapshots/
git commit -m "feat(sid-widgets): empty-state hint on Workspaces tab — \"press N to add one\"

Now that auto-discovery is gone, fresh installs see an empty list.
A single muted-styled hint line tells the user how to populate it.
Snapshot test covers the case."
```

---

## Task 3 — `Enter` emits `workspaces.open_detail`; `Right` toggles umbrella expand

**Files:**
- Modify: `crates/sid-widgets/src/workspaces.rs:1895-1909`
- Test: `crates/sid-widgets/tests/workspaces_actions.rs`

`★ Insight ─────────────────────────────────────`
`WidgetCtx::emit_action(&str)` is the existing mechanism widgets use to bubble actions up to the binary's wire layer. We send `workspaces.open_detail`. The action body (build the detail widget, push the tab) lives in branch #3. This branch's job is just to emit cleanly.
`─────────────────────────────────────────────────`

- [ ] **Step 3.1: Confirm the `emit_action` API surface**

Read `crates/sid-core/src/context.rs` and find the public method on `WidgetCtx`. Confirm it's named `emit_action(&str)` or similar. If the method has a different name (e.g. `send_action`), use the actual name.

```bash
grep -n "pub fn emit\|pub fn send\|emit_action\|action_tx" crates/sid-core/src/context.rs | head -20
```

- [ ] **Step 3.2: Add failing tests**

Append to `crates/sid-widgets/tests/workspaces_actions.rs`:

```rust
use crossterm::event::{KeyCode, KeyModifiers};
use sid_core::context::WidgetCtx;
use sid_core::event::{Event, KeyChord};
use sid_core::widget::Widget;
use sid_core::workspace_metadata::WorkspaceKind;
use sid_store::Workspace;
use std::path::PathBuf;
use sid_widgets::WorkspacesWidget;

fn repo(path: &str, name: &str) -> Workspace {
    Workspace {
        path: PathBuf::from(path),
        name: name.into(),
        kind: WorkspaceKind::Repo,
        manifest_hash: 0,
        last_seen: 0,
        parent: None,
    }
}

fn umbrella(path: &str, name: &str) -> Workspace {
    Workspace {
        path: PathBuf::from(path),
        name: name.into(),
        kind: WorkspaceKind::Umbrella,
        manifest_hash: 0,
        last_seen: 0,
        parent: None,
    }
}

fn make_ctx() -> (WidgetCtx, tokio::sync::mpsc::UnboundedReceiver<sid_core::action::ActionId>) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    (WidgetCtx::new(tx), rx)
}

#[test]
fn enter_on_repo_emits_open_detail_action() {
    let mut w = WorkspacesWidget::new(vec![repo("/vcs/eggsight-stack", "eggsight-stack")], None);
    let (mut ctx, mut rx) = make_ctx();
    let ev = Event::Key(KeyChord::new(KeyCode::Enter, KeyModifiers::NONE));
    let _ = w.handle_event(&ev, &mut ctx);
    let action = rx.try_recv().expect("expected an action to be emitted");
    assert_eq!(action.as_str(), "workspaces.open_detail");
}

#[test]
fn enter_on_umbrella_does_not_emit_open_detail() {
    let mut w = WorkspacesWidget::new(
        vec![umbrella("/vcs/monorepo", "monorepo")],
        None,
    );
    let (mut ctx, mut rx) = make_ctx();
    let ev = Event::Key(KeyChord::new(KeyCode::Enter, KeyModifiers::NONE));
    let _ = w.handle_event(&ev, &mut ctx);
    assert!(
        rx.try_recv().is_err(),
        "Enter on umbrella should toggle expand, not emit open_detail"
    );
}

#[test]
fn right_arrow_toggles_umbrella_expansion() {
    let umb = umbrella("/vcs/monorepo", "monorepo");
    let child = Workspace {
        parent: Some(PathBuf::from("/vcs/monorepo")),
        ..repo("/vcs/monorepo/child", "child")
    };
    let mut w = WorkspacesWidget::new(vec![umb, child], None);
    // Initially collapsed: only umbrella is visible.
    assert_eq!(w.state().visible_count(), 1);
    let (mut ctx, _rx) = make_ctx();
    let ev = Event::Key(KeyChord::new(KeyCode::Right, KeyModifiers::NONE));
    let _ = w.handle_event(&ev, &mut ctx);
    // Expanded: umbrella + child.
    assert_eq!(w.state().visible_count(), 2);
}

#[test]
fn left_arrow_collapses_umbrella() {
    let umb = umbrella("/vcs/monorepo", "monorepo");
    let child = Workspace {
        parent: Some(PathBuf::from("/vcs/monorepo")),
        ..repo("/vcs/monorepo/child", "child")
    };
    let mut w = WorkspacesWidget::new(vec![umb, child], None);
    let (mut ctx, _rx) = make_ctx();
    // Expand first.
    let _ = w.handle_event(
        &Event::Key(KeyChord::new(KeyCode::Right, KeyModifiers::NONE)),
        &mut ctx,
    );
    assert_eq!(w.state().visible_count(), 2);
    // Collapse with Left.
    let _ = w.handle_event(
        &Event::Key(KeyChord::new(KeyCode::Left, KeyModifiers::NONE)),
        &mut ctx,
    );
    assert_eq!(w.state().visible_count(), 1);
}
```

- [ ] **Step 3.3: Run tests, verify failure**

```bash
cargo test -p sid-widgets --test workspaces_actions enter_on_repo enter_on_umbrella right_arrow left_arrow
```

Expected: FAIL — Enter still expands (or does nothing); Right/Left don't toggle.

- [ ] **Step 3.4: Update the handle_event branch in `workspaces.rs:1895-1909`**

Replace the `(KeyCode::Enter, KeyModifiers::NONE)` arm in the Tree-focused branch:

```rust
match self.focused_pane {
    WsFocus::Tree => match (chord.code, chord.mods) {
        (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
            self.state.select_next();
            return EventOutcome::Consumed;
        }
        (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
            self.state.select_prev();
            return EventOutcome::Consumed;
        }
        (KeyCode::Right | KeyCode::Char('l'), KeyModifiers::NONE) => {
            // Expand if umbrella; no-op on leaf.
            self.state.toggle_expand_selected();
            return EventOutcome::Consumed;
        }
        (KeyCode::Left | KeyCode::Char('h'), KeyModifiers::NONE) => {
            // Collapse if currently expanded umbrella; no-op otherwise.
            // toggle_expand_selected is idempotent — only collapse when expanded.
            if let Some(ws) = self.state.selected_workspace()
                && ws.kind == WorkspaceKind::Umbrella
                && self.state.is_expanded(&ws.path)
            {
                self.state.toggle_expand_selected();
            }
            return EventOutcome::Consumed;
        }
        (KeyCode::Enter, KeyModifiers::NONE) => {
            // Leaf workspaces emit "open_detail" so the wire layer can
            // build a WorkspaceDetailWidget and push it as a new tab
            // (handled in branch #3). Umbrella workspaces continue to
            // toggle expand for the muscle-memory user.
            match self.state.selected_workspace().map(|w| w.kind) {
                Some(WorkspaceKind::Umbrella) => {
                    self.state.toggle_expand_selected();
                }
                Some(WorkspaceKind::Repo) => {
                    _ctx.emit_action("workspaces.open_detail");
                }
                None => {}
            }
            return EventOutcome::Consumed;
        }
        _ => {}
    },
    WsFocus::SubView => { /* existing — unchanged */ }
}
```

(Use the actual `WidgetCtx` method name confirmed in Step 3.1.)

- [ ] **Step 3.5: Add `WorkspacesState::is_expanded` if it doesn't exist**

```bash
grep -n "fn is_expanded\|pub fn is_expanded" crates/sid-widgets/src/workspaces.rs
```

If absent, add it near the existing `toggle_expand_selected`:

```rust
    /// Whether `path` is currently expanded. Used by callers that want to
    /// collapse only when expanded (e.g., the Left-arrow handler).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::workspaces::WorkspacesState;
    /// let s = WorkspacesState::new(vec![]);
    /// assert!(!s.is_expanded(std::path::Path::new("/nonexistent")));
    /// ```
    pub fn is_expanded(&self, path: &std::path::Path) -> bool {
        self.expanded.contains(path)
    }
```

- [ ] **Step 3.6: Rename `_ctx` to `ctx` in `handle_event`'s signature so the new emit call works**

The current signature in `workspaces.rs:1858` is `fn handle_event(&mut self, ev: &Event, _ctx: &mut WidgetCtx)`. Change the underscore-prefix to remove it:

```rust
fn handle_event(&mut self, ev: &Event, ctx: &mut WidgetCtx) -> EventOutcome {
```

Use `ctx.emit_action("workspaces.open_detail")` (or the equivalent confirmed in Step 3.1).

- [ ] **Step 3.7: Run tests, verify PASS**

```bash
cargo test -p sid-widgets --test workspaces_actions
```

Expected: all four tests PASS.

- [ ] **Step 3.8: Commit Task 3**

```bash
git add crates/sid-widgets/src/workspaces.rs crates/sid-widgets/tests/workspaces_actions.rs
git commit -m "feat(sid-widgets): workspaces — Enter on leaf emits open_detail; Right/Left toggle expand

Enter on a leaf workspace (Repo kind) emits the workspaces.open_detail
action via WidgetCtx; the wire layer will pick this up in branch #3
to push a WorkspaceDetailWidget as a new tab. Enter on an umbrella
keeps the existing toggle-expand behaviour for muscle memory.

Right/l now also toggles umbrella expand (matches tree-style file
pickers); Left/h collapses only when expanded. This is the arrow-key
affordance the user expected from the substrate change in branch #1."
```

---

## Task 4 — Wire layer no-op handler for `workspaces.open_detail` (placeholder)

**Files:**
- Modify: `crates/sid/src/wire.rs` — add a placeholder handler

`★ Insight ─────────────────────────────────────`
This branch doesn't build the detail tab (that's branch #3). But the action will start firing as soon as branch #2 merges. We add a placeholder handler that logs the event and toasts "Detail tab coming soon" so the user sees feedback rather than silence. Branch #3 replaces the placeholder with the real implementation.
`─────────────────────────────────────────────────`

- [ ] **Step 4.1: Find the action dispatch table in `wire.rs`**

```bash
grep -n "dispatch.*action\|fn handle_action\|app.action.*recv\|app.handle_event" crates/sid/src/wire.rs | head -20
```

Locate the place where the binary receives and dispatches actions emitted via `WidgetCtx`. There is likely a `match action.as_str()` block somewhere downstream of `app.handle_event`.

If no central dispatch exists, the binary may rely on `App::run_action` defined in `sid-core/src/app.rs`. In that case, add the placeholder there as a new arm:

In `crates/sid-core/src/app.rs:422`, add an arm:

```rust
"workspaces.open_detail" => {
    // Placeholder — branch #3 replaces this with the real detail-tab
    // open flow. For now: continue and let the binary's wire layer
    // see the action via run_action's return.
    Dispatch::Continue
}
```

If the binary handles this in `wire.rs` instead (more likely — it's where `SidApp` lives), add a placeholder in the dispatch:

```rust
if action.as_str() == "workspaces.open_detail" {
    sid_app.toasts.push_info("Workspace detail tab — coming soon");
}
```

- [ ] **Step 4.2: Add a test that the action is consumed cleanly**

Confirm that running the binary with a single workspace registered, pressing Enter, does NOT crash. This is hard to automate without a full TUI; instead, add a unit test on the `WorkspacesWidget` itself that asserts `EventOutcome::Consumed` and a clean action emit. The test in Task 3.2 already covers this.

- [ ] **Step 4.3: Commit Task 4**

```bash
git add crates/sid/src/wire.rs crates/sid-core/src/app.rs
git commit -m "feat(sid): placeholder dispatch for workspaces.open_detail action

Branch #3 will replace the placeholder with the actual detail-tab
build + push_detail flow. For now: toast \"coming soon\" so users
see feedback when they press Enter, instead of silent acceptance."
```

---

## Task 5 — Criterion bench: `bench_workspaces_visible_workspaces_for_n`

**Files:**
- Create: `crates/sid-widgets/benches/visible_workspaces.rs`
- Modify: `crates/sid-widgets/Cargo.toml`

`★ Insight ─────────────────────────────────────`
`WorkspacesState::visible_workspaces` allocates a new `Vec<&Workspace>` on every call, and the render path may call it several times per frame (once for the tree, once for selection, once for hint). Benchmarking at n=5/50/500 catches O(n²) regressions if anyone later decides to "optimize" with `iter().filter` chains that allocate intermediate Vecs.
`─────────────────────────────────────────────────`

- [ ] **Step 5.1: Declare the bench**

Append to `crates/sid-widgets/Cargo.toml`:

```toml
[[bench]]
name = "visible_workspaces"
harness = false
```

- [ ] **Step 5.2: Write the bench**

Create `crates/sid-widgets/benches/visible_workspaces.rs`:

```rust
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use sid_core::workspace_metadata::WorkspaceKind;
use sid_store::Workspace;
use sid_widgets::workspaces::WorkspacesState;
use std::path::PathBuf;

fn make_workspaces(n: usize) -> Vec<Workspace> {
    (0..n)
        .map(|i| Workspace {
            path: PathBuf::from(format!("/vcs/repo_{i}")),
            name: format!("repo_{i}"),
            kind: WorkspaceKind::Repo,
            manifest_hash: 0,
            last_seen: 0,
            parent: None,
        })
        .collect()
}

fn bench_visible_workspaces(c: &mut Criterion) {
    let mut group = c.benchmark_group("visible_workspaces");
    for n in [5usize, 50, 500] {
        let ws = make_workspaces(n);
        let state = WorkspacesState::new(ws);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let v = state.visible_workspaces();
                black_box(v.len())
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_visible_workspaces);
criterion_main!(benches);
```

- [ ] **Step 5.3: Run the bench, confirm budget**

```bash
cargo bench -p sid-widgets --bench visible_workspaces
```

Budget per the spec: ≤ 100 µs at n=500. Confirm.

- [ ] **Step 5.4: Save the baseline**

```bash
cargo bench -p sid-widgets --bench visible_workspaces -- --save-baseline main
```

- [ ] **Step 5.5: Commit Task 5**

```bash
git add crates/sid-widgets/Cargo.toml crates/sid-widgets/benches/visible_workspaces.rs
git commit -m "perf(sid-widgets): criterion bench for WorkspacesState::visible_workspaces

100 µs budget at n=500 per the interaction spec. The function is
called multiple times per frame from the render path; gating on this
budget keeps tab-switch hesitation from creeping in if anyone later
'optimizes' with allocation-heavy iterator chains."
```

---

## Task 6 — Adversarial: workspace with non-existent path still emits cleanly

**Files:**
- Modify: `crates/sid-widgets/tests/workspaces_actions.rs`

- [ ] **Step 6.1: Add the adversarial test**

Append to `crates/sid-widgets/tests/workspaces_actions.rs`:

```rust
#[test]
fn enter_on_workspace_with_missing_path_still_emits() {
    let missing = repo("/nonexistent/path/that/does/not/exist", "ghost");
    let mut w = WorkspacesWidget::new(vec![missing], None);
    let (mut ctx, mut rx) = make_ctx();
    let _ = w.handle_event(
        &Event::Key(KeyChord::new(KeyCode::Enter, KeyModifiers::NONE)),
        &mut ctx,
    );
    let action = rx.try_recv().expect("Enter must emit even on missing-path workspaces");
    assert_eq!(action.as_str(), "workspaces.open_detail");
    // The widget itself does not check existence; that's branch #3's job
    // (it renders the "(path missing — press R to remove)" empty state).
}
```

- [ ] **Step 6.2: Run, verify PASS**

```bash
cargo test -p sid-widgets --test workspaces_actions enter_on_workspace_with_missing
```

Expected: PASS.

- [ ] **Step 6.3: Commit Task 6**

```bash
git add crates/sid-widgets/tests/workspaces_actions.rs
git commit -m "test(sid-widgets): adversarial — Enter on missing-path workspace still emits

Locks in that the widget does not check FS existence at emit time;
that's the detail-widget's job. Prevents a future 'helpful' guard
from regressing the action contract."
```

---

## Task 7 — Migration sanity: existing auto-discovered workspaces survive the upgrade

**Files:**
- Modify: `crates/sid-store/tests/workspaces.rs` OR create a new integration test

- [ ] **Step 7.1: Add the migration test**

Append to an existing `crates/sid-store/tests/workspaces.rs` (or create a new file `crates/sid-store/tests/migration_v2_workspaces.rs`):

```rust
use sid_core::workspace_metadata::WorkspaceKind;
use sid_store::{OpenStore, RedbStore, Store, Workspace};
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn pre_existing_workspaces_survive_branch_2_upgrade() {
    // Simulate a user who had auto-discovered workspaces persisted from
    // a pre-branch-#2 sid run. They should still be there after the
    // upgrade.
    let tmp = TempDir::new().unwrap();
    let db = tmp.path().join("sid.redb");
    {
        let store = RedbStore::open(&db).unwrap();
        let w = Workspace {
            path: PathBuf::from("/vcs/auto-discovered-repo"),
            name: "auto-discovered-repo".into(),
            kind: WorkspaceKind::Repo,
            manifest_hash: 0,
            last_seen: 1_000_000,
            parent: None,
        };
        store.upsert_workspace(&w).unwrap();
    }
    // Reopen — the new sid version starts without calling startup_discover.
    let store = RedbStore::open(&db).unwrap();
    let list = store.list_workspaces().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "auto-discovered-repo");
}
```

- [ ] **Step 7.2: Run, verify PASS**

```bash
cargo test -p sid-store pre_existing_workspaces_survive
```

Expected: PASS.

- [ ] **Step 7.3: Commit Task 7**

```bash
git add crates/sid-store/tests/
git commit -m "test(sid-store): pre-existing auto-discovered workspaces survive branch #2 upgrade

Confirms the store is not touched by the discovery removal. Users
who had repos auto-registered will keep seeing them; they just won't
get NEW ones added on launch."
```

---

## Task 8 — Workspace-wide gate

- [ ] **Step 8.1: Run the full sid-gate**

```bash
/sid-gate
```

Expected: green.

- [ ] **Step 8.2: Run criterion benches and confirm budgets**

```bash
cargo bench -p sid-widgets --bench visible_workspaces
cargo bench -p sid-widgets --bench tab_switch
cargo bench -p sid-core --bench handle_event
```

Compare against the baselines saved in branch #1. `/sid-perf-check` automates this:

```bash
/sid-perf-check
```

Expected: no regression ≥ 10% on any bench.

- [ ] **Step 8.3: Merge to main**

```bash
git checkout main
git merge --no-ff feat/workspace-overview-self-defined -m "Merge branch #2: workspace overview self-defined (no auto-discovery)"
```

---

## Definition of done

- [x] Launching sid against a fresh store shows zero workspaces.
- [x] Empty workspace list renders the "press N to add one" hint.
- [x] `Enter` on a leaf workspace emits `workspaces.open_detail`; on an umbrella, toggles expand.
- [x] `Right`/`l` expands umbrella; `Left`/`h` collapses.
- [x] Existing user data in the store is untouched (migration test passes).
- [x] Criterion bench for `visible_workspaces` saved at budget ≤ 100 µs (n=500).
- [x] `/sid-gate` clean; `/sid-perf-check` no regressions.
- [x] Branch merged.

## Risks and rollback

- Users who relied on `~/vcs/` auto-discovery will see an empty Workspaces tab on first launch after upgrade. The empty-state hint mitigates this. If frequent complaints come in, branch #2 follow-up (the `workspaces.scan_now` palette action) jumps to high priority.
- The `--skip-discovery` CLI flag is now a no-op. Scripts that pass it will still work; the flag is removed in a subsequent release after one cycle.
- `Right`/`Left` on umbrellas is a new affordance. Users who had `Enter` muscle memory for expand still work (Enter on umbrella keeps the old behaviour).

## Branch dependencies satisfied

After this merge, branch #3 (`feat/workspace-detail-as-tab`) can begin. The `workspaces.open_detail` action is firing; branch #3's job is to act on it.
