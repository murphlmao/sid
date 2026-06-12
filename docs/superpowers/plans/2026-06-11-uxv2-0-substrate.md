# UX-v2 Branch 0 — Interaction Substrate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking. Read `2026-06-11-uxv2-master.md` first for the
> binding design decisions. Testing is TARGETED per task (decision 11 in the master plan):
> run only the tests named in each step, not the whole workspace.

**Goal:** The shared components every other UX-v2 branch consumes: `FormSpec`/`FormPane`
(side-pane forms with framed fields, Info vs Editable sections, validation, dynamic reshape),
the `SplitView` focus helper (context-aware Tab + drill-in view stack), `ListCursor` with the
synthetic "+ add new" row, `TabManager::push_background`, kitty keyboard-protocol enablement
with `Ctrl+Tab`/`Ctrl+Enter` chord helpers, and wire-layer hosting of the active form pane.

**Architecture:** `sid-widgets` gains a `form` module (spec + event handling + ratatui render)
and small pure helpers (`ListCursor`, `SplitView`). `sid-core` gains `push_background` and
chord-classification helpers (it owns crossterm). The `sid` binary enables keyboard
enhancements at terminal setup and hosts `Option<FormPane>` on the app — rendered as the right
60% of the active tab's body, intercepting events exactly the way the modal layer does today
(see `crates/sid/src/wire.rs` modal routing). No external crate names leak into `sid-widgets`
beyond the existing ratatui exception; `sid-core` still never names ratatui.

**Tech Stack:** Rust, ratatui (sid-widgets only), crossterm (sid-core), insta snapshots,
proptest for the focus-never-strands property.

---

### Task 1: `ListCursor` — selection math with an optional synthetic add-new row

**Files:**
- Create: `crates/sid-widgets/src/list_cursor.rs`
- Modify: `crates/sid-widgets/src/lib.rs` (add `pub mod list_cursor;` next to the existing `pub mod modal;`)

- [x] **Step 1: Write the type + failing tests**

```rust
//! Cursor math for list panes that may carry a synthetic "+ add new" first row.
//!
//! Pure logic — no rendering. Widgets translate their row count into a
//! `ListCursor` and ask it which *item* (if any) is selected. Index 0 is the
//! synthetic add-new row when `add_new` is true; item indices are offset by 1.

/// What the cursor currently points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorTarget {
    /// The synthetic "+ add new" row.
    AddNew,
    /// A real item, by index into the widget's backing vec.
    Item(usize),
    /// List is empty and there is no add-new row.
    Nothing,
}

/// Cursor over `len` items plus an optional synthetic first row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListCursor {
    /// Number of real items.
    pub len: usize,
    /// Whether the synthetic "+ add new" row is shown (governed by the
    /// `show_add_new_row` behavior toggle, hydrated by the binary).
    pub add_new: bool,
    /// Raw cursor position over the combined rows.
    pub pos: usize,
}

impl ListCursor {
    /// Total navigable rows (items + synthetic row).
    pub fn total(&self) -> usize {
        self.len + usize::from(self.add_new)
    }

    /// Build a cursor clamped into range. Position clamps to the last row;
    /// an empty list with no add-new row pins `pos` to 0.
    pub fn new(len: usize, add_new: bool, pos: usize) -> Self {
        let total = len + usize::from(add_new);
        Self { len, add_new, pos: pos.min(total.saturating_sub(1)) }
    }

    /// What the cursor points at.
    pub fn target(&self) -> CursorTarget {
        if self.total() == 0 {
            CursorTarget::Nothing
        } else if self.add_new && self.pos == 0 {
            CursorTarget::AddNew
        } else {
            CursorTarget::Item(self.pos - usize::from(self.add_new))
        }
    }

    /// Move down one row, saturating at the bottom.
    pub fn down(&mut self) {
        if self.pos + 1 < self.total() {
            self.pos += 1;
        }
    }

    /// Move up one row, saturating at the top.
    pub fn up(&mut self) {
        self.pos = self.pos.saturating_sub(1);
    }
}
```

Tests in the same file (`#[cfg(test)] mod tests`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_no_addnew_is_nothing() {
        assert_eq!(ListCursor::new(0, false, 0).target(), CursorTarget::Nothing);
    }

    #[test]
    fn empty_with_addnew_targets_addnew() {
        assert_eq!(ListCursor::new(0, true, 0).target(), CursorTarget::AddNew);
    }

    #[test]
    fn addnew_offsets_item_indices() {
        let c = ListCursor::new(3, true, 2);
        assert_eq!(c.target(), CursorTarget::Item(1));
    }

    #[test]
    fn no_addnew_indices_are_identity() {
        let c = ListCursor::new(3, false, 2);
        assert_eq!(c.target(), CursorTarget::Item(2));
    }

    #[test]
    fn pos_clamps_into_range_and_motion_saturates() {
        let mut c = ListCursor::new(2, true, 99);
        assert_eq!(c.pos, 2); // clamped to last row
        c.down();
        assert_eq!(c.pos, 2); // saturates
        c.up();
        c.up();
        c.up();
        assert_eq!(c.pos, 0); // saturates at top
        assert_eq!(c.target(), CursorTarget::AddNew);
    }

    #[test]
    fn toggling_addnew_off_keeps_selection_valid() {
        // Simulates the behavior toggle flipping mid-session: rebuild with same pos.
        let on = ListCursor::new(3, true, 3); // Item(2)
        let off = ListCursor::new(3, false, on.pos);
        assert_eq!(off.target(), CursorTarget::Item(2)); // pos clamped 3 -> 2
    }
}
```

- [x] **Step 2: Run the tests**

Run: `cargo test -p sid-widgets list_cursor`
Expected: all 6 pass (write type and tests together; the failing-first ritual is waived for
pure-data types this small, per the targeted-testing policy).

- [x] **Step 3: Commit**

```bash
git add crates/sid-widgets/src/list_cursor.rs crates/sid-widgets/src/lib.rs
git commit -m "feat(sid-widgets): ListCursor — selection math with synthetic add-new row"
```

---

### Task 2: `show_add_new_row` settings key + loader

**Files:**
- Modify: `crates/sid-store/src/lib.rs` (the `pub mod settings_keys` block — add one constant beside `AUTO_RESTORE_SESSION`)
- Modify: `crates/sid/src/wire.rs` (add loader fn near `load_animation_config`, ~line 534)

- [x] **Step 1: Add the constant with doc + doc test (match the existing constants' style exactly)**

```rust
/// Whether list panels render the synthetic "+ add new" first row.
///
/// ```
/// use sid_store::settings_keys;
/// assert_eq!(settings_keys::SHOW_ADD_NEW_ROW, "show_add_new_row");
/// ```
pub const SHOW_ADD_NEW_ROW: &str = "show_add_new_row";
```

- [x] **Step 2: Add the loader in `wire.rs` (default ON when unset or unreadable)**

```rust
/// Read the `show_add_new_row` behavior toggle. Defaults to `true` when the
/// key is unset or the stored value is malformed — the add-new row is the
/// discoverable path, so absence of config must never hide it.
pub fn load_show_add_new_row(store: &dyn Store) -> bool {
    match store.get_setting(sid_store::settings_keys::SHOW_ADD_NEW_ROW) {
        Ok(Some(val)) => val.0 != b"false",
        _ => true,
    }
}
```

Note for the executor: `get_setting` returns raw bytes (`SettingValue`); the existing
bool toggles in this codebase persist `b"true"`/`b"false"` via `put_bool` — check
`drain`/`apply_pending_settings_outcomes` (~wire.rs:1716) and mirror whatever encoding
`put_bool` actually writes. If `put_bool` stores something else (e.g. a single byte 0/1),
compare against that instead; the doc-tested contract is only "unset/malformed → true".

- [x] **Step 3: Tests (in `wire.rs` test mod, next to the existing `load_animation_config` tests if present, else new `#[cfg(test)]` fns)**

```rust
#[test]
fn show_add_new_row_defaults_true_when_unset() {
    let store = test_store(); // reuse the existing in-memory/tempdir store helper in this test mod
    assert!(load_show_add_new_row(&*store));
}

#[test]
fn show_add_new_row_respects_stored_false() {
    let store = test_store();
    store
        .put_bool(sid_store::settings_keys::SHOW_ADD_NEW_ROW, false)
        .unwrap();
    assert!(!load_show_add_new_row(&*store));
}
```

Run: `cargo test -p sid show_add_new_row` and `cargo test -p sid-store --doc settings_keys`
Expected: PASS.

- [x] **Step 4: Commit**

```bash
git add crates/sid-store/src/lib.rs crates/sid/src/wire.rs
git commit -m "feat(sid-store,sid): show_add_new_row behavior toggle key + loader (default on)"
```

---

### Task 3: Form spec types — sections, keyed fields, validators, values, reshape

**Files:**
- Create: `crates/sid-widgets/src/form/mod.rs`
- Create: `crates/sid-widgets/src/form/spec.rs`
- Modify: `crates/sid-widgets/src/lib.rs` (add `pub mod form;` and re-export `pub use form::{FormEvent, FormField, FormPane, FormSection, FormSpec, FormValues, PaneFocusState, SectionKind, Validate, render_form_pane};` — the pane/render names land in Tasks 4–5; add them to this re-export in those tasks)

- [x] **Step 1: Write `form/mod.rs`**

```rust
//! Side-pane form substrate (UX-v2).
//!
//! A [`FormSpec`] is a declarative description of an add/edit form or a
//! read-only inspector: ordered sections, each `Editable` or `Info`, holding
//! keyed fields. [`FormPane`] (see `pane.rs`) owns a spec plus focus/dirty
//! state and turns key events into value edits; `render.rs` draws it as the
//! right side pane of a tab body. The binary crate constructs specs and
//! dispatches submits by [`FormId`] — exactly the pattern `ModalSpec` uses,
//! relocated from a centered popup to a framed side pane.

mod pane;
mod render;
mod spec;

pub use pane::{FormEvent, FormPane, PaneFocusState};
pub use render::render_form_pane;
pub use spec::{FormField, FormId, FormSection, FormSpec, FormValues, SectionKind, Validate};
```

(`pane.rs` / `render.rs` arrive in Tasks 4–5; to keep this task compiling, create them as
empty files with just `//! placeholder, filled by Tasks 4-5` and comment the two `pub use`
lines out — uncomment in their tasks.)

- [x] **Step 2: Write `form/spec.rs`**

Reuses the existing `crate::modal::Field` enum for field payloads (Text, Password, Toggle,
Choice, Picker, Display) — do not duplicate it.

```rust
use crate::modal::Field;
use std::collections::BTreeMap;

/// Stable identifier for a form so the binary's submit handler can dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormId(pub String);

/// Whether a section's fields are user-editable or read-only facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionKind {
    /// Framed input boxes; participates in Tab focus order.
    Editable,
    /// Muted key→value rows; skipped by focus, never editable.
    Info,
}

/// Declarative validators — data, not closures, so specs stay `Clone + Debug`
/// and validators are unit-testable in isolation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Validate {
    /// Value must be non-empty after trimming.
    NonEmpty,
    /// Value must parse as a u16 >= 1 (TCP port).
    Port,
    /// Value must parse as u64.
    Unsigned,
}

impl Validate {
    /// Check `value`; `None` means valid, `Some(msg)` is the error rendered
    /// under the field box.
    pub fn check(self, value: &str) -> Option<String> {
        match self {
            Validate::NonEmpty => {
                if value.trim().is_empty() {
                    Some("required".to_string())
                } else {
                    None
                }
            }
            Validate::Port => match value.trim().parse::<u16>() {
                Ok(p) if p >= 1 => None,
                _ => Some("must be a port (1-65535)".to_string()),
            },
            Validate::Unsigned => {
                if value.trim().parse::<u64>().is_ok() {
                    None
                } else {
                    Some("must be a whole number".to_string())
                }
            }
        }
    }
}

/// One keyed field inside a section.
#[derive(Debug, Clone)]
pub struct FormField {
    /// Stable key — survives reshapes, names the value in [`FormValues`].
    pub key: String,
    /// Visual + input payload (reuses the modal `Field` enum).
    pub field: Field,
    /// Validators run on every edit and on submit.
    pub validate: Vec<Validate>,
    /// Current validation error, if any (set by `FormPane`, rendered red).
    pub error: Option<String>,
}

impl FormField {
    /// Convenience constructor with no validators.
    pub fn new(key: impl Into<String>, field: Field) -> Self {
        Self { key: key.into(), field, validate: Vec::new(), error: None }
    }

    /// Builder: attach validators.
    pub fn with_validate(mut self, v: Vec<Validate>) -> Self {
        self.validate = v;
        self
    }

    /// The field's current value as a string: Text/Password/Picker → the raw
    /// value, Choice → the selected option text, Toggle → "true"/"false",
    /// Display → its body.
    pub fn value_string(&self) -> String {
        match &self.field {
            Field::Text { value, .. }
            | Field::Password { value, .. }
            | Field::Picker { value, .. } => value.clone(),
            Field::Choice { options, selected, .. } => {
                options.get(*selected).cloned().unwrap_or_default()
            }
            Field::Toggle { value, .. } => value.to_string(),
            Field::Display { body, .. } => body.clone(),
        }
    }
}

/// A titled group of fields.
#[derive(Debug, Clone)]
pub struct FormSection {
    /// Section heading (e.g. "Connection", "Derived").
    pub title: String,
    /// Editable vs Info.
    pub kind: SectionKind,
    /// Ordered fields.
    pub fields: Vec<FormField>,
}

/// Snapshot of all field values by key. Reshape hooks and submit handlers
/// consume this; it's a plain map so the binary crate never touches widget
/// internals.
pub type FormValues = BTreeMap<String, String>;

/// Rebuilds the section list when a watched field changes. A plain `fn`
/// pointer (not a boxed closure) keeps `FormSpec: Clone + Debug` and forces
/// reshape logic to be a pure, testable function of the values.
pub type ReshapeFn = fn(&FormValues) -> Vec<FormSection>;

/// A whole side-pane form.
#[derive(Debug, Clone)]
pub struct FormSpec {
    /// Dispatch identity (e.g. `database.connection.edit`).
    pub id: FormId,
    /// Pane title.
    pub title: String,
    /// Ordered sections.
    pub sections: Vec<FormSection>,
    /// Primary button label (default "Save").
    pub primary_label: String,
    /// Keys that trigger `reshape` when their value changes.
    pub watch: Vec<String>,
    /// Optional reshape hook.
    pub reshape: Option<ReshapeFn>,
}

impl FormSpec {
    /// Standard form with a "Save" primary button and no reshape.
    pub fn new(id: impl Into<String>, title: impl Into<String>, sections: Vec<FormSection>) -> Self {
        Self {
            id: FormId(id.into()),
            title: title.into(),
            sections,
            primary_label: "Save".to_string(),
            watch: Vec::new(),
            reshape: None,
        }
    }

    /// Builder: watch `keys` and rebuild sections via `f` when one changes.
    pub fn with_reshape(mut self, keys: Vec<String>, f: ReshapeFn) -> Self {
        self.watch = keys;
        self.reshape = Some(f);
        self
    }

    /// Current values of every field, keyed.
    pub fn values(&self) -> FormValues {
        self.sections
            .iter()
            .flat_map(|s| s.fields.iter())
            .map(|f| (f.key.clone(), f.value_string()))
            .collect()
    }

    /// Apply a reshape: rebuild sections from `f`, then copy back the values
    /// of every surviving editable key so user input is never lost.
    pub fn run_reshape(&mut self) {
        let Some(f) = self.reshape else { return };
        let old = self.values();
        let mut next = f(&old);
        for section in &mut next {
            if section.kind != SectionKind::Editable {
                continue;
            }
            for field in &mut section.fields {
                if let Some(prev) = old.get(&field.key) {
                    restore_value(&mut field.field, prev);
                }
            }
        }
        self.sections = next;
    }

    /// First validation error across all fields, if any (submit gate).
    pub fn first_error(&self) -> Option<(String, String)> {
        self.sections.iter().flat_map(|s| s.fields.iter()).find_map(|f| {
            f.validate
                .iter()
                .find_map(|v| v.check(&f.value_string()))
                .map(|e| (f.key.clone(), e))
        })
    }
}

/// Write `prev` back into a rebuilt field, shape-aware: free-text fields take
/// the string verbatim; a Choice re-selects a matching option (else keeps the
/// reshape's default); a Toggle parses "true"/"false".
fn restore_value(field: &mut Field, prev: &str) {
    match field {
        Field::Text { value, .. } | Field::Password { value, .. } | Field::Picker { value, .. } => {
            *value = prev.to_string();
        }
        Field::Choice { options, selected, .. } => {
            if let Some(idx) = options.iter().position(|o| o == prev) {
                *selected = idx;
            }
        }
        Field::Toggle { value, .. } => {
            if let Ok(b) = prev.parse::<bool>() {
                *value = b;
            }
        }
        Field::Display { .. } => {}
    }
}
```

- [x] **Step 3: Tests (sibling `#[cfg(test)] mod tests` in `spec.rs`)**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::modal::Field;

    fn text(key: &str, val: &str) -> FormField {
        FormField::new(
            key,
            Field::Text { label: key.to_string(), value: val.to_string(), placeholder: None },
        )
    }

    fn pg_or_sqlite(values: &FormValues) -> Vec<FormSection> {
        let kind = values.get("kind").map(String::as_str).unwrap_or("Postgres");
        let mut fields = vec![FormField::new(
            "kind",
            Field::Choice {
                label: "kind".into(),
                options: vec!["Postgres".into(), "SQLite".into()],
                selected: if kind == "SQLite" { 1 } else { 0 },
            },
        )];
        if kind == "SQLite" {
            fields.push(text("path", ""));
        } else {
            fields.push(text("host", ""));
            fields.push(text("port", "5432").with_validate(vec![Validate::Port]));
        }
        vec![FormSection { title: "Connection".into(), kind: SectionKind::Editable, fields }]
    }

    #[test]
    fn validate_port_rejects_junk_and_zero() {
        assert!(Validate::Port.check("5432").is_none());
        assert!(Validate::Port.check("0").is_some());
        assert!(Validate::Port.check("notaport").is_some());
        assert!(Validate::Port.check("70000").is_some());
    }

    #[test]
    fn values_snapshot_covers_all_field_shapes() {
        let spec = FormSpec::new(
            "t",
            "T",
            vec![FormSection {
                title: "s".into(),
                kind: SectionKind::Editable,
                fields: vec![
                    text("name", "prod"),
                    FormField::new(
                        "kind",
                        Field::Choice {
                            label: "kind".into(),
                            options: vec!["A".into(), "B".into()],
                            selected: 1,
                        },
                    ),
                    FormField::new("on", Field::Toggle { label: "on".into(), value: true }),
                ],
            }],
        );
        let v = spec.values();
        assert_eq!(v["name"], "prod");
        assert_eq!(v["kind"], "B");
        assert_eq!(v["on"], "true");
    }

    #[test]
    fn reshape_preserves_surviving_keys_and_drops_dead_ones() {
        let mut spec = FormSpec::new("t", "T", pg_or_sqlite(&FormValues::new()))
            .with_reshape(vec!["kind".into()], pg_or_sqlite);
        // user types a host, then flips kind to SQLite
        if let Field::Text { value, .. } = &mut spec.sections[0].fields[1].field {
            *value = "10.0.0.5".into();
        }
        if let Field::Choice { selected, .. } = &mut spec.sections[0].fields[0].field {
            *selected = 1;
        }
        spec.run_reshape();
        let v = spec.values();
        assert_eq!(v["kind"], "SQLite");
        assert!(v.contains_key("path"));
        assert!(!v.contains_key("host")); // dead key dropped
        // flip back: port default restored, host is empty again (dead keys are not resurrected)
        if let Field::Choice { selected, .. } = &mut spec.sections[0].fields[0].field {
            *selected = 0;
        }
        spec.run_reshape();
        assert_eq!(spec.values()["port"], "5432");
    }

    #[test]
    fn reshape_is_idempotent_when_nothing_changed() {
        let mut spec = FormSpec::new("t", "T", pg_or_sqlite(&FormValues::new()))
            .with_reshape(vec!["kind".into()], pg_or_sqlite);
        spec.run_reshape();
        let once = spec.values();
        spec.run_reshape();
        assert_eq!(once, spec.values());
    }

    #[test]
    fn first_error_finds_invalid_port() {
        let mut spec = FormSpec::new("t", "T", pg_or_sqlite(&FormValues::new()));
        if let Field::Text { value, .. } = &mut spec.sections[0].fields[2].field {
            *value = "nope".into();
        }
        let (key, _msg) = spec.first_error().expect("port error");
        assert_eq!(key, "port");
    }
}
```

- [x] **Step 4: Run the tests**

Run: `cargo test -p sid-widgets form::spec`
Expected: 5 tests pass.

- [x] **Step 5: Commit**

```bash
git add crates/sid-widgets/src/form/ crates/sid-widgets/src/lib.rs
git commit -m "feat(sid-widgets): FormSpec — keyed sections, declarative validators, value-preserving reshape"
```

---

### Task 4: `FormPane` — focus, editing, dirty tracking, submit/cancel events

**Files:**
- Create: `crates/sid-widgets/src/form/pane.rs` (replace the Task-3 placeholder; uncomment its `pub use` in `form/mod.rs`)

- [x] **Step 1: Write the pane state machine**

Editing primitives (`type_char`, `backspace`, choice cycling) already exist on `ModalSpec` —
copy their per-`Field` match logic into private helpers here rather than calling `ModalSpec`
(the modal keeps its own focus model; sharing state types would couple the two lifecycles).

```rust
use super::spec::{FormSpec, FormValues, SectionKind};
use crate::modal::Field;
use crossterm::event::{KeyCode, KeyModifiers};
use sid_core::event::KeyChord;

/// Where focus sits inside the pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneFocusState {
    /// A field, by flat index over editable sections only.
    Field(usize),
    /// The primary (Save) button.
    Primary,
}

/// What the host (wire layer) must do after a key event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormEvent {
    /// Keep showing the pane; redraw.
    Continue,
    /// User confirmed submit and validation passed — values snapshot attached.
    Submit(FormValues),
    /// User left the pane (Esc / ← on a clean form, or confirmed discard).
    Cancel,
    /// Form is dirty and user pressed Esc/← — host must show the standard
    /// "discard changes?" confirm modal; on confirm, host closes the pane.
    RequestDiscardConfirm,
}

/// Live form pane: spec + focus + dirty flag.
#[derive(Debug, Clone)]
pub struct FormPane {
    /// The declarative form.
    pub spec: FormSpec,
    /// Current focus.
    pub focus: PaneFocusState,
    /// Set on first successful edit; drives the discard confirm.
    pub dirty: bool,
    /// Values at open time, for dirty comparison.
    baseline: FormValues,
}

impl FormPane {
    /// Open a pane focused on the first editable field.
    pub fn new(spec: FormSpec) -> Self {
        let baseline = spec.values();
        Self { spec, focus: PaneFocusState::Field(0), dirty: false, baseline }
    }

    /// Flat list of `(section_idx, field_idx)` for editable-section fields,
    /// in render order — the Tab traversal order.
    fn editable_slots(&self) -> Vec<(usize, usize)> {
        self.spec
            .sections
            .iter()
            .enumerate()
            .filter(|(_, s)| s.kind == SectionKind::Editable)
            .flat_map(|(si, s)| (0..s.fields.len()).map(move |fi| (si, fi)))
            .collect()
    }

    fn focused_slot(&self) -> Option<(usize, usize)> {
        match self.focus {
            PaneFocusState::Field(i) => self.editable_slots().get(i).copied(),
            PaneFocusState::Primary => None,
        }
    }

    /// Route one key chord. Returns what the host must do.
    pub fn handle_key(&mut self, chord: KeyChord) -> FormEvent {
        let slots = self.editable_slots().len();
        match (chord.code, chord.mods) {
            (KeyCode::Tab, m) if m.is_empty() => {
                self.focus = match self.focus {
                    PaneFocusState::Field(i) if i + 1 < slots => PaneFocusState::Field(i + 1),
                    PaneFocusState::Field(_) => PaneFocusState::Primary,
                    PaneFocusState::Primary => PaneFocusState::Field(0),
                };
                FormEvent::Continue
            }
            // NB: KeyModifiers is a bitflags struct — it cannot appear as a
            // match *pattern*; classify via guards.
            (KeyCode::BackTab, _) => self.focus_prev(slots),
            (KeyCode::Tab, m) if m.contains(KeyModifiers::SHIFT) => self.focus_prev(slots),
            (KeyCode::Esc, _) => self.leave(),
            (KeyCode::Left, _) if !self.focused_field_is_text() => self.leave(),
            (KeyCode::Enter, _) => match self.focus {
                PaneFocusState::Primary => self.try_submit(),
                PaneFocusState::Field(_) => {
                    // Enter on a field = advance (form-filling muscle memory);
                    // Enter on the last field falls onto Save.
                    self.handle_key(KeyChord { code: KeyCode::Tab, mods: KeyModifiers::empty() })
                }
            },
            _ => {
                self.edit_focused(chord);
                FormEvent::Continue
            }
        }
    }

    /// Shift+Tab / BackTab: previous field, wrapping list ↔ Save button.
    fn focus_prev(&mut self, slots: usize) -> FormEvent {
        self.focus = match self.focus {
            PaneFocusState::Field(0) => PaneFocusState::Primary,
            PaneFocusState::Field(i) => PaneFocusState::Field(i - 1),
            PaneFocusState::Primary if slots == 0 => PaneFocusState::Primary,
            PaneFocusState::Primary => PaneFocusState::Field(slots - 1),
        };
        FormEvent::Continue
    }

    fn leave(&mut self) -> FormEvent {
        if self.dirty && self.spec.values() != self.baseline {
            FormEvent::RequestDiscardConfirm
        } else {
            FormEvent::Cancel
        }
    }

    fn try_submit(&mut self) -> FormEvent {
        self.revalidate_all();
        if self.spec.first_error().is_some() {
            // Jump focus to the first offending field.
            if let Some(idx) = self.first_error_slot() {
                self.focus = PaneFocusState::Field(idx);
            }
            return FormEvent::Continue;
        }
        FormEvent::Submit(self.spec.values())
    }

    fn first_error_slot(&self) -> Option<usize> {
        let slots = self.editable_slots();
        slots.iter().position(|&(si, fi)| self.spec.sections[si].fields[fi].error.is_some())
    }

    fn revalidate_all(&mut self) {
        for section in &mut self.spec.sections {
            for field in &mut section.fields {
                field.error =
                    field.validate.iter().find_map(|v| v.check(&field.value_string()));
            }
        }
    }

    fn focused_field_is_text(&self) -> bool {
        self.focused_slot().is_some_and(|(si, fi)| {
            matches!(
                self.spec.sections[si].fields[fi].field,
                Field::Text { .. } | Field::Password { .. } | Field::Picker { .. }
            )
        })
    }

    /// Apply a printable/backspace/arrow edit to the focused field; runs the
    /// field's validators, marks dirty, and fires reshape on watched keys.
    fn edit_focused(&mut self, chord: KeyChord) {
        let Some((si, fi)) = self.focused_slot() else { return };
        let key = self.spec.sections[si].fields[fi].key.clone();
        let changed = {
            let f = &mut self.spec.sections[si].fields[fi];
            let changed = match (&mut f.field, chord.code) {
                (
                    Field::Text { value, .. }
                    | Field::Password { value, .. }
                    | Field::Picker { value, .. },
                    KeyCode::Char(c),
                ) => {
                    value.push(c);
                    true
                }
                (
                    Field::Text { value, .. }
                    | Field::Password { value, .. }
                    | Field::Picker { value, .. },
                    KeyCode::Backspace,
                ) => value.pop().is_some(),
                (Field::Choice { options, selected, .. }, KeyCode::Right | KeyCode::Char(' ')) => {
                    *selected = (*selected + 1) % options.len().max(1);
                    true
                }
                // No Left arms for Choice/Toggle: ← on a non-text field LEAVES
                // the pane (handled before edit_focused is reached). Choice
                // cycles forward-only via Right/Space, wrapping.
                (Field::Toggle { value, .. }, KeyCode::Char(' ') | KeyCode::Right) => {
                    *value = !*value;
                    true
                }
                _ => false,
            };
            if changed {
                f.error = f.validate.iter().find_map(|v| v.check(&f.value_string()));
            }
            changed
        };
        if changed {
            self.dirty = true;
            if self.spec.watch.contains(&key) {
                self.spec.run_reshape();
                // Reshape may shrink the slot list; clamp focus.
                let slots = self.editable_slots().len();
                if let PaneFocusState::Field(i) = self.focus {
                    if i >= slots && slots > 0 {
                        self.focus = PaneFocusState::Field(slots - 1);
                    }
                }
            }
        }
    }
}
```

- [x] **Step 2: Tests (`#[cfg(test)] mod tests` in `pane.rs`)**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use super::super::spec::{FormField, FormSection, FormSpec, SectionKind, Validate};
    use crate::modal::Field;
    use crossterm::event::{KeyCode, KeyModifiers};

    fn chord(code: KeyCode) -> KeyChord {
        KeyChord { code, mods: KeyModifiers::empty() }
    }

    fn two_field_form() -> FormPane {
        FormPane::new(FormSpec::new(
            "t",
            "T",
            vec![FormSection {
                title: "s".into(),
                kind: SectionKind::Editable,
                fields: vec![
                    FormField::new(
                        "name",
                        Field::Text { label: "name".into(), value: String::new(), placeholder: None },
                    )
                    .with_validate(vec![Validate::NonEmpty]),
                    FormField::new(
                        "port",
                        Field::Text { label: "port".into(), value: "5432".into(), placeholder: None },
                    )
                    .with_validate(vec![Validate::Port]),
                ],
            }],
        ))
    }

    #[test]
    fn tab_cycles_fields_then_save_then_wraps() {
        let mut p = two_field_form();
        assert_eq!(p.focus, PaneFocusState::Field(0));
        p.handle_key(chord(KeyCode::Tab));
        assert_eq!(p.focus, PaneFocusState::Field(1));
        p.handle_key(chord(KeyCode::Tab));
        assert_eq!(p.focus, PaneFocusState::Primary);
        p.handle_key(chord(KeyCode::Tab));
        assert_eq!(p.focus, PaneFocusState::Field(0));
    }

    #[test]
    fn esc_on_clean_form_cancels_but_dirty_requests_confirm() {
        let mut p = two_field_form();
        assert_eq!(p.handle_key(chord(KeyCode::Esc)), FormEvent::Cancel);
        p.handle_key(chord(KeyCode::Char('x')));
        assert_eq!(p.handle_key(chord(KeyCode::Esc)), FormEvent::RequestDiscardConfirm);
    }

    #[test]
    fn typing_then_backspace_to_baseline_is_clean_again() {
        let mut p = two_field_form();
        p.handle_key(chord(KeyCode::Char('x')));
        p.handle_key(chord(KeyCode::Backspace));
        // dirty flag is sticky but leave() compares values to baseline
        assert_eq!(p.handle_key(chord(KeyCode::Esc)), FormEvent::Cancel);
    }

    #[test]
    fn submit_blocked_on_invalid_field_and_focus_jumps_there() {
        let mut p = two_field_form();
        // empty name violates NonEmpty
        p.focus = PaneFocusState::Primary;
        assert_eq!(p.handle_key(chord(KeyCode::Enter)), FormEvent::Continue);
        assert_eq!(p.focus, PaneFocusState::Field(0));
    }

    #[test]
    fn valid_form_submits_values() {
        let mut p = two_field_form();
        for c in "prod".chars() {
            p.handle_key(chord(KeyCode::Char(c)));
        }
        p.focus = PaneFocusState::Primary;
        match p.handle_key(chord(KeyCode::Enter)) {
            FormEvent::Submit(v) => {
                assert_eq!(v["name"], "prod");
                assert_eq!(v["port"], "5432");
            }
            other => panic!("expected Submit, got {other:?}"),
        }
    }

    #[test]
    fn left_arrow_leaves_pane_only_on_non_text_fields() {
        let mut p = two_field_form();
        // focused field is Text — Left must NOT leave (it's a no-op edit here)
        assert_eq!(p.handle_key(chord(KeyCode::Left)), FormEvent::Continue);
        p.focus = PaneFocusState::Primary;
        assert_eq!(p.handle_key(chord(KeyCode::Left)), FormEvent::Cancel);
    }

    #[test]
    fn enter_on_field_advances_instead_of_submitting() {
        let mut p = two_field_form();
        assert_eq!(p.handle_key(chord(KeyCode::Enter)), FormEvent::Continue);
        assert_eq!(p.focus, PaneFocusState::Field(1));
    }
}
```

- [x] **Step 3: Run the tests**

Run: `cargo test -p sid-widgets form::pane`
Expected: 7 tests pass.

- [x] **Step 4: Property test — no chord sequence strands focus or panics**

Append to the test mod (proptest is already a workspace dev-dependency):

```rust
    use proptest::prelude::*;

    fn arbitrary_chord() -> impl Strategy<Value = KeyChord> {
        prop_oneof![
            Just(chord(KeyCode::Tab)),
            Just(KeyChord { code: KeyCode::Tab, mods: KeyModifiers::SHIFT }),
            Just(chord(KeyCode::BackTab)),
            Just(chord(KeyCode::Enter)),
            Just(chord(KeyCode::Esc)),
            Just(chord(KeyCode::Left)),
            Just(chord(KeyCode::Right)),
            Just(chord(KeyCode::Backspace)),
            any::<char>().prop_filter("printable", |c| c.is_ascii_graphic())
                .prop_map(|c| chord(KeyCode::Char(c))),
        ]
    }

    proptest! {
        #[test]
        fn focus_never_strands(keys in prop::collection::vec(arbitrary_chord(), 0..64)) {
            let mut p = two_field_form();
            for k in keys {
                let _ = p.handle_key(k);
                // invariant: focus always points at a real slot or Primary
                match p.focus {
                    PaneFocusState::Field(i) => prop_assert!(i < 2),
                    PaneFocusState::Primary => {}
                }
            }
        }
    }
```

Run: `cargo test -p sid-widgets form::pane`
Expected: PASS (including the property).

- [x] **Step 5: Commit**

```bash
git add crates/sid-widgets/src/form/
git commit -m "feat(sid-widgets): FormPane — focus cycling, validation gate, dirty-aware cancel, reshape on watched keys"
```

---

### Task 5: Form rendering — framed boxes, Info rows, errors, buttons

**Files:**
- Create: `crates/sid-widgets/src/form/render.rs` (replace placeholder; uncomment `pub use` in `form/mod.rs`)
- Create: `crates/sid-widgets/tests/snapshots/` entries via insta (auto)

- [x] **Step 1: Write the renderer**

Follow the rendering conventions of `crates/sid-widgets/src/modal.rs` (theme access, style
helpers) — read its render fn first and reuse its theme-to-ratatui plumbing. Public surface:

```rust
use super::pane::{FormPane, PaneFocusState};
use super::spec::SectionKind;
use crate::modal::Field;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget as _};
use sid_ui::Theme;

/// Render the form pane into `area` (the right split of the tab body).
///
/// Layout, top to bottom: title bar (pane border carries the form title),
/// then per section: section heading, then per field —
/// * Editable: label gutter (left, 12 cols) + one framed single-row box with
///   the value (Password renders bullets; Choice renders segmented
///   `( opt )(·opt·)` spans; Toggle renders `[x]`/`[ ]`), focused field's
///   frame uses the accent color, error line in red directly underneath.
/// * Info: `label  value` muted row, no frame.
/// Bottom: `[ <primary_label> ⏎ ]` button, accent-inverted when focused.
pub fn render_form_pane(buf: &mut Buffer, area: Rect, pane: &FormPane, theme: &Theme) {
    // implementation per the docstring — see modal.rs for the existing
    // bordered-block + line-by-line writing idiom this must follow
}
```

The executor writes the body following `modal.rs`'s idiom (Block with border + title, then
manual `Line`/`Span` rows into the buffer, advancing a `y` cursor; skip rendering when
`area.width < 20` — return early, matching modal's small-terminal guard). Field boxes are
`Block::default().borders(Borders::ALL)` of height 3 around a single value line; the label
sits in a left gutter column (width 12, right-aligned, truncated with `…`). Concrete style
rules: focused frame `fg(theme.accent_primary)`, unfocused `fg(theme.border)`, error text
`fg(theme.danger)` (check the actual Theme field names in `crates/sid-ui/src/themes.rs` —
use the same fields modal.rs uses for its accent/border/danger styling).

- [x] **Step 2: Snapshot tests**

In `render.rs` test mod, render into a fixed 60x24 `Buffer` and snapshot the ASCII (same
golden-file pattern used by existing widget snapshot tests — find one with
`rg -l "insta::assert_snapshot" crates/sid-widgets/src` and copy its buffer→string helper):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    // build the same two_field_form as pane.rs tests, plus an Info section:
    //   Section "Connection" (Editable): name Text (focused), port Text
    //   Section "Derived" (Info): dsn = "postgres://10.0.0.5:5432/app"

    #[test]
    fn snapshot_default_form() { /* render, insta::assert_snapshot! */ }

    #[test]
    fn snapshot_validation_error_under_field() {
        // empty name + revalidate_all() -> error row appears under the name box
    }

    #[test]
    fn snapshot_info_section_is_unframed_and_muted() { /* … */ }

    #[test]
    fn snapshot_focused_save_button() { /* focus = Primary */ }

    #[test]
    fn narrow_area_renders_without_panic() {
        // 10x5 area: early-return guard; buffer unchanged, no panic
    }
}
```

Run: `cargo test -p sid-widgets form::render` then `cargo insta review` (accept the 4 new snapshots).
Expected: 5 tests pass after snapshot acceptance.

- [x] **Step 3: Commit**

```bash
git add crates/sid-widgets/src/form/ crates/sid-widgets/src/snapshots/
git commit -m "feat(sid-widgets): form pane renderer — framed fields, info rows, inline errors, save button"
```

---

### Task 6: `SplitView` — list/pane focus + drill-in view stack

**Files:**
- Create: `crates/sid-widgets/src/split_view.rs`
- Modify: `crates/sid-widgets/src/lib.rs` (add `pub mod split_view;`)

- [x] **Step 1: Write the helper**

This is the focus model for widgets with *internal* right panes (workspace detail's ops →
commits → diff stack). Generic over the widget's own view enum.

```rust
//! List/pane focus state + drill-in stack for split-layout widgets.
//!
//! `V` is the widget's own view enum (e.g. workspace-detail's
//! `OpsMenu | Commits | Diff`). The widget pushes views as the user drills in;
//! `←` pops one level and finally returns focus to the list. This is the
//! single source of truth for "context-aware Tab": when `focus()` is `List`,
//! the widget must return `EventOutcome::Bubble` for Tab so the global
//! tab-strip cycling sees it; when `Pane`, the widget consumes Tab itself.

/// Which side owns key events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitFocus {
    /// Left list: ↑↓ select, → or Enter dives in, Tab bubbles to tab strip.
    List,
    /// Right pane: keys go to the top view of the stack, ← pops.
    Pane,
}

/// Focus + drill-in stack.
#[derive(Debug, Clone)]
pub struct SplitView<V> {
    stack: Vec<V>,
    focus: SplitFocus,
}

impl<V> Default for SplitView<V> {
    fn default() -> Self {
        Self { stack: Vec::new(), focus: SplitFocus::List }
    }
}

impl<V> SplitView<V> {
    /// Current focus side.
    pub fn focus(&self) -> SplitFocus {
        self.focus
    }

    /// Top of the drill-in stack, if any.
    pub fn top(&self) -> Option<&V> {
        self.stack.last()
    }

    /// Depth of the drill-in stack (for breadcrumb rendering).
    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// Enter the pane, pushing `view` onto the stack.
    pub fn push(&mut self, view: V) {
        self.stack.push(view);
        self.focus = SplitFocus::Pane;
    }

    /// Replace the whole stack with a single root view and focus the pane.
    /// (Used when list selection changes: the ops menu re-roots.)
    pub fn reroot(&mut self, view: V) {
        self.stack.clear();
        self.stack.push(view);
        self.focus = SplitFocus::Pane;
    }

    /// Pop one level; when the stack empties, focus returns to the list.
    /// Returns `true` if a pop happened (the caller consumed the key).
    pub fn pop(&mut self) -> bool {
        if self.stack.pop().is_some() {
            if self.stack.is_empty() {
                self.focus = SplitFocus::List;
            }
            true
        } else if self.focus == SplitFocus::Pane {
            self.focus = SplitFocus::List;
            true
        } else {
            false
        }
    }

    /// Drop everything and focus the list (e.g. list contents reloaded).
    pub fn reset(&mut self) {
        self.stack.clear();
        self.focus = SplitFocus::List;
    }
}
```

- [x] **Step 2: Tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum V {
        Ops,
        Commits,
        Diff,
    }

    #[test]
    fn starts_on_list_with_empty_stack() {
        let s: SplitView<V> = SplitView::default();
        assert_eq!(s.focus(), SplitFocus::List);
        assert_eq!(s.top(), None);
    }

    #[test]
    fn push_enters_pane_and_pop_unwinds_to_list() {
        let mut s = SplitView::default();
        s.push(V::Ops);
        s.push(V::Commits);
        s.push(V::Diff);
        assert_eq!(s.depth(), 3);
        assert!(s.pop()); // Diff -> Commits
        assert_eq!(s.top(), Some(&V::Commits));
        assert!(s.pop());
        assert!(s.pop()); // stack empty -> back to list
        assert_eq!(s.focus(), SplitFocus::List);
        assert!(!s.pop()); // on list: ← is not ours, let the widget bubble it
    }

    #[test]
    fn reroot_replaces_stack() {
        let mut s = SplitView::default();
        s.push(V::Ops);
        s.push(V::Diff);
        s.reroot(V::Ops);
        assert_eq!(s.depth(), 1);
        assert_eq!(s.focus(), SplitFocus::Pane);
    }

    #[test]
    fn pop_with_pane_focus_but_empty_stack_returns_to_list() {
        let mut s: SplitView<V> = SplitView::default();
        s.push(V::Ops);
        s.pop();
        // second pop on list focus is a no-op
        assert!(!s.pop());
        assert_eq!(s.focus(), SplitFocus::List);
    }
}
```

Run: `cargo test -p sid-widgets split_view`
Expected: 4 tests pass.

- [x] **Step 3: Commit**

```bash
git add crates/sid-widgets/src/split_view.rs crates/sid-widgets/src/lib.rs
git commit -m "feat(sid-widgets): SplitView — list/pane focus with drill-in stack, left-pop semantics"
```

---

### Task 7: `TabManager::push_background`

**Files:**
- Modify: `crates/sid-core/src/tab.rs` (next to `push_detail`, ~line 309 — read `push_detail` first and mirror its validation/error behavior exactly)

- [x] **Step 1: Implementation**

```rust
/// Push a detail tab WITHOUT switching the active index — the background
/// variant of [`TabManager::push_detail`] backing `Ctrl+Enter` / `O`
/// "open in background tab".
///
/// Same validation and error conditions as `push_detail`; the only
/// difference is that the currently-active tab stays active.
///
/// # Examples
///
/// ```
/// # use sid_core::tab::{Tab, TabId, TabKind, TabManager};
/// # let base = Tab::new(TabId::new("home"), "home", TabKind::Primary);
/// # let detail = Tab::new(TabId::new("ws-detail"), "gen4", TabKind::Detail);
/// let mut mgr = TabManager::new(vec![base]);
/// let before = mgr.active_index();
/// mgr.push_background(detail).unwrap();
/// assert_eq!(mgr.active_index(), before); // focus unchanged
/// assert_eq!(mgr.tabs().len(), 2);
/// ```
pub fn push_background(&mut self, tab: Tab) -> Result<(), crate::SidError> {
    let active = self.active_index();
    self.push_detail(tab)?;
    self.jump(active);
    Ok(())
}
```

Note for the executor: this delegates to `push_detail` then restores the index — read
`push_detail` first; if it inserts the new tab *before* the active index (it appends in
the current implementation, but verify), compute the restore index accordingly. Adjust the
`Tab::new` doc-test constructor call to the real `Tab` constructor signature in this file
(check how existing doc tests in tab.rs build a `Tab`).

- [x] **Step 2: Tests (in tab.rs's existing test mod)**

```rust
#[test]
fn push_background_keeps_active_and_adds_tab() {
    let mut mgr = manager_with_two_tabs(); // reuse this test mod's existing fixture helper
    let before = mgr.active_index();
    let n = mgr.tabs().len();
    mgr.push_background(detail_tab("bg")).unwrap();
    assert_eq!(mgr.active_index(), before);
    assert_eq!(mgr.tabs().len(), n + 1);
}

#[test]
fn push_background_propagates_push_detail_errors() {
    // whatever invalid-tab condition push_detail rejects (duplicate id is the
    // documented one — verify in push_detail's tests above) must error the
    // same way here, leaving active index unchanged.
    let mut mgr = manager_with_two_tabs();
    let dup = mgr.tabs()[0].clone();
    let before = mgr.active_index();
    assert!(mgr.push_background(dup).is_err());
    assert_eq!(mgr.active_index(), before);
}
```

Run: `cargo test -p sid-core tab::` and `cargo test -p sid-core --doc tab`
Expected: PASS.

- [x] **Step 3: Commit**

```bash
git add crates/sid-core/src/tab.rs
git commit -m "feat(sid-core): TabManager::push_background — open detail tab without focus switch"
```

---

### Task 8: Chord classification + kitty keyboard protocol

**Files:**
- Modify: `crates/sid-core/src/event.rs` (chord helper fns next to `KeyChord`)
- Modify: the terminal setup/teardown in the `sid` binary (find it: `rg -n "EnableMouseCapture|enable_raw_mode" crates/sid/src` — flags go in the same `execute!` calls)

- [x] **Step 1: Chord helpers in `sid-core/src/event.rs`**

```rust
/// Intent of a chord at the tab-strip level (list focus only — pane-focused
/// widgets consume Tab themselves and these are never consulted).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripNav {
    /// `Tab` / `Ctrl+Tab` — next tab.
    Next,
    /// `Shift+Tab` / `BackTab` / `Ctrl+Shift+Tab` — previous tab.
    Prev,
    /// Not a strip-navigation chord.
    None,
}

impl KeyChord {
    /// Classify this chord for tab-strip navigation.
    ///
    /// `Ctrl+Tab` arrives only from terminals with the kitty keyboard
    /// protocol; legacy terminals send plain `Tab`/`BackTab`, which is why
    /// `Shift+Tab` is the universal "previous" fallback.
    ///
    /// ```
    /// use crossterm::event::{KeyCode, KeyModifiers};
    /// use sid_core::event::{KeyChord, StripNav};
    /// let tab = KeyChord { code: KeyCode::Tab, mods: KeyModifiers::NONE };
    /// assert_eq!(tab.strip_nav(), StripNav::Next);
    /// let ctab = KeyChord { code: KeyCode::Tab, mods: KeyModifiers::CONTROL };
    /// assert_eq!(ctab.strip_nav(), StripNav::Prev);
    /// let btab = KeyChord { code: KeyCode::BackTab, mods: KeyModifiers::NONE };
    /// assert_eq!(btab.strip_nav(), StripNav::Prev);
    /// ```
    pub fn strip_nav(&self) -> StripNav {
        match self.code {
            crossterm::event::KeyCode::Tab => {
                if self.mods.contains(crossterm::event::KeyModifiers::CONTROL)
                    || self.mods.contains(crossterm::event::KeyModifiers::SHIFT)
                {
                    StripNav::Prev
                } else {
                    StripNav::Next
                }
            }
            crossterm::event::KeyCode::BackTab => StripNav::Prev,
            _ => StripNav::None,
        }
    }

    /// True when this chord means "open in background tab": `Ctrl+Enter`
    /// (kitty-protocol terminals) or `Shift+O` (universal fallback).
    ///
    /// ```
    /// use crossterm::event::{KeyCode, KeyModifiers};
    /// use sid_core::event::KeyChord;
    /// let ce = KeyChord { code: KeyCode::Enter, mods: KeyModifiers::CONTROL };
    /// assert!(ce.is_background_open());
    /// let o = KeyChord { code: KeyCode::Char('O'), mods: KeyModifiers::SHIFT };
    /// assert!(o.is_background_open());
    /// let plain = KeyChord { code: KeyCode::Enter, mods: KeyModifiers::NONE };
    /// assert!(!plain.is_background_open());
    /// ```
    pub fn is_background_open(&self) -> bool {
        match self.code {
            crossterm::event::KeyCode::Enter => {
                self.mods.contains(crossterm::event::KeyModifiers::CONTROL)
            }
            crossterm::event::KeyCode::Char('O') => true,
            _ => false,
        }
    }
}
```

(If `KeyModifiers::NONE` doesn't exist in the vendored crossterm version, use
`KeyModifiers::empty()` in the doc tests.)

- [x] **Step 2: Unit tests for chord classification edge cases (event.rs test mod)**

```rust
#[test]
fn ctrl_shift_tab_still_prev() {
    let c = KeyChord {
        code: crossterm::event::KeyCode::Tab,
        mods: crossterm::event::KeyModifiers::CONTROL | crossterm::event::KeyModifiers::SHIFT,
    };
    assert_eq!(c.strip_nav(), StripNav::Prev);
}

#[test]
fn unrelated_keys_are_none_and_not_background_open() {
    let c = KeyChord {
        code: crossterm::event::KeyCode::Char('x'),
        mods: crossterm::event::KeyModifiers::empty(),
    };
    assert_eq!(c.strip_nav(), StripNav::None);
    assert!(!c.is_background_open());
}
```

Run: `cargo test -p sid-core event::` and `cargo test -p sid-core --doc event`
Expected: PASS.

- [x] **Step 3: Enable kitty protocol in the binary's terminal setup**

In the terminal init (alongside the existing raw-mode/mouse-capture `execute!`):

```rust
use crossterm::event::{
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};

// After enable_raw_mode()/EnterAlternateScreen — harmless on terminals that
// don't support it (the sequence is ignored), unlocks Ctrl+Tab / Ctrl+Enter
// where supported (kitty, wezterm, ghostty, foot).
let _ = crossterm::execute!(
    std::io::stdout(),
    PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
);
```

And the matching `PopKeyboardEnhancementFlags` in the teardown path (the same fn that
disables raw mode / leaves the alternate screen — including any panic-hook teardown the
binary installs; grep `disable_raw_mode` for every call site and pair each one).

Manual check (no automated test for terminal I/O): run `cargo run` inside a kitty-protocol
terminal, press `Ctrl+Tab`, confirm the strip cycles forward (`Ctrl+Shift+Tab` backward); in a legacy terminal confirm
`Shift+Tab` does, and that startup/exit leave the terminal sane (no stuck flags after exit,
including after a forced panic).

- [x] **Step 4: Commit**

```bash
git add crates/sid-core/src/event.rs crates/sid/src
git commit -m "feat(sid-core,sid): StripNav/background-open chord helpers + kitty keyboard protocol flags"
```

---

### Task 9: Wire hosting — `Option<FormPane>` on the app, event interception, split render

**Files:**
- Modify: `crates/sid/src/wire.rs` — three places:
  1. the `SidApp` struct (where `modal`/`toasts`/`fx_state` fields live — add `pub form: Option<sid_widgets::FormPane>` and `pub form_origin_tab: Option<sid_core::tab::TabId>`)
  2. the event-routing section of the main loop (where modal events are intercepted — form interception goes immediately after modal interception, modal wins when both are somehow open)
  3. the `draw` fn (where the body rect is computed — split it when a form is active for the active tab)

- [x] **Step 1: Event interception**

Mirror the modal interception block. Shape:

```rust
// Form pane interception: when a form is open AND belongs to the active tab,
// key events go to it before the widget. The modal (confirms) still wins.
if sid_app.modal.is_none() {
    if let Some(form) = sid_app.form.as_mut() {
        if sid_app.form_origin_tab.as_ref() == Some(sid_app.app.tabs().active().id()) {
            if let SidEvent::Key(chord) = &ev {
                match form.handle_key(*chord) {
                    sid_widgets::FormEvent::Continue => { /* redraw */ }
                    sid_widgets::FormEvent::Cancel => {
                        sid_app.form = None;
                        sid_app.form_origin_tab = None;
                    }
                    sid_widgets::FormEvent::Submit(values) => {
                        let id = form.spec.id.0.clone();
                        dispatch_form_submit(sid_app, &id, values);
                    }
                    sid_widgets::FormEvent::RequestDiscardConfirm => {
                        open_discard_confirm_modal(sid_app);
                    }
                }
                continue; // form consumed the key
            }
        }
    }
}
```

With:

```rust
/// Route a submitted form's values by form id. Branches 1-5 register their
/// arms here — substrate ships with only the wildcard.
fn dispatch_form_submit(sid_app: &mut SidApp, id: &str, values: sid_widgets::FormValues) {
    match id {
        _ => {
            sid_app.toasts.push(format!("unhandled form submit: {id}"));
        }
    }
    sid_app.form = None;
    sid_app.form_origin_tab = None;
}

/// Standard "Discard changes?" confirm — reuses the existing tiny-confirm
/// modal substrate (same pattern as the delete-confirm modals; find one with
/// rg "confirm" crates/sid/src/wire.rs and mirror it). On confirm: close the
/// form; on cancel: keep editing.
fn open_discard_confirm_modal(sid_app: &mut SidApp) {
    // ModalSpec::new("form.discard_confirm", "Discard changes?", vec![]) with
    // primary "Discard" / secondary "Keep editing"; the modal-submit dispatch
    // arm for "form.discard_confirm" sets sid_app.form = None.
}
```

(`open_discard_confirm_modal`'s body and the `form.discard_confirm` submit arm are real code
the executor writes by copying an existing delete-confirm modal — exact field shapes live
there; the behavior contract is in the comment above.)

Tab-strip semantics in the same routing section: where the loop currently handles tab
cycling keys, use `chord.strip_nav()` — **interim rule (orchestrator ruling, 2026-06-12)**:
wire-level cycling fires ONLY on Ctrl-modified chords (`Ctrl+Tab` → `tabs.next()`,
`Ctrl+Shift+Tab` → `tabs.prev()`). Plain `Tab`/`Shift+Tab`/`BackTab` fall through to
widgets, which consume them for intra-widget focus today. Branches 1–5 adopt `strip_nav`
for plain Tab as they migrate widgets to the list/pane focus model. Gated on no form/modal
being active for the active tab (when a form is active, the interception above already
consumed the key, so no extra gating code is needed — verify by test).

- [x] **Step 2: Split render**

In `draw`, where the active widget's body rect is computed:

```rust
let body = /* existing body rect */;
let (widget_area, form_area) = match (&app.form, &app.form_origin_tab) {
    (Some(_), Some(origin)) if origin == app.app.tabs().active().id() => {
        let list_w = (body.width as u32 * 40 / 100) as u16;
        (
            Rect { width: list_w, ..body },
            Rect { x: body.x + list_w, width: body.width - list_w, ..body },
        )
    }
    _ => (body, Rect { width: 0, ..body }),
};
// render active widget into widget_area (existing call, rect swapped)
if form_area.width > 0 {
    if let Some(form) = &app.form {
        sid_widgets::render_form_pane(frame.buffer_mut(), form_area, form, &theme);
    }
}
```

Footer: when the form is active, the footer hint line renders the form contract instead of
the widget's hints: `Tab fields · ⏎ save · ⎋ cancel`. Hook this where the footer queries
`active widget.footer_hint()` — a form-active check that substitutes a fixed
`Vec<FooterHint>`:

```rust
fn form_footer_hints() -> Vec<sid_core::FooterHint> {
    vec![
        sid_core::FooterHint { chord: "Tab".into(), label: "fields".into() },
        sid_core::FooterHint { chord: "⏎".into(), label: "save".into() },
        sid_core::FooterHint { chord: "⎋".into(), label: "cancel".into() },
    ]
}
```

- [x] **Step 3: Helper to open a form (the API branches 1-5 call)**

```rust
/// Open `spec` as the side-pane form of the currently-active tab.
/// Any prior form is replaced (callers confirm-dirty themselves if needed —
/// in practice openers run from list focus where no form is open).
pub fn open_form(sid_app: &mut SidApp, spec: sid_widgets::FormSpec) {
    sid_app.form_origin_tab = Some(sid_app.app.tabs().active().id().clone());
    sid_app.form = Some(sid_widgets::FormPane::new(spec));
}
```

- [x] **Step 4: Integration tests (wire.rs test mod — use the existing test fixtures that build a `SidApp` for modal tests; same construction)**

```rust
#[test]
fn open_form_renders_split_and_form_consumes_tab() {
    // build app, open_form with the two-field demo spec,
    // send Tab as a key event through the routing fn under test:
    // assert form.focus advanced AND tabs.active_index() unchanged.
}

#[test]
fn form_only_intercepts_on_origin_tab() {
    // open form on tab 0, switch to tab 1 programmatically (tabs.jump(1)),
    // send a Char key: the active widget receives it (form untouched,
    // form.spec.values() unchanged).
}

#[test]
fn submit_unknown_form_id_toasts_and_closes() {
    // open form, drive to Submit; assert sid_app.form.is_none() and a toast
    // containing "unhandled form submit" was pushed.
}

#[test]
fn strip_nav_cycles_tabs_when_no_form_active() {
    // Ctrl+Tab -> active_index+1; Ctrl+Shift+Tab -> back.
    // (Interim rule: plain Tab/BackTab fall through — see strip-nav routing note above.)
}
```

Run: `cargo test -p sid wire::` (scope to the new test names if the mod is huge:
`cargo test -p sid form_only_intercepts strip_nav_cycles open_form_renders submit_unknown`)
Expected: PASS.

- [x] **Step 5: Commit**

```bash
git add crates/sid/src/wire.rs
git commit -m "feat(sid): host FormPane side panes — event interception, 40/60 split render, submit dispatch, form footer"
```

---

### Task 10: Help overlay — `?` keybind cheatsheet

**Files:**
- Modify: `crates/sid/src/wire.rs` (global key routing + an overlay builder fn)

- [x] **Step 1: Overlay builder on the existing modal substrate**

The modal `Field::Display` was designed for exactly this ("help drawers" per its doc
comment). Build the cheatsheet as a modal of Display fields — no new render surface needed:

```rust
/// Build the `?` help overlay: global chords, then the active tab's bindings.
///
/// Sources, in order:
/// 1. A fixed global section (tab strip, background-open, form keys, quit).
/// 2. The active widget's `footer_hint()` list — every entry, not just the
///    few the slim footer shows.
/// 3. The keybind map's bindings scoped to the active tab's action prefix
///    (e.g. `database.*`) — read from the same KeybindMap the dispatcher
///    uses, so the overlay can never drift from reality.
fn build_help_overlay(sid_app: &SidApp) -> sid_widgets::ModalSpec {
    let mut body = String::new();
    body.push_str("Tab/S-Tab  cycle tabs (C-Tab next, C-S-Tab back on kitty terms)\n");
    body.push_str("C-Enter/O  open in background tab\n");
    body.push_str("→ / ←      enter / leave pane\n");
    body.push_str("C-W        close tab\n");
    let mut tab_body = String::new();
    for hint in active_widget_footer_hints_full(sid_app) {
        tab_body.push_str(&format!("{:<10} {}\n", hint.chord, hint.label));
    }
    sid_widgets::ModalSpec::new(
        "help.overlay",
        "Keybinds",
        vec![
            sid_widgets::Field::Display { label: "Global".into(), body },
            sid_widgets::Field::Display { label: "This tab".into(), body: tab_body },
        ],
    )
}
```

(`active_widget_footer_hints_full` = whatever accessor the footer renderer already uses to
pull `footer_hint()` from the active widget — reuse it, do not duplicate. The executor also
appends the KeybindMap-scoped section per the doc comment; read how the dispatcher resolves
chords→actions to find the iteration API.)

Routing: in the global key handling (same neighborhood as the strip-nav match), `?` with
list focus and no form/modal open → `sid_app.modal = Some(build_help_overlay(sid_app))`.
Any key dismisses (the modal's existing Esc/secondary path).

- [x] **Step 2: Slim the footer**

Where the footer renders the active widget's hints, cap at the first 4 entries and always
append `? help`. (Branches keep their `footer_hint()` lists ordered most-used-first, per
master plan decision 13.)

- [x] **Step 3: Tests (wire.rs test mod)**

```rust
#[test]
fn question_mark_opens_help_overlay_with_tab_section() {
    // build app on database tab, send '?', assert modal id == "help.overlay"
    // and the second Display body contains a known database hint label.
}

#[test]
fn question_mark_inside_form_types_literally() {
    // open_form(...), send '?', assert no modal opened and the focused text
    // field's value now ends with '?'.
}

#[test]
fn footer_caps_hints_and_appends_help() {
    // active widget with 6 hints -> rendered footer shows 4 + "? help".
}
```

Run: `cargo test -p sid help_overlay question_mark footer_caps`
Expected: PASS.

- [x] **Step 4: Commit**

```bash
git add crates/sid/src/wire.rs
git commit -m "feat(sid): ? help overlay — keybind cheatsheet from footer hints + keybind map; slim footer"
```

---

### Task 11: Branch wrap-up

- [ ] **Step 1: Targeted regression sweep of touched crates only**

Run: `cargo test -p sid-widgets -p sid-core -p sid-store -p sid`
Expected: all green. Fix anything red before proceeding (most likely suspects: doc tests
with stale constructor signatures, snapshot churn from the new lib.rs exports).

- [ ] **Step 2: Clippy on touched crates**

Run: `cargo clippy -p sid-widgets -p sid-core -p sid-store -p sid --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 3: Tick this plan's checkboxes, then merge to main**

```bash
git add docs/superpowers/plans/2026-06-11-uxv2-0-substrate.md
git commit -m "docs(plans): tick uxv2-0 substrate tasks"
# fast-forward or merge per current branch convention (see git log for prior 'Merge branch' style)
```

Branches 1–7 may now start from main in parallel worktrees.
