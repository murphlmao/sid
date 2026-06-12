# UX-v2 Branch 2 — Database Tab: Connection Form on the New Substrate

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or
> superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax for tracking. Read
> `2026-06-11-uxv2-master.md` first for the binding design decisions, then
> `2026-06-11-uxv2-0-substrate.md` for the substrate APIs this plan consumes. Run only the
> targeted tests named in each step — not the full workspace.

**Goal:** Replace the modal add/edit-connection flow in the Database tab with a side-pane
form that uses the UX-v2 substrate. Name + Kind (segmented); Kind change reshapes the form —
Postgres → Host/Port/Database/User/Password, SQLite → Path. Surviving field values preserved
across reshape. Generated DSN shown as a live-updating read-only Info row. `Ctrl+T` tests the
connection off-thread via `DbClient` (job queue). Enter on an existing row opens the form
pre-filled. `+add new` row at the top of the connections list, governed by
`show_add_new_row`. `N` and `D`/`Delete` keybinds kept as convenience aliases (they now open
the same form rather than the modal). Secrets/password storage is left exactly as it is today
— no changes to `submit_database_new`'s secret path, only the UI surface that feeds it
changes.

**Architecture:** `DatabaseWidget` gains a `ListCursor` for the connections list and a
`SplitView<()>` for list/pane focus tracking. The widget's event handler emits a new
`DbCommand::OpenConnectionForm { prefill: Option<DbConnection> }` instead of bubbling for `N`
/ `Enter`; `wire.rs` catches it in the existing `DbCommand` drain loop and calls
`open_form(sid_app, db_connection_form_spec(prefill))`. Form submit dispatches to a new
`submit_db_connection_form` helper in `wire.rs`. Test-connection is a new
`DbCommand::TestConnection { conn_id }` drained into a `jobs.spawn` block that returns
`JobOutcome::Success` / `JobOutcome::Failure`. All `DatabaseWidget` changes are in
`crates/sid-widgets/src/database.rs`; all wiring changes are in `crates/sid/src/wire.rs`.

**Tech Stack:** Rust, ratatui (sid-widgets only), crossterm (sid-core), insta snapshots for
rendered widget buffer. Substrate APIs (`ListCursor`, `SplitView`, `FormSpec`, `FormPane`,
`FormSection`, `FormField`, `FormValues`, `SectionKind`, `Validate`, `render_form_pane`,
`open_form`, `dispatch_form_submit`) arrive from branch 0 before this branch runs.

---

### Task 1: `ListCursor` integration — connections list + `+add new` row

**Files:**
- Modify: `crates/sid-widgets/src/database.rs`
  - `DatabaseState` struct (~L392): add `pub cursor: sid_widgets::list_cursor::ListCursor`
  - `DatabaseState::new` (~L395): initialise cursor from connections length + `show_add_new`
    flag passed in as a new parameter
  - `DatabaseState::set_connections` (~L431): rebuild cursor, preserve position where valid
  - `select_next` (~L506) / `select_prev` (~L515): delegate to `cursor.move_next()` /
    `cursor.move_prev()`; derive `selected_idx` from `cursor.target()`
  - `selected_connection` (~L426): derive from `cursor.target()`
  - `render_connection_list` (~L739): render the `+add new` row when
    `cursor.add_new == true`, accent-styled, selected when `cursor.target() == CursorTarget::AddNew`

- [ ] **Step 1: Write the failing test**

```rust
// In the `#[cfg(test)]` block at the bottom of database.rs:
#[test]
fn cursor_add_new_row_at_top_when_enabled() {
    let conns = vec![stub_conn("a"), stub_conn("b")];
    let mut state = DatabaseState::new_with_add_new(conns, true);
    // initial position is 0 — the synthetic +add new row
    assert!(matches!(
        state.cursor.target(),
        sid_widgets::list_cursor::CursorTarget::AddNew
    ));
    state.select_next();
    assert!(matches!(
        state.cursor.target(),
        sid_widgets::list_cursor::CursorTarget::Item(0)
    ));
    state.select_prev();
    assert!(matches!(
        state.cursor.target(),
        sid_widgets::list_cursor::CursorTarget::AddNew
    ));
}

#[test]
fn cursor_wraps_to_add_new_from_last_item() {
    let conns = vec![stub_conn("a"), stub_conn("b")];
    let mut state = DatabaseState::new_with_add_new(conns, true);
    // drive to last item
    state.select_next(); // Item(0)
    state.select_next(); // Item(1)
    state.select_next(); // wraps back to AddNew
    assert!(matches!(
        state.cursor.target(),
        sid_widgets::list_cursor::CursorTarget::AddNew
    ));
}

#[test]
fn cursor_no_add_new_row_when_disabled() {
    let conns = vec![stub_conn("a")];
    let mut state = DatabaseState::new_with_add_new(conns, false);
    assert!(matches!(
        state.cursor.target(),
        sid_widgets::list_cursor::CursorTarget::Item(0)
    ));
}

// Helper — minimal DbConnection for tests:
fn stub_conn(id: &str) -> sid_store::DbConnection {
    use sid_core::adapters::db_client::DbKind;
    use sid_store::now_epoch;
    sid_store::DbConnection {
        id: id.to_string(),
        kind: DbKind::Postgres,
        name: id.to_string(),
        dsn: format!("postgres://localhost/{id}"),
        secret_ref: None,
        created_at: now_epoch(),
    }
}
```

Run: `cargo test -p sid-widgets database` — expected: COMPILE ERROR (cursor fields not added yet).

- [ ] **Step 2: Implement cursor integration**

In `database.rs`, add `use sid_widgets::list_cursor::{CursorTarget, ListCursor};` to the
import block (~L12).

Change `DatabaseState`:

```rust
pub struct DatabaseState {
    connections: Vec<DbConnection>,
    /// Cursor over the connections list including the optional +add new row.
    pub cursor: ListCursor,
    active_client: Option<Arc<dyn DbClient>>,
    active_conn_id: Option<String>,
    right_pane: RightPane,
    editor: EditorState,
    results: ResultsState,
    history: HistoryState,
    pending: Vec<DbCommand>,
}
```

Add a constructor that takes the `add_new` flag:

```rust
/// Create state. `add_new` mirrors the `show_add_new_row` setting.
pub fn new_with_add_new(connections: Vec<DbConnection>, add_new: bool) -> Self {
    let len = connections.len();
    Self {
        cursor: ListCursor::new(len, add_new, 0),
        connections,
        selected_idx: 0,
        active_client: None,
        active_conn_id: None,
        right_pane: RightPane::Editor,
        editor: EditorState::default_blank(),
        results: ResultsState::default(),
        history: HistoryState::default(),
        pending: Vec::new(),
    }
}
```

Keep the existing `DatabaseState::new(connections)` calling `new_with_add_new(connections,
true)` for backward compatibility with test code that calls it.

Update `set_connections` to rebuild the cursor preserving position:

```rust
pub fn set_connections(&mut self, c: Vec<DbConnection>, add_new: bool) {
    let new_len = c.len();
    self.connections = c;
    let old_pos = self.cursor.pos;
    self.cursor = ListCursor::new(new_len, add_new, old_pos);
    // keep selected_idx consistent
    self.selected_idx = match self.cursor.target() {
        CursorTarget::AddNew => 0,
        CursorTarget::Item(i) => i,
    };
}
```

Update `select_next` / `select_prev`:

```rust
pub fn select_next(&mut self) {
    self.cursor.move_next();
    if let CursorTarget::Item(i) = self.cursor.target() {
        self.selected_idx = i;
    }
}

pub fn select_prev(&mut self) {
    self.cursor.move_prev();
    if let CursorTarget::Item(i) = self.cursor.target() {
        self.selected_idx = i;
    }
}
```

Update `selected_connection`:

```rust
pub fn selected_connection(&self) -> Option<&DbConnection> {
    match self.cursor.target() {
        CursorTarget::Item(i) => self.connections.get(i),
        CursorTarget::AddNew => None,
    }
}

/// True when the cursor sits on the synthetic +add new row.
pub fn is_add_new_selected(&self) -> bool {
    matches!(self.cursor.target(), CursorTarget::AddNew)
}
```

Update `render_connection_list` to render the `+add new` row when `cursor.add_new` is true.
At the start of the list rendering loop, before iterating connections, insert the add-new row:

```rust
fn render_connection_list(&self, frame: &mut Frame<'_>, rect: Rect, theme: &Theme) {
    let focused = self.focused_pane == DbFocus::Connections;
    let border_color = if focused { theme.accent_primary } else { theme.muted };
    // ... existing block setup ...

    let mut lines: Vec<Line<'_>> = Vec::new();

    // +add new synthetic row
    if self.state.cursor.add_new {
        let add_new_selected = matches!(
            self.state.cursor.target(),
            CursorTarget::AddNew
        );
        let style = if add_new_selected {
            Style::default().fg(theme.accent_primary.into()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.accent_secondary.into())
        };
        lines.push(Line::from(Span::styled("+ add new", style)));
    }

    // existing connection rows (replace prior iteration):
    for (i, conn) in self.state.connections().iter().enumerate() {
        let selected = matches!(self.state.cursor.target(), CursorTarget::Item(j) if j == i);
        // ... rest of existing row rendering logic unchanged ...
    }
    // ... rest of render unchanged ...
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p sid-widgets database`
Expected: all 3 new cursor tests pass; existing `id_and_title_correct` test passes.

- [ ] **Step 4: Snapshot the connection list with +add new**

Add a snapshot test in the `#[cfg(test)]` block:

```rust
#[test]
fn snapshot_connection_list_with_add_new() {
    let conns = vec![stub_conn("local-pg"), stub_conn("staging")];
    let mut w = DatabaseWidget::new_with_add_new(conns, true);
    let s = render_to_string(&w, 80, 24);
    insta::assert_snapshot!(s);
}
```

`DatabaseWidget` needs a `new_with_add_new(connections, add_new)` constructor that mirrors
`DatabaseState::new_with_add_new`. Add it (~L550):

```rust
pub fn new_with_add_new(connections: Vec<DbConnection>, add_new: bool) -> Self {
    Self {
        state: DatabaseState::new_with_add_new(connections, add_new),
        id: WidgetId::new("database.root"),
        body: ComingSoonBody::new(
            "Database",
            "Postgres + SQLite query runner — Plan 4 wires the editor + results table in a follow-up.",
        ),
        focused_pane: DbFocus::default(),
        split: SplitView::default(),
    }
}
```

Run: `cargo test -p sid-widgets snapshot_connection_list_with_add_new`
Then: `cargo insta review` — accept the new snapshot.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-widgets/src/database.rs
git commit -m "feat(sid-widgets): database connections list — ListCursor + +add new row"
```

---

### Task 2: `SplitView` integration + form-open commands

**Files:**
- Modify: `crates/sid-widgets/src/database.rs`
  - `DatabaseWidget` struct (~L540): add `pub split: SplitView<()>`
  - `DbCommand` enum (~L100): add two new variants
  - `handle_event` (~L1075): replace `N`/`Enter`/`Delete`/`D` handling in the Connections
    branch to emit the new commands; update Tab handling to delegate via `SplitView`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn enter_on_connection_emits_open_form_with_prefill() {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::event::KeyChord;
    use sid_core::widget::EventOutcome;
    let conns = vec![stub_conn("pg")];
    let mut w = DatabaseWidget::new_with_add_new(conns, false);
    // cursor is on Item(0) since add_new=false
    let ev = sid_core::event::Event::Key(KeyChord { code: KeyCode::Enter, mods: KeyModifiers::NONE });
    let mut ctx = stub_ctx();
    w.handle_event(&ev, &mut ctx);
    let cmds = w.state.drain_commands();
    assert!(
        cmds.iter().any(|c| matches!(c, DbCommand::OpenConnectionForm { prefill: Some(_) })),
        "expected OpenConnectionForm with prefill, got: {:?}", cmds
    );
}

#[test]
fn enter_on_add_new_row_emits_open_form_no_prefill() {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::event::KeyChord;
    let conns = vec![stub_conn("pg")];
    let mut w = DatabaseWidget::new_with_add_new(conns, true);
    // cursor starts at AddNew
    let ev = sid_core::event::Event::Key(KeyChord { code: KeyCode::Enter, mods: KeyModifiers::NONE });
    let mut ctx = stub_ctx();
    w.handle_event(&ev, &mut ctx);
    let cmds = w.state.drain_commands();
    assert!(
        cmds.iter().any(|c| matches!(c, DbCommand::OpenConnectionForm { prefill: None })),
        "expected OpenConnectionForm with no prefill, got: {:?}", cmds
    );
}

#[test]
fn n_key_emits_open_form_no_prefill() {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::event::KeyChord;
    let mut w = DatabaseWidget::default();
    let ev = sid_core::event::Event::Key(KeyChord { code: KeyCode::Char('N'), mods: KeyModifiers::NONE });
    let mut ctx = stub_ctx();
    w.handle_event(&ev, &mut ctx);
    let cmds = w.state.drain_commands();
    assert!(
        cmds.iter().any(|c| matches!(c, DbCommand::OpenConnectionForm { prefill: None })),
        "expected OpenConnectionForm, got: {:?}", cmds
    );
}

#[test]
fn ctrl_t_emits_test_connection() {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::event::KeyChord;
    let conns = vec![stub_conn("pg")];
    let mut w = DatabaseWidget::new_with_add_new(conns, false);
    // connect first so active_conn_id is set
    w.state.set_active_conn_id_for_tests("pg".to_string());
    let ev = sid_core::event::Event::Key(KeyChord {
        code: KeyCode::Char('t'),
        mods: KeyModifiers::CONTROL,
    });
    let mut ctx = stub_ctx();
    w.handle_event(&ev, &mut ctx);
    let cmds = w.state.drain_commands();
    assert!(
        cmds.iter().any(|c| matches!(c, DbCommand::TestConnection { conn_id } if conn_id == "pg")),
        "expected TestConnection, got: {:?}", cmds
    );
}

// Minimal WidgetCtx stub for tests:
fn stub_ctx() -> sid_core::context::WidgetCtx {
    sid_core::context::WidgetCtx::default()
}
```

Run: `cargo test -p sid-widgets database` — expected: COMPILE ERROR (new variants not added).

- [ ] **Step 2: Add `DbCommand` variants**

In `database.rs`, extend `DbCommand` (~L100):

```rust
/// Open the add/edit connection form. `prefill` is `Some` when editing an
/// existing connection, `None` when creating a new one.
OpenConnectionForm {
    prefill: Option<sid_store::DbConnection>,
},

/// Test the named connection through `DbClient::open` off-thread via the
/// job queue. Result surfaces as a toast.
TestConnection {
    conn_id: String,
},
```

- [ ] **Step 3: Add `SplitView` to `DatabaseWidget`**

Add `use sid_widgets::split_view::{SplitFocus, SplitView};` to the import block.

Add the field to `DatabaseWidget` struct:

```rust
pub struct DatabaseWidget {
    state: DatabaseState,
    id: WidgetId,
    body: ComingSoonBody,
    focused_pane: DbFocus,
    /// List/pane focus for the connections split.
    pub split: SplitView<()>,
}
```

Update both constructors (`new` and `new_with_add_new`) to initialise `split: SplitView::default()`.

- [ ] **Step 4: Update `handle_event` for the Connections branch**

Replace the `DbFocus::Connections` arm's `N`/`n`, `Enter`, `Delete`/`D`/`d` match arms:

```rust
DbFocus::Connections => match (chord.code, chord.mods) {
    (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
        self.state.select_next();
        return EventOutcome::Consumed;
    }
    (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
        self.state.select_prev();
        return EventOutcome::Consumed;
    }
    // Enter: open add form (on +add new) or edit form (on existing connection).
    (KeyCode::Enter, KeyModifiers::NONE) => {
        let prefill = if self.state.is_add_new_selected() {
            None
        } else {
            self.state.selected_connection().cloned()
        };
        self.state.push_command(DbCommand::OpenConnectionForm { prefill });
        return EventOutcome::Consumed;
    }
    // N — convenience alias for "new", always opens add form.
    (KeyCode::Char('N') | KeyCode::Char('n'), KeyModifiers::NONE) => {
        self.state.push_command(DbCommand::OpenConnectionForm { prefill: None });
        return EventOutcome::Consumed;
    }
    // D / Delete — remove selected connection (keep existing delete modal flow).
    (KeyCode::Delete | KeyCode::Char('D') | KeyCode::Char('d'), KeyModifiers::NONE) => {
        // wire.rs still handles this via database_modal_for_key on Delete/D;
        // bubble so the existing modal path fires.
        return EventOutcome::Bubble;
    }
    // Ctrl+T — test active or selected connection.
    (KeyCode::Char('t'), KeyModifiers::CONTROL) => {
        let conn_id = self.state
            .active_conn_id()
            .or_else(|| self.state.selected_connection().map(|c| c.id.as_str()))
            .map(|s| s.to_string());
        if let Some(id) = conn_id {
            self.state.push_command(DbCommand::TestConnection { conn_id: id });
        }
        return EventOutcome::Consumed;
    }
    _ => {}
}
```

Also update Tab in the Connections focus to use `SplitView`: when `split.focus() ==
SplitFocus::List`, Tab should bubble (so tab-strip cycling fires); when
`SplitFocus::Pane`, consume (internal pane navigation — not applicable in the connections
list, but the pattern must be consistent with the substrate). Right-arrow (`→`) enters the
pane (`split.push(())`); left-arrow (`←`) pops (`split.pop()`).

- [ ] **Step 5: Run the tests**

Run: `cargo test -p sid-widgets database`
Expected: all 4 new tests pass; all prior tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/sid-widgets/src/database.rs
git commit -m "feat(sid-widgets): DbCommand::OpenConnectionForm + TestConnection; SplitView on connections list"
```

---

### Task 3: `db_connection_form_spec` — FormSpec builder with reshape

**Files:**
- Modify: `crates/sid/src/wire.rs` — add `db_connection_form_spec` free function and its
  private DSN builder helper; add `SHOW_ADD_NEW_ROW` to `sid-store` settings keys as a
  prerequisite (if branch 0 Task 2 hasn't landed it yet, add it here)

> **Note:** `SHOW_ADD_NEW_ROW` is added to `sid-store::settings_keys` by substrate branch 0
> Task 2. If that task has already merged, skip the `settings_keys` addition below and use
> the constant from the substrate. If it has not merged yet, add it here.

- [ ] **Step 1: Write the failing test**

```rust
// In the wire.rs #[cfg(test)] block:
#[test]
fn db_form_spec_postgres_sections_reshape_on_kind_change() {
    use sid_widgets::{FormValues, SectionKind};
    let mut spec = db_connection_form_spec(None);
    // default kind is Postgres; should have name, kind, host/port/database/user/password, dsn
    let pg_keys: Vec<&str> = spec
        .sections
        .iter()
        .filter(|s| s.kind == SectionKind::Editable)
        .flat_map(|s| s.fields.iter().map(|f| f.key.as_str()))
        .collect();
    assert!(pg_keys.contains(&"name"), "missing name in pg spec");
    assert!(pg_keys.contains(&"host"), "missing host in pg spec");
    assert!(pg_keys.contains(&"port"), "missing port in pg spec");
    assert!(pg_keys.contains(&"database"), "missing database in pg spec");
    assert!(pg_keys.contains(&"user"), "missing user in pg spec");
    assert!(pg_keys.contains(&"password"), "missing password in pg spec");
    // Info section should have dsn key
    let info_keys: Vec<&str> = spec
        .sections
        .iter()
        .filter(|s| s.kind == SectionKind::Info)
        .flat_map(|s| s.fields.iter().map(|f| f.key.as_str()))
        .collect();
    assert!(info_keys.contains(&"dsn"), "missing dsn info row");
}

#[test]
fn db_form_spec_reshapes_to_sqlite_on_kind_change() {
    use sid_widgets::{FormValues, SectionKind};
    let mut spec = db_connection_form_spec(None);
    // change kind to SQLite then run reshape
    let kind_section = spec.sections.iter_mut()
        .find(|s| s.kind == SectionKind::Editable)
        .expect("editable section");
    let kind_field = kind_section.fields.iter_mut()
        .find(|f| f.key == "kind")
        .expect("kind field");
    if let sid_widgets::Field::Choice { selected, options, .. } = &mut kind_field.field {
        *selected = options.iter().position(|o| o == "SQLite").unwrap_or(0);
    }
    spec.run_reshape();
    let editable_keys: Vec<&str> = spec
        .sections
        .iter()
        .filter(|s| s.kind == SectionKind::Editable)
        .flat_map(|s| s.fields.iter().map(|f| f.key.as_str()))
        .collect();
    assert!(editable_keys.contains(&"path"), "missing path in sqlite spec");
    assert!(!editable_keys.contains(&"host"), "host should be absent in sqlite spec");
    assert!(!editable_keys.contains(&"password"), "password should be absent in sqlite spec");
}

#[test]
fn db_form_spec_prefill_populates_values() {
    use sid_core::adapters::db_client::DbKind;
    use sid_store::{DbConnection, now_epoch};
    let conn = DbConnection {
        id: "local-pg".to_string(),
        kind: DbKind::Postgres,
        name: "Local Postgres".to_string(),
        dsn: "postgres://dbuser@localhost:5432/mydb".to_string(),
        secret_ref: None,
        created_at: now_epoch(),
    };
    let spec = db_connection_form_spec(Some(&conn));
    let values = spec.values();
    assert_eq!(values.get("name").and_then(|v| v.as_text()), Some("Local Postgres"));
    assert_eq!(values.get("kind").and_then(|v| v.as_choice()), Some("Postgres"));
    // DSN fields parsed out:
    assert_eq!(values.get("host").and_then(|v| v.as_text()), Some("localhost"));
    assert_eq!(values.get("port").and_then(|v| v.as_text()), Some("5432"));
    assert_eq!(values.get("database").and_then(|v| v.as_text()), Some("mydb"));
    assert_eq!(values.get("user").and_then(|v| v.as_text()), Some("dbuser"));
}

#[test]
fn db_form_spec_dsn_info_row_reflects_postgres_fields() {
    use sid_widgets::SectionKind;
    let mut spec = db_connection_form_spec(None);
    // set host + port + database + user
    for section in spec.sections.iter_mut().filter(|s| s.kind == SectionKind::Editable) {
        for field in &mut section.fields {
            match field.key.as_str() {
                "host"     => { if let sid_widgets::Field::Text { value, .. } = &mut field.field { *value = "db.example.com".into(); } }
                "port"     => { if let sid_widgets::Field::Text { value, .. } = &mut field.field { *value = "5432".into(); } }
                "database" => { if let sid_widgets::Field::Text { value, .. } = &mut field.field { *value = "app".into(); } }
                "user"     => { if let sid_widgets::Field::Text { value, .. } = &mut field.field { *value = "alice".into(); } }
                _ => {}
            }
        }
    }
    spec.run_reshape();
    let dsn_value = spec.sections.iter()
        .filter(|s| s.kind == SectionKind::Info)
        .flat_map(|s| s.fields.iter())
        .find(|f| f.key == "dsn")
        .and_then(|f| if let sid_widgets::Field::Display { body, .. } = &f.field { Some(body.as_str()) } else { None })
        .unwrap_or("");
    assert!(dsn_value.contains("db.example.com"), "dsn should contain host");
    assert!(dsn_value.contains("app"), "dsn should contain database name");
    assert!(dsn_value.contains("alice"), "dsn should contain user");
}
```

Run: `cargo test -p sid db_form_spec` — expected: COMPILE ERROR (function not added yet).

- [ ] **Step 2: Implement `db_connection_form_spec`**

Add immediately after `submit_database_remove` (~L4610) in `wire.rs`:

```rust
/// Build a [`sid_widgets::FormSpec`] for adding or editing a database connection.
///
/// When `prefill` is `Some`, all fields are pre-populated from the existing
/// connection record and its DSN is parsed into individual Host/Port/Database/User
/// fields. The form id is `"database.connection"`.
///
/// The spec carries a reshape hook watching `"kind"`: selecting `Postgres` shows
/// Host/Port/Database/User/Password fields; selecting `SQLite` replaces them with
/// a single Path field. Surviving key values are preserved across the reshape.
/// The Info section's DSN row is recomputed on every reshape from the current
/// editable-field values.
fn db_connection_form_spec(
    prefill: Option<&sid_store::DbConnection>,
) -> sid_widgets::FormSpec {
    use sid_core::adapters::db_client::DbKind;
    use sid_widgets::{Field, FormField, FormSection, FormSpec, SectionKind, Validate};

    // Parse a postgres DSN (postgres://user@host:port/database) into parts.
    // Returns (host, port, database, user). Tolerates missing components.
    fn parse_pg_dsn(dsn: &str) -> (String, String, String, String) {
        // strip scheme
        let rest = dsn
            .strip_prefix("postgres://")
            .or_else(|| dsn.strip_prefix("postgresql://"))
            .unwrap_or(dsn);
        // split user@hostpart/database
        let (user_host, db) = rest.split_once('/').unwrap_or((rest, ""));
        let (user, host_port) = user_host.split_once('@').unwrap_or(("", user_host));
        let (host, port) = host_port.split_once(':').unwrap_or((host_port, "5432"));
        (
            host.to_string(),
            port.to_string(),
            db.to_string(),
            user.to_string(),
        )
    }

    // Build the DSN Info row body from current field values.
    fn build_dsn(values: &sid_widgets::FormValues) -> String {
        let kind = values
            .get("kind")
            .and_then(|v| v.as_choice())
            .unwrap_or("Postgres");
        match kind {
            "SQLite" => values
                .get("path")
                .and_then(|v| v.as_text())
                .unwrap_or("")
                .to_string(),
            _ => {
                let user = values.get("user").and_then(|v| v.as_text()).unwrap_or("");
                let host = values.get("host").and_then(|v| v.as_text()).unwrap_or("localhost");
                let port = values.get("port").and_then(|v| v.as_text()).unwrap_or("5432");
                let db   = values.get("database").and_then(|v| v.as_text()).unwrap_or("");
                if user.is_empty() {
                    format!("postgres://{host}:{port}/{db}")
                } else {
                    format!("postgres://{user}@{host}:{port}/{db}")
                }
            }
        }
    }

    // Sections builder; invoked initially and by the reshape hook.
    fn make_sections(values: &sid_widgets::FormValues) -> Vec<sid_widgets::FormSection> {
        use sid_widgets::{Field, FormField, FormSection, SectionKind, Validate};
        let kind = values
            .get("kind")
            .and_then(|v| v.as_choice())
            .unwrap_or("Postgres");
        let name_val  = values.get("name").and_then(|v| v.as_text()).unwrap_or("").to_string();
        let kind_idx  = if kind == "SQLite" { 1 } else { 0 };

        let mut editable_fields = vec![
            FormField {
                key: "name".into(),
                field: Field::Text {
                    label: "Name".into(),
                    value: name_val,
                    placeholder: Some("Local Postgres".into()),
                    validators: vec![Validate::NonEmpty],
                },
            },
            FormField {
                key: "kind".into(),
                field: Field::Choice {
                    label: "Kind".into(),
                    options: vec!["Postgres".into(), "SQLite".into()],
                    selected: kind_idx,
                },
            },
        ];

        if kind == "SQLite" {
            let path_val = values.get("path").and_then(|v| v.as_text()).unwrap_or("").to_string();
            editable_fields.push(FormField {
                key: "path".into(),
                field: Field::Picker {
                    label: "Path".into(),
                    value: path_val,
                    validators: vec![Validate::NonEmpty],
                },
            });
        } else {
            let (def_host, def_port, def_db, def_user) =
                values.get("host").and_then(|v| v.as_text())
                .map(|_| (
                    values.get("host").and_then(|v| v.as_text()).unwrap_or("localhost").to_string(),
                    values.get("port").and_then(|v| v.as_text()).unwrap_or("5432").to_string(),
                    values.get("database").and_then(|v| v.as_text()).unwrap_or("").to_string(),
                    values.get("user").and_then(|v| v.as_text()).unwrap_or("").to_string(),
                ))
                .unwrap_or_else(|| ("localhost".into(), "5432".into(), "".into(), "".into()));
            let pw_val = values.get("password").and_then(|v| v.as_text()).unwrap_or("").to_string();

            editable_fields.push(FormField {
                key: "host".into(),
                field: Field::Text {
                    label: "Host".into(),
                    value: def_host,
                    placeholder: Some("localhost".into()),
                    validators: vec![Validate::NonEmpty],
                },
            });
            editable_fields.push(FormField {
                key: "port".into(),
                field: Field::Text {
                    label: "Port".into(),
                    value: def_port,
                    placeholder: Some("5432".into()),
                    validators: vec![Validate::Port],
                },
            });
            editable_fields.push(FormField {
                key: "database".into(),
                field: Field::Text {
                    label: "Database".into(),
                    value: def_db,
                    placeholder: Some("mydb".into()),
                    validators: vec![Validate::NonEmpty],
                },
            });
            editable_fields.push(FormField {
                key: "user".into(),
                field: Field::Text {
                    label: "User".into(),
                    value: def_user,
                    placeholder: Some("postgres".into()),
                    validators: vec![],
                },
            });
            editable_fields.push(FormField {
                key: "password".into(),
                field: Field::Password {
                    label: "Password".into(),
                    value: pw_val,
                    validators: vec![],
                },
            });
        }

        let dsn_body = build_dsn(values);
        let info_section = FormSection {
            title: "Connection string".into(),
            kind: SectionKind::Info,
            fields: vec![FormField {
                key: "dsn".into(),
                field: Field::Display { label: "DSN".into(), body: dsn_body },
            }],
        };

        vec![
            FormSection { title: "Connection".into(), kind: SectionKind::Editable, fields: editable_fields },
            info_section,
        ]
    }

    // --- Seed initial values from prefill ---
    let mut seed = sid_widgets::FormValues::new();

    if let Some(conn) = prefill {
        seed.insert("name".into(), conn.name.clone());
        match conn.kind {
            DbKind::Postgres => {
                seed.insert("kind".into(), "Postgres".into());
                let (host, port, db, user) = parse_pg_dsn(&conn.dsn);
                seed.insert("host".into(), host);
                seed.insert("port".into(), port);
                seed.insert("database".into(), db);
                seed.insert("user".into(), user);
                // password is never pre-filled (it lives in the secrets table)
            }
            DbKind::Sqlite => {
                seed.insert("kind".into(), "SQLite".into());
                seed.insert("path".into(), conn.dsn.clone());
            }
        }
        // Store the existing id so submit_db_connection_form can detect edit vs create.
        seed.insert("_id".into(), conn.id.clone());
    }

    let initial_sections = make_sections(&seed);

    FormSpec::new("database.connection", "Database connection", initial_sections)
        .with_reshape(vec!["kind".into()], make_sections)
}
```

Note: `FormValues` is `pub type FormValues = BTreeMap<String, String>` (substrate Task 3) —
a plain type alias, so seeding uses ordinary `BTreeMap::insert`. Choice fields store the
selected option label (e.g. `"Postgres"`); toggles store `"true"`/`"false"`.

- [ ] **Step 3: Run the tests**

Run: `cargo test -p sid db_form_spec`
Expected: all 4 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/sid/src/wire.rs
git commit -m "feat(sid): db_connection_form_spec — Postgres/SQLite reshape, DSN info row, prefill from existing record"
```

---

### Task 4: Wire `OpenConnectionForm` command dispatch and `submit_db_connection_form`

**Files:**
- Modify: `crates/sid/src/wire.rs`
  - `drain_database_commands` (or the loop at ~L1127 that handles `DbCommand::Connect`):
    add `DbCommand::OpenConnectionForm` and `DbCommand::TestConnection` arms
  - `dispatch_form_submit` (~L1543 in the substrate spec): add the `"database.connection"` arm
  - Add `submit_db_connection_form` free function
  - Add `spawn_test_connection` free function

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn open_connection_form_no_prefill_opens_form_on_database_tab() {
    use sid_core::adapters::db_client::DbKind;
    use sid_widgets::SectionKind;
    let mut app = build_test_sid_app(Some("database"));
    // inject an OpenConnectionForm command into the database widget
    inject_db_command(&mut app, sid_widgets::database::DbCommand::OpenConnectionForm { prefill: None });
    drain_database_commands(&mut app);
    assert!(app.form.is_some(), "form should be open after OpenConnectionForm");
    let form = app.form.as_ref().unwrap();
    assert_eq!(form.spec.id.0, "database.connection");
    // editable section should have a 'kind' field
    let has_kind = form.spec.sections.iter()
        .filter(|s| s.kind == SectionKind::Editable)
        .flat_map(|s| s.fields.iter())
        .any(|f| f.key == "kind");
    assert!(has_kind, "form spec should have kind field");
}

#[test]
fn open_connection_form_with_prefill_pre_fills_name() {
    let mut app = build_test_sid_app(Some("database"));
    let conn = stub_wire_conn("myconn");
    inject_db_command(&mut app, sid_widgets::database::DbCommand::OpenConnectionForm {
        prefill: Some(conn),
    });
    drain_database_commands(&mut app);
    let form = app.form.as_ref().expect("form should be open");
    let name_val = form.spec.sections.iter()
        .flat_map(|s| s.fields.iter())
        .find(|f| f.key == "name")
        .and_then(|f| if let sid_widgets::Field::Text { value, .. } = &f.field { Some(value.as_str()) } else { None });
    assert_eq!(name_val, Some("myconn"));
}

#[test]
fn submit_db_connection_form_persists_new_connection() {
    use sid_widgets::FormValues;
    let mut app = build_test_sid_app(Some("database"));
    let mut values = FormValues::new();
    values.insert("name".into(), "Dev Postgres".into());
    values.insert("kind".into(), "Postgres".into());
    values.insert("host".into(), "localhost".into());
    values.insert("port".into(), "5432".into());
    values.insert("database".into(), "devdb".into());
    values.insert("user".into(), "dev".into());
    // no _id → create path
    let result = submit_db_connection_form(&mut app, values);
    assert!(result.is_ok(), "submit should succeed: {:?}", result);
    let conns = app.store.list_db_connections().unwrap();
    assert!(conns.iter().any(|c| c.name == "Dev Postgres"), "connection should be persisted");
}

#[test]
fn submit_db_connection_form_updates_existing_connection() {
    use sid_widgets::FormValues;
    use sid_core::adapters::db_client::DbKind;
    use sid_store::{DbConnection, now_epoch};
    let mut app = build_test_sid_app(Some("database"));
    // pre-seed a connection in the store
    let existing = DbConnection {
        id: "existing-pg".to_string(),
        kind: DbKind::Postgres,
        name: "Old Name".to_string(),
        dsn: "postgres://localhost:5432/olddb".to_string(),
        secret_ref: None,
        created_at: now_epoch(),
    };
    app.store.upsert_db_connection(&existing).unwrap();

    let mut values = FormValues::new();
    values.insert("_id".into(), "existing-pg".into());
    values.insert("name".into(), "New Name".into());
    values.insert("kind".into(), "Postgres".into());
    values.insert("host".into(), "localhost".into());
    values.insert("port".into(), "5432".into());
    values.insert("database".into(), "newdb".into());
    values.insert("user".into(), "".into());
    let result = submit_db_connection_form(&mut app, values);
    assert!(result.is_ok());
    let conns = app.store.list_db_connections().unwrap();
    assert!(conns.iter().any(|c| c.id == "existing-pg" && c.name == "New Name"),
        "existing connection should be updated");
}

// Helpers for wire.rs tests:
fn inject_db_command(app: &mut SidApp, cmd: sid_widgets::database::DbCommand) {
    for tab in app.app.tabs_mut().tabs_mut() {
        if tab.id.as_str() == "database" {
            if let Some(w) = tab.layout.iter_widgets_mut().next()
                .and_then(|w| w.as_any_mut().downcast_mut::<DatabaseWidget>())
            {
                w.state.push_command(cmd);
                return;
            }
        }
    }
    panic!("database tab or widget not found");
}

fn stub_wire_conn(name: &str) -> sid_store::DbConnection {
    use sid_core::adapters::db_client::DbKind;
    use sid_store::now_epoch;
    sid_store::DbConnection {
        id: name.to_string(),
        kind: DbKind::Postgres,
        name: name.to_string(),
        dsn: format!("postgres://localhost:5432/{name}"),
        secret_ref: None,
        created_at: now_epoch(),
    }
}
```

Run: `cargo test -p sid open_connection_form submit_db_connection_form` — expected: COMPILE ERROR.

- [ ] **Step 2: Wire `DbCommand::OpenConnectionForm` into the drain loop**

In `wire.rs`, find the section around L1127 where `("database", Some(w)) =>` dispatches
`DbCommand::Connect`. In that same match or the equivalent drain function, add:

```rust
DbCommand::OpenConnectionForm { prefill } => {
    let spec = db_connection_form_spec(prefill.as_ref());
    open_form(sid_app, spec);
}
DbCommand::TestConnection { conn_id } => {
    spawn_test_connection(sid_app, conn_id);
}
```

- [ ] **Step 3: Add `submit_db_connection_form`**

Add after `submit_database_remove` (~L4610):

```rust
/// Handle a `"database.connection"` form submit. If `values` contains `"_id"`,
/// updates the existing record (preserving `created_at`); otherwise generates a
/// new id from the name. Persists to the store and refreshes the widget.
///
/// Password handling is identical to `submit_database_new`: Postgres password
/// is written to the secrets table via `secrets.put`; the DSN stored in the
/// record never includes the password.
pub(crate) fn submit_db_connection_form(
    sid_app: &mut SidApp,
    values: sid_widgets::FormValues,
) -> Result<()> {
    use sid_core::adapters::db_client::DbKind;
    use sid_core::adapters::secrets::SecretId;
    use sid_store::{DbConnection, now_epoch};

    let name      = values.get_text("name").unwrap_or_default();
    let kind_str  = values.get_choice("kind").unwrap_or_default();
    let password  = values.get_text("password").unwrap_or_default();
    let existing_id = values.get_text("_id");

    if name.is_empty() {
        return Err(anyhow::anyhow!("connection name is required"));
    }

    let kind = match kind_str.as_deref().unwrap_or("Postgres") {
        "SQLite" => DbKind::Sqlite,
        _        => DbKind::Postgres,
    };

    let dsn = match kind {
        DbKind::Sqlite => values.get_text("path").unwrap_or_default(),
        DbKind::Postgres => {
            let host = values.get_text("host").unwrap_or_else(|| "localhost".to_string());
            let port = values.get_text("port").unwrap_or_else(|| "5432".to_string());
            let db   = values.get_text("database").unwrap_or_default();
            let user = values.get_text("user").unwrap_or_default();
            if user.is_empty() {
                format!("postgres://{host}:{port}/{db}")
            } else {
                format!("postgres://{user}@{host}:{port}/{db}")
            }
        }
    };

    if dsn.trim().is_empty() {
        return Err(anyhow::anyhow!("connection path/host is required"));
    }

    // Derive a stable id: reuse existing id when editing, slug from name when creating.
    let id = existing_id.unwrap_or_else(|| {
        name.to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
            .collect::<String>()
            .trim_matches('-')
            .to_string()
    });

    let secret_ref = if kind == DbKind::Postgres && !password.is_empty() {
        let sid_key = SecretId::new(format!("db.connection.{id}.password"));
        sid_app
            .secrets
            .put(&sid_key, password.as_bytes())
            .map_err(|e| anyhow::anyhow!("write db password: {e}"))?;
        Some(sid_key)
    } else {
        None
    };

    // Preserve created_at when updating.
    let created_at = sid_app
        .store
        .get_db_connection(&id)
        .ok()
        .flatten()
        .map(|c| c.created_at)
        .unwrap_or_else(now_epoch);

    let conn = DbConnection { id: id.clone(), kind, name: name.clone(), dsn, secret_ref, created_at };
    sid_app
        .store
        .upsert_db_connection(&conn)
        .map_err(|e| anyhow::anyhow!("upsert db connection: {e}"))?;
    refresh_database_widget(sid_app);
    sid_app.toasts.push(Toast::success(format!("connection '{name}' saved")));
    Ok(())
}
```

Note: `values.get_text(key)` / `values.get_choice(key)` — check the exact accessor method
names on `FormValues` in `form/spec.rs` after branch 0 merges; they mirror how
`string_value` / `choice_value` work on the existing `&[(String, FieldValue)]` slice, but as
methods on the `FormValues` map type. Substitute the real names.

- [ ] **Step 4: Register in `dispatch_form_submit`**

In `dispatch_form_submit` in `wire.rs` (~L1543 per substrate Task 9), add before the wildcard:

```rust
"database.connection" => {
    if let Err(e) = submit_db_connection_form(sid_app, values) {
        sid_app.toasts.push(Toast::error(format!("save connection: {e}")));
    }
}
```

- [ ] **Step 5: Add `spawn_test_connection`**

```rust
/// Spawn an off-thread connection test for `conn_id` via the configured
/// `DbClient` factory (Postgres or SQLite). Returns `JobOutcome::Success`
/// with a round-trip latency message, or `JobOutcome::Failure` with the
/// driver error text. The result surfaces as a toast via `drain_job_outcomes`.
fn spawn_test_connection(sid_app: &mut SidApp, conn_id: String) {
    use sid_core::adapters::db_client::OpenParams;
    // Retrieve connection record.
    let record = match sid_app.store.get_db_connection(&conn_id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            sid_app.toasts.push(Toast::error(format!("connection '{conn_id}' not found")));
            return;
        }
        Err(e) => {
            sid_app.toasts.push(Toast::error(format!("read connection: {e}")));
            return;
        }
    };

    // Retrieve password from secrets store if a secret_ref is present.
    let password = record.secret_ref.as_ref().and_then(|sid| {
        sid_app.secrets.get(sid).ok().flatten()
            .and_then(|bytes| String::from_utf8(bytes).ok())
    });

    let factory: Arc<dyn sid_core::adapters::db_client::DbClient> = match record.kind {
        sid_core::adapters::db_client::DbKind::Postgres => Arc::clone(&sid_app.postgres),
        sid_core::adapters::db_client::DbKind::Sqlite   => Arc::clone(&sid_app.sqlite),
    };

    let label = format!("test-connection:{conn_id}");
    let params = OpenParams { kind: record.kind, dsn: record.dsn.clone(), password };

    sid_app.toasts.push(Toast::info(format!("testing connection '{conn_id}'...")));

    sid_app.jobs.spawn(async move {
        let start = std::time::Instant::now();
        match factory.open(params).await {
            Ok(_client) => {
                let elapsed_ms = start.elapsed().as_millis();
                JobOutcome::Success {
                    label,
                    message: format!("connected in {elapsed_ms}ms"),
                }
            }
            Err(e) => JobOutcome::Failure {
                label,
                message: e.to_string(),
            }
        }
    });
}
```

- [ ] **Step 6: Run the tests**

Run: `cargo test -p sid open_connection_form submit_db_connection_form`
Expected: all 4 new tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/sid/src/wire.rs
git commit -m "feat(sid): wire OpenConnectionForm → open_form; submit_db_connection_form; spawn_test_connection off-thread"
```

---

### Task 5: Remove the old `database.new` modal path; update footer hints

**Files:**
- Modify: `crates/sid/src/wire.rs`
  - `database_modal_for_key` (~L2860): remove the `KeyCode::Char('N') | KeyCode::Char('n')`
    arm (N now goes through the widget → `OpenConnectionForm`); keep Delete/D arm as-is for
    the remove-confirm modal
  - `dispatch_form_submit` (~L3804): remove the `"database.new"` arm (replaced by
    `"database.connection"`)
  - `submit_database_new` (~L4540): keep the function but mark it `#[allow(dead_code)]` with
    a `// TODO: remove in follow-up cleanup once database.connection path is proven stable`
    comment, unless the compiler warning breaks the build — in which case, delete it here
  - `DatabaseWidget::footer_hint` in `crates/sid-widgets/src/database.rs` (~L1057): update
    hints to reflect new bindings

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn n_key_on_database_tab_does_not_open_old_modal() {
    use crossterm::event::{KeyCode, KeyModifiers};
    use sid_core::event::KeyChord;
    let mut app = build_test_sid_app(Some("database"));
    // Send 'N' through the global modal-for-key path (which is what happens when
    // the global routing currently calls database_modal_for_key).
    // After this task, database_modal_for_key should return None for 'N'.
    let chord = KeyChord { code: KeyCode::Char('N'), mods: KeyModifiers::NONE };
    let result = database_modal_for_key_for_test(&app, chord);
    assert!(result.is_none(), "N should no longer open a modal; got: {:?}", result);
}
```

(Expose `database_modal_for_key` to tests by making it `pub(crate)` or adding a thin
`pub(crate) fn database_modal_for_key_for_test` wrapper that calls the real function —
whichever is least invasive given the function's current visibility.)

Run: `cargo test -p sid n_key_on_database_tab_does_not_open_old_modal` — expected: FAIL (N still returns a modal).

- [ ] **Step 2: Remove N arm from `database_modal_for_key`**

In `database_modal_for_key` (~L2866), remove or comment out the `KeyCode::Char('N') |
KeyCode::Char('n') =>` arm. Keep only the `KeyCode::Delete | KeyCode::Char('D') |
KeyCode::Char('d')` arm and the `_` fallthrough.

- [ ] **Step 3: Remove `"database.new"` arm from modal submit dispatch**

In the modal submit routing block (~L3804), remove:

```rust
} else if key == "database.new" {
    let conn_id = submit_database_new(sid_app, values)?;
    sid_app.toasts.push(Toast::success(format!("connection …")));
    Some(conn_id)
```

Replace with a dead-code marker comment:
```rust
// database.new modal path removed — connections now use the form substrate
// via "database.connection". submit_database_new kept temporarily.
```

- [ ] **Step 4: Update `DatabaseWidget::footer_hint`**

In `database.rs` (~L1057), update the `Connections` focus hints:

```rust
fn footer_hint(&self) -> Vec<FooterHint> {
    match self.focused_pane {
        DbFocus::Connections => vec![
            FooterHint::new("Enter", "add/edit"),
            FooterHint::new("N", "new"),
            FooterHint::new("D", "delete"),
            FooterHint::new("Ctrl+T", "test"),
            FooterHint::new("→", "pane"),
        ],
        DbFocus::Editor => vec![
            FooterHint::new("Ctrl+R", "run"),
            FooterHint::new("Ctrl+D", "disconnect"),
            FooterHint::new("Tab", "results"),
        ],
        DbFocus::Results => vec![
            FooterHint::new("j/k", "row"),
            FooterHint::new("h/l", "col"),
            FooterHint::new("c", "copy"),
            FooterHint::new("Tab", "history"),
        ],
        DbFocus::History => vec![
            FooterHint::new("j/k", "select"),
            FooterHint::new("Enter", "load"),
            FooterHint::new("Tab", "editor"),
        ],
    }
}
```

(The `footer_hint` method currently returns a flat list regardless of focus. This task makes
it context-aware. The master plan decision 13 says "most-used-first"; the substrate's help
overlay shows the full list, so only the top 4–5 per pane need to be in the footer.)

- [ ] **Step 5: Run the tests**

Run: `cargo test -p sid n_key_on_database_tab_does_not_open_old_modal`
Also run: `cargo test -p sid-widgets database` and `cargo test -p sid database`
Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add crates/sid/src/wire.rs crates/sid-widgets/src/database.rs
git commit -m "feat(sid,sid-widgets): retire database.new modal path; context-aware footer hints per pane focus"
```

---

### Task 6: Snapshot tests — connection list rendering with +add new, and form open split

**Files:**
- Modify: `crates/sid-widgets/src/database.rs` (snapshot tests in `#[cfg(test)]`)
- Modify: `crates/sid/src/wire.rs` (integration snapshot in `#[cfg(test)]`)

- [ ] **Step 1: Add connection-list snapshots in `database.rs`**

```rust
#[test]
fn snapshot_connection_list_empty_with_add_new() {
    let w = DatabaseWidget::new_with_add_new(vec![], true);
    let s = render_to_string(&w, 80, 24);
    insta::assert_snapshot!("connection_list_empty_add_new", s);
}

#[test]
fn snapshot_connection_list_two_items_cursor_on_add_new() {
    // cursor at pos 0 → +add new is highlighted
    let w = DatabaseWidget::new_with_add_new(vec![stub_conn("pg"), stub_conn("staging")], true);
    let s = render_to_string(&w, 80, 24);
    insta::assert_snapshot!("connection_list_add_new_selected", s);
}

#[test]
fn snapshot_connection_list_two_items_cursor_on_first_item() {
    let mut w = DatabaseWidget::new_with_add_new(vec![stub_conn("pg"), stub_conn("staging")], true);
    w.state.select_next(); // moves to Item(0)
    let s = render_to_string(&w, 80, 24);
    insta::assert_snapshot!("connection_list_first_item_selected", s);
}

#[test]
fn snapshot_connection_list_no_add_new_row() {
    let w = DatabaseWidget::new_with_add_new(vec![stub_conn("pg")], false);
    let s = render_to_string(&w, 80, 24);
    insta::assert_snapshot!("connection_list_no_add_new", s);
}
```

Run: `cargo test -p sid-widgets snapshot_connection_list`
Then: `cargo insta review` — accept all 4 snapshots.

- [ ] **Step 2: Add integration snapshot in `wire.rs` — database tab with form open**

```rust
#[test]
fn snapshot_database_tab_form_open_split() {
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};
    let mut app = build_test_sid_app(Some("database"));
    // Open the connection form
    let spec = db_connection_form_spec(None);
    open_form(&mut app, spec);
    // Render into a 120x30 buffer
    let backend = TestBackend::new(120, 30);
    let mut term = Terminal::new(backend).unwrap();
    // Call the draw function under test — use the same draw helper the wire.rs
    // test for the modal uses (rg "draw\b" crates/sid/src/wire.rs | grep test)
    let (_theme, _) = load_active_theme(&app.store);
    // The executor fills in the exact draw call here by examining how
    // existing draw snapshot tests in wire.rs work (~L5490 region).
    let buf = term.backend().buffer();
    let s: String = (0..buf.area.height).flat_map(|y| {
        (0..buf.area.width)
            .map(move |x| buf.cell((x, y)).map(|c| c.symbol().to_string()).unwrap_or(" ".to_string()))
            .chain(std::iter::once("\n".to_string()))
    }).collect();
    insta::assert_snapshot!("database_tab_form_open_split", s);
}
```

Run: `cargo test -p sid snapshot_database_tab_form_open_split`
Then: `cargo insta review` — accept the snapshot.

- [ ] **Step 3: Commit**

```bash
git add crates/sid-widgets/src/database.rs crates/sid/src/wire.rs crates/sid-widgets/src/snapshots/ crates/sid/src/snapshots/
git commit -m "test(sid-widgets,sid): insta snapshots for database connection list +add new and form split render"
```

---

### Task 7: End-to-end wiring — `show_add_new_row` setting + live widget refresh

**Files:**
- Modify: `crates/sid/src/wire.rs`
  - `build_app` or the `DatabaseWidget` construction site (~L714): pass `show_add_new_row`
    flag to `DatabaseWidget::new_with_add_new`
  - `refresh_database_widget` (~L4820): pass the current `show_add_new_row` flag to
    `set_connections`

> **Note:** `load_show_add_new_row` and `settings_keys::SHOW_ADD_NEW_ROW` are provided by
> substrate branch 0 Task 2. If that task has not yet merged, add them here using the pattern
> from `2026-06-11-uxv2-0-substrate.md` Task 2 (a `settings_keys::SHOW_ADD_NEW_ROW` constant
> in `sid-store` and a `load_show_add_new_row(&dyn Store) -> bool` function in `wire.rs`).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn database_widget_respects_show_add_new_row_setting() {
    let mut app = build_test_sid_app(Some("database"));
    // Default (unset) → add_new = true
    {
        let w = database_widget_ref(&app);
        assert!(w.state.cursor.add_new, "cursor should have add_new=true by default");
    }
    // Turn it off in the store
    app.store.put_bool(sid_store::settings_keys::SHOW_ADD_NEW_ROW, false).unwrap();
    // Trigger a refresh (as if a connection was saved)
    refresh_database_widget(&mut app);
    {
        let w = database_widget_ref(&app);
        assert!(!w.state.cursor.add_new, "cursor should have add_new=false after setting stored false");
    }
}

fn database_widget_ref(app: &SidApp) -> &DatabaseWidget {
    app.app.tabs().tabs().iter()
        .find(|t| t.id.as_str() == "database")
        .and_then(|t| t.layout.iter_widgets().next())
        .and_then(|w| w.as_any().downcast_ref::<DatabaseWidget>())
        .expect("database widget not found")
}
```

Run: `cargo test -p sid database_widget_respects_show_add_new_row` — expected: COMPILE ERROR.

- [ ] **Step 2: Thread the setting through construction and refresh**

In `build_app` (~L714, where `DatabaseWidget::new(data.db_connections)` is called):

```rust
let show_add_new = load_show_add_new_row(&*store);
// ...
Box::new(DatabaseWidget::new_with_add_new(data.db_connections, show_add_new))
```

(The executor reads the actual `build_app` signature and the `data` struct at L644+ to place
this correctly. The `store` reference is available at the `build_app` call site.)

In `refresh_database_widget` (~L4820):

```rust
fn refresh_database_widget(sid_app: &mut SidApp) {
    let conns = match sid_app.store.list_db_connections() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("list_db_connections after form submit failed: {e}");
            return;
        }
    };
    let show_add_new = load_show_add_new_row(&*sid_app.store);
    for t in sid_app.app.tabs_mut().tabs_mut() {
        if t.id.as_str() == "database" {
            if let Some(w) = t.layout.iter_widgets_mut().next()
                .and_then(|w| w.as_any_mut().downcast_mut::<DatabaseWidget>())
            {
                w.state.set_connections(conns.clone(), show_add_new);
            }
        }
    }
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p sid database_widget_respects_show_add_new_row`
Expected: PASS.

Also run: `cargo test -p sid database` and `cargo test -p sid-widgets database`
Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add crates/sid/src/wire.rs
git commit -m "feat(sid): thread show_add_new_row setting into DatabaseWidget construction and refresh"
```

---

### Task 8: Targeted gate — clippy + fmt + focused tests

This task runs the targeted checks for this branch before declaring it ready to merge into
the UX-v2 integration branch.

- [ ] **Step 1: Run the targeted test suite**

```bash
cargo test -p sid-widgets database
cargo test -p sid database
cargo test -p sid db_form_spec
cargo test -p sid open_connection_form submit_db_connection_form n_key
```

Expected: all tests pass with no failures.

- [ ] **Step 2: Clippy (crates touched by this branch)**

```bash
cargo clippy -p sid-widgets -p sid --all-targets -- -D warnings
```

Expected: zero warnings.

- [ ] **Step 3: Format check**

```bash
cargo fmt --check -p sid-widgets -p sid
```

Expected: no diffs.

- [ ] **Step 4: Review snapshot files**

```bash
cargo insta test -p sid-widgets
cargo insta test -p sid
```

Expected: all snapshots pass (none pending review).

- [ ] **Step 5: Commit**

If clippy or fmt flagged anything, fix and commit:

```bash
git add -p   # stage only the fmt/clippy fixes
git commit -m "chore(sid,sid-widgets): clippy + fmt fixes for UX-v2 database branch"
```

If nothing needed fixing, no commit is required.
