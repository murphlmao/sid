# Branch #1 — Modal substrate + keybind fallbacks + dynamic-tab API

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every popup in sid keyboard-navigable (arrow keys + Left/Right cycle), make Ctrl-keybinds fall back to Alt on terminals that swallow them, and add a dynamic-tab API to `TabManager` so workspaces can open as closable tabs. This branch unblocks plans #2–#5.

**Architecture:** Three coordinated changes in two crates. (1) `sid-widgets::modal::route_key_to_modal` learns arrow keys and `Left/Right` cycle delegating to new typed mutators on `ModalSpec`. (2) `sid-core::keybind::cosmos_default` gets Alt-modifier alternates for the chords terminals don't deliver, plus a new `tab.close` action. (3) `sid-core::tab::Tab` grows a `TabKind { Core, Detail }` discriminator and `TabManager` grows `push_detail` / `close_active` / `detail_count`. All three land in the same branch because the `Tab` field addition is a breaking change downstream.

**Tech Stack:** Rust 2024 edition, ratatui (rendering only), crossterm (key types), proptest (property tests), insta (snapshot tests), criterion (benchmarks).

**Branch:** `feat/modal-arrows-and-keybind-fallbacks`

**Spec reference:** [`docs/superpowers/specs/2026-05-22-tui-ux-interaction-design.md`](../specs/2026-05-22-tui-ux-interaction-design.md) §§ 5.1, 5.2, 5.3, 6.

---

## File map

| File | Purpose | Action |
|---|---|---|
| `crates/sid-widgets/src/modal.rs` | substrate; route_key_to_modal + ModalSpec mutators + renderer | Modify (lines 312-403 add `cycle_focused_value`; 537-580 replace match; 851-926 add hint glyph) |
| `crates/sid-widgets/tests/modal.rs` | integration tests for substrate | Modify (add ~20 tests) |
| `crates/sid-widgets/benches/modal_route.rs` | criterion bench for route_key_to_modal hot path | Create |
| `crates/sid-core/src/keybind.rs` | cosmos_default — add Alt fallbacks + `tab.close` | Modify (lines 232-288) |
| `crates/sid-core/src/app.rs` | `tab.close` action arm in `run_action` | Modify (lines 422-461) |
| `crates/sid-core/tests/app_dispatch.rs` | dispatch tests for `tab.close` | Modify |
| `crates/sid-core/tests/keybind.rs` | keybind tests for Alt fallback | Modify |
| `crates/sid-core/src/tab.rs` | TabKind, Tab.kind, push_detail, close_active, detail_count | Modify (lines 60-211) |
| `crates/sid-core/tests/tab_manager.rs` | TabManager tests for dynamic tabs | Modify (add ~15 tests) |
| `crates/sid-core/benches/handle_event.rs` | bench_app_handle_event_noop | Create |
| `crates/sid-core/Cargo.toml` | declare bench harness | Modify ([[bench]] section) |
| `crates/sid-widgets/Cargo.toml` | declare bench harness | Modify ([[bench]] section) |
| `crates/sid/src/wire.rs` | callers of `Tab { ... }` literal updated for `kind` field | Modify (lines 695-717, 748-753) |
| `docs/DEVELOPMENT.md` | how to run a dhat profiling session | Modify (append section) |

---

## Task 1 — TabManager: introduce `TabKind` and dynamic-tab API

**Files:**
- Modify: `crates/sid-core/src/tab.rs`
- Test: `crates/sid-core/tests/tab_manager.rs`
- Modify (downstream): `crates/sid/src/wire.rs:695-753`

`★ Insight ─────────────────────────────────────`
Adding a field to a public struct (`Tab`) is a breaking change for anyone constructing the struct outside the crate. We mitigate by landing every downstream caller in the same commit (the binary at `wire.rs:695-717` is the only one today). We do NOT introduce a builder pattern — it's overengineering for a struct with five fields used by one binary.
`─────────────────────────────────────────────────`

- [ ] **Step 1.1: Add failing test for `push_detail` rejection of `Core` kind**

Append to `crates/sid-core/tests/tab_manager.rs`:

```rust
use sid_core::tab::{Tab, TabId, TabKind, TabManager};
use sid_core::layout::Layout;
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
use sid_core::event::Event;
use sid_core::context::WidgetCtx;

struct Stub { id: WidgetId }
impl Widget for Stub {
    fn id(&self) -> &WidgetId { &self.id }
    fn title(&self) -> &str { "stub" }
    fn render(&self, _: &mut dyn RenderTarget) {}
    fn handle_event(&mut self, _: &Event, _: &mut WidgetCtx) -> EventOutcome { EventOutcome::Bubble }
    fn as_any(&self) -> &dyn std::any::Any { self }
}

fn core_tab(id: &str) -> Tab {
    Tab {
        id: TabId::new(id),
        title: id.into(),
        layout: Layout::Single(Box::new(Stub { id: WidgetId::new(id) })),
        hotkey: None,
        kind: TabKind::Core,
    }
}

fn detail_tab(id: &str, parent_idx: usize) -> Tab {
    Tab {
        id: TabId::new(id),
        title: id.into(),
        layout: Layout::Single(Box::new(Stub { id: WidgetId::new(id) })),
        hotkey: None,
        kind: TabKind::Detail { parent_idx },
    }
}

#[test]
fn push_detail_rejects_core_kind() {
    let mut mgr = TabManager::new(vec![core_tab("a")]);
    let bad = core_tab("b");
    let err = mgr.push_detail(bad).expect_err("must reject core kind");
    assert!(format!("{err}").contains("Detail"));
    assert_eq!(mgr.tabs().len(), 1);
}
```

- [ ] **Step 1.2: Run the test, verify it fails to compile (TabKind not yet defined)**

```bash
cargo test -p sid-core --test tab_manager push_detail_rejects_core_kind 2>&1 | head -20
```

Expected: error[E0432] `unresolved import sid_core::tab::TabKind` or similar.

- [ ] **Step 1.3: Add `TabKind` enum and the `kind` field to `Tab`**

Edit `crates/sid-core/src/tab.rs` around line 59 (the `Tab` struct):

```rust
/// Kind of tab — distinguishes pinned cockpit tabs from dynamically-opened
/// detail tabs.
///
/// # Examples
///
/// ```
/// use sid_core::tab::TabKind;
///
/// assert_eq!(TabKind::Core, TabKind::Core);
/// assert_ne!(TabKind::Core, TabKind::Detail { parent_idx: 0 });
/// ```
#[derive(Clone, Eq, PartialEq, Debug)]
pub enum TabKind {
    /// One of the six pinned cockpit tabs. Cannot be closed at runtime.
    Core,
    /// Dynamically opened, closable. Carries the index of the core tab
    /// that spawned it so `close_active` can snap focus back there.
    Detail {
        /// Index of the spawning core tab (always `< 6` in v1).
        parent_idx: usize,
    },
}

/// A single tab in the sid cockpit. Each tab owns a [`Layout`] of widgets.
///
/// # Examples
///
/// ```
/// use sid_core::tab::{Tab, TabId, TabKind};
/// use sid_core::layout::Layout;
/// # use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
/// # use sid_core::event::Event;
/// # use sid_core::context::WidgetCtx;
/// # struct W { id: WidgetId }
/// # impl Widget for W {
/// #     fn id(&self) -> &WidgetId { &self.id }
/// #     fn title(&self) -> &str { "t" }
/// #     fn render(&self, _: &mut dyn RenderTarget) {}
/// #     fn handle_event(&mut self, _: &Event, _: &mut WidgetCtx) -> EventOutcome { EventOutcome::Bubble }
/// #     fn as_any(&self) -> &dyn std::any::Any { self }
/// # }
/// let t = Tab {
///     id: TabId::new("workspaces"),
///     title: "Workspaces".into(),
///     layout: Layout::Single(Box::new(W { id: WidgetId::new("w") })),
///     hotkey: Some('1'),
///     kind: TabKind::Core,
/// };
/// assert_eq!(t.kind, TabKind::Core);
/// ```
pub struct Tab {
    pub id: TabId,
    pub title: String,
    pub layout: Layout,
    pub hotkey: Option<char>,
    /// Pinned-vs-dynamic discriminator. Drives `close_active` and
    /// `push_detail` validation.
    pub kind: TabKind,
}
```

- [ ] **Step 1.4: Add `push_detail`, `close_active`, `detail_count` to `TabManager`**

In `crates/sid-core/src/tab.rs`, after the existing `switch_to` method (around line 210), add:

```rust
    /// Push a new detail tab onto the manager. Returns `Err` if `tab.kind`
    /// is `TabKind::Core` — only detail tabs can be added at runtime; the
    /// six cores are fixed by `new`.
    ///
    /// On success the new tab is appended at the end of the tab list. The
    /// active tab is *not* changed; callers that want to focus the new tab
    /// must call `switch_to(&tab.id)` afterwards.
    ///
    /// # Examples
    ///
    /// ```
    /// # use sid_core::tab::{Tab, TabId, TabKind, TabManager};
    /// # use sid_core::layout::Layout;
    /// # use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
    /// # use sid_core::event::Event;
    /// # use sid_core::context::WidgetCtx;
    /// # struct W { id: WidgetId }
    /// # impl Widget for W {
    /// #     fn id(&self) -> &WidgetId { &self.id }
    /// #     fn title(&self) -> &str { "t" }
    /// #     fn render(&self, _: &mut dyn RenderTarget) {}
    /// #     fn handle_event(&mut self, _: &Event, _: &mut WidgetCtx) -> EventOutcome { EventOutcome::Bubble }
    /// #     fn as_any(&self) -> &dyn std::any::Any { self }
    /// # }
    /// # fn mk(id: &str, k: TabKind) -> Tab {
    /// #     Tab { id: TabId::new(id), title: id.into(),
    /// #           layout: Layout::Single(Box::new(W { id: WidgetId::new(id) })),
    /// #           hotkey: None, kind: k }
    /// # }
    /// let mut mgr = TabManager::new(vec![mk("a", TabKind::Core)]);
    /// mgr.push_detail(mk("d1", TabKind::Detail { parent_idx: 0 })).unwrap();
    /// assert_eq!(mgr.detail_count(), 1);
    /// assert!(mgr.push_detail(mk("bad", TabKind::Core)).is_err());
    /// ```
    pub fn push_detail(&mut self, tab: Tab) -> Result<(), crate::SidError> {
        match tab.kind {
            TabKind::Detail { .. } => {
                self.tabs.push(tab);
                Ok(())
            }
            TabKind::Core => Err(crate::SidError::InvalidArgument(
                "push_detail rejects TabKind::Core — only detail tabs can be added at runtime"
                    .into(),
            )),
        }
    }

    /// Close the active tab if it is a `Detail`. Returns `true` when a tab
    /// was actually removed. Snaps `active_idx` back to the saved
    /// `parent_idx` of the closed tab.
    ///
    /// No-op (and returns `false`) when the active tab is `Core`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use sid_core::tab::{Tab, TabId, TabKind, TabManager};
    /// # use sid_core::layout::Layout;
    /// # use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
    /// # use sid_core::event::Event;
    /// # use sid_core::context::WidgetCtx;
    /// # struct W { id: WidgetId }
    /// # impl Widget for W {
    /// #     fn id(&self) -> &WidgetId { &self.id }
    /// #     fn title(&self) -> &str { "t" }
    /// #     fn render(&self, _: &mut dyn RenderTarget) {}
    /// #     fn handle_event(&mut self, _: &Event, _: &mut WidgetCtx) -> EventOutcome { EventOutcome::Bubble }
    /// #     fn as_any(&self) -> &dyn std::any::Any { self }
    /// # }
    /// # fn mk(id: &str, k: TabKind) -> Tab {
    /// #     Tab { id: TabId::new(id), title: id.into(),
    /// #           layout: Layout::Single(Box::new(W { id: WidgetId::new(id) })),
    /// #           hotkey: None, kind: k }
    /// # }
    /// let mut mgr = TabManager::new(vec![mk("ws", TabKind::Core)]);
    /// mgr.push_detail(mk("d1", TabKind::Detail { parent_idx: 0 })).unwrap();
    /// mgr.switch_to(&TabId::new("d1"));
    /// assert_eq!(mgr.active().id.as_str(), "d1");
    /// assert!(mgr.close_active());
    /// assert_eq!(mgr.active().id.as_str(), "ws");
    /// // Closing again on a Core tab is a no-op.
    /// assert!(!mgr.close_active());
    /// ```
    pub fn close_active(&mut self) -> bool {
        let parent_idx = match &self.tabs[self.active_idx].kind {
            TabKind::Core => return false,
            TabKind::Detail { parent_idx } => *parent_idx,
        };
        self.tabs.remove(self.active_idx);
        self.active_idx = parent_idx.min(self.tabs.len().saturating_sub(1));
        true
    }

    /// Number of detail (closable) tabs currently in the manager.
    ///
    /// # Examples
    ///
    /// ```
    /// # use sid_core::tab::{Tab, TabId, TabKind, TabManager};
    /// # use sid_core::layout::Layout;
    /// # use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
    /// # use sid_core::event::Event;
    /// # use sid_core::context::WidgetCtx;
    /// # struct W { id: WidgetId }
    /// # impl Widget for W {
    /// #     fn id(&self) -> &WidgetId { &self.id }
    /// #     fn title(&self) -> &str { "t" }
    /// #     fn render(&self, _: &mut dyn RenderTarget) {}
    /// #     fn handle_event(&mut self, _: &Event, _: &mut WidgetCtx) -> EventOutcome { EventOutcome::Bubble }
    /// #     fn as_any(&self) -> &dyn std::any::Any { self }
    /// # }
    /// # fn mk(id: &str, k: TabKind) -> Tab {
    /// #     Tab { id: TabId::new(id), title: id.into(),
    /// #           layout: Layout::Single(Box::new(W { id: WidgetId::new(id) })),
    /// #           hotkey: None, kind: k }
    /// # }
    /// let mgr = TabManager::new(vec![mk("a", TabKind::Core)]);
    /// assert_eq!(mgr.detail_count(), 0);
    /// ```
    pub fn detail_count(&self) -> usize {
        self.tabs
            .iter()
            .filter(|t| matches!(t.kind, TabKind::Detail { .. }))
            .count()
    }
```

- [ ] **Step 1.5: Update the binary's `Tab { ... }` literals at `crates/sid/src/wire.rs:748-754` and `695-717`**

Find the `fn tab(...)` helper at `crates/sid/src/wire.rs:748`:

```rust
fn tab(id: &str, title: &str, widget: Box<dyn Widget>, hotkey: Option<char>) -> Tab {
    Tab {
        id: TabId::new(id),
        title: title.to_string(),
        layout: Layout::Single(widget),
        hotkey,
        kind: TabKind::Core,
    }
}
```

Add `use sid_core::tab::TabKind;` at the existing `use sid_core::tab::{Tab, TabId, TabManager};` line.

- [ ] **Step 1.6: Run the test and verify it now passes**

```bash
cargo test -p sid-core --test tab_manager push_detail_rejects_core_kind
```

Expected: PASS.

- [ ] **Step 1.7: Add unit tests for `close_active` and `detail_count`**

Append to `crates/sid-core/tests/tab_manager.rs`:

```rust
#[test]
fn close_active_returns_false_on_core() {
    let mut mgr = TabManager::new(vec![core_tab("a"), core_tab("b")]);
    assert!(!mgr.close_active());
    assert_eq!(mgr.tabs().len(), 2);
}

#[test]
fn close_active_removes_detail_and_snaps_to_parent() {
    let mut mgr = TabManager::new(vec![core_tab("workspaces"), core_tab("ssh")]);
    mgr.push_detail(detail_tab("eggsight-stack", 0)).unwrap();
    mgr.switch_to(&TabId::new("eggsight-stack"));
    assert_eq!(mgr.active().id.as_str(), "eggsight-stack");
    assert!(mgr.close_active());
    assert_eq!(mgr.active().id.as_str(), "workspaces");
    assert_eq!(mgr.detail_count(), 0);
}

#[test]
fn detail_count_tracks_pushed_details() {
    let mut mgr = TabManager::new(vec![core_tab("a")]);
    assert_eq!(mgr.detail_count(), 0);
    mgr.push_detail(detail_tab("d1", 0)).unwrap();
    assert_eq!(mgr.detail_count(), 1);
    mgr.push_detail(detail_tab("d2", 0)).unwrap();
    assert_eq!(mgr.detail_count(), 2);
}
```

- [ ] **Step 1.8: Add a property test for arbitrary push/close sequences**

Append to `crates/sid-core/tests/tab_manager.rs`:

```rust
use proptest::prelude::*;

#[derive(Clone, Debug)]
enum Op { Push, CloseActive, Switch(usize) }

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        Just(Op::Push),
        Just(Op::CloseActive),
        (0usize..10).prop_map(Op::Switch),
    ]
}

proptest! {
    #[test]
    fn arbitrary_push_close_keeps_invariants(ops in prop::collection::vec(op_strategy(), 0..50)) {
        let mut mgr = TabManager::new(vec![core_tab("ws"), core_tab("ssh")]);
        let mut counter = 0u32;
        for op in ops {
            match op {
                Op::Push => {
                    let name = format!("d{counter}");
                    counter += 1;
                    mgr.push_detail(detail_tab(&name, 0)).unwrap();
                }
                Op::CloseActive => { mgr.close_active(); }
                Op::Switch(i) => { mgr.jump(i); }
            }
            // Invariants:
            prop_assert!(mgr.active_index() < mgr.tabs().len(), "active idx out of range");
            prop_assert!(mgr.tabs().iter().take(2).all(|t| t.kind == TabKind::Core),
                "first two tabs must remain Core");
            prop_assert_eq!(
                mgr.detail_count() + 2,
                mgr.tabs().len(),
                "core + detail counts must sum to total"
            );
        }
    }
}
```

- [ ] **Step 1.9: Run all `tab_manager` tests**

```bash
cargo test -p sid-core --test tab_manager
```

Expected: PASS.

- [ ] **Step 1.10: Commit Task 1**

```bash
git add crates/sid-core/src/tab.rs crates/sid-core/tests/tab_manager.rs crates/sid/src/wire.rs
git commit -m "feat(sid-core,sid): TabKind + dynamic-tab API (push_detail / close_active)

Extends Tab with a TabKind { Core, Detail { parent_idx } } discriminator
so detail tabs (workspace dashboards, ad-hoc views) can be added and
closed at runtime while the six cockpit tabs stay pinned.

TabManager grows three methods:
- push_detail(tab) — rejects Core kind, appends Detail
- close_active() — removes the active tab when Detail; no-op on Core;
  snaps active_idx back to parent_idx on success
- detail_count() — convenience for tests/assertions

Tests: unit + property test (arbitrary push/close sequences) +
doc tests on every new pub item.

Why: branch #1 substrate for tab.close action (this commit) and the
WorkspaceDetailWidget (branch #3, depends on this API)."
```

---

## Task 2 — `tab.close` action wired into `App::run_action`

**Files:**
- Modify: `crates/sid-core/src/app.rs` (lines 422-461)
- Test: `crates/sid-core/tests/app_dispatch.rs`

- [ ] **Step 2.1: Add failing test for `tab.close` action dispatch**

Append to `crates/sid-core/tests/app_dispatch.rs`:

```rust
use sid_core::action::{ActionId, ActionRegistry};
use sid_core::app::App;
use sid_core::keybind::KeybindMap;
use sid_core::tab::{Tab, TabId, TabKind, TabManager};
use sid_core::layout::Layout;
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};
use sid_core::event::Event;
use sid_core::context::WidgetCtx;

struct Stub { id: WidgetId }
impl Widget for Stub {
    fn id(&self) -> &WidgetId { &self.id }
    fn title(&self) -> &str { "stub" }
    fn render(&self, _: &mut dyn RenderTarget) {}
    fn handle_event(&mut self, _: &Event, _: &mut WidgetCtx) -> EventOutcome { EventOutcome::Bubble }
    fn as_any(&self) -> &dyn std::any::Any { self }
}

fn core(id: &str) -> Tab {
    Tab {
        id: TabId::new(id),
        title: id.into(),
        layout: Layout::Single(Box::new(Stub { id: WidgetId::new(id) })),
        hotkey: None,
        kind: TabKind::Core,
    }
}

fn detail(id: &str, parent_idx: usize) -> Tab {
    Tab {
        id: TabId::new(id),
        title: id.into(),
        layout: Layout::Single(Box::new(Stub { id: WidgetId::new(id) })),
        hotkey: None,
        kind: TabKind::Detail { parent_idx },
    }
}

#[test]
fn tab_close_action_drops_detail_tab() {
    let tabs = TabManager::new(vec![core("workspaces")]);
    let mut app = App::new(tabs, KeybindMap::cosmos_default(), ActionRegistry::new());
    app.tabs_mut().push_detail(detail("d1", 0)).unwrap();
    app.tabs_mut().switch_to(&TabId::new("d1"));
    assert_eq!(app.tabs().active().id.as_str(), "d1");
    app.run_action(&ActionId::new("tab.close"));
    assert_eq!(app.tabs().active().id.as_str(), "workspaces");
    assert_eq!(app.tabs().detail_count(), 0);
}

#[test]
fn tab_close_action_on_core_is_noop() {
    let tabs = TabManager::new(vec![core("workspaces"), core("ssh")]);
    let mut app = App::new(tabs, KeybindMap::cosmos_default(), ActionRegistry::new());
    app.run_action(&ActionId::new("tab.close"));
    assert_eq!(app.tabs().tabs().len(), 2);
    assert_eq!(app.tabs().active().id.as_str(), "workspaces");
}
```

- [ ] **Step 2.2: Run test, verify failure**

```bash
cargo test -p sid-core --test app_dispatch tab_close
```

Expected: FAIL — unknown action `tab.close`, no removal.

- [ ] **Step 2.3: Add `"tab.close"` arm to `App::run_action`**

In `crates/sid-core/src/app.rs:422` (match block in `run_action`), replace the `"tab.detach" | "tab.attach" | "tab.reload"` arm:

```rust
"tab.close" => {
    let _ = self.tabs.close_active();
    Dispatch::Continue
}
// No-ops in Plan 1; implemented in Plan 8.
"tab.detach" | "tab.attach" | "tab.reload" => Dispatch::Continue,
```

- [ ] **Step 2.4: Run tests, verify PASS**

```bash
cargo test -p sid-core --test app_dispatch tab_close
```

Expected: PASS for both tests.

- [ ] **Step 2.5: Commit Task 2**

```bash
git add crates/sid-core/src/app.rs crates/sid-core/tests/app_dispatch.rs
git commit -m "feat(sid-core): tab.close action wired into App::run_action

Dispatches the new tab.close action by calling TabManager::close_active.
No-op on Core tabs (returns Continue regardless). Tests cover the
detail-close path and the core-no-op path."
```

---

## Task 3 — Keybind fallbacks (Alt for tabs.jump.N, app.open_settings, tab.close)

**Files:**
- Modify: `crates/sid-core/src/keybind.rs:232-288`
- Test: `crates/sid-core/tests/keybind.rs`

`★ Insight ─────────────────────────────────────`
On most Linux terminals, `Ctrl+digit` and `Ctrl+,` have no distinct ASCII control byte, so crossterm never sees the chord. Modern terminals can opt into the kitty keyboard protocol via `CSI > 1 u` and deliver them, but that's terminal-specific. Adding `Alt+digit` / `Alt+,` as alternates gives sid a universally-working chord without forcing a protocol negotiation at startup.
`─────────────────────────────────────────────────`

- [ ] **Step 3.1: Add failing tests for Alt fallbacks**

Append to `crates/sid-core/tests/keybind.rs`:

```rust
use crossterm::event::{KeyCode, KeyModifiers};
use sid_core::event::KeyChord;
use sid_core::keybind::KeybindMap;

#[test]
fn alt_digit_maps_to_tabs_jump() {
    let m = KeybindMap::cosmos_default();
    for i in 1..=6 {
        let c = char::from_digit(i as u32, 10).unwrap();
        let chord = KeyChord::new(KeyCode::Char(c), KeyModifiers::ALT);
        let action = m.lookup(&chord).expect("Alt+digit must be bound");
        assert_eq!(action.as_str(), &format!("tabs.jump.{i}"));
    }
}

#[test]
fn alt_comma_opens_settings() {
    let m = KeybindMap::cosmos_default();
    let chord = KeyChord::new(KeyCode::Char(','), KeyModifiers::ALT);
    assert_eq!(
        m.lookup(&chord).map(|a| a.as_str().to_string()),
        Some("app.open_settings".to_string()),
    );
}

#[test]
fn ctrl_w_and_alt_w_both_close_tab() {
    let m = KeybindMap::cosmos_default();
    let c = KeyChord::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
    let a = KeyChord::new(KeyCode::Char('w'), KeyModifiers::ALT);
    assert_eq!(m.lookup(&c).map(|x| x.as_str().to_string()), Some("tab.close".into()));
    assert_eq!(m.lookup(&a).map(|x| x.as_str().to_string()), Some("tab.close".into()));
}

#[test]
fn ctrl_digit_remains_bound_for_kitty_protocol_terminals() {
    let m = KeybindMap::cosmos_default();
    let c1 = KeyChord::new(KeyCode::Char('1'), KeyModifiers::CONTROL);
    assert_eq!(
        m.lookup(&c1).map(|a| a.as_str().to_string()),
        Some("tabs.jump.1".to_string()),
    );
}
```

- [ ] **Step 3.2: Run tests, verify failure**

```bash
cargo test -p sid-core --test keybind alt_digit alt_comma ctrl_w
```

Expected: FAIL — Alt fallbacks not bound, `tab.close` not bound.

- [ ] **Step 3.3: Add Alt fallbacks and `tab.close` in `cosmos_default`**

In `crates/sid-core/src/keybind.rs:232`, inside `cosmos_default` after the existing `for i in 1..=6` loop, modify to:

```rust
        for i in 1..=6 {
            let c = char::from_digit(i, 10).unwrap();
            bind(
                &mut m,
                KeyCode::Char(c),
                KeyModifiers::CONTROL,
                &format!("tabs.jump.{i}"),
            );
            // Alt fallback for terminals that don't deliver Ctrl+digit.
            bind(
                &mut m,
                KeyCode::Char(c),
                KeyModifiers::ALT,
                &format!("tabs.jump.{i}"),
            );
        }
```

After the existing `app.open_settings` bind (line 263-268), add:

```rust
        bind(
            &mut m,
            KeyCode::Char(','),
            KeyModifiers::ALT,
            "app.open_settings",
        );
        bind(
            &mut m,
            KeyCode::Char('w'),
            KeyModifiers::CONTROL,
            "tab.close",
        );
        bind(
            &mut m,
            KeyCode::Char('w'),
            KeyModifiers::ALT,
            "tab.close",
        );
```

- [ ] **Step 3.4: Register the `tab.close` action in `ActionRegistry`**

In `crates/sid/src/wire.rs` around line 720, the `for a in [...]` block that registers global action ids — add `"tab.close"` to the list:

```rust
    for a in [
        "app.quit",
        "palette.open",
        "tabs.next",
        "tabs.prev",
        "app.open_settings",
        "tab.close",
        "tab.detach",
        "tab.attach",
        "tab.reload",
    ] {
        reg.register(Action::new(a, pretty_label(a)));
    }
```

- [ ] **Step 3.5: Run tests, verify PASS**

```bash
cargo test -p sid-core --test keybind
cargo test -p sid-core --test app_dispatch
```

Expected: PASS for all new tests and existing tests.

- [ ] **Step 3.6: Commit Task 3**

```bash
git add crates/sid-core/src/keybind.rs crates/sid-core/tests/keybind.rs crates/sid/src/wire.rs
git commit -m "feat(sid-core,sid): Alt-modifier keybind fallbacks + tab.close binding

Most terminals don't deliver Ctrl+digit or Ctrl+, as distinct chords
(no ASCII control byte). Add Alt+digit (1..6) and Alt+, alternates
so the bindings work on every terminal. Existing Ctrl bindings stay
for kitty-protocol-aware terminals.

Also binds Ctrl+W and Alt+W to the new tab.close action and
registers tab.close in the ActionRegistry so the command palette
can surface it."
```

---

## Task 4 — Modal substrate: typed value mutators on `ModalSpec`

**Files:**
- Modify: `crates/sid-widgets/src/modal.rs:312-403` (add `cycle_focused_value`)
- Test: `crates/sid-widgets/tests/modal.rs`

`★ Insight ─────────────────────────────────────`
The skill's "Field methods over direct enum match" pattern lives in `ModalSpec::space_or_enter_on_field` (lines 383-403). We're extending the same pattern — caller dispatches into one method, the method matches on the focused field and does the right thing. Keeps `route_key_to_modal` flat.
`─────────────────────────────────────────────────`

- [ ] **Step 4.1: Add failing tests for `cycle_focused_value`**

Append to `crates/sid-widgets/tests/modal.rs`:

```rust
use sid_widgets::modal::{Field, ModalSpec};

#[test]
fn cycle_focused_value_advances_choice_forward() {
    let mut m = ModalSpec::new("id", "t", vec![Field::Choice {
        label: "c".into(),
        options: vec!["a".into(), "b".into(), "c".into()],
        selected: 0,
    }]);
    m.cycle_focused_value(1);
    if let Field::Choice { selected, .. } = &m.fields[0] {
        assert_eq!(*selected, 1);
    }
}

#[test]
fn cycle_focused_value_choice_wraps_backward() {
    let mut m = ModalSpec::new("id", "t", vec![Field::Choice {
        label: "c".into(),
        options: vec!["a".into(), "b".into(), "c".into()],
        selected: 0,
    }]);
    m.cycle_focused_value(-1);
    if let Field::Choice { selected, .. } = &m.fields[0] {
        assert_eq!(*selected, 2);
    }
}

#[test]
fn cycle_focused_value_toggle_flips() {
    let mut m = ModalSpec::new("id", "t", vec![Field::Toggle {
        label: "on".into(),
        value: false,
    }]);
    m.cycle_focused_value(1);
    if let Field::Toggle { value, .. } = &m.fields[0] {
        assert!(*value);
    }
    m.cycle_focused_value(-1);
    if let Field::Toggle { value, .. } = &m.fields[0] {
        assert!(!*value);
    }
}

#[test]
fn cycle_focused_value_text_is_noop() {
    let mut m = ModalSpec::new("id", "t", vec![Field::Text {
        label: "n".into(),
        value: "hello".into(),
        placeholder: None,
    }]);
    m.cycle_focused_value(1);
    if let Field::Text { value, .. } = &m.fields[0] {
        assert_eq!(value, "hello");
    }
}

#[test]
fn cycle_focused_value_on_empty_modal_is_noop() {
    let mut m = ModalSpec::new("id", "t", vec![]);
    m.cycle_focused_value(1);
    assert!(m.fields.is_empty());
}
```

- [ ] **Step 4.2: Run tests, verify failure**

```bash
cargo test -p sid-widgets --test modal cycle_focused_value
```

Expected: FAIL — `cycle_focused_value` method does not exist.

- [ ] **Step 4.3: Add `Field::U64` variant to support bumpable integers (used in Settings)**

We do NOT add a `U64` field to the Field enum in this branch — only `Choice` and `Toggle` exist today. Bumpable u64 fields are introduced by branch #5 (Settings live-apply) and that branch will add cycle_focused_value support for it. This task keeps the substrate change minimal: Choice + Toggle only.

(No code change for this step — documentation step. Mark complete by acknowledging the boundary.)

- [ ] **Step 4.4: Implement `cycle_focused_value(dir: i8)` on `ModalSpec`**

In `crates/sid-widgets/src/modal.rs` after `space_or_enter_on_field` (around line 403), add:

```rust
    /// Cycle the value of the focused [`Field::Choice`] or flip the focused
    /// [`Field::Toggle`]. `dir > 0` advances forward, `dir < 0` goes
    /// backward, `dir == 0` is a no-op. Non-value fields (Text / Password /
    /// Picker / Display) and an empty modal are also no-ops.
    ///
    /// Routed to from [`route_key_to_modal`] on Left/Right arrow keys.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::modal::{Field, ModalSpec};
    ///
    /// let mut m = ModalSpec::new("id", "t", vec![Field::Choice {
    ///     label: "k".into(),
    ///     options: vec!["a".into(), "b".into()],
    ///     selected: 0,
    /// }]);
    /// m.cycle_focused_value(1);
    /// if let Field::Choice { selected, .. } = &m.fields[0] {
    ///     assert_eq!(*selected, 1);
    /// }
    /// ```
    pub fn cycle_focused_value(&mut self, dir: i8) {
        if dir == 0 {
            return;
        }
        let Some(field) = self.fields.get_mut(self.focus) else {
            return;
        };
        match field {
            Field::Choice {
                options, selected, ..
            } => {
                if options.is_empty() {
                    return;
                }
                let n = options.len();
                let s = *selected;
                *selected = if dir > 0 {
                    (s + 1) % n
                } else {
                    (s + n - 1) % n
                };
            }
            Field::Toggle { value, .. } => {
                *value = !*value;
            }
            Field::Text { .. }
            | Field::Password { .. }
            | Field::Picker { .. }
            | Field::Display { .. } => {}
        }
    }
```

- [ ] **Step 4.5: Run tests, verify PASS**

```bash
cargo test -p sid-widgets --test modal cycle_focused_value
cargo test -p sid-widgets --doc modal::ModalSpec::cycle_focused_value
```

Expected: PASS.

- [ ] **Step 4.6: Add property test for cycle round-trip**

Append to `crates/sid-widgets/tests/modal.rs`:

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn cycle_choice_then_reverse_is_identity(
        n_options in 2usize..10,
        start in 0usize..10,
        steps in 0usize..50,
    ) {
        let start = start % n_options;
        let opts: Vec<String> = (0..n_options).map(|i| format!("o{i}")).collect();
        let mut m = ModalSpec::new("id", "t", vec![Field::Choice {
            label: "k".into(),
            options: opts,
            selected: start,
        }]);
        for _ in 0..steps {
            m.cycle_focused_value(1);
        }
        for _ in 0..steps {
            m.cycle_focused_value(-1);
        }
        if let Field::Choice { selected, .. } = &m.fields[0] {
            prop_assert_eq!(*selected, start);
        }
    }
}
```

- [ ] **Step 4.7: Run property test**

```bash
cargo test -p sid-widgets --test modal cycle_choice_then_reverse_is_identity
```

Expected: PASS.

- [ ] **Step 4.8: Commit Task 4**

```bash
git add crates/sid-widgets/src/modal.rs crates/sid-widgets/tests/modal.rs
git commit -m "feat(sid-widgets): ModalSpec::cycle_focused_value for arrow-key support

Adds the typed value mutator used by Left/Right arrow key routing in
the next commit. Choice cycles with wrap, Toggle flips, value-less
fields are no-ops. Doc test + unit tests + property test for the
round-trip identity (cycle forward N then back N returns to start)."
```

---

## Task 5 — Modal substrate: rewrite `route_key_to_modal` with arrow keys

**Files:**
- Modify: `crates/sid-widgets/src/modal.rs:537-580`
- Test: `crates/sid-widgets/tests/modal.rs`

- [ ] **Step 5.1: Add failing tests for arrow-key routing**

Append to `crates/sid-widgets/tests/modal.rs`:

```rust
use crossterm::event::{KeyCode, KeyModifiers};
use sid_core::event::KeyChord;
use sid_widgets::modal::{route_key_to_modal, ModalKeyOutcome};

fn chord(code: KeyCode, mods: KeyModifiers) -> KeyChord {
    KeyChord { code, mods }
}

#[test]
fn up_cycles_focus_backward() {
    let mut m = ModalSpec::new("id", "t", vec![
        Field::Toggle { label: "a".into(), value: false },
        Field::Toggle { label: "b".into(), value: false },
    ]);
    m.focus = 1;
    let outcome = route_key_to_modal(&mut m, chord(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(outcome, ModalKeyOutcome::Consumed);
    assert_eq!(m.focus, 0);
}

#[test]
fn down_cycles_focus_forward() {
    let mut m = ModalSpec::new("id", "t", vec![
        Field::Toggle { label: "a".into(), value: false },
        Field::Toggle { label: "b".into(), value: false },
    ]);
    let outcome = route_key_to_modal(&mut m, chord(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(outcome, ModalKeyOutcome::Consumed);
    assert_eq!(m.focus, 1);
}

#[test]
fn right_cycles_choice_value() {
    let mut m = ModalSpec::new("id", "t", vec![Field::Choice {
        label: "c".into(),
        options: vec!["a".into(), "b".into(), "c".into()],
        selected: 0,
    }]);
    route_key_to_modal(&mut m, chord(KeyCode::Right, KeyModifiers::NONE));
    if let Field::Choice { selected, .. } = &m.fields[0] {
        assert_eq!(*selected, 1);
    }
}

#[test]
fn left_cycles_choice_value_backward() {
    let mut m = ModalSpec::new("id", "t", vec![Field::Choice {
        label: "c".into(),
        options: vec!["a".into(), "b".into(), "c".into()],
        selected: 0,
    }]);
    route_key_to_modal(&mut m, chord(KeyCode::Left, KeyModifiers::NONE));
    if let Field::Choice { selected, .. } = &m.fields[0] {
        assert_eq!(*selected, 2);
    }
}

#[test]
fn enter_on_choice_now_submits_not_cycles() {
    let mut m = ModalSpec::new("id", "t", vec![Field::Choice {
        label: "c".into(),
        options: vec!["a".into(), "b".into()],
        selected: 0,
    }]);
    let outcome = route_key_to_modal(&mut m, chord(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(outcome, ModalKeyOutcome::Submit);
    if let Field::Choice { selected, .. } = &m.fields[0] {
        // Submission does NOT advance the choice.
        assert_eq!(*selected, 0);
    }
}

#[test]
fn space_on_choice_still_cycles_for_backward_compat() {
    let mut m = ModalSpec::new("id", "t", vec![Field::Choice {
        label: "c".into(),
        options: vec!["a".into(), "b".into()],
        selected: 0,
    }]);
    route_key_to_modal(&mut m, chord(KeyCode::Char(' '), KeyModifiers::NONE));
    if let Field::Choice { selected, .. } = &m.fields[0] {
        assert_eq!(*selected, 1);
    }
}

#[test]
fn shift_tab_cycles_focus_backward() {
    let mut m = ModalSpec::new("id", "t", vec![
        Field::Toggle { label: "a".into(), value: false },
        Field::Toggle { label: "b".into(), value: false },
    ]);
    m.focus = 0;
    route_key_to_modal(&mut m, chord(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(m.focus, 1);
}

#[test]
fn arrow_keys_on_empty_modal_are_noop_not_panic() {
    let mut m = ModalSpec::new("id", "t", vec![]);
    let _ = route_key_to_modal(&mut m, chord(KeyCode::Up, KeyModifiers::NONE));
    let _ = route_key_to_modal(&mut m, chord(KeyCode::Down, KeyModifiers::NONE));
    let _ = route_key_to_modal(&mut m, chord(KeyCode::Left, KeyModifiers::NONE));
    let _ = route_key_to_modal(&mut m, chord(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(m.focus, 0);
}
```

- [ ] **Step 5.2: Run tests, verify failure**

```bash
cargo test -p sid-widgets --test modal up_cycles down_cycles right_cycles left_cycles enter_on_choice space_on_choice shift_tab arrow_keys_on_empty
```

Expected: FAIL — arrow keys not yet routed.

- [ ] **Step 5.3: Replace `route_key_to_modal`**

In `crates/sid-widgets/src/modal.rs:537-580`, replace the function body:

```rust
pub fn route_key_to_modal(
    modal: &mut ModalSpec,
    key: sid_core::event::KeyChord,
) -> ModalKeyOutcome {
    use crossterm::event::{KeyCode, KeyModifiers};
    match (key.code, key.mods) {
        (KeyCode::Esc, _) => ModalKeyOutcome::Cancel,
        (KeyCode::Up, _) | (KeyCode::BackTab, _) => {
            modal.cycle_focus_backward();
            ModalKeyOutcome::Consumed
        }
        (KeyCode::Down, _) => {
            modal.cycle_focus_forward();
            ModalKeyOutcome::Consumed
        }
        (KeyCode::Tab, m) if !m.contains(KeyModifiers::SHIFT) => {
            modal.cycle_focus_forward();
            ModalKeyOutcome::Consumed
        }
        (KeyCode::Tab, _) => {
            modal.cycle_focus_backward();
            ModalKeyOutcome::Consumed
        }
        (KeyCode::Left, _) => {
            modal.cycle_focused_value(-1);
            ModalKeyOutcome::Consumed
        }
        (KeyCode::Right, _) => {
            modal.cycle_focused_value(1);
            ModalKeyOutcome::Consumed
        }
        (KeyCode::Backspace, _) => {
            modal.backspace();
            ModalKeyOutcome::Consumed
        }
        (KeyCode::Enter, _) => ModalKeyOutcome::Submit,
        (KeyCode::Char(' '), _)
            if matches!(
                modal.fields.get(modal.focus),
                Some(Field::Toggle { .. } | Field::Choice { .. })
            ) =>
        {
            modal.space_or_enter_on_field();
            ModalKeyOutcome::Consumed
        }
        (KeyCode::Char(c), m)
            if !m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT) =>
        {
            modal.type_char(c);
            ModalKeyOutcome::Consumed
        }
        _ => ModalKeyOutcome::Consumed,
    }
}
```

Update the doc comment for `route_key_to_modal` (the listing above the function) to mention the new arrow-key behaviour:

```rust
/// Route a single crossterm `KeyEvent` into `modal` and return what the caller
/// should do next.
///
/// - `Esc`                       → Cancel
/// - `Up` / `Shift+Tab`          → cycle focus backward, Consumed
/// - `Down` / `Tab`              → cycle focus forward, Consumed
/// - `Left` / `Right`            → cycle focused Choice/Toggle value, Consumed
/// - `Backspace`                 → backspace on focused text/password/picker, Consumed
/// - `Enter`                     → Submit
/// - `Space` on Toggle / Choice  → flip/cycle the field, Consumed (legacy)
/// - `Char(c)` (no Ctrl/Alt)     → type_char, Consumed
/// - any other key               → Consumed (modal swallows it)
```

- [ ] **Step 5.4: Update the existing test `enter_on_choice` if present and verify all modal tests pass**

```bash
cargo test -p sid-widgets --test modal
```

Expected: ALL PASS. The previously-existing test that relied on Enter cycling a Choice now expects Submit; if any pre-existing test breaks, update it to reflect the new semantics (Enter is always Submit).

- [ ] **Step 5.5: Verify the doc test on `route_key_to_modal` still passes**

```bash
cargo test -p sid-widgets --doc modal::route_key_to_modal
```

Expected: PASS.

- [ ] **Step 5.6: Commit Task 5**

```bash
git add crates/sid-widgets/src/modal.rs crates/sid-widgets/tests/modal.rs
git commit -m "feat(sid-widgets): route_key_to_modal — arrow keys + L/R value cycle

Up / Shift+Tab cycle focus backward; Down / Tab cycle forward.
Left / Right delegate to cycle_focused_value for the focused
Choice/Toggle. Enter always submits (no longer cycles Choice on
Enter — Space keeps the legacy cycle for muscle memory).

This single substrate change unlocks arrow-key navigation in every
modal in sid (~30 callers across workspaces/ssh/database/system/
network). No widget code changes; substrate-only."
```

---

## Task 6 — Modal renderer: show `‹ ›` hint glyph on focused Choice/Toggle

**Files:**
- Modify: `crates/sid-widgets/src/modal.rs:851-926` (`render_field_value`)
- Test: `crates/sid-widgets/tests/modal.rs` (insta snapshot)

`★ Insight ─────────────────────────────────────`
The cycle hint is rendered as a styled span at the end of the focused row, not as a separate UI primitive. This keeps the renderer flat and the snapshot test stable: the hint is just a string concatenation, not a layout choice.
`─────────────────────────────────────────────────`

- [ ] **Step 6.1: Add failing insta snapshot for focused-Choice cycle hint**

Append to `crates/sid-widgets/tests/modal.rs`:

```rust
use sid_widgets::modal::render_modal_to_string;

#[test]
fn focused_choice_renders_cycle_hint() {
    let mut m = ModalSpec::new("id", "Test", vec![Field::Choice {
        label: "action".into(),
        options: vec!["Resume".into(), "Start fresh".into()],
        selected: 0,
    }]);
    m.focus = 0;
    let rendered = render_modal_to_string(&m, 60, 12);
    assert!(
        rendered.contains("‹") && rendered.contains("›"),
        "expected cycle hint glyphs in:\n{rendered}",
    );
}

#[test]
fn unfocused_choice_does_not_render_cycle_hint() {
    let mut m = ModalSpec::new("id", "Test", vec![
        Field::Toggle { label: "first".into(), value: false },
        Field::Choice {
            label: "action".into(),
            options: vec!["Resume".into(), "Start fresh".into()],
            selected: 0,
        },
    ]);
    m.focus = 0; // Toggle focused; Choice unfocused.
    let rendered = render_modal_to_string(&m, 60, 12);
    // Hint should appear on the focused row (Toggle) but the Choice
    // row should NOT carry the hint.
    let choice_line = rendered
        .lines()
        .find(|l| l.contains("Resume"))
        .expect("Choice row should be rendered");
    assert!(
        !choice_line.contains("‹") && !choice_line.contains("›"),
        "unfocused Choice row leaked cycle hint: {choice_line}",
    );
}
```

- [ ] **Step 6.2: Run tests, verify failure**

```bash
cargo test -p sid-widgets --test modal focused_choice_renders unfocused_choice
```

Expected: FAIL — no `‹ ›` glyphs in output.

- [ ] **Step 6.3: Modify `render_field_value` to append the hint on focused Choice/Toggle rows**

In `crates/sid-widgets/src/modal.rs:851-926`, append the hint at the end of the value spans when the field is focused AND a Choice or Toggle:

```rust
fn render_field_value<'a>(theme: &'a Theme, field: &'a Field, focused: bool) -> Line<'a> {
    let prefix = if focused { "> " } else { "  " };
    let mut spans = vec![Span::raw(prefix)];
    match field {
        Field::Text {
            value, placeholder, ..
        } => { /* existing */ }
        Field::Password { value, .. } => { /* existing */ }
        Field::Toggle { value, .. } => {
            let mark = if *value { "[x]" } else { "[ ]" };
            spans.push(Span::styled(
                mark.to_string(),
                Style::default().fg(theme.accent_primary.into()),
            ));
            spans.push(Span::raw(" "));
            spans.push(Span::raw(if *value { "on" } else { "off" }));
            if focused {
                spans.push(Span::raw("   "));
                spans.push(Span::styled(
                    "‹ ›".to_string(),
                    Style::default().fg(theme.muted.into()),
                ));
            }
        }
        Field::Choice {
            options, selected, ..
        } => {
            for (i, opt) in options.iter().enumerate() {
                let glyph = if i == *selected { "(●)" } else { "( )" };
                spans.push(Span::raw(glyph.to_string()));
                spans.push(Span::raw(" "));
                spans.push(Span::raw(opt.clone()));
                if i + 1 < options.len() {
                    spans.push(Span::raw("  "));
                }
            }
            if focused {
                spans.push(Span::raw("   "));
                spans.push(Span::styled(
                    "‹ ›".to_string(),
                    Style::default().fg(theme.muted.into()),
                ));
            }
        }
        Field::Picker { value, hint, .. } => { /* existing */ }
        Field::Display { body, .. } => { /* existing */ }
    }
    Line::from(spans)
}
```

Keep the existing arms for Text/Password/Picker/Display intact — only Toggle and Choice gain the hint.

- [ ] **Step 6.4: Run tests, verify PASS**

```bash
cargo test -p sid-widgets --test modal focused_choice_renders unfocused_choice
```

Expected: PASS.

- [ ] **Step 6.5: Refresh any pre-existing insta snapshots that include modal renderings**

```bash
cargo insta test -p sid-widgets --review
```

Walk through each snapshot diff; if the only change is the new `‹ ›` hint, accept. If a snapshot's expected output didn't include the hint but should now (focused Choice/Toggle in a different test), accept.

- [ ] **Step 6.6: Commit Task 6**

```bash
git add crates/sid-widgets/src/modal.rs crates/sid-widgets/tests/modal.rs crates/sid-widgets/tests/snapshots/
git commit -m "feat(sid-widgets): modal renderer shows ‹ › cycle hint on focused choice/toggle

The hint glyph signals the new Left/Right arrow-key affordance from
the previous commit. Rendered in theme.muted so it doesn't compete
with the value itself. Unfocused rows stay clean."
```

---

## Task 7 — Criterion bench: `bench_app_handle_event_noop`

**Files:**
- Create: `crates/sid-core/benches/handle_event.rs`
- Modify: `crates/sid-core/Cargo.toml` — declare `[[bench]]` entry

`★ Insight ─────────────────────────────────────`
The bench's budget is 1 µs per CLAUDE.md. If this regresses, the entire event loop slows down. We gate on the criterion compare against the saved baseline (CLAUDE.md 10%-regression rule), which is checked by `/sid-perf-check` post-commit.
`─────────────────────────────────────────────────`

- [ ] **Step 7.1: Declare the bench in `Cargo.toml`**

Append to `crates/sid-core/Cargo.toml`:

```toml
[[bench]]
name = "handle_event"
harness = false
```

Confirm `criterion` is already in `[dev-dependencies]` (via workspace inheritance). If not, add:

```toml
[dev-dependencies]
criterion = { workspace = true }
```

- [ ] **Step 7.2: Write the bench**

Create `crates/sid-core/benches/handle_event.rs`:

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use crossterm::event::{KeyCode, KeyModifiers};
use sid_core::action::ActionRegistry;
use sid_core::app::App;
use sid_core::context::WidgetCtx;
use sid_core::event::{Event, KeyChord};
use sid_core::keybind::KeybindMap;
use sid_core::layout::Layout;
use sid_core::tab::{Tab, TabId, TabKind, TabManager};
use sid_core::widget::{EventOutcome, RenderTarget, Widget, WidgetId};

struct Stub {
    id: WidgetId,
}
impl Widget for Stub {
    fn id(&self) -> &WidgetId {
        &self.id
    }
    fn title(&self) -> &str {
        "stub"
    }
    fn render(&self, _: &mut dyn RenderTarget) {}
    fn handle_event(&mut self, _: &Event, _: &mut WidgetCtx) -> EventOutcome {
        EventOutcome::Bubble
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn make_app() -> App {
    let tabs = TabManager::new(vec![Tab {
        id: TabId::new("a"),
        title: "A".into(),
        layout: Layout::Single(Box::new(Stub {
            id: WidgetId::new("w"),
        })),
        hotkey: None,
        kind: TabKind::Core,
    }]);
    App::new(tabs, KeybindMap::cosmos_default(), ActionRegistry::new())
}

fn bench_app_handle_event_noop(c: &mut Criterion) {
    let mut app = make_app();
    // F12 is unbound; the dispatch will fall through to the widget's
    // EventOutcome::Bubble, exercising the full path without touching
    // any persistent state.
    let chord = Event::Key(KeyChord::new(KeyCode::F(12), KeyModifiers::NONE));
    c.bench_function("app_handle_event_noop", |b| {
        b.iter(|| {
            let _ = app.handle_event(black_box(&chord));
        });
    });
}

criterion_group!(benches, bench_app_handle_event_noop);
criterion_main!(benches);
```

- [ ] **Step 7.3: Run the bench once to establish a baseline**

```bash
cargo bench -p sid-core --bench handle_event
```

Expected: completes in a few seconds; reports a `time` like `[800 ns 850 ns 900 ns]`. If it reports > 1 µs, investigate before continuing — the budget per CLAUDE.md is 1 µs.

- [ ] **Step 7.4: Save the baseline so `/sid-perf-check` can compare**

```bash
cargo bench -p sid-core --bench handle_event -- --save-baseline main
```

Expected: writes `target/criterion/.../main/...` snapshot.

- [ ] **Step 7.5: Commit Task 7**

```bash
git add crates/sid-core/Cargo.toml crates/sid-core/benches/handle_event.rs
git commit -m "perf(sid-core): criterion bench for App::handle_event noop path

Establishes the 1 µs baseline mandated by CLAUDE.md for the event
dispatch hot path. Future changes must run /sid-perf-check post-
commit and confirm no ≥10% regression."
```

---

## Task 8 — Criterion bench: `bench_tab_switch_render` per tab

**Files:**
- Create: `crates/sid-widgets/benches/tab_switch.rs`
- Modify: `crates/sid-widgets/Cargo.toml` — declare `[[bench]]` entry

- [ ] **Step 8.1: Declare the bench in `Cargo.toml`**

Append to `crates/sid-widgets/Cargo.toml`:

```toml
[[bench]]
name = "tab_switch"
harness = false
```

- [ ] **Step 8.2: Write the bench**

Create `crates/sid-widgets/benches/tab_switch.rs`:

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use sid_ui::themes::cosmos;
use sid_widgets::{
    DatabaseWidget, NetworkWidget, SettingsWidget, SshWidget, SystemWidget, WorkspacesWidget,
};

fn bench_workspaces_render(c: &mut Criterion) {
    let w = WorkspacesWidget::new(vec![], None);
    let backend = TestBackend::new(120, 40);
    let mut term = Terminal::new(backend).unwrap();
    let theme = cosmos();
    c.bench_function("tab_render_workspaces", |b| {
        b.iter(|| {
            term.draw(|f| w.render_into_frame(f, f.area(), &theme)).unwrap();
            black_box(())
        });
    });
}

fn bench_ssh_render(c: &mut Criterion) {
    let w = SshWidget::default();
    let backend = TestBackend::new(120, 40);
    let mut term = Terminal::new(backend).unwrap();
    let theme = cosmos();
    c.bench_function("tab_render_ssh", |b| {
        b.iter(|| {
            term.draw(|f| w.render_into_frame(f, f.area(), &theme)).unwrap();
        });
    });
}

fn bench_database_render(c: &mut Criterion) {
    let w = DatabaseWidget::new(vec![]);
    let backend = TestBackend::new(120, 40);
    let mut term = Terminal::new(backend).unwrap();
    let theme = cosmos();
    c.bench_function("tab_render_database", |b| {
        b.iter(|| {
            term.draw(|f| w.render_into_frame(f, f.area(), &theme)).unwrap();
        });
    });
}

fn bench_network_render(c: &mut Criterion) {
    let w = NetworkWidget::new();
    let backend = TestBackend::new(120, 40);
    let mut term = Terminal::new(backend).unwrap();
    let theme = cosmos();
    c.bench_function("tab_render_network", |b| {
        b.iter(|| {
            term.draw(|f| w.render_into_frame(f, f.area(), &theme)).unwrap();
        });
    });
}

fn bench_system_render(c: &mut Criterion) {
    let w = SystemWidget::default();
    let backend = TestBackend::new(120, 40);
    let mut term = Terminal::new(backend).unwrap();
    let theme = cosmos();
    c.bench_function("tab_render_system", |b| {
        b.iter(|| {
            term.draw(|f| w.render_into_frame(f, f.area(), &theme)).unwrap();
        });
    });
}

fn bench_settings_render(c: &mut Criterion) {
    let w = SettingsWidget::new();
    let backend = TestBackend::new(120, 40);
    let mut term = Terminal::new(backend).unwrap();
    let theme = cosmos();
    c.bench_function("tab_render_settings", |b| {
        b.iter(|| {
            term.draw(|f| w.render_into_frame(f, f.area(), &theme)).unwrap();
        });
    });
}

criterion_group!(
    benches,
    bench_workspaces_render,
    bench_ssh_render,
    bench_database_render,
    bench_network_render,
    bench_system_render,
    bench_settings_render,
);
criterion_main!(benches);
```

> NOTE: if any widget does not have a `render_into_frame(frame, area, theme)` method, drop that bench function and add a comment in this file naming the widget plus a follow-up TODO. The render benches that pass are sufficient to gate on; missing widgets get their own follow-up task.

- [ ] **Step 8.3: Run the bench**

```bash
cargo bench -p sid-widgets --bench tab_switch
```

Expected: each bench reports a time. The 8 ms / 120 Hz budget applies — anything over 8 ms gets a follow-up perf-investigation task created here (do NOT skip; flag it).

- [ ] **Step 8.4: Save the baseline**

```bash
cargo bench -p sid-widgets --bench tab_switch -- --save-baseline main
```

- [ ] **Step 8.5: Commit Task 8**

```bash
git add crates/sid-widgets/Cargo.toml crates/sid-widgets/benches/tab_switch.rs
git commit -m "perf(sid-widgets): criterion benches for per-tab render hot path

Six benches, one per cockpit tab, gated at 8 ms / 120 Hz per the
performance budget in the interaction spec. Establishes baselines
so subsequent UX changes can be guarded against regressions via
/sid-perf-check."
```

---

## Task 9 — DEVELOPMENT.md: how to run a dhat profiling session

**Files:**
- Modify: `docs/DEVELOPMENT.md` (append)

- [ ] **Step 9.1: Append a "Profiling with dhat" section to DEVELOPMENT.md**

Append to `docs/DEVELOPMENT.md`:

```markdown
## Profiling with dhat

`dhat` is wired in `Cargo.toml` as a workspace dev-dependency. Use it to
catch allocation hot spots in the render loop or any other path that
shows up in a benchmark as slower than the budget.

### One-shot profile

1. Enable the `dhat-heap` feature in a temporary `Cargo.toml` edit on
   the binary crate:
   ```toml
   [features]
   default = []
   dhat-heap = ["dhat"]
   ```
2. Add the dhat profiler initialization to `crates/sid/src/main.rs` inside
   a `#[cfg(feature = "dhat-heap")]` block at the top of `main`:
   ```rust
   #[cfg(feature = "dhat-heap")]
   let _profiler = dhat::Profiler::new_heap();
   ```
3. Run a typical session and exit cleanly:
   ```bash
   cargo run --features dhat-heap -- --skip-discovery
   # interact with the UI for ~30 seconds, switch tabs, open a modal, then quit (Ctrl+Q)
   ```
4. dhat writes `dhat-heap.json`. Open it in the viewer:
   ```bash
   # https://nnethercote.github.io/dh_view/dh_view.html — load dhat-heap.json
   ```
5. Look for entries with high `total_blocks` × `bytes`. Typical
   offenders in TUIs: per-frame `String` allocations, `Vec<Row>` rebuilds,
   per-frame `Vec<&Workspace>` materializations.

### Interpreting results

- Allocations per frame should be near-constant. A blow-up at the moment
  you switch tabs is the smoking gun for a hot-path regression.
- Total bytes is less important than allocation churn — the heap
  allocator's lock contention is what causes UI jank, not memory
  pressure.
```

- [ ] **Step 9.2: Commit Task 9**

```bash
git add docs/DEVELOPMENT.md
git commit -m "docs(dev): how to run a dhat profiling session against the cockpit

References the interaction spec's perf section and gives a concrete
procedure for catching allocation churn when a bench reports a
slowdown. dhat is already a workspace dev-dep."
```

---

## Task 10 — Adversarial test: synthetic terminal that swallows Ctrl+digit

**Files:**
- Modify: `crates/sid-core/tests/keybind.rs`

`★ Insight ─────────────────────────────────────`
We can't actually run a real terminal in a test, but we can simulate the failure mode: a synthetic `KeyEvent` with `KeyCode::Char('1')` and `KeyModifiers::NONE` (because the terminal stripped the Ctrl) should fall through to the active widget, NOT fire `tabs.jump.1`. Then a `KeyEvent` with `KeyModifiers::ALT` should fire it. This proves the fallback works.
`─────────────────────────────────────────────────`

- [ ] **Step 10.1: Add the adversarial test**

Append to `crates/sid-core/tests/keybind.rs`:

```rust
#[test]
fn bare_digit_does_not_fire_tabs_jump_simulating_terminal_swallow() {
    let m = KeybindMap::cosmos_default();
    // Simulate a terminal that swallowed Ctrl+1 and delivered bare '1'.
    let bare = KeyChord::new(KeyCode::Char('1'), KeyModifiers::NONE);
    assert!(
        m.lookup(&bare).is_none(),
        "bare '1' must not be bound (would prevent typing)",
    );
    // Alt+1 must still fire as the fallback.
    let alt1 = KeyChord::new(KeyCode::Char('1'), KeyModifiers::ALT);
    assert_eq!(
        m.lookup(&alt1).map(|a| a.as_str().to_string()),
        Some("tabs.jump.1".to_string()),
    );
}
```

- [ ] **Step 10.2: Run the test, verify PASS**

```bash
cargo test -p sid-core --test keybind bare_digit_does_not_fire_tabs_jump
```

Expected: PASS.

- [ ] **Step 10.3: Commit Task 10**

```bash
git add crates/sid-core/tests/keybind.rs
git commit -m "test(sid-core): adversarial — bare digit (terminal stripped Ctrl) doesn't fire

Bare '1' (no modifier) must not be bound, otherwise typing '1' in a
text field would jump tabs. Alt+1 is the working fallback. Locks
in the contract so a future binding cleanup doesn't accidentally
break this."
```

---

## Task 11 — Workspace-wide gate: confirm everything green

- [ ] **Step 11.1: Run the full sid-gate**

```bash
/sid-gate
```

Expected: all four gates green (`cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all-features --workspace`, `cargo deny check`).

If clippy fires on any of the new code, fix it before continuing.

- [ ] **Step 11.2: Run criterion benches one more time and confirm budgets**

```bash
cargo bench -p sid-core --bench handle_event
cargo bench -p sid-widgets --bench tab_switch
```

Expected: every reported time is at or below its budget.
- `app_handle_event_noop` ≤ 1 µs
- `tab_render_*` ≤ 8 ms

- [ ] **Step 11.3: Final branch-level commit (no-op if Task 11.1 changes nothing)**

```bash
git status
# If clean, no commit needed. If clippy fixes were applied, commit them now.
```

- [ ] **Step 11.4: Merge back to main**

```bash
git checkout main
git merge --no-ff feat/modal-arrows-and-keybind-fallbacks -m "Merge branch #1: modal substrate + keybind fallbacks + dynamic-tab API

Unblocks branches #2 (workspaces overview cleanup), #3 (workspace
detail tab), #4 (network drill-in), #5 (settings live-apply)."
```

---

## Definition of done

- [x] All modals respond to ↑ ↓ → ← Tab Shift+Tab Enter Esc with documented semantics.
- [x] Focused Choice/Toggle rows render the `‹ ›` cycle hint.
- [x] `Alt+1..6` switches tabs; `Alt+,` opens Settings; `Ctrl+W` / `Alt+W` closes the active detail tab.
- [x] `TabManager::push_detail` and `close_active` work; `Core` tabs are unkillable.
- [x] `App::run_action("tab.close")` dispatches to `TabManager::close_active`.
- [x] Criterion benches saved for `app_handle_event_noop` and the six tab render paths.
- [x] All tests green; `/sid-gate` clean.
- [x] Branch merged to `main`.

## Risks and rollback

- The `Tab` struct gaining a required field is a breaking change for any third-party caller. There are no third-party callers today — the only consumer is the binary, updated in the same branch. Rollback is `git revert` of the merge commit.
- Renderer hint glyph (`‹ ›`) might not render in some terminals (uncommon — both are valid BMP characters in CP-437 and standard UTF-8). If a snapshot test fails on a CI runner with a non-UTF-8 locale, fall back to ASCII `< >`.
- Criterion baselines committed under `target/` are gitignored, so they live per-machine. The `--save-baseline main` step needs to run on each developer's machine (or in CI) before `/sid-perf-check` can compare meaningfully. Document this in DEVELOPMENT.md if not already covered.
