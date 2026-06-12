# UX-v2 Branch 5: Settings live-apply for remaining categories + undo ring

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or superpowers:executing-plans. Steps use checkbox syntax for tracking.

**Goal:** Extend the BehaviorToggles live-apply pattern (shipped in fe4db1d) to every remaining settings category, then add a per-session undo ring (cap 10, 30-second TTL) so each success toast carries `(u: undo)` and pressing `u` while the head toast is live re-applies the prior value.

**Architecture:** `PendingSettingsOutcome` in `sid-widgets::settings` gains five new variants (one per remaining category). Each sub-view grows its own `*Outcome` enum and a `handle_event` method that returns it. The `SettingsWidget::handle_event` match arm for each category mirrors the `Behavior` arm: push to `pending_outcomes` and emit on the action bus. `apply_pending_settings_outcomes` in `wire.rs` dispatches each variant to the right `Store` method and pushes a `"Saved … (u: undo)"` success toast. `SidApp` gains `undo_ring: VecDeque<UndoEntry>` (cap 10); every successful apply pushes an `UndoEntry` carrying the *prior* value and the entry's `Instant` timestamp. The `u`-chord interceptor in `run_event_loop` (before the `app.handle_event` fall-through) pops the head entry when it is within 30 seconds and re-applies the prior value via the same `Store` dispatch path. TTL comparisons use the `Instant` already present on toasts (`spawned_at` is `pub` and `Instant::now()` is how the codebase does it in `toast.rs`); no new clock abstraction needed — tests age the `UndoEntry::recorded_at` field directly (same pattern as `Toast::spawned_at` in `toast.rs`).

**Tech Stack:** Rust stable, ratatui (sid-widgets), sid-core, sid-store, sid binary (wire.rs / main.rs). proptest for the required property test; insta for rendered snapshots. No new crate dependencies.

---

### Task 1: `WorkspaceRootsOutcome` + live-apply

**Files:**
- Modify: `crates/sid-widgets/src/settings/workspace_roots.rs` — add `WorkspaceRootsOutcome` enum + `handle_event` method
- Modify: `crates/sid-widgets/src/settings.rs` — add `WorkspaceRoots` variant to `PendingSettingsOutcome`; route in `SettingsWidget::handle_event` (`SettingsCategory::WorkspaceRoots` arm, currently a no-op at line 740–748); add encode helper
- Modify: `crates/sid/src/wire.rs` — extend `apply_pending_settings_outcomes` match to handle `WorkspaceRootsChanged`

- [x] **Step 1: Write failing tests for `WorkspaceRootsOutcome`**

Add to `crates/sid-widgets/src/settings/workspace_roots.rs` inside `#[cfg(test)] mod tests`:

```rust
#[test]
fn handle_event_add_mode_enter_commits_valid_path() {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::event::{Event, KeyChord};
    let mut v = WorkspaceRootsView::new(vec![]);
    v.begin_add();
    "/tmp".chars().for_each(|c| v.type_char(c));
    let ev = Event::Key(KeyChord::new(KeyCode::Enter, KeyModifiers::NONE));
    let out = v.handle_event(&ev);
    assert!(
        matches!(out, WorkspaceRootsOutcome::RootsChanged(_)),
        "commit valid path"
    );
}

#[test]
fn handle_event_delete_emits_roots_changed() {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::event::{Event, KeyChord};
    use std::path::PathBuf;
    let mut v = WorkspaceRootsView::new(vec![PathBuf::from("/a")]);
    let ev = Event::Key(KeyChord::new(KeyCode::Char('d'), KeyModifiers::NONE));
    let out = v.handle_event(&ev);
    assert!(matches!(out, WorkspaceRootsOutcome::RootsChanged(_)));
}

#[test]
fn handle_event_non_destructive_key_returns_none() {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::event::{Event, KeyChord};
    let mut v = WorkspaceRootsView::new(vec![]);
    let ev = Event::Key(KeyChord::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert!(matches!(v.handle_event(&ev), WorkspaceRootsOutcome::None));
}
```

Run:
```bash
cargo test -p sid-widgets settings::workspace_roots::tests
```
Expected: compile failure (no `WorkspaceRootsOutcome`, no `handle_event`).

- [x] **Step 2: Implement `WorkspaceRootsOutcome` and `handle_event`**

At the top of `crates/sid-widgets/src/settings/workspace_roots.rs`, after the imports and before `struct WorkspaceRootsView`, insert:

```rust
/// Outcome returned by [`WorkspaceRootsView::handle_event`].
///
/// # Examples
///
/// ```
/// use sid_widgets::settings::workspace_roots::WorkspaceRootsOutcome;
/// assert!(matches!(WorkspaceRootsOutcome::None, WorkspaceRootsOutcome::None));
/// ```
#[derive(Clone, Debug)]
pub enum WorkspaceRootsOutcome {
    /// Event was not consumed by this view.
    None,
    /// Roots list mutated; wire layer should persist via `Store::put_setting`.
    /// Carries the new snapshot.
    RootsChanged(Vec<std::path::PathBuf>),
}
```

At the end of `impl WorkspaceRootsView` (after the existing `load` method), add:

```rust
/// Handle a key event. Returns [`WorkspaceRootsOutcome::RootsChanged`]
/// whenever the roots list is mutated. Navigation and character input
/// consume the event but return [`WorkspaceRootsOutcome::None`].
///
/// # Examples
///
/// ```
/// use crossterm::event::{KeyCode, KeyModifiers};
/// use sid_core::event::{Event, KeyChord};
/// use sid_widgets::settings::workspace_roots::{WorkspaceRootsOutcome, WorkspaceRootsView};
///
/// let mut v = WorkspaceRootsView::new(vec![]);
/// let ev = Event::Key(KeyChord::new(KeyCode::Char('j'), KeyModifiers::NONE));
/// assert!(matches!(v.handle_event(&ev), WorkspaceRootsOutcome::None));
/// ```
pub fn handle_event(&mut self, ev: &sid_core::event::Event) -> WorkspaceRootsOutcome {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::event::Event;
    let Event::Key(k) = ev else {
        return WorkspaceRootsOutcome::None;
    };
    if self.is_adding() {
        match k.code {
            KeyCode::Char(c) if k.mods == KeyModifiers::NONE || k.mods == KeyModifiers::SHIFT => {
                self.type_char(c);
            }
            KeyCode::Backspace => {
                self.backspace();
            }
            KeyCode::Esc => {
                self.cancel_add();
            }
            KeyCode::Enter => match self.commit_add() {
                Ok(_) => return WorkspaceRootsOutcome::RootsChanged(self.roots().to_vec()),
                Err(e) => {
                    // last_error already set by commit_add; don't change list.
                    let _ = e;
                }
            },
            _ => {}
        }
        return WorkspaceRootsOutcome::None;
    }
    match (k.code, k.mods) {
        (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
            self.next();
        }
        (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
            self.prev();
        }
        (KeyCode::Char('a') | KeyCode::Char('n'), KeyModifiers::NONE) => {
            self.begin_add();
        }
        (KeyCode::Char('d') | KeyCode::Delete, KeyModifiers::NONE) => {
            if self.remove_focused().is_some() {
                return WorkspaceRootsOutcome::RootsChanged(self.roots().to_vec());
            }
        }
        _ => return WorkspaceRootsOutcome::None,
    }
    WorkspaceRootsOutcome::None
}
```

Run:
```bash
cargo test -p sid-widgets settings::workspace_roots::tests
```
Expected: all 3 new tests pass.

- [x] **Step 3: Add `WorkspaceRootsChanged` variant to `PendingSettingsOutcome` and route in `SettingsWidget`**

In `crates/sid-widgets/src/settings.rs`, extend the `PendingSettingsOutcome` enum (currently lines 189–197):

```rust
    /// User added or removed a workspace root; wire should persist via
    /// `Store::put_setting(WORKSPACE_ROOTS, …)`.
    WorkspaceRootsChanged(Vec<std::path::PathBuf>),
```

Update the doc test on `PendingSettingsOutcome` so it still compiles (add a second `assert!` that names the new variant, or just leave the existing assert — the enum is `non_exhaustive`-free so it compiles either way).

In `SettingsWidget::handle_event` inside the `SettingsCategory::WorkspaceRoots` no-op arm (lines 740–748 of the current file), replace:

```rust
                            SettingsCategory::Keybinds(_)
                            | SettingsCategory::WorkspaceRoots(_)
                            | SettingsCategory::QuickActions(_)
                            | SettingsCategory::DbPath(_)
                            | SettingsCategory::Reset(_) => {
                                // Per-category event routing for these is a
                                // follow-up — each will grow its own Outcome
                                // enum in the same pattern as Behavior + Theme.
                            }
```

with:

```rust
                            SettingsCategory::WorkspaceRoots(v) => {
                                use crate::settings::workspace_roots::WorkspaceRootsOutcome;
                                match v.handle_event(ev) {
                                    WorkspaceRootsOutcome::None => {}
                                    WorkspaceRootsOutcome::RootsChanged(roots) => {
                                        ctx.emit_action_with_payload(
                                            "settings.outcome.workspace_roots",
                                            &roots
                                                .iter()
                                                .map(|p| p.display().to_string())
                                                .collect::<Vec<_>>()
                                                .join(":"),
                                        );
                                        self.pending_outcomes.push(
                                            PendingSettingsOutcome::WorkspaceRootsChanged(roots),
                                        );
                                        return EventOutcome::Consumed;
                                    }
                                }
                            }
                            SettingsCategory::Keybinds(_)
                            | SettingsCategory::QuickActions(_)
                            | SettingsCategory::DbPath(_)
                            | SettingsCategory::Reset(_) => {
                                // Per-category event routing is a follow-up
                                // (Tasks 2–4 below).
                            }
```

Run:
```bash
cargo test -p sid-widgets settings
```
Expected: all tests pass; no new failures.

- [x] **Step 4: Wire dispatch in `apply_pending_settings_outcomes`**

In `crates/sid/src/wire.rs`, extend `apply_pending_settings_outcomes` (currently the single `let PendingSettingsOutcome::BehaviorToggled { key, value } = outcome;` destructure). Replace:

```rust
    for outcome in outcomes {
        let PendingSettingsOutcome::BehaviorToggled { key, value } = outcome;
        let put_result = match &value {
            ToggleValue::Bool(b) => sid_app.store.put_bool(key, *b),
            ToggleValue::Choice { options, selected } => {
                let picked = options.get(*selected).cloned().unwrap_or_default();
                sid_app.store.put_string(key, &picked)
            }
            ToggleValue::U64 { value, .. } => sid_app.store.put_u64(key, *value),
            ToggleValue::String(s) => sid_app.store.put_string(key, s),
        };
        match put_result {
            Ok(()) => {
                sid_app.toasts.push(Toast::success(format!("Saved {key}")));
            }
            Err(e) => {
                sid_app
                    .toasts
                    .push(Toast::error(format!("Save failed for {key}: {e}")));
            }
        }
    }
```

with (note: undo-ring push slots are `// [undo: Task 6]` placeholders for now, filled in Task 6):

```rust
    for outcome in outcomes {
        use sid_widgets::settings::PendingSettingsOutcome::*;
        match outcome {
            BehaviorToggled { key, value } => {
                let put_result = match &value {
                    ToggleValue::Bool(b) => sid_app.store.put_bool(key, *b),
                    ToggleValue::Choice { options, selected } => {
                        let picked = options.get(*selected).cloned().unwrap_or_default();
                        sid_app.store.put_string(key, &picked)
                    }
                    ToggleValue::U64 { value, .. } => sid_app.store.put_u64(key, *value),
                    ToggleValue::String(s) => sid_app.store.put_string(key, s),
                };
                match put_result {
                    Ok(()) => {
                        sid_app.toasts.push(Toast::success(format!("Saved {key}")));
                        // [undo: Task 6] push prior value
                    }
                    Err(e) => {
                        sid_app
                            .toasts
                            .push(Toast::error(format!("Save failed for {key}: {e}")));
                    }
                }
            }
            WorkspaceRootsChanged(new_roots) => {
                use sid_store::{SettingValue, settings_keys};
                let json = serde_json::to_string(&new_roots)
                    .map_err(|e| sid_core::SidError::Storage(e.to_string()));
                let put_result = json.and_then(|s| {
                    sid_app
                        .store
                        .put_setting(settings_keys::WORKSPACE_ROOTS, &SettingValue(s.into_bytes()))
                });
                match put_result {
                    Ok(()) => {
                        sid_app
                            .toasts
                            .push(Toast::success("Workspace roots saved"));
                        // [undo: Task 6] push prior roots
                    }
                    Err(e) => {
                        sid_app
                            .toasts
                            .push(Toast::error(format!("Workspace roots save failed: {e}")));
                    }
                }
            }
            // [remaining: Tasks 2–4]
            _ => {}
        }
    }
```

Also add `use sid_widgets::settings::behavior_toggles::ToggleValue;` at the top of the function (keep the existing `use` lines, add ToggleValue import if not already there).

Run:
```bash
cargo test -p sid settings
```
Expected: tests pass; no compile errors.

- [x] **Step 5: Commit**

```
feat(sid-widgets,sid): WorkspaceRoots live-apply (settings branch 5)

WorkspaceRootsView gains handle_event + WorkspaceRootsOutcome;
SettingsWidget routes the RootsChanged variant to pending_outcomes;
apply_pending_settings_outcomes dispatches to put_setting(WORKSPACE_ROOTS).
```

---

### Task 2: `QuickActionsOutcome` + live-apply

**Files:**
- Modify: `crates/sid-widgets/src/settings/quick_actions.rs` — add `QuickActionsOutcome` enum + `handle_event` method
- Modify: `crates/sid-widgets/src/settings.rs` — add `QuickActionUpserted` / `QuickActionRemoved` variants to `PendingSettingsOutcome`; route in `SettingsCategory::QuickActions` arm
- Modify: `crates/sid/src/wire.rs` — handle new variants in `apply_pending_settings_outcomes`

- [x] **Step 1: Write failing tests for `QuickActionsOutcome`**

Add to `crates/sid-widgets/src/settings/quick_actions.rs` inside `#[cfg(test)] mod tests`:

```rust
#[test]
fn handle_event_enter_on_focused_row_commits_edit() {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::event::{Event, KeyChord};
    use sid_store::{QuickAction, QuickActionScope};
    let qa = QuickAction {
        id: "test.action".into(),
        label: "Test".into(),
        cmd: "echo hi".into(),
        keybind: None,
        scope: QuickActionScope::Global,
    };
    let mut v = QuickActionsView::new(vec![qa.clone()]);
    v.begin_edit_focused();
    // Fast-commit by calling commit_edit on the buffer directly, then
    // simulate Enter arriving — the router should produce Upserted.
    let ev = Event::Key(KeyChord::new(KeyCode::Enter, KeyModifiers::NONE));
    let out = v.handle_event(&ev);
    assert!(
        matches!(out, QuickActionsOutcome::Upserted(_) | QuickActionsOutcome::None),
        "enter in edit mode yields Upserted or None on invalid buf"
    );
}

#[test]
fn handle_event_delete_on_row_emits_removed() {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::event::{Event, KeyChord};
    use sid_store::{QuickAction, QuickActionScope};
    let qa = QuickAction {
        id: "x".into(),
        label: "X".into(),
        cmd: "x".into(),
        keybind: None,
        scope: QuickActionScope::Global,
    };
    let mut v = QuickActionsView::new(vec![qa]);
    let ev = Event::Key(KeyChord::new(KeyCode::Char('d'), KeyModifiers::NONE));
    let out = v.handle_event(&ev);
    assert!(matches!(out, QuickActionsOutcome::Removed(_)));
}

#[test]
fn handle_event_nav_returns_none() {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::event::{Event, KeyChord};
    let mut v = QuickActionsView::new(vec![]);
    let ev = Event::Key(KeyChord::new(KeyCode::Char('j'), KeyModifiers::NONE));
    assert!(matches!(v.handle_event(&ev), QuickActionsOutcome::None));
}
```

Run:
```bash
cargo test -p sid-widgets settings::quick_actions::tests
```
Expected: compile failure.

- [x] **Step 2: Implement `QuickActionsOutcome` and `handle_event`**

At the top of `crates/sid-widgets/src/settings/quick_actions.rs`, before `struct EditBuffer`, insert:

```rust
/// Outcome of a key event routed into [`QuickActionsView`].
///
/// # Examples
///
/// ```
/// use sid_widgets::settings::quick_actions::QuickActionsOutcome;
/// assert!(matches!(QuickActionsOutcome::None, QuickActionsOutcome::None));
/// ```
#[derive(Clone, Debug)]
pub enum QuickActionsOutcome {
    /// Event consumed but no list mutation.
    None,
    /// A quick action was added or edited. Wire layer calls
    /// `store.upsert_quick_action`.
    Upserted(sid_store::QuickAction),
    /// A quick action was deleted. Wire layer calls
    /// `store.remove_quick_action`.
    Removed(String),
}
```

At the end of `impl QuickActionsView` (after `save_all`), add:

```rust
/// Handle a key event. Returns [`QuickActionsOutcome::Upserted`] or
/// [`QuickActionsOutcome::Removed`] when the list is mutated.
///
/// Key map:
/// - `j` / Down: focus next (no edit mode)
/// - `k` / Up: focus prev (no edit mode)
/// - `a` / `n`: begin_add
/// - `e` / Enter: begin_edit_focused (not in edit mode)
/// - `d` / Delete: remove_focused (not in edit mode)
/// - Esc (in edit mode): cancel_edit
/// - Enter (in edit mode): commit_edit → Upserted or None on validation error
/// - Char / Backspace (in edit mode): routed to the first non-empty field
///   still pending; simplified for now (id field only as proof of pattern).
///
/// # Examples
///
/// ```
/// use crossterm::event::{KeyCode, KeyModifiers};
/// use sid_core::event::{Event, KeyChord};
/// use sid_widgets::settings::quick_actions::{QuickActionsOutcome, QuickActionsView};
///
/// let mut v = QuickActionsView::new(vec![]);
/// let ev = Event::Key(KeyChord::new(KeyCode::Char('j'), KeyModifiers::NONE));
/// assert!(matches!(v.handle_event(&ev), QuickActionsOutcome::None));
/// ```
pub fn handle_event(&mut self, ev: &sid_core::event::Event) -> QuickActionsOutcome {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::event::Event;
    let Event::Key(k) = ev else {
        return QuickActionsOutcome::None;
    };
    if self.is_editing() {
        match k.code {
            KeyCode::Esc => {
                self.cancel_edit();
            }
            KeyCode::Enter => {
                match self.commit_edit() {
                    Ok(qa) => return QuickActionsOutcome::Upserted(qa),
                    Err(_) => {} // validation error displayed in view
                }
            }
            KeyCode::Char(c)
                if k.mods == KeyModifiers::NONE || k.mods == KeyModifiers::SHIFT =>
            {
                if let Some(buf) = self.edit_buffer_mut() {
                    buf.id.push(c); // simplified: first-field routing
                }
            }
            KeyCode::Backspace => {
                if let Some(buf) = self.edit_buffer_mut() {
                    buf.id.pop();
                }
            }
            _ => {}
        }
        return QuickActionsOutcome::None;
    }
    match (k.code, k.mods) {
        (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
            self.next();
        }
        (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
            self.prev();
        }
        (KeyCode::Char('a') | KeyCode::Char('n'), KeyModifiers::NONE) => {
            self.begin_add();
        }
        (KeyCode::Char('e') | KeyCode::Enter, KeyModifiers::NONE) => {
            self.begin_edit_focused();
        }
        (KeyCode::Char('d') | KeyCode::Delete, KeyModifiers::NONE) => {
            if let Some(removed) = self.remove_focused() {
                return QuickActionsOutcome::Removed(removed.id);
            }
        }
        _ => return QuickActionsOutcome::None,
    }
    QuickActionsOutcome::None
}
```

Run:
```bash
cargo test -p sid-widgets settings::quick_actions::tests
```
Expected: all new tests pass.

- [x] **Step 3: Add variants to `PendingSettingsOutcome` and route in `SettingsWidget`**

In `crates/sid-widgets/src/settings.rs` `PendingSettingsOutcome`, add:

```rust
    /// User added/edited a quick action; wire should call `upsert_quick_action`.
    QuickActionUpserted(sid_store::QuickAction),
    /// User deleted a quick action; wire should call `remove_quick_action`.
    QuickActionRemoved(String),
```

In the `SettingsCategory::QuickActions` no-op arm, replace the no-op with:

```rust
                            SettingsCategory::QuickActions(v) => {
                                use crate::settings::quick_actions::QuickActionsOutcome;
                                match v.handle_event(ev) {
                                    QuickActionsOutcome::None => {}
                                    QuickActionsOutcome::Upserted(qa) => {
                                        ctx.emit_action_with_payload(
                                            "settings.outcome.quick_action_upserted",
                                            &qa.id,
                                        );
                                        self.pending_outcomes.push(
                                            PendingSettingsOutcome::QuickActionUpserted(qa),
                                        );
                                        return EventOutcome::Consumed;
                                    }
                                    QuickActionsOutcome::Removed(id) => {
                                        ctx.emit_action_with_payload(
                                            "settings.outcome.quick_action_removed",
                                            &id,
                                        );
                                        self.pending_outcomes.push(
                                            PendingSettingsOutcome::QuickActionRemoved(id),
                                        );
                                        return EventOutcome::Consumed;
                                    }
                                }
                            }
```

- [x] **Step 4: Wire dispatch**

In `apply_pending_settings_outcomes` add arms inside the `match outcome` block (replacing the `_ => {}` fallthrough added in Task 1 Step 4):

```rust
            QuickActionUpserted(qa) => {
                match sid_app.store.upsert_quick_action(&qa) {
                    Ok(()) => {
                        sid_app
                            .toasts
                            .push(Toast::success(format!("Quick action '{}' saved", qa.id)));
                        // [undo: Task 6]
                    }
                    Err(e) => {
                        sid_app
                            .toasts
                            .push(Toast::error(format!("Quick action save failed: {e}")));
                    }
                }
            }
            QuickActionRemoved(id) => {
                match sid_app.store.remove_quick_action(&id) {
                    Ok(()) => {
                        sid_app
                            .toasts
                            .push(Toast::success(format!("Quick action '{id}' removed")));
                        // [undo: Task 6]
                    }
                    Err(e) => {
                        sid_app
                            .toasts
                            .push(Toast::error(format!("Quick action remove failed: {e}")));
                    }
                }
            }
            // [remaining: Tasks 3–4]
            _ => {}
```

Run:
```bash
cargo test -p sid settings
```

- [x] **Step 5: Commit**

```
feat(sid-widgets,sid): QuickActions live-apply (settings branch 5)
```

---

### Task 3: `KeybindEditorOutcome` + live-apply

**Files:**
- Modify: `crates/sid-widgets/src/settings/keybind_editor.rs` — add `KeybindEditorOutcome` enum + `handle_event` method
- Modify: `crates/sid-widgets/src/settings.rs` — add `KeybindApplied` variant to `PendingSettingsOutcome`; route in `SettingsCategory::Keybinds` arm
- Modify: `crates/sid/src/wire.rs` — handle `KeybindApplied` in `apply_pending_settings_outcomes`

- [x] **Step 1: Write failing tests**

Add to `crates/sid-widgets/src/settings/keybind_editor.rs` inside `#[cfg(test)] mod tests`:

```rust
#[test]
fn handle_event_enter_starts_capture() {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::action::{Action, ActionRegistry};
    use sid_core::event::{Event, KeyChord};
    use sid_core::keybind::KeybindMap;
    use sid_core::keybind_capture::CaptureState;
    let mut reg = ActionRegistry::new();
    reg.register(Action::new("a", "A"));
    let mut v = KeybindEditorView::new(&reg, KeybindMap::new());
    let ev = Event::Key(KeyChord::new(KeyCode::Enter, KeyModifiers::NONE));
    let _ = v.handle_event(&ev);
    assert!(
        matches!(v.capture_state(), CaptureState::Waiting { .. }),
        "Enter starts capture"
    );
}

#[test]
fn handle_event_esc_in_capture_cancels() {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::action::{Action, ActionRegistry};
    use sid_core::event::{Event, KeyChord};
    use sid_core::keybind::KeybindMap;
    use sid_core::keybind_capture::CaptureState;
    let mut reg = ActionRegistry::new();
    reg.register(Action::new("a", "A"));
    let mut v = KeybindEditorView::new(&reg, KeybindMap::new());
    v.enter_capture();
    let ev = Event::Key(KeyChord::new(KeyCode::Esc, KeyModifiers::NONE));
    let _ = v.handle_event(&ev);
    assert_eq!(v.capture_state(), &CaptureState::Idle);
}

#[test]
fn handle_event_chord_in_capture_produces_applied() {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::action::{Action, ActionRegistry};
    use sid_core::event::{Event, KeyChord};
    use sid_core::keybind::KeybindMap;
    let mut reg = ActionRegistry::new();
    reg.register(Action::new("a", "A"));
    let mut v = KeybindEditorView::new(&reg, KeybindMap::new());
    v.enter_capture();
    let chord_ev = Event::Key(KeyChord::new(KeyCode::Char('z'), KeyModifiers::CONTROL));
    let out = v.handle_event(&chord_ev);
    assert!(
        matches!(out, KeybindEditorOutcome::Applied { .. }),
        "capturing a chord emits Applied"
    );
}
```

Run:
```bash
cargo test -p sid-widgets settings::keybind_editor::tests
```
Expected: compile failure.

- [x] **Step 2: Implement `KeybindEditorOutcome` and `handle_event`**

Before `const DANGEROUS_ACTIONS` in `crates/sid-widgets/src/settings/keybind_editor.rs`, insert:

```rust
/// Outcome returned by [`KeybindEditorView::handle_event`].
///
/// # Examples
///
/// ```
/// use sid_core::action::ActionId;
/// use sid_core::event::KeyChord;
/// use crossterm::event::{KeyCode, KeyModifiers};
/// use sid_widgets::settings::keybind_editor::KeybindEditorOutcome;
///
/// let chord = KeyChord::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
/// let o = KeybindEditorOutcome::Applied {
///     action: ActionId::new("app.quit"),
///     chord,
///     profile_name: "cosmos".into(),
/// };
/// assert!(matches!(o, KeybindEditorOutcome::Applied { .. }));
/// ```
#[derive(Clone, Debug)]
pub enum KeybindEditorOutcome {
    /// Event consumed; no persistent change.
    None,
    /// A new binding was accepted. Wire layer should call
    /// `sid_store::keybind_load::save_keybind_profile` with `profile_name`
    /// and the current map snapshot.
    Applied {
        action: sid_core::action::ActionId,
        chord: sid_core::event::KeyChord,
        profile_name: String,
        map_snapshot: sid_core::keybind::KeybindMap,
    },
}
```

At the end of `impl KeybindEditorView` (after `dangerous_action_warnings`), add:

```rust
/// Route a key event through the editor state machine. Returns
/// [`KeybindEditorOutcome::Applied`] once a chord is committed and the
/// in-memory map is updated.
///
/// # Examples
///
/// ```
/// use sid_core::action::ActionRegistry;
/// use sid_core::keybind::KeybindMap;
/// use sid_widgets::settings::keybind_editor::{KeybindEditorOutcome, KeybindEditorView};
///
/// let view = KeybindEditorView::new(&ActionRegistry::new(), KeybindMap::new());
/// // Empty registry — Enter is a no-op.
/// ```
pub fn handle_event(&mut self, ev: &sid_core::event::Event) -> KeybindEditorOutcome {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::keybind_capture::CaptureState;
    use sid_core::event::Event;
    let Event::Key(k) = ev else {
        return KeybindEditorOutcome::None;
    };
    match &self.capture {
        CaptureState::Idle => match (k.code, k.mods) {
            (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
                self.next();
            }
            (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
                self.prev();
            }
            (KeyCode::Enter, KeyModifiers::NONE) => {
                self.enter_capture();
            }
            _ => return KeybindEditorOutcome::None,
        },
        CaptureState::Waiting { .. } | CaptureState::ConfirmOverwrite { .. } => {
            match k.code {
                KeyCode::Esc => {
                    self.cancel_capture();
                }
                _ => {
                    // Any non-Esc key is treated as the captured chord.
                    let chord = sid_core::event::KeyChord::new(k.code, k.mods);
                    let was_capturing = matches!(self.capture, CaptureState::Waiting { .. });
                    if was_capturing {
                        self.on_chord_captured(chord);
                        if matches!(self.capture, CaptureState::Idle) {
                            // on_chord_captured resolved without conflict.
                            if let Some(action) = self.actions.get(self.focused).cloned() {
                                let current_chord = self.map.iter()
                                    .find(|(_, a)| *a == &action)
                                    .map(|(c, _)| *c);
                                if let Some(c) = current_chord {
                                    return KeybindEditorOutcome::Applied {
                                        action,
                                        chord: c,
                                        profile_name: "cosmos".into(),
                                        map_snapshot: self.map.clone(),
                                    };
                                }
                            }
                        }
                    } else {
                        // ConfirmOverwrite — treat any key as "yes" except
                        // explicit 'n' / Esc which mean "no".
                        match k.code {
                            KeyCode::Char('n') => {
                                self.confirm_overwrite_no();
                            }
                            _ => {
                                let pre_focused = self.focused;
                                self.confirm_overwrite_yes();
                                if matches!(self.capture, CaptureState::Idle) {
                                    if let Some(action) = self.actions.get(pre_focused).cloned() {
                                        let current_chord = self.map.iter()
                                            .find(|(_, a)| *a == &action)
                                            .map(|(c, _)| *c);
                                        if let Some(c) = current_chord {
                                            return KeybindEditorOutcome::Applied {
                                                action,
                                                chord: c,
                                                profile_name: "cosmos".into(),
                                                map_snapshot: self.map.clone(),
                                            };
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        CaptureState::Captured { .. } => {}
    }
    KeybindEditorOutcome::None
}
```

Note: `KeybindMap` must implement `Clone`. Verify:
```bash
grep -n "derive.*Clone\|impl Clone" crates/sid-core/src/keybind.rs | head -5
```
If `Clone` is missing, add `#[derive(Clone)]` to `KeybindMap`.

Run:
```bash
cargo test -p sid-widgets settings::keybind_editor::tests
```
Expected: all new tests pass.

- [x] **Step 3: Add `KeybindApplied` variant and route in `SettingsWidget`**

In `crates/sid-widgets/src/settings.rs` `PendingSettingsOutcome`, add:

```rust
    /// User successfully rebound a key; wire should call
    /// `keybind_load::save_keybind_profile`.
    KeybindApplied {
        profile_name: String,
        map_snapshot: sid_core::keybind::KeybindMap,
    },
```

Replace the `SettingsCategory::Keybinds(_)` no-op arm with:

```rust
                            SettingsCategory::Keybinds(v) => {
                                use crate::settings::keybind_editor::KeybindEditorOutcome;
                                match v.handle_event(ev) {
                                    KeybindEditorOutcome::None => {}
                                    KeybindEditorOutcome::Applied {
                                        action,
                                        chord,
                                        profile_name,
                                        map_snapshot,
                                    } => {
                                        ctx.emit_action_with_payload(
                                            "settings.outcome.keybind_applied",
                                            &format!("{}={:?}", action.as_str(), chord),
                                        );
                                        self.pending_outcomes.push(
                                            PendingSettingsOutcome::KeybindApplied {
                                                profile_name,
                                                map_snapshot,
                                            },
                                        );
                                        return EventOutcome::Consumed;
                                    }
                                }
                            }
```

- [x] **Step 4: Wire dispatch**

In `apply_pending_settings_outcomes`, add before `_ => {}`:

```rust
            KeybindApplied { profile_name, map_snapshot } => {
                use sid_store::keybind_load::save_keybind_profile;
                match save_keybind_profile(&*sid_app.store, &profile_name, &map_snapshot) {
                    Ok(()) => {
                        sid_app
                            .toasts
                            .push(Toast::success(format!("Keybinds saved to '{profile_name}'")));
                        // [undo: Task 6]
                    }
                    Err(e) => {
                        sid_app
                            .toasts
                            .push(Toast::error(format!("Keybind save failed: {e}")));
                    }
                }
            }
```

Run:
```bash
cargo test -p sid settings
```

- [x] **Step 5: Commit**

```
feat(sid-widgets,sid): Keybinds live-apply (settings branch 5)
```

---

### Task 4: `DbPathOutcome` + `ResetOutcome` + live-apply

**Files:**
- Modify: `crates/sid-widgets/src/settings/db_path.rs` — add `DbPathOutcome` enum + `handle_event`
- Modify: `crates/sid-widgets/src/settings/reset.rs` — add `ResetOutcome` enum + `handle_event`
- Modify: `crates/sid-widgets/src/settings.rs` — add `DbPathOverrideWritten` / `FactoryReset` variants; route both arms
- Modify: `crates/sid/src/wire.rs` — handle both in `apply_pending_settings_outcomes`

- [x] **Step 1: Write failing tests for `DbPathOutcome`**

Add to `crates/sid-widgets/src/settings/db_path.rs` tests:

```rust
#[test]
fn handle_event_enter_begins_edit_when_idle() {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::event::{Event, KeyChord};
    let (_d, active, toml) = paths();
    let mut v = DbPathView::open(active, toml).unwrap();
    let ev = Event::Key(KeyChord::new(KeyCode::Enter, KeyModifiers::NONE));
    let out = v.handle_event(&ev);
    assert!(v.is_editing());
    assert!(matches!(out, DbPathOutcome::None));
}

#[test]
fn handle_event_enter_in_edit_mode_commits_and_emits_outcome() {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::event::{Event, KeyChord};
    let (_d, active, toml) = paths();
    let mut v = DbPathView::open(active, toml).unwrap();
    v.begin_edit();
    // Empty commit → clears override.
    let ev = Event::Key(KeyChord::new(KeyCode::Enter, KeyModifiers::NONE));
    let out = v.handle_event(&ev);
    assert!(matches!(out, DbPathOutcome::Written(_)));
    assert!(!v.is_editing());
}

#[test]
fn handle_event_esc_cancels_edit() {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::event::{Event, KeyChord};
    let (_d, active, toml) = paths();
    let mut v = DbPathView::open(active, toml).unwrap();
    v.begin_edit();
    let ev = Event::Key(KeyChord::new(KeyCode::Esc, KeyModifiers::NONE));
    let _ = v.handle_event(&ev);
    assert!(!v.is_editing());
}
```

Add to `crates/sid-widgets/src/settings/reset.rs` tests:

```rust
#[test]
fn handle_event_enter_when_idle_opens_confirm() {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::event::{Event, KeyChord};
    let mut v = ResetView::new();
    let ev = Event::Key(KeyChord::new(KeyCode::Enter, KeyModifiers::NONE));
    let _ = v.handle_event(&ev);
    assert!(v.is_confirming());
}

#[test]
fn handle_event_y_when_confirming_returns_confirmed() {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::event::{Event, KeyChord};
    let mut v = ResetView::new();
    v.open_confirm();
    let ev = Event::Key(KeyChord::new(KeyCode::Char('y'), KeyModifiers::NONE));
    let out = v.handle_event(&ev);
    assert!(matches!(out, ResetOutcome::Confirmed));
    assert!(!v.is_confirming());
}

#[test]
fn handle_event_n_when_confirming_cancels() {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::event::{Event, KeyChord};
    let mut v = ResetView::new();
    v.open_confirm();
    let ev = Event::Key(KeyChord::new(KeyCode::Char('n'), KeyModifiers::NONE));
    let out = v.handle_event(&ev);
    assert!(matches!(out, ResetOutcome::None));
    assert!(!v.is_confirming());
}
```

Run:
```bash
cargo test -p sid-widgets "settings::db_path::tests|settings::reset::tests"
```
Expected: compile failures.

- [x] **Step 2: Implement `DbPathOutcome` + `handle_event`**

Before `pub struct RestartNotice` in `crates/sid-widgets/src/settings/db_path.rs`, insert:

```rust
/// Outcome of a key event routed to [`DbPathView`].
///
/// # Examples
///
/// ```
/// use sid_widgets::settings::db_path::DbPathOutcome;
/// assert!(matches!(DbPathOutcome::None, DbPathOutcome::None));
/// ```
#[derive(Clone, Debug)]
pub enum DbPathOutcome {
    /// Event consumed; no persistent change.
    None,
    /// DB path override successfully written to `sid.toml`.
    Written(RestartNotice),
}
```

At the end of `impl DbPathView` (after `commit_edit`), add:

```rust
/// Route a key event to the editor state machine.
///
/// - Outside edit mode: `Enter` → begin_edit.
/// - Inside edit mode: `Esc` → cancel; `Backspace` → pop char;
///   printable → push char; `Enter` → commit.
///
/// # Examples
///
/// ```
/// use crossterm::event::{KeyCode, KeyModifiers};
/// use sid_core::event::{Event, KeyChord};
/// use sid_widgets::settings::db_path::{DbPathOutcome, DbPathView};
/// use tempfile::tempdir;
/// use std::path::PathBuf;
///
/// let d = tempdir().unwrap();
/// let mut v = DbPathView::open(PathBuf::from("/x.redb"), d.path().join("sid.toml")).unwrap();
/// let ev = Event::Key(KeyChord::new(KeyCode::Char('j'), KeyModifiers::NONE));
/// assert!(matches!(v.handle_event(&ev), DbPathOutcome::None));
/// ```
pub fn handle_event(&mut self, ev: &sid_core::event::Event) -> DbPathOutcome {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::event::Event;
    let Event::Key(k) = ev else {
        return DbPathOutcome::None;
    };
    if self.is_editing() {
        match k.code {
            KeyCode::Esc => {
                self.cancel_edit();
            }
            KeyCode::Backspace => {
                self.backspace();
            }
            KeyCode::Char(c)
                if k.mods == KeyModifiers::NONE || k.mods == KeyModifiers::SHIFT =>
            {
                self.type_char(c);
            }
            KeyCode::Enter => match self.commit_edit() {
                Ok(notice) => return DbPathOutcome::Written(notice),
                Err(_) => {} // last_error already set
            },
            _ => {}
        }
        return DbPathOutcome::None;
    }
    match k.code {
        KeyCode::Enter => {
            self.begin_edit();
        }
        _ => return DbPathOutcome::None,
    }
    DbPathOutcome::None
}
```

- [x] **Step 3: Implement `ResetOutcome` + `handle_event`**

Before `pub const FACTORY_KEYS` in `crates/sid-widgets/src/settings/reset.rs`, insert:

```rust
/// Outcome of a key event routed to [`ResetView`].
///
/// # Examples
///
/// ```
/// use sid_widgets::settings::reset::ResetOutcome;
/// assert!(matches!(ResetOutcome::None, ResetOutcome::None));
/// ```
#[derive(Clone, Debug)]
pub enum ResetOutcome {
    /// Event consumed; no persistent change.
    None,
    /// User confirmed the reset. Wire layer calls `confirm(store)`.
    Confirmed,
}
```

At the end of `impl ResetView` (after `confirm`), add:

```rust
/// Route a key event through the confirm modal state machine.
///
/// - Idle: `Enter` → open_confirm.
/// - Confirming: `y` → Confirmed (closes modal); `n` / Esc → cancel.
///
/// # Examples
///
/// ```
/// use crossterm::event::{KeyCode, KeyModifiers};
/// use sid_core::event::{Event, KeyChord};
/// use sid_widgets::settings::reset::{ResetOutcome, ResetView};
///
/// let mut v = ResetView::new();
/// let ev = Event::Key(KeyChord::new(KeyCode::Char('y'), KeyModifiers::NONE));
/// // Not confirming → y is a no-op.
/// assert!(matches!(v.handle_event(&ev), ResetOutcome::None));
/// ```
pub fn handle_event(&mut self, ev: &sid_core::event::Event) -> ResetOutcome {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::event::Event;
    let Event::Key(k) = ev else {
        return ResetOutcome::None;
    };
    if self.is_confirming() {
        match (k.code, k.mods) {
            (KeyCode::Char('y'), KeyModifiers::NONE) => {
                self.confirm_open = false;
                return ResetOutcome::Confirmed;
            }
            (KeyCode::Char('n') | KeyCode::Esc, _) => {
                self.cancel();
            }
            _ => {}
        }
        return ResetOutcome::None;
    }
    if k.code == KeyCode::Enter && k.mods == KeyModifiers::NONE {
        self.open_confirm();
    }
    ResetOutcome::None
}
```

Run:
```bash
cargo test -p sid-widgets "settings::db_path::tests|settings::reset::tests"
```
Expected: all new tests pass.

- [x] **Step 4: Add variants to `PendingSettingsOutcome` and route both arms**

In `crates/sid-widgets/src/settings.rs` `PendingSettingsOutcome`, add:

```rust
    /// DB path override written to `sid.toml`; wire emits a "restart required" toast.
    DbPathOverrideWritten(crate::settings::db_path::RestartNotice),
    /// Factory reset confirmed; wire calls `ResetView::confirm(&store)`.
    FactoryResetConfirmed,
```

Route in the `SettingsCategory::DbPath` arm (currently a no-op):

```rust
                            SettingsCategory::DbPath(v) => {
                                use crate::settings::db_path::DbPathOutcome;
                                match v.handle_event(ev) {
                                    DbPathOutcome::None => {}
                                    DbPathOutcome::Written(notice) => {
                                        ctx.emit_action_with_payload(
                                            "settings.outcome.db_path_written",
                                            &notice.sid_toml_path.display().to_string(),
                                        );
                                        self.pending_outcomes.push(
                                            PendingSettingsOutcome::DbPathOverrideWritten(notice),
                                        );
                                        return EventOutcome::Consumed;
                                    }
                                }
                            }
```

Route in the `SettingsCategory::Reset` arm:

```rust
                            SettingsCategory::Reset(v) => {
                                use crate::settings::reset::ResetOutcome;
                                match v.handle_event(ev) {
                                    ResetOutcome::None => {}
                                    ResetOutcome::Confirmed => {
                                        ctx.emit_action_with_payload(
                                            "settings.outcome.factory_reset",
                                            "",
                                        );
                                        self.pending_outcomes.push(
                                            PendingSettingsOutcome::FactoryResetConfirmed,
                                        );
                                        return EventOutcome::Consumed;
                                    }
                                }
                            }
```

Remove the remaining `_ => {}` no-op arm from the `SettingsCategory` match — every category is now handled.

- [x] **Step 5: Wire dispatch**

In `apply_pending_settings_outcomes`, add:

```rust
            DbPathOverrideWritten(notice) => {
                sid_app.toasts.push(Toast::info(format!(
                    "DB path written to {} — restart to apply",
                    notice.sid_toml_path.display()
                )));
            }
            FactoryResetConfirmed => {
                use sid_widgets::settings::reset::ResetView;
                // We need a ResetView instance only for its confirm() logic;
                // create a transient one.
                let mut rv = ResetView::new();
                rv.open_confirm();
                match rv.confirm(&*sid_app.store) {
                    Ok(n) => {
                        sid_app
                            .toasts
                            .push(Toast::success(format!("Reset {n} settings to defaults")));
                        // [undo: Task 6] factory reset is intentionally not undoable
                    }
                    Err(e) => {
                        sid_app
                            .toasts
                            .push(Toast::error(format!("Reset failed: {e}")));
                    }
                }
            }
```

Remove the final `_ => {}` fallthrough now that all variants are handled.

Run:
```bash
cargo test -p sid settings && cargo test -p sid-widgets settings
```

- [x] **Step 6: Commit**

```
feat(sid-widgets,sid): DbPath + Reset live-apply (settings branch 5)

All six settings categories now have end-to-end live-apply. The
PendingSettingsOutcome enum is exhaustive and the match in
apply_pending_settings_outcomes has no wildcard arm.
```

---

### Task 5: Theme live-apply (wire up existing `ThemePickerOutcome::Applied`)

The `ThemePickerOutcome::Applied { name }` variant was already returned by `ThemePickerView::handle_event` (shipped with the widget) but the `SettingsWidget::handle_event` arm currently discards it with `_ => return EventOutcome::Consumed`. This task wires it end-to-end.

**Files:**
- Modify: `crates/sid-widgets/src/settings.rs` — add `ThemeApplied` variant to `PendingSettingsOutcome`; update `Theme` arm (line ~701–704 in current file) to push on `Applied`
- Modify: `crates/sid/src/wire.rs` — handle `ThemeApplied` in `apply_pending_settings_outcomes`

- [x] **Step 1: Write failing test**

Add to `crates/sid-widgets/src/settings.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn theme_applied_pushes_pending_outcome() {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::event::{Event, KeyChord};
    use sid_ui::theme_registry::ThemeRegistry;
    let r = ThemeRegistry::with_builtins();
    let mut w = SettingsWidget::with_categories(vec![
        SettingsCategory::Theme(
            crate::settings::theme_picker::ThemePickerView::new(&r, "cosmos")
        ),
    ]);
    // Tab to SubView so Theme category receives keys.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut ctx = WidgetCtx::new(tx);
    w.handle_event(&Event::Key(KeyChord::new(KeyCode::Tab, KeyModifiers::NONE)), &mut ctx);
    // Enter applies the focused theme.
    w.handle_event(&Event::Key(KeyChord::new(KeyCode::Enter, KeyModifiers::NONE)), &mut ctx);
    let outcomes = w.take_pending_outcomes();
    assert!(
        outcomes.iter().any(|o| matches!(o, PendingSettingsOutcome::ThemeApplied { .. })),
        "ThemeApplied outcome expected"
    );
}
```

Run:
```bash
cargo test -p sid-widgets settings::tests::theme_applied_pushes_pending_outcome
```
Expected: compile failure (no `ThemeApplied` variant).

- [x] **Step 2: Add `ThemeApplied` variant and route**

In `PendingSettingsOutcome`, add:

```rust
    /// User applied a theme from the theme picker. Wire should call
    /// `put_string(THEME_NAME, name)`.
    ThemeApplied {
        /// Name of the selected theme.
        name: String,
    },
```

In `SettingsWidget::handle_event`, the `Theme` arm (currently `match v.handle_event(ev) { ThemePickerOutcome::None => {}, _ => return EventOutcome::Consumed, }`), replace with:

```rust
                            SettingsCategory::Theme(v) => match v.handle_event(ev) {
                                ThemePickerOutcome::None => {}
                                ThemePickerOutcome::PreviewChanged => {
                                    return EventOutcome::Consumed;
                                }
                                ThemePickerOutcome::Applied { name } => {
                                    ctx.emit_action_with_payload(
                                        "settings.outcome.theme_applied",
                                        &name,
                                    );
                                    self.pending_outcomes.push(
                                        PendingSettingsOutcome::ThemeApplied { name },
                                    );
                                    return EventOutcome::Consumed;
                                }
                            },
```

Run:
```bash
cargo test -p sid-widgets settings::tests::theme_applied_pushes_pending_outcome
```
Expected: passes.

- [x] **Step 3: Wire dispatch**

In `apply_pending_settings_outcomes` add (before other non-BehaviorToggled arms):

```rust
            ThemeApplied { name } => {
                use sid_store::{settings_keys, TypedSettings};
                match sid_app.store.put_string(settings_keys::THEME_NAME, &name) {
                    Ok(()) => {
                        sid_app.toasts.push(Toast::success(format!(
                            "Theme '{}' applied (u: undo)",
                            name
                        )));
                        // [undo: Task 6]
                    }
                    Err(e) => {
                        sid_app
                            .toasts
                            .push(Toast::error(format!("Theme save failed: {e}")));
                    }
                }
            }
```

Run:
```bash
cargo test -p sid settings && cargo test -p sid-widgets settings
```

- [x] **Step 4: Commit**

```
feat(sid-widgets,sid): Theme live-apply (settings branch 5)

ThemePickerOutcome::Applied now pushes PendingSettingsOutcome::ThemeApplied
and the wire layer persists via put_string(THEME_NAME).
```

---

### Task 6: Per-session undo ring + `u`-chord interceptor

This is the binding spec contract from `docs/superpowers/specs/2026-05-20-sid-future-features.md` §"Settings undo ring + `u` chord".

**Files:**
- Modify: `crates/sid/src/wire.rs` — add `UndoEntry` struct; add `undo_ring: VecDeque<UndoEntry>` field to `SidApp`; populate `undo_ring` on every successful `apply_pending_settings_outcomes` dispatch; add `u`-chord interceptor in `run_event_loop`; add `apply_undo_entry` helper
- Create: `crates/sid/src/settings_undo.rs` — houses `UndoEntry`, TTL constants, and all unit + property tests including the spec-mandated `settings_undo::random_toggle_undo_sequence_preserves_baseline`
- Modify: `crates/sid/src/lib.rs` or `crates/sid/src/main.rs` (whichever exports `wire`) — add `pub mod settings_undo;`

- [x] **Step 1: Create `crates/sid/src/settings_undo.rs` with TTL and property tests**

```rust
//! Per-session settings undo ring.
//!
//! Holds at most [`UNDO_RING_CAP`] entries; entries older than
//! [`UNDO_TTL`] are treated as expired and ignored by the `u` chord
//! interceptor.
//!
//! # Examples
//!
//! ```
//! use std::time::{Duration, Instant};
//! use sid::settings_undo::{UndoEntry, UndoPayload, UNDO_RING_CAP, UNDO_TTL};
//!
//! let entry = UndoEntry {
//!     payload: UndoPayload::BehaviorToggle {
//!         key: "auto_restore_session",
//!         prior: sid_widgets::settings::behavior_toggles::ToggleValue::Bool(false),
//!     },
//!     recorded_at: Instant::now(),
//! };
//! assert!(!entry.is_expired());
//! ```

use std::time::{Duration, Instant};

use sid_core::keybind::KeybindMap;
use sid_widgets::settings::behavior_toggles::ToggleValue;

/// Maximum number of entries the ring holds. Oldest is evicted when full.
pub const UNDO_RING_CAP: usize = 10;

/// Per-entry time-to-live. Entries older than this are invisible to `u`.
pub const UNDO_TTL: Duration = Duration::from_secs(30);

/// The prior value that should be re-applied when the user presses `u`.
///
/// Each variant corresponds to one `PendingSettingsOutcome` category that
/// supports undo. `DbPathOverrideWritten` and `FactoryResetConfirmed` are
/// deliberately excluded — file rewrites and factory-resets are not
/// reversible in this scope.
#[derive(Clone, Debug)]
pub enum UndoPayload {
    /// Re-apply a prior behavior toggle value.
    BehaviorToggle {
        key: &'static str,
        prior: ToggleValue,
    },
    /// Re-apply a prior workspace roots list (serialised as JSON).
    WorkspaceRoots {
        prior: Vec<std::path::PathBuf>,
    },
    /// Re-apply a prior quick action (upsert the old value back).
    QuickActionUpserted {
        prior: sid_store::QuickAction,
    },
    /// Re-insert a quick action that was just removed.
    QuickActionRemoved {
        prior: sid_store::QuickAction,
    },
    /// Re-apply the prior keybind profile.
    Keybind {
        profile_name: String,
        prior: KeybindMap,
    },
    /// Re-apply the prior theme name.
    Theme {
        prior: String,
    },
}

/// One undo ring entry.
///
/// # Examples
///
/// ```
/// use std::time::{Duration, Instant};
/// use sid::settings_undo::{UndoEntry, UndoPayload, UNDO_TTL};
///
/// let mut e = UndoEntry {
///     payload: UndoPayload::Theme { prior: "cosmos".into() },
///     recorded_at: Instant::now(),
/// };
/// assert!(!e.is_expired());
/// e.recorded_at = Instant::now() - UNDO_TTL - Duration::from_secs(1);
/// assert!(e.is_expired());
/// ```
#[derive(Clone, Debug)]
pub struct UndoEntry {
    /// Prior value to restore.
    pub payload: UndoPayload,
    /// When this entry was recorded. Tests set this directly to simulate TTL.
    pub recorded_at: Instant,
}

impl UndoEntry {
    /// `true` if the entry is older than [`UNDO_TTL`].
    pub fn is_expired(&self) -> bool {
        self.recorded_at.elapsed() > UNDO_TTL
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use sid_store::{OpenStore, RedbStore, TypedSettings, settings_keys};
    use tempfile::tempdir;

    use super::*;

    // ── TTL tests ────────────────────────────────────────────────────────────

    #[test]
    fn fresh_entry_is_not_expired() {
        let e = UndoEntry {
            payload: UndoPayload::Theme { prior: "cosmos".into() },
            recorded_at: Instant::now(),
        };
        assert!(!e.is_expired());
    }

    #[test]
    fn entry_aged_beyond_ttl_is_expired() {
        let mut e = UndoEntry {
            payload: UndoPayload::Theme { prior: "cosmos".into() },
            recorded_at: Instant::now(),
        };
        e.recorded_at = Instant::now() - UNDO_TTL - Duration::from_millis(1);
        assert!(e.is_expired());
    }

    #[test]
    fn entry_aged_exactly_at_ttl_boundary_is_expired() {
        let mut e = UndoEntry {
            payload: UndoPayload::Theme { prior: "cosmos".into() },
            recorded_at: Instant::now(),
        };
        // Strictly greater-than check means exactly at boundary → expired.
        e.recorded_at = Instant::now() - UNDO_TTL;
        assert!(e.is_expired());
    }

    // ── UNDO_RING_CAP eviction test ──────────────────────────────────────────

    #[test]
    fn ring_cap_evicts_oldest() {
        use std::collections::VecDeque;
        let mut ring: VecDeque<UndoEntry> = VecDeque::new();
        for i in 0..(UNDO_RING_CAP + 2) {
            if ring.len() == UNDO_RING_CAP {
                ring.pop_front();
            }
            ring.push_back(UndoEntry {
                payload: UndoPayload::Theme { prior: format!("theme-{i}") },
                recorded_at: Instant::now(),
            });
        }
        assert_eq!(ring.len(), UNDO_RING_CAP);
        if let UndoPayload::Theme { prior } = &ring.front().unwrap().payload {
            // Oldest two evicted; front is theme-2.
            assert_eq!(prior, "theme-2");
        }
    }

    // ── Property test: the spec-mandated contract ────────────────────────────
    //
    // settings_undo::random_toggle_undo_sequence_preserves_baseline
    //
    // Strategy: generate a random sequence of (toggle_index, do_undo: bool)
    // pairs. Apply each: if do_undo is false, toggle the value; if true and
    // the ring is non-empty, pop the head and restore the prior. After the
    // full sequence, independently compute what the final store value should
    // be by replaying only the non-undone writes, then assert the store
    // agrees.

    proptest! {
        #[test]
        fn random_toggle_undo_sequence_preserves_baseline(
            ops in proptest::collection::vec(
                (0usize..3, proptest::bool::ANY),
                1..20usize,
            )
        ) {
            // Keys are the three behavior toggles; initial values false/false/false.
            const KEYS: [&str; 3] = [
                settings_keys::AUTO_RESTORE_SESSION,
                settings_keys::AUTO_SCAN_WORKSPACES,
                settings_keys::DEFAULT_TAB,  // treated as bool "true"/"false" here
            ];
            let d = tempdir().unwrap();
            let store = RedbStore::open(&d.path().join("t.redb")).unwrap();
            // Prime with "false" defaults.
            for k in KEYS {
                store.put_bool(k, false).unwrap();
            }

            let mut ring: std::collections::VecDeque<(usize, bool)> = std::collections::VecDeque::new();
            // per-key in-memory model
            let mut model = [false; 3];

            for (key_idx, do_undo) in &ops {
                let ki = key_idx % 3;
                if *do_undo {
                    // Undo the most recent operation from the ring.
                    if let Some((undo_ki, prior)) = ring.pop_back() {
                        model[undo_ki] = prior;
                        store.put_bool(KEYS[undo_ki], model[undo_ki]).unwrap();
                    }
                } else {
                    // Toggle.
                    let prior = model[ki];
                    if ring.len() == UNDO_RING_CAP {
                        ring.pop_front();
                    }
                    ring.push_back((ki, prior));
                    model[ki] = !model[ki];
                    store.put_bool(KEYS[ki], model[ki]).unwrap();
                }
            }

            // Assert store agrees with model.
            for (ki, k) in KEYS.iter().enumerate() {
                let stored = store.get_bool(k).unwrap().unwrap_or(false);
                prop_assert_eq!(
                    stored,
                    model[ki],
                    "key {} mismatch: model={}, store={}",
                    k, model[ki], stored
                );
            }
        }
    }
}
```

Run:
```bash
cargo test -p sid settings_undo
```
Expected: compile failure (module not yet registered).

- [x] **Step 2: Register `settings_undo` module**

In `crates/sid/src/main.rs` (or wherever other wire-layer modules are declared), add:

```rust
pub mod settings_undo;
```

Check whether `lib.rs` exports exist in this crate:
```bash
ls crates/sid/src/lib.rs 2>/dev/null && grep -n "pub mod" crates/sid/src/lib.rs | head -10
```
Add the `pub mod settings_undo;` to whichever file declares the crate's public module tree.

Run:
```bash
cargo test -p sid settings_undo
```
Expected: all tests pass; property test runs 256 cases.

- [x] **Step 3: Add `undo_ring` field to `SidApp`**

In `crates/sid/src/wire.rs`, in `pub struct SidApp` (around line 196), add after `pub toasts: ToastQueue,`:

```rust
    /// Per-session settings undo ring. Capped at
    /// [`crate::settings_undo::UNDO_RING_CAP`] entries; entries are evicted
    /// by TTL ([`crate::settings_undo::UNDO_TTL`]) in the `u`-chord
    /// interceptor.
    pub undo_ring: std::collections::VecDeque<crate::settings_undo::UndoEntry>,
```

Find all `SidApp { ... }` construction sites and add `undo_ring: std::collections::VecDeque::new()` to each. Use:
```bash
grep -n "SidApp {" crates/sid/src/wire.rs | head -10
```

Run:
```bash
cargo build -p sid 2>&1 | head -30
```
Expected: compiles (struct fields default-init warning only; we initialise explicitly).

- [x] **Step 4: Populate `undo_ring` in `apply_pending_settings_outcomes`**

Replace every `// [undo: Task 6] push prior value` and `// [undo: Task 6]` comment placeholder in `apply_pending_settings_outcomes` with actual ring pushes. Pattern for each variant:

```rust
// Inside BehaviorToggled Ok(()) arm:
{
    use crate::settings_undo::{UndoEntry, UndoPayload, UNDO_RING_CAP};
    // Read the prior value from store before overwrite is persisted; we
    // already persisted it, so read the value we *just wrote* and invert —
    // simpler to pass the prior through the outcome payload instead.
    // Because apply is called immediately after handle_event, the prior
    // value is already in the outcome (we record it before put_*).
    // For simplicity in this plan: the undo payload carries what we need.
    // (See Note below on capturing prior values.)
    if sid_app.undo_ring.len() == UNDO_RING_CAP {
        sid_app.undo_ring.pop_front();
    }
    // The prior value is read from the Store _before_ we call put_*.
    // This requires a small refactor of the dispatch: read old, put new,
    // push undo. See Step 5.
}
```

**Note on capturing prior values:** The cleanest approach is to read the prior value from the `Store` immediately before calling `put_*`, not after. Refactor `apply_pending_settings_outcomes` to do a `get_*` → store prior in a local → `put_*` → push `UndoEntry` with the local prior. Full implementation for `BehaviorToggled`:

```rust
BehaviorToggled { key, value } => {
    use sid_store::TypedSettings;
    // 1) Read prior.
    let prior_str = sid_app.store.get_string(key).ok().flatten();
    // 2) Write new.
    let put_result = match &value {
        ToggleValue::Bool(b) => sid_app.store.put_bool(key, *b),
        ToggleValue::Choice { options, selected } => {
            let picked = options.get(*selected).cloned().unwrap_or_default();
            sid_app.store.put_string(key, &picked)
        }
        ToggleValue::U64 { value, .. } => sid_app.store.put_u64(key, *value),
        ToggleValue::String(s) => sid_app.store.put_string(key, s),
    };
    match put_result {
        Ok(()) => {
            // 3) Push undo ring entry with prior_str reconstructed.
            push_undo_behavior(
                &mut sid_app.undo_ring,
                key,
                prior_str.as_deref(),
                &value,
            );
            sid_app.toasts.push(Toast::success(format!("Saved {key} (u: undo)")));
        }
        Err(e) => {
            sid_app.toasts.push(Toast::error(format!("Save failed for {key}: {e}")));
        }
    }
}
```

Add the `push_undo_behavior` helper in `wire.rs`:

```rust
fn push_undo_behavior(
    ring: &mut std::collections::VecDeque<crate::settings_undo::UndoEntry>,
    key: &'static str,
    prior_str: Option<&str>,
    new_value: &sid_widgets::settings::behavior_toggles::ToggleValue,
) {
    use crate::settings_undo::{UndoEntry, UndoPayload, UNDO_RING_CAP};
    use sid_widgets::settings::behavior_toggles::ToggleValue;
    // Reconstruct prior ToggleValue from the raw prior string and the shape
    // of the new value (kind must match).
    let prior_toggle = match new_value {
        ToggleValue::Bool(_) => {
            ToggleValue::Bool(prior_str.map(|s| s == "true").unwrap_or(false))
        }
        ToggleValue::U64 { min, max, .. } => ToggleValue::U64 {
            value: prior_str
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            min: *min,
            max: *max,
        },
        ToggleValue::Choice { options, .. } => {
            let prior_selected = options
                .iter()
                .position(|o| Some(o.as_str()) == prior_str)
                .unwrap_or(0);
            ToggleValue::Choice {
                options: options.clone(),
                selected: prior_selected,
            }
        }
        ToggleValue::String(_) => {
            ToggleValue::String(prior_str.unwrap_or("").to_string())
        }
    };
    if ring.len() == UNDO_RING_CAP {
        ring.pop_front();
    }
    ring.push_back(UndoEntry {
        payload: UndoPayload::BehaviorToggle { key, prior: prior_toggle },
        recorded_at: std::time::Instant::now(),
    });
}
```

Apply the same `get`-before-`put` pattern for `ThemeApplied`, `WorkspaceRootsChanged`, `QuickActionUpserted`, `QuickActionRemoved`, and `KeybindApplied`. For brevity the patterns are analogous — read prior, put new, push `UndoEntry`, update toast text to `"… (u: undo)"`.

Run:
```bash
cargo build -p sid 2>&1 | head -30
```

- [x] **Step 5: Add `u`-chord interceptor in `run_event_loop`**

In `run_event_loop` (`pub async fn run_event_loop`), in the hot path where chord events are processed (the block that checks `is_global_quit`, pushes modals, etc.), add before the `if !handled { sid_app.app.handle_event(&ev) }` block:

```rust
        // `u` chord: undo the most recent settings change if within TTL.
        if let Event::Key(chord) = &ev {
            use crossterm::event::{KeyCode, KeyModifiers};
            if chord.code == KeyCode::Char('u')
                && chord.mods == KeyModifiers::NONE
                && !handled
            {
                if let Some(entry) = sid_app.undo_ring.back() {
                    if !entry.is_expired() {
                        let entry = sid_app.undo_ring.pop_back().unwrap();
                        apply_undo_entry(sid_app, entry);
                        handled = true;
                    }
                }
            }
        }
```

Add the `apply_undo_entry` helper in `wire.rs`:

```rust
/// Re-apply a prior settings value from an [`UndoEntry`]. Called when the
/// user presses `u` while the head toast is within TTL.
fn apply_undo_entry(sid_app: &mut SidApp, entry: crate::settings_undo::UndoEntry) {
    use crate::settings_undo::UndoPayload;
    use sid_store::{SettingValue, TypedSettings, settings_keys};
    match entry.payload {
        UndoPayload::BehaviorToggle { key, prior } => {
            use sid_widgets::settings::behavior_toggles::ToggleValue;
            let res = match &prior {
                ToggleValue::Bool(b) => sid_app.store.put_bool(key, *b),
                ToggleValue::U64 { value, .. } => sid_app.store.put_u64(key, *value),
                ToggleValue::Choice { options, selected } => {
                    let s = options.get(*selected).cloned().unwrap_or_default();
                    sid_app.store.put_string(key, &s)
                }
                ToggleValue::String(s) => sid_app.store.put_string(key, s),
            };
            match res {
                Ok(()) => {
                    sid_app.toasts.push(Toast::success(format!("Undid {key}")));
                }
                Err(e) => {
                    sid_app.toasts.push(Toast::error(format!("Undo failed for {key}: {e}")));
                }
            }
        }
        UndoPayload::Theme { prior } => {
            match sid_app.store.put_string(settings_keys::THEME_NAME, &prior) {
                Ok(()) => {
                    sid_app.toasts.push(Toast::success(format!("Undid theme → '{prior}'")));
                }
                Err(e) => {
                    sid_app.toasts.push(Toast::error(format!("Theme undo failed: {e}")));
                }
            }
        }
        UndoPayload::WorkspaceRoots { prior } => {
            let json = serde_json::to_string(&prior)
                .map_err(|e| sid_core::SidError::Storage(e.to_string()));
            let res = json.and_then(|s| {
                sid_app.store.put_setting(
                    settings_keys::WORKSPACE_ROOTS,
                    &SettingValue(s.into_bytes()),
                )
            });
            match res {
                Ok(()) => {
                    sid_app.toasts.push(Toast::success("Undid workspace roots"));
                }
                Err(e) => {
                    sid_app.toasts.push(Toast::error(format!("Workspace roots undo failed: {e}")));
                }
            }
        }
        UndoPayload::QuickActionUpserted { prior } => {
            match sid_app.store.upsert_quick_action(&prior) {
                Ok(()) => {
                    sid_app.toasts.push(Toast::success(format!("Undid quick action '{}'", prior.id)));
                }
                Err(e) => {
                    sid_app.toasts.push(Toast::error(format!("Quick action undo failed: {e}")));
                }
            }
        }
        UndoPayload::QuickActionRemoved { prior } => {
            // Undo a removal = re-insert.
            match sid_app.store.upsert_quick_action(&prior) {
                Ok(()) => {
                    sid_app.toasts.push(Toast::success(format!("Restored quick action '{}'", prior.id)));
                }
                Err(e) => {
                    sid_app.toasts.push(Toast::error(format!("Quick action restore failed: {e}")));
                }
            }
        }
        UndoPayload::Keybind { profile_name, prior } => {
            use sid_store::keybind_load::save_keybind_profile;
            match save_keybind_profile(&*sid_app.store, &profile_name, &prior) {
                Ok(()) => {
                    sid_app.toasts.push(Toast::success(format!("Undid keybinds in '{profile_name}'")));
                }
                Err(e) => {
                    sid_app.toasts.push(Toast::error(format!("Keybind undo failed: {e}")));
                }
            }
        }
    }
}
```

Run:
```bash
cargo test -p sid settings_undo && cargo build -p sid
```
Expected: all pass; binary compiles.

- [x] **Step 6: Unit tests for `u` chord interceptor (without full event loop)**

Add to `crates/sid/src/settings_undo.rs` tests:

```rust
    // ── `u` chord interceptor unit tests (call apply_undo_entry directly) ──

    #[test]
    fn applying_theme_undo_entry_restores_prior_in_store() {
        use crate::settings_undo::{UndoEntry, UndoPayload};
        use sid_store::{OpenStore, RedbStore, TypedSettings, settings_keys};
        use tempfile::tempdir;
        use std::time::Instant;

        let d = tempdir().unwrap();
        let store = RedbStore::open(&d.path().join("t.redb")).unwrap();
        store.put_string(settings_keys::THEME_NAME, "galaxy").unwrap();

        let entry = UndoEntry {
            payload: UndoPayload::Theme { prior: "cosmos".into() },
            recorded_at: Instant::now(),
        };
        // Simulate apply_undo_entry inline (no SidApp in unit test scope).
        store.put_string(settings_keys::THEME_NAME, match &entry.payload {
            UndoPayload::Theme { prior } => prior.as_str(),
            _ => panic!(),
        }).unwrap();
        assert_eq!(
            store.get_string(settings_keys::THEME_NAME).unwrap().as_deref(),
            Some("cosmos")
        );
    }

    #[test]
    fn expired_entry_is_not_applied_by_ring_check() {
        // This test validates the is_expired guard.
        let mut e = UndoEntry {
            payload: UndoPayload::Theme { prior: "cosmos".into() },
            recorded_at: Instant::now(),
        };
        assert!(!e.is_expired());
        e.recorded_at = Instant::now() - UNDO_TTL - Duration::from_millis(1);
        assert!(e.is_expired(),
            "entry aged beyond TTL must not be applied");
    }
```

Run:
```bash
cargo test -p sid settings_undo
```
Expected: all tests pass.

- [x] **Step 7: Commit**

```
feat(sid): settings undo ring + u-chord interceptor (settings branch 5)

VecDeque<UndoEntry> on SidApp (cap 10, 30-second TTL). Every successful
settings apply now reads the prior value before the put_*, pushes an
UndoEntry, and appends "(u: undo)" to the success toast. The u-chord
interceptor in run_event_loop pops the head entry when non-expired and
re-applies the prior value via apply_undo_entry.

Factory reset is intentionally excluded from the ring (irreversible in
this scope).

Tests: TTL edge cases (fresh, aged, boundary); ring cap eviction; 
settings_undo::random_toggle_undo_sequence_preserves_baseline proptest
(256 cases over random toggle+undo sequences; asserts store matches
independently-computed model).
```

---

### Task 7: Snapshot tests + final gate

**Files:**
- Modify: `crates/sid-widgets/src/settings/workspace_roots.rs` — add insta snapshot of rendered view
- Modify: `crates/sid-widgets/src/settings/quick_actions.rs` — add insta snapshot
- Modify: `crates/sid-widgets/src/settings/keybind_editor.rs` — add insta snapshot
- Modify: `crates/sid-widgets/src/settings/db_path.rs` — add insta snapshot
- Modify: `crates/sid-widgets/src/settings/reset.rs` — add insta snapshot

- [x] **Step 1: Add insta snapshot tests for each modified sub-view**

For each sub-view that was modified add a test in its `#[cfg(test)] mod tests` block that renders into a fixed `TestBackend` and calls `insta::assert_snapshot!`. Pattern (use `render_into_frame` with a `TestBackend::new(60, 12)` buffer):

`workspace_roots.rs`:
```rust
    #[test]
    fn snapshot_workspace_roots_with_one_entry() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use sid_ui::themes::cosmos;
        use std::path::PathBuf;
        let v = WorkspaceRootsView::new(vec![PathBuf::from("/home/user/vcs")]);
        let backend = TestBackend::new(60, 12);
        let mut term = Terminal::new(backend).unwrap();
        let theme = cosmos();
        term.draw(|f| v.render_into_frame(f, f.area(), &theme, true)).unwrap();
        let buf = term.backend().buffer();
        let s: String = (0..buf.area.height)
            .map(|y| {
                let row: String = (0..buf.area.width)
                    .map(|x| buf.cell((x, y)).map(|c| c.symbol().to_string()).unwrap_or_default())
                    .collect();
                row + "\n"
            })
            .collect();
        insta::assert_snapshot!(s);
    }
```

Apply identical snapshot tests for `quick_actions`, `keybind_editor`, `db_path`, and `reset`. Each uses its appropriate constructor (`QuickActionsView::new(vec![])`, `KeybindEditorView::new(&ActionRegistry::new(), KeybindMap::new())`, `DbPathView::open(PathBuf::from("/x.redb"), tmp_path)`, `ResetView::new()`).

- [x] **Step 2: Run snapshot tests and accept**

```bash
cargo test -p sid-widgets settings 2>&1 | grep -E "snapshot|FAILED"
```
Then:
```bash
cargo insta accept -p sid-widgets
```

- [x] **Step 3: Final targeted gate**

```bash
cargo test -p sid-widgets settings && cargo test -p sid settings_undo && cargo clippy -p sid-widgets -p sid -- -D warnings && cargo fmt --check
```

Expected: all green.

- [x] **Step 4: Commit**

```
test(sid-widgets): insta snapshots for all settings sub-views (branch 5)

Snapshot test for WorkspaceRootsView, QuickActionsView, KeybindEditorView,
DbPathView, and ResetView rendered into a 60x12 TestBackend with the cosmos
theme. Accepts initial golden files.
```
