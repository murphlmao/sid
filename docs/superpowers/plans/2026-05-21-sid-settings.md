# sid Plan 7 — Settings tab (theme picker, keybind editor, behavior toggles, workspace roots, quick actions, DB path override)

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. CLAUDE.md applies — every new pub fn needs a doc test, every function with invariants needs property tests, every parser-shaped function gets adversarial coverage. Per `no-claude-coauthor-trailer`, **no** commit in this plan carries a `Co-Authored-By: Claude…` trailer.

**Goal:** When this plan is done, the **Settings** tab is fully functional. It is sid's user-facing configuration editor — the surface that lets the project keep its README promise of "no config-file scavenger hunt". All knobs live in the redb file and are edited in-app; the only filesystem config that exists is the optional one-line `~/.config/sid/sid.toml` for overriding the DB path. The tab exposes six sub-views — Theme picker (with live preview), Keybind editor (with capture-mode state machine + conflict detection), Behavior toggles, Workspace roots editor, Quick actions editor, and DB path override — plus a Reset-to-defaults flow. A `sid settings get/set` CLI provides scripted access to the same surface.

**Architecture:** Plan 7 introduces **no new crates**. It extends:

- `sid-store` with strongly-typed setting accessors (`get_theme_name`, `put_theme_name`, `get_keybind_profile_name`, …) layered on top of the existing `settings` raw-bytes table, and a `sid_toml` module exposing the tiny `~/.config/sid/sid.toml` reader/writer (`db_path_override` only).
- `sid-ui` with a `theme_registry` module that enumerates built-in themes (`cosmos`, `void`, `dusk`, `cosmos-light`) plus user-authored themes stored in the (already-spec'd) future `themes` table — Plan 7 adds the `themes` table.
- `sid-core` with a `keybind_profile` module that round-trips a `KeybindMap` through postcard for the `keybinds` table, and a `keybind_capture` state machine module for the editor capture flow.
- `sid-widgets` with the full `SettingsWidget` impl, replacing the Plan 1 stub. The widget composes six sub-views (`ThemePickerView`, `KeybindEditorView`, `BehaviorTogglesView`, `WorkspaceRootsView`, `QuickActionsView`, `DbPathView`) glued together by a left-pane category list.
- `sid/` (binary) with `sid settings get <key>` / `sid settings set <key> <value>` subcommands and the wiring needed to inject the new engines into App construction.

Because Plan 6's System tab also touches `quick_actions`, Plan 7 builds on the same table; if Plan 6 has not yet landed, Plan 7's Phase F lands the schema and Plan 6 reads it.

**Tech stack additions:** None beyond what is already in `[workspace.dependencies]`. The plan exercises existing deps only (`toml`, `serde`, `postcard`, `proptest`, `insta`, `criterion`, `tempfile`, `tracing`, `directories`).

**Out of scope (deferred, see `2026-05-20-sid-future-features.md`):**

- In-app **theme editor** (Plan 7 ships a theme **picker**, not a per-color editor; the editor is a "Someday" item under UI/UX polish)
- **Themable star animations** ("Someday")
- **Vim-style modal keybind profiles** ("Someday")
- **Status bar customization** ("Someday")
- **Notification center** ("Someday")
- **Keyring secrets** (Plan 5 — `SecretStore`-trait swap)
- **User-configurable storage backend** (v2)
- **Sync of settings across machines** ("Someday")

These items either belong in a later plan or are explicitly catalogued as post-v1 features. Plan 7 commits to the v1 surface: picker, editor (chord remap, not palette painting), toggles, roots, quick actions, DB-path override, reset.

---

## File structure (new and modified only — existing crates unchanged unless noted)

```
sid/
├── crates/
│   ├── sid-core/
│   │   └── src/
│   │       ├── lib.rs                    # MODIFY: declare new modules
│   │       ├── keybind_profile.rs        # NEW — postcard round-trip for KeybindMap
│   │       └── keybind_capture.rs        # NEW — capture-mode state machine
│   ├── sid-store/
│   │   ├── src/
│   │   │   ├── lib.rs                    # MODIFY: + typed setting accessors, + ThemeSpec, + KeybindProfile, + QuickAction types
│   │   │   ├── schema.rs                 # MODIFY: + THEMES, KEYBINDS, QUICK_ACTIONS tables
│   │   │   ├── redb_impl.rs              # MODIFY: + theme/keybind/quick-action methods
│   │   │   └── sid_toml.rs               # NEW — sid.toml reader/writer (db_path_override only)
│   │   └── tests/
│   │       ├── settings_typed.rs         # NEW
│   │       ├── themes.rs                 # NEW
│   │       ├── keybinds.rs               # NEW
│   │       ├── quick_actions.rs          # NEW
│   │       └── sid_toml.rs               # NEW
│   ├── sid-ui/
│   │   └── src/
│   │       ├── lib.rs                    # MODIFY: + theme_registry
│   │       └── theme_registry.rs         # NEW — enumerate built-in + user themes
│   ├── sid-widgets/
│   │   └── src/
│   │       ├── settings.rs               # MODIFY: replace stub with composer
│   │       └── settings/                 # NEW sub-module dir
│   │           ├── mod.rs
│   │           ├── theme_picker.rs
│   │           ├── keybind_editor.rs
│   │           ├── behavior_toggles.rs
│   │           ├── workspace_roots.rs
│   │           ├── quick_actions.rs
│   │           ├── db_path.rs
│   │           ├── reset.rs
│   │           └── live_preview.rs
│   └── sid/
│       └── src/
│           ├── main.rs                   # MODIFY: + `sid settings get/set` subcommands
│           └── wire.rs                   # MODIFY: + theme_registry + keybind profile load, + settings injection
└── README.md                             # MODIFY
```

---

## Task index

| # | Task | Phase |
|---|------|-------|
| 1 | Typed setting accessors on `Store` + `RedbStore` | A. Storage |
| 2 | `themes` / `keybinds` / `quick_actions` table schema + redb impl | A. Storage |
| 3 | `sid_toml` module — read/write `sid.toml` (DB path only) | A. Storage |
| 4 | `theme_registry` — enumerate built-in + user themes | B. Theme picker |
| 5 | `ThemePickerView` state + navigation | B. Theme picker |
| 6 | `live_preview` — render a representative block in the focused theme | B. Theme picker |
| 7 | Apply-theme action + persist to `setting: theme_name` | B. Theme picker |
| 8 | `KeybindProfile` postcard round-trip + load/save | C. Keybind editor |
| 9 | `KeybindCaptureState` state machine + transitions | C. Keybind editor |
| 10 | `KeybindEditorView` — action list + chord display + capture entry | C. Keybind editor |
| 11 | Conflict detection + confirm-overwrite flow | C. Keybind editor |
| 12 | `BehaviorTogglesView` — state + (label, value) list | D. Behavior toggles |
| 13 | Behavior toggle persistence + reload hooks | D. Behavior toggles |
| 14 | `WorkspaceRootsView` — list + add (with validation) + remove | E. Workspace roots |
| 15 | Workspace roots persistence (JSON-blob in settings) | E. Workspace roots |
| 16 | `QuickActionsView` — list + add/edit/remove | F. Quick actions |
| 17 | Quick action persistence + keybind validation | F. Quick actions |
| 18 | `DbPathView` — display current + edit writes sid.toml | G. DB path |
| 19 | `ResetView` — confirm modal + factory-list reset | H. Reset |
| 20 | `SettingsWidget` composes sub-views with left/right pane | I. Composer |
| 21 | `SettingsWidget` Tab/Shift+Tab category cycling + save/load state | I. Composer |
| 22 | `sid settings get <key>` CLI | J. CLI |
| 23 | `sid settings set <key> <value>` CLI | J. CLI |
| 24 | Wire theme registry + keybind profile load on startup | K. Integration |
| 25 | Integration test — full settings round-trip across processes | K. Integration |
| 26 | README + Settings.md docs | K. Integration |

---

## Phase A — Storage foundations

### Task 1: Typed setting accessors on `Store` + `RedbStore`

**Files:**
- Modify: `crates/sid-store/src/lib.rs`
- Modify: `crates/sid-store/src/redb_impl.rs`
- Create: `crates/sid-store/tests/settings_typed.rs`

The existing `Store::{get_setting, put_setting}` takes raw bytes (`SettingValue(Vec<u8>)`). Plan 7 layers strongly-typed helpers on top so widgets never re-implement encode/decode for the well-known keys.

- [ ] **Step 1: Enumerate the well-known setting keys**

Append a module to `crates/sid-store/src/lib.rs`:

```rust
/// Canonical keys for settings persisted in the `settings` table.
///
/// Centralised so the Settings widget, the `sid settings get/set` CLI, and the
/// reset-to-defaults flow agree on the names byte-for-byte.
///
/// # Examples
///
/// ```
/// use sid_store::settings_keys;
/// assert_eq!(settings_keys::THEME_NAME, "theme_name");
/// assert_eq!(settings_keys::KEYBIND_PROFILE_NAME, "keybind_profile_name");
/// assert_eq!(settings_keys::WORKSPACE_ROOTS, "workspace_roots");
/// assert_eq!(settings_keys::PERSIST_DEBOUNCE_MS, "persist_debounce_ms");
/// assert_eq!(settings_keys::HEARTBEAT_INTERVAL_SECS, "heartbeat_interval_secs");
/// assert_eq!(settings_keys::AUTO_RESTORE_SESSION, "auto_restore_session");
/// assert_eq!(settings_keys::AUTO_SCAN_WORKSPACES, "auto_scan_workspaces");
/// assert_eq!(settings_keys::DEFAULT_TAB, "default_tab");
/// ```
pub mod settings_keys {
    pub const THEME_NAME: &str = "theme_name";
    pub const KEYBIND_PROFILE_NAME: &str = "keybind_profile_name";
    pub const WORKSPACE_ROOTS: &str = "workspace_roots";
    pub const PERSIST_DEBOUNCE_MS: &str = "persist_debounce_ms";
    pub const HEARTBEAT_INTERVAL_SECS: &str = "heartbeat_interval_secs";
    pub const AUTO_RESTORE_SESSION: &str = "auto_restore_session";
    pub const AUTO_SCAN_WORKSPACES: &str = "auto_scan_workspaces";
    pub const DEFAULT_TAB: &str = "default_tab";
}
```

- [ ] **Step 2: Add typed accessor extension trait**

```rust
/// String-typed setting helpers. Default impls call `get_setting`/`put_setting`
/// and codec-encode/decode the value with postcard.
///
/// All accessors return `Ok(None)` when a key is unset — they never fabricate
/// defaults; defaulting is a widget-level concern.
pub trait TypedSettings: Store {
    fn get_string(&self, key: &str) -> Result<Option<String>, SidError> {
        match self.get_setting(key)? {
            None => Ok(None),
            Some(v) => Ok(Some(String::from_utf8(v.0).map_err(|e| SidError::Storage(e.to_string()))?)),
        }
    }
    fn put_string(&self, key: &str, val: &str) -> Result<(), SidError> {
        self.put_setting(key, &SettingValue(val.as_bytes().to_vec()))
    }
    fn get_u64(&self, key: &str) -> Result<Option<u64>, SidError> {
        match self.get_setting(key)? {
            None => Ok(None),
            Some(v) => Ok(Some(std::str::from_utf8(&v.0)
                .map_err(|e| SidError::Storage(e.to_string()))?
                .parse::<u64>().map_err(|e| SidError::Storage(e.to_string()))?)),
        }
    }
    fn put_u64(&self, key: &str, val: u64) -> Result<(), SidError> {
        self.put_string(key, &val.to_string())
    }
    fn get_bool(&self, key: &str) -> Result<Option<bool>, SidError> {
        match self.get_string(key)? {
            None => Ok(None),
            Some(s) => match s.as_str() {
                "true" => Ok(Some(true)),
                "false" => Ok(Some(false)),
                other => Err(SidError::Storage(format!("invalid bool '{other}' for key '{key}'"))),
            },
        }
    }
    fn put_bool(&self, key: &str, val: bool) -> Result<(), SidError> {
        self.put_string(key, if val { "true" } else { "false" })
    }
}

impl<S: Store + ?Sized> TypedSettings for S {}
```

Storing values as UTF-8 strings (not postcard-encoded) keeps the on-disk shape inspectable by `sid settings get` without invoking the codec, and matches how raw `SettingValue` is already used in Plan 1.

- [ ] **Step 3: Write failing tests**

Create `crates/sid-store/tests/settings_typed.rs`:

```rust
use sid_store::{settings_keys, OpenStore, RedbStore, SettingValue, Store, TypedSettings};
use tempfile::tempdir;

fn store() -> (tempfile::TempDir, RedbStore) {
    let d = tempdir().unwrap();
    let s = RedbStore::open(&d.path().join("sid.redb")).unwrap();
    (d, s)
}

#[test]
fn string_round_trip() {
    let (_d, s) = store();
    assert!(s.get_string(settings_keys::THEME_NAME).unwrap().is_none());
    s.put_string(settings_keys::THEME_NAME, "cosmos").unwrap();
    assert_eq!(s.get_string(settings_keys::THEME_NAME).unwrap().as_deref(), Some("cosmos"));
}

#[test]
fn u64_round_trip() {
    let (_d, s) = store();
    s.put_u64(settings_keys::PERSIST_DEBOUNCE_MS, 250).unwrap();
    assert_eq!(s.get_u64(settings_keys::PERSIST_DEBOUNCE_MS).unwrap(), Some(250));
}

#[test]
fn bool_round_trip() {
    let (_d, s) = store();
    s.put_bool(settings_keys::AUTO_RESTORE_SESSION, true).unwrap();
    assert_eq!(s.get_bool(settings_keys::AUTO_RESTORE_SESSION).unwrap(), Some(true));
    s.put_bool(settings_keys::AUTO_RESTORE_SESSION, false).unwrap();
    assert_eq!(s.get_bool(settings_keys::AUTO_RESTORE_SESSION).unwrap(), Some(false));
}

#[test]
fn invalid_bool_returns_error() {
    let (_d, s) = store();
    s.put_setting(settings_keys::AUTO_RESTORE_SESSION, &SettingValue(b"maybe".to_vec())).unwrap();
    assert!(s.get_bool(settings_keys::AUTO_RESTORE_SESSION).is_err());
}

#[test]
fn invalid_u64_returns_error() {
    let (_d, s) = store();
    s.put_setting(settings_keys::PERSIST_DEBOUNCE_MS, &SettingValue(b"not-a-number".to_vec())).unwrap();
    assert!(s.get_u64(settings_keys::PERSIST_DEBOUNCE_MS).is_err());
}

#[test]
fn invalid_utf8_string_returns_error() {
    let (_d, s) = store();
    s.put_setting(settings_keys::THEME_NAME, &SettingValue(vec![0xFF, 0xFE])).unwrap();
    assert!(s.get_string(settings_keys::THEME_NAME).is_err());
}
```

- [ ] **Step 4: Run tests** — expected 6 passed.

- [ ] **Step 5: Property + adversarial coverage**

Append:

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_string_round_trip(s in "[\\p{L}\\p{N}_.-]{0,128}") {
        let (_d, st) = store();
        st.put_string("test_key", &s).unwrap();
        prop_assert_eq!(st.get_string("test_key").unwrap(), Some(s));
    }
    #[test]
    fn prop_u64_round_trip(v in any::<u64>()) {
        let (_d, st) = store();
        st.put_u64("k", v).unwrap();
        prop_assert_eq!(st.get_u64("k").unwrap(), Some(v));
    }
    #[test]
    fn prop_bool_round_trip(v in any::<bool>()) {
        let (_d, st) = store();
        st.put_bool("k", v).unwrap();
        prop_assert_eq!(st.get_bool("k").unwrap(), Some(v));
    }
}

#[test]
fn empty_string_round_trips() {
    let (_d, s) = store();
    s.put_string("k", "").unwrap();
    assert_eq!(s.get_string("k").unwrap().as_deref(), Some(""));
}

#[test]
fn very_long_string_round_trips() {
    let (_d, s) = store();
    let big = "x".repeat(64 * 1024);
    s.put_string("k", &big).unwrap();
    assert_eq!(s.get_string("k").unwrap().unwrap().len(), 64 * 1024);
}

#[test]
fn unicode_string_round_trips() {
    let (_d, s) = store();
    s.put_string("k", "héllo · ✦ ★ 🐕").unwrap();
    assert_eq!(s.get_string("k").unwrap().as_deref(), Some("héllo · ✦ ★ 🐕"));
}
```

- [ ] **Step 6: Doc tests on every new pub fn**

Add `# Examples` blocks to each `TypedSettings` method (8 doc tests) and to the `settings_keys` module constants (already shown).

- [ ] **Step 7: Commit**

```bash
git add crates/sid-store
git commit -m "feat(store): add TypedSettings extension trait + canonical setting keys"
```

---

### Task 2: `themes`, `keybinds`, `quick_actions` tables + redb impl

**Files:**
- Modify: `crates/sid-store/src/schema.rs`
- Modify: `crates/sid-store/src/lib.rs`
- Modify: `crates/sid-store/src/redb_impl.rs`
- Create: `crates/sid-store/tests/themes.rs`
- Create: `crates/sid-store/tests/keybinds.rs`
- Create: `crates/sid-store/tests/quick_actions.rs`

- [ ] **Step 1: Add table definitions to `schema.rs`**

```rust
/// User-saved themes. Key: theme name. Value: versioned-postcard `ThemeSpec`.
pub const THEMES: TableDefinition<&str, &[u8]> = TableDefinition::new("themes");

/// Keybind profiles. Key: profile name. Value: versioned-postcard `KeybindProfile`.
pub const KEYBINDS: TableDefinition<&str, &[u8]> = TableDefinition::new("keybinds");

/// Global quick-actions (System tab + Settings tab share this table).
/// Key: action id string. Value: versioned-postcard `QuickAction`.
pub const QUICK_ACTIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("quick_actions");
```

Update the `table_names_are_stable` test to assert the three new names.

- [ ] **Step 2: Domain types in `sid-store/src/lib.rs`**

```rust
/// A theme stored in the `themes` table. The palette + glyphs are the same
/// shape `sid_ui::theme::Theme` carries; we redeclare here to avoid making
/// `sid-store` depend on `sid-ui`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThemeSpec {
    pub name: String,
    pub palette: ThemePalette,
    pub glyphs: ThemeGlyphs,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThemePalette {
    pub background: u32,
    pub surface: u32,
    pub foreground: u32,
    pub muted: u32,
    pub accent_primary: u32,
    pub accent_success: u32,
    pub accent_warning: u32,
    pub accent_error: u32,
    pub border: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ThemeGlyphs {
    pub star: char,
    pub small_star: char,
    pub dot: char,
}

/// A keybind profile stored in the `keybinds` table. A profile is a vector of
/// (chord-string, action-id) pairs. The chord string format mirrors the
/// `ChordKey` debug shape (`"{KeyCode:?}|{u8 mods}"`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct KeybindProfile {
    pub name: String,
    pub bindings: Vec<KeybindEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct KeybindEntry {
    pub chord: String,
    pub action: String,
}

/// A global quick-action. Shared between Plan 6 (System tab) and Plan 7
/// (Settings tab editor).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct QuickAction {
    pub id: String,
    pub label: String,
    pub cmd: String,
    /// Optional chord (string format same as `KeybindEntry.chord`).
    pub keybind: Option<String>,
    pub scope: QuickActionScope,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum QuickActionScope {
    Global,
    Workspace,
}
```

- [ ] **Step 3: Extend `Store` trait**

Add the methods:

```rust
fn list_themes(&self) -> Result<Vec<ThemeSpec>, SidError>;
fn get_theme(&self, name: &str) -> Result<Option<ThemeSpec>, SidError>;
fn upsert_theme(&self, t: &ThemeSpec) -> Result<(), SidError>;
fn remove_theme(&self, name: &str) -> Result<(), SidError>;

fn list_keybind_profiles(&self) -> Result<Vec<KeybindProfile>, SidError>;
fn get_keybind_profile(&self, name: &str) -> Result<Option<KeybindProfile>, SidError>;
fn upsert_keybind_profile(&self, p: &KeybindProfile) -> Result<(), SidError>;
fn remove_keybind_profile(&self, name: &str) -> Result<(), SidError>;

fn list_quick_actions(&self) -> Result<Vec<QuickAction>, SidError>;
fn get_quick_action(&self, id: &str) -> Result<Option<QuickAction>, SidError>;
fn upsert_quick_action(&self, a: &QuickAction) -> Result<(), SidError>;
fn remove_quick_action(&self, id: &str) -> Result<(), SidError>;
```

Update the `Store` doc-test MemStore impl in `lib.rs` to add no-op stubs for these methods so the doc test still compiles.

- [ ] **Step 4: Implement on `RedbStore`**

Each pair (list/get/upsert/remove) follows the existing `workspaces` pattern in `redb_impl.rs`: open a write or read transaction, postcard-encode/decode via the `codec` module (versioned, schema-evolution-friendly), iterate via `range(..)` for `list`.

- [ ] **Step 5: Write failing tests**

Create `crates/sid-store/tests/themes.rs`, `keybinds.rs`, `quick_actions.rs` — each follows the `workspaces.rs` pattern: open store, upsert N records, list returns N in stable order, get returns the value, remove removes, get of a missing id returns None.

- [ ] **Step 6: Property tests**

For each domain type, a `proptest!` block: generate an arbitrary value, postcard-encode-then-decode, assert structural equality. Add `arbitrary_…` helper functions using `proptest`'s `prop_compose!`.

- [ ] **Step 7: Adversarial coverage**

In each test file, add:
- empty-name rejection (upsert with `name: ""` — current behaviour: allowed; assert it round-trips intact)
- very long names (4 KiB) round-trip intact
- unicode in names + labels (glyph chars, RTL, combining marks)
- corrupted blob detection — write raw garbage to the table under a known key via redb directly, assert the `get_*` returns `Err` not panic
- remove of a missing key is a no-op (matches `remove_workspace`)

- [ ] **Step 8: Run** — expected ~30 passing tests across the three files.

- [ ] **Step 9: Doc tests** on every new `Store` method + every new domain type. Each one constructs the type and asserts a field value, or opens a tempdir store and demonstrates a put/get round-trip.

- [ ] **Step 10: Commit**

```bash
git add crates/sid-store
git commit -m "feat(store): add themes, keybinds, quick_actions tables + Store methods"
```

---

### Task 3: `sid_toml` — read/write the one-line `sid.toml`

**Files:**
- Create: `crates/sid-store/src/sid_toml.rs`
- Modify: `crates/sid-store/src/lib.rs` (`pub mod sid_toml;`)
- Create: `crates/sid-store/tests/sid_toml.rs`

The spec is clear: `sid.toml` exists for exactly one reason — overriding the DB path. Anything else lives in the DB.

- [ ] **Step 1: Failing tests**

Create `crates/sid-store/tests/sid_toml.rs`:

```rust
use std::path::PathBuf;
use sid_store::sid_toml::{read_sid_toml, write_sid_toml, SidToml};
use tempfile::tempdir;

#[test]
fn read_returns_none_when_file_absent() {
    let d = tempdir().unwrap();
    let p = d.path().join("sid.toml");
    let got = read_sid_toml(&p).unwrap();
    assert!(got.db_path_override.is_none());
}

#[test]
fn write_then_read_round_trips() {
    let d = tempdir().unwrap();
    let p = d.path().join("sid.toml");
    let cfg = SidToml { db_path_override: Some(PathBuf::from("/custom/sid.redb")) };
    write_sid_toml(&p, &cfg).unwrap();
    let got = read_sid_toml(&p).unwrap();
    assert_eq!(got.db_path_override.as_deref().map(|p| p.to_str().unwrap()),
               Some("/custom/sid.redb"));
}

#[test]
fn unknown_keys_are_ignored() {
    let d = tempdir().unwrap();
    let p = d.path().join("sid.toml");
    std::fs::write(&p, "db_path_override = \"/x\"\nunknown_key = \"y\"\n").unwrap();
    let got = read_sid_toml(&p).unwrap();
    assert_eq!(got.db_path_override.as_deref().map(|p| p.to_str().unwrap()), Some("/x"));
}

#[test]
fn malformed_toml_returns_error() {
    let d = tempdir().unwrap();
    let p = d.path().join("sid.toml");
    std::fs::write(&p, "this is = = not valid toml [[[").unwrap();
    assert!(read_sid_toml(&p).is_err());
}

#[test]
fn write_creates_parent_dir() {
    let d = tempdir().unwrap();
    let p = d.path().join("nested/dir/sid.toml");
    let cfg = SidToml { db_path_override: Some(PathBuf::from("/x")) };
    write_sid_toml(&p, &cfg).unwrap();
    assert!(p.exists());
}
```

- [ ] **Step 2: Implement `sid_toml.rs`**

```rust
//! Read/write the tiny `~/.config/sid/sid.toml` file. The *only* setting that
//! lives in this file is `db_path_override`. Everything else lives in the DB.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SidToml {
    pub db_path_override: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum SidTomlError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse: {0}")]
    Parse(String),
}

pub fn read_sid_toml(path: &Path) -> Result<SidToml, SidTomlError> {
    match std::fs::read_to_string(path) {
        Ok(s) => toml::from_str(&s).map_err(|e| SidTomlError::Parse(e.to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(SidToml::default()),
        Err(e) => Err(SidTomlError::Io(e)),
    }
}

pub fn write_sid_toml(path: &Path, cfg: &SidToml) -> Result<(), SidTomlError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let s = toml::to_string_pretty(cfg).map_err(|e| SidTomlError::Parse(e.to_string()))?;
    std::fs::write(path, s)?;
    Ok(())
}
```

- [ ] **Step 3: Run tests** — expected 5 passed.

- [ ] **Step 4: Adversarial coverage**

Append to the test file:
- A 1 MiB `sid.toml` with one valid line and 1 MiB of comments — should parse fine.
- Write a sid.toml then chmod the parent dir read-only — `write_sid_toml` returns an io error.
- A file with `db_path_override = 42` (wrong type) returns a parse error.
- A file containing only a UTF-8 BOM and whitespace parses to default.

- [ ] **Step 5: Doc tests** on `read_sid_toml`, `write_sid_toml`, `SidToml` (use `no_run` for examples that name `~/.config/sid/sid.toml`; runnable examples use a `tempdir`).

- [ ] **Step 6: Commit**

```bash
git add crates/sid-store
git commit -m "feat(store): add sid_toml reader/writer for the tiny db-path override"
```

---

## Phase B — Theme picker sub-view

### Task 4: `theme_registry` — enumerate built-in + user themes

**Files:**
- Create: `crates/sid-ui/src/theme_registry.rs`
- Modify: `crates/sid-ui/src/lib.rs` (`pub mod theme_registry;`)

- [ ] **Step 1: Failing tests** (use `cargo test -p sid-ui --test theme_registry` — create `crates/sid-ui/tests/theme_registry.rs`):

```rust
use sid_ui::theme_registry::ThemeRegistry;

#[test]
fn builtins_present() {
    let r = ThemeRegistry::with_builtins();
    let names: Vec<_> = r.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"cosmos"));
    assert!(names.contains(&"void"));
    assert!(names.contains(&"dusk"));
    assert!(names.contains(&"cosmos-light"));
}

#[test]
fn get_by_name_returns_theme() {
    let r = ThemeRegistry::with_builtins();
    let t = r.get("cosmos").unwrap();
    assert_eq!(t.name, "cosmos");
}

#[test]
fn get_unknown_returns_none() {
    let r = ThemeRegistry::with_builtins();
    assert!(r.get("nonexistent-theme").is_none());
}

#[test]
fn user_themes_override_builtins() {
    use sid_ui::theme::{Color, GlyphSet, Theme};
    let mut r = ThemeRegistry::with_builtins();
    let custom = Theme {
        name: "cosmos".into(),
        background: Color::rgb(1, 2, 3),
        surface: Color::rgb(0, 0, 0),
        foreground: Color::rgb(0, 0, 0),
        muted: Color::rgb(0, 0, 0),
        accent_primary: Color::rgb(0, 0, 0),
        accent_success: Color::rgb(0, 0, 0),
        accent_warning: Color::rgb(0, 0, 0),
        accent_error: Color::rgb(0, 0, 0),
        border: Color::rgb(0, 0, 0),
        glyphs: GlyphSet::default(),
    };
    r.register(custom);
    assert_eq!(r.get("cosmos").unwrap().background.r, 1);
}
```

- [ ] **Step 2: Implement**

```rust
use std::collections::BTreeMap;

use crate::theme::Theme;
use crate::themes::{cosmos, cosmos_light, dusk, void};

pub struct ThemeRegistry {
    by_name: BTreeMap<String, Theme>,
}

impl ThemeRegistry {
    pub fn empty() -> Self { Self { by_name: BTreeMap::new() } }
    pub fn with_builtins() -> Self {
        let mut r = Self::empty();
        for t in [cosmos(), void(), dusk(), cosmos_light()] {
            r.register(t);
        }
        r
    }
    pub fn register(&mut self, t: Theme) { self.by_name.insert(t.name.clone(), t); }
    pub fn get(&self, name: &str) -> Option<&Theme> { self.by_name.get(name) }
    pub fn iter(&self) -> impl Iterator<Item = &Theme> { self.by_name.values() }
    pub fn names(&self) -> Vec<&str> { self.by_name.keys().map(|s| s.as_str()).collect() }
    pub fn len(&self) -> usize { self.by_name.len() }
    pub fn is_empty(&self) -> bool { self.by_name.is_empty() }
}

impl Default for ThemeRegistry {
    fn default() -> Self { Self::with_builtins() }
}
```

- [ ] **Step 3: Adversarial + property**

```rust
#[test]
fn empty_registry_has_no_themes() {
    let r = ThemeRegistry::empty();
    assert_eq!(r.len(), 0);
    assert!(r.is_empty());
}

#[test]
fn iter_yields_themes_in_sorted_order() {
    let r = ThemeRegistry::with_builtins();
    let names: Vec<_> = r.iter().map(|t| t.name.clone()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
}
```

- [ ] **Step 4: Doc tests** on every pub fn (8 doc tests).

- [ ] **Step 5: Commit**

```bash
git add crates/sid-ui
git commit -m "feat(ui): add ThemeRegistry enumerating built-in + user themes"
```

---

### Task 5: `ThemePickerView` — state + navigation

**Files:**
- Create: `crates/sid-widgets/src/settings/mod.rs`
- Create: `crates/sid-widgets/src/settings/theme_picker.rs`
- Modify: `crates/sid-widgets/src/settings.rs` (later — Task 20 makes this the composer)

- [ ] **Step 1: Failing tests**

In `crates/sid-widgets/src/settings/theme_picker.rs`:

```rust
//! Theme picker sub-view: a list of available themes with a focused index.

use sid_ui::theme::Theme;
use sid_ui::theme_registry::ThemeRegistry;

pub struct ThemePickerView {
    themes: Vec<Theme>,
    focused: usize,
    /// The name currently *applied* (persisted), which may differ from the
    /// focused one (live-preview vs persisted distinction).
    applied: String,
}

impl ThemePickerView {
    pub fn new(registry: &ThemeRegistry, applied_name: &str) -> Self {
        let themes: Vec<Theme> = registry.iter().cloned().collect();
        let focused = themes.iter().position(|t| t.name == applied_name).unwrap_or(0);
        Self { themes, focused, applied: applied_name.to_string() }
    }
    pub fn focused(&self) -> &Theme { &self.themes[self.focused] }
    pub fn focused_index(&self) -> usize { self.focused }
    pub fn applied_name(&self) -> &str { &self.applied }
    pub fn next(&mut self) {
        if self.themes.is_empty() { return; }
        self.focused = (self.focused + 1) % self.themes.len();
    }
    pub fn prev(&mut self) {
        if self.themes.is_empty() { return; }
        self.focused = if self.focused == 0 { self.themes.len() - 1 } else { self.focused - 1 };
    }
    pub fn jump(&mut self, idx: usize) -> bool {
        if idx >= self.themes.len() { return false; }
        self.focused = idx;
        true
    }
    pub fn apply_focused(&mut self) -> &str {
        self.applied = self.focused().name.clone();
        &self.applied
    }
    pub fn themes(&self) -> &[Theme] { &self.themes }
}
```

Tests (in a `#[cfg(test)] mod tests` block at the bottom):
- `new_with_known_applied_starts_focused_on_applied`
- `new_with_unknown_applied_starts_at_zero`
- `next_wraps`
- `prev_wraps`
- `jump_in_bounds_succeeds`
- `jump_out_of_bounds_returns_false`
- `apply_focused_updates_applied`
- Property test: `next` then `prev` from any starting index returns to start (cycling invariant, per CLAUDE.md tab-manager pattern).

- [ ] **Step 2: Adversarial coverage**

- Empty registry: `next`/`prev`/`focused_index` do not panic. `focused()` is undefined for empty — assert `panic!` via `#[should_panic]` test (we mandate empty-registry isn't a real state in production; the registry always has built-ins). This matches the Plan 1 `TabManager::new(vec![])` panic contract.

- [ ] **Step 3: Doc tests** on every pub fn (9 doc tests).

- [ ] **Step 4: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): add ThemePickerView state + navigation"
```

---

### Task 6: `live_preview` — render a representative block in the focused theme

**Files:**
- Create: `crates/sid-widgets/src/settings/live_preview.rs`

The right pane of the theme picker shows a small representative block painted in the *focused* (not necessarily applied) theme. Per the spec's open-items resolution: re-render on hover.

- [ ] **Step 1: Failing test (insta snapshot)**

```rust
use sid_widgets::settings::live_preview::render_preview;
use sid_ui::themes::cosmos;

#[test]
fn preview_snapshot_cosmos() {
    let theme = cosmos();
    let buffer = render_preview(&theme, 40, 12); // returns a string-rendered buffer
    insta::assert_snapshot!(buffer);
}
```

- [ ] **Step 2: Implement `render_preview(theme, w, h) -> String`**

Use ratatui's `Buffer::empty(Rect)` + paint a small mock layout:
- Title line (foreground on background, with the `glyphs.small_star`)
- Block border in `theme.border`
- A list of 3 fake rows: one with `accent_primary`, one muted, one with `accent_success`
- Footer hint line

Convert the `Buffer` to a stable ASCII representation using a helper that prints `<theme.name>`-prefixed ANSI-less text rows. Snapshot via `insta`.

- [ ] **Step 3: Snapshot each built-in theme**

```rust
#[test] fn preview_snapshot_void() { … }
#[test] fn preview_snapshot_dusk() { … }
#[test] fn preview_snapshot_cosmos_light() { … }
```

- [ ] **Step 4: Adversarial coverage**

- `render_preview(&theme, 0, 0)` returns an empty string (no panic on zero-area).
- `render_preview(&theme, 1, 1)` returns a 1-cell render (no panic).
- A theme with `glyphs.star = '\0'` renders without panic (no zero-byte injection in the output).

- [ ] **Step 5: Criterion bench**

Add a bench in `crates/sid-widgets/benches/live_preview.rs`:

```rust
use criterion::{criterion_group, criterion_main, Criterion};
fn bench_preview(c: &mut Criterion) {
    let theme = sid_ui::themes::cosmos();
    c.bench_function("live_preview 40x12", |b| b.iter(|| {
        sid_widgets::settings::live_preview::render_preview(&theme, 40, 12)
    }));
}
criterion_group!(benches, bench_preview);
criterion_main!(benches);
```

Target: well under 100 µs per render (re-render-on-hover must feel instant).

- [ ] **Step 6: Doc tests** on `render_preview`.

- [ ] **Step 7: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): live theme preview rendering with insta snapshots"
```

---

### Task 7: Apply-theme action + persist to `setting: theme_name`

**Files:**
- Modify: `crates/sid-widgets/src/settings/theme_picker.rs`
- Modify: `crates/sid-widgets/src/settings/mod.rs` (wire `ThemePickerView::handle_event`)

- [ ] **Step 1: Failing test**

```rust
#[test]
fn apply_persists_theme_name_to_store() {
    use sid_store::{settings_keys, OpenStore, RedbStore, Store, TypedSettings};
    let d = tempfile::tempdir().unwrap();
    let store = RedbStore::open(&d.path().join("s.redb")).unwrap();
    let registry = ThemeRegistry::with_builtins();
    let mut view = ThemePickerView::new(&registry, "cosmos");
    view.next();
    view.next(); // focus "dusk" assuming sorted order: cosmos, cosmos-light, dusk, void
    let name = view.apply_focused();
    store.put_string(settings_keys::THEME_NAME, name).unwrap();
    assert_eq!(store.get_string(settings_keys::THEME_NAME).unwrap().as_deref(),
               Some(view.focused().name.as_str()));
}
```

- [ ] **Step 2: Wire `handle_event`**

```rust
pub enum ThemePickerOutcome {
    None,
    PreviewChanged,
    Applied { name: String },
}

impl ThemePickerView {
    pub fn handle_event(&mut self, ev: &sid_core::event::Event) -> ThemePickerOutcome {
        use crossterm::event::KeyCode;
        use sid_core::event::Event;
        match ev {
            Event::Key(k) => match k.code {
                KeyCode::Down | KeyCode::Char('j') => { self.next(); ThemePickerOutcome::PreviewChanged }
                KeyCode::Up   | KeyCode::Char('k') => { self.prev(); ThemePickerOutcome::PreviewChanged }
                KeyCode::Enter => ThemePickerOutcome::Applied { name: self.apply_focused().to_string() },
                _ => ThemePickerOutcome::None,
            }
            _ => ThemePickerOutcome::None,
        }
    }
}
```

- [ ] **Step 3: Tests for handle_event**

- Down arrow yields `PreviewChanged` and advances focus.
- Enter yields `Applied { name }` matching the focused theme.
- An unrelated key yields `None` with no state change.

- [ ] **Step 4: Adversarial coverage**

- After applying, `applied_name() == focused().name` (idempotent re-apply yields the same name).
- Apply, navigate away, re-apply — `applied` updates to the new focused name.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): theme picker apply persists theme_name setting"
```

---

## Phase C — Keybind editor sub-view

### Task 8: `KeybindProfile` postcard round-trip + load/save

**Files:**
- Create: `crates/sid-core/src/keybind_profile.rs`
- Modify: `crates/sid-core/src/lib.rs` (`pub mod keybind_profile;`)
- Modify: `crates/sid-store/src/redb_impl.rs` (wire up load/save against the trait)

`KeybindMap` (Plan 1) lives in memory; `KeybindProfile` (Plan 7) is the persisted form. This task adds a deterministic encoder/decoder that converts between them and uses `sid-store::KeybindEntry` as the wire shape.

- [ ] **Step 1: Failing tests** (in a new `crates/sid-core/tests/keybind_profile.rs`):

```rust
use crossterm::event::{KeyCode, KeyModifiers};
use sid_core::action::ActionId;
use sid_core::event::KeyChord;
use sid_core::keybind::{KeyBinding, KeybindMap};
use sid_core::keybind_profile::{from_map, to_map, ProfileEntry};

#[test]
fn empty_map_round_trips() {
    let m = KeybindMap::new();
    let entries = from_map(&m);
    assert!(entries.is_empty());
    let m2 = to_map(&entries);
    assert!(m2.lookup(&KeyChord::new(KeyCode::Char('q'), KeyModifiers::CONTROL)).is_none());
}

#[test]
fn cosmos_default_round_trips() {
    let m = KeybindMap::cosmos_default();
    let entries = from_map(&m);
    let m2 = to_map(&entries);
    // Quit chord present in both
    let quit = KeyChord::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
    assert_eq!(m.lookup(&quit).map(|a| a.as_str()),
               m2.lookup(&quit).map(|a| a.as_str()));
}

#[test]
fn entries_are_deterministic_order() {
    let m = KeybindMap::cosmos_default();
    let a = from_map(&m);
    let b = from_map(&m);
    assert_eq!(a, b);
}
```

- [ ] **Step 2: Implement `keybind_profile.rs`**

```rust
use crossterm::event::{KeyCode, KeyModifiers};

use crate::action::ActionId;
use crate::event::KeyChord;
use crate::keybind::{KeyBinding, KeybindMap};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileEntry {
    pub chord: String,
    pub action: String,
}

pub fn chord_to_string(c: &KeyChord) -> String {
    format!("{:?}|{}", c.code, c.mods.bits())
}

pub fn chord_from_string(s: &str) -> Result<KeyChord, String> {
    let (code_s, mods_s) = s.rsplit_once('|').ok_or_else(|| format!("missing '|' in {s:?}"))?;
    let bits: u8 = mods_s.parse().map_err(|e| format!("bad mods bits: {e}"))?;
    let code = parse_keycode(code_s)?;
    let mods = KeyModifiers::from_bits(bits)
        .ok_or_else(|| format!("invalid mod bits {bits}"))?;
    Ok(KeyChord::new(code, mods))
}

fn parse_keycode(s: &str) -> Result<KeyCode, String> {
    // Inverse of `format!("{:?}", code)` for the variants we use.
    // Examples: "Char('q')", "Left", "Enter", "F(5)"
    if let Some(rest) = s.strip_prefix("Char('").and_then(|r| r.strip_suffix("')")) {
        let mut chars = rest.chars();
        let c = chars.next().ok_or("empty Char")?;
        if chars.next().is_some() { return Err(format!("multi-char Char({s})")); }
        return Ok(KeyCode::Char(c));
    }
    match s {
        "Left"  => Ok(KeyCode::Left),
        "Right" => Ok(KeyCode::Right),
        "Up"    => Ok(KeyCode::Up),
        "Down"  => Ok(KeyCode::Down),
        "Enter" => Ok(KeyCode::Enter),
        "Esc"   => Ok(KeyCode::Esc),
        "Tab"   => Ok(KeyCode::Tab),
        "BackTab" => Ok(KeyCode::BackTab),
        "Backspace" => Ok(KeyCode::Backspace),
        "Delete" => Ok(KeyCode::Delete),
        "Home" => Ok(KeyCode::Home),
        "End" => Ok(KeyCode::End),
        "PageUp" => Ok(KeyCode::PageUp),
        "PageDown" => Ok(KeyCode::PageDown),
        other => {
            if let Some(rest) = other.strip_prefix("F(").and_then(|r| r.strip_suffix(')')) {
                let n: u8 = rest.parse().map_err(|e| format!("bad F-key {other}: {e}"))?;
                return Ok(KeyCode::F(n));
            }
            Err(format!("unknown KeyCode: {other}"))
        }
    }
}

pub fn from_map(map: &KeybindMap) -> Vec<ProfileEntry> {
    // KeybindMap currently has no public iterator; this requires a small Plan 1
    // extension: add `pub fn iter(&self) -> impl Iterator<Item = (KeyChord, &ActionId)>`.
    // (Add in Task 8 Step 0 — see below.)
    map.iter()
        .map(|(chord, action)| ProfileEntry {
            chord: chord_to_string(&chord),
            action: action.as_str().to_string(),
        })
        .collect()
}

pub fn to_map(entries: &[ProfileEntry]) -> KeybindMap {
    let mut m = KeybindMap::new();
    for e in entries {
        if let Ok(chord) = chord_from_string(&e.chord) {
            m.bind(KeyBinding { chord, action: ActionId::new(&e.action) });
        }
    }
    m
}
```

- [ ] **Step 0 (prerequisite to Step 2): add `KeybindMap::iter`**

`crates/sid-core/src/keybind.rs` currently has no iterator. Add:

```rust
impl KeybindMap {
    pub fn iter(&self) -> impl Iterator<Item = (KeyChord, &ActionId)> + '_ {
        self.by_chord.iter().filter_map(|(k, a)| chord_from_chord_key(k).map(|c| (c, a)))
    }
}
fn chord_from_chord_key(k: &ChordKey) -> Option<KeyChord> {
    sid_core::keybind_profile::chord_from_string(&k.0).ok()
}
```

(Or expose the chord by storing it alongside the action — small refactor; either approach works. The plan recommends storing `(KeyChord, ActionId)` pairs in `KeybindMap` and recomputing the `ChordKey` on lookup, to avoid the round-trip-through-string hack. This is a minor refactor of Plan 1's `keybind.rs`.)

- [ ] **Step 3: Adversarial / property coverage**

```rust
proptest! {
    #[test]
    fn prop_chord_string_round_trip(
        code in prop_oneof![
            Just(KeyCode::Left), Just(KeyCode::Right), Just(KeyCode::Up), Just(KeyCode::Down),
            Just(KeyCode::Enter), Just(KeyCode::Esc), Just(KeyCode::Tab),
            (any::<char>().prop_filter("printable", |c| !c.is_control())).prop_map(KeyCode::Char),
        ],
        mods_bits in 0u8..=15u8,
    ) {
        let mods = KeyModifiers::from_bits(mods_bits).unwrap_or(KeyModifiers::NONE);
        let c = KeyChord::new(code, mods);
        let s = chord_to_string(&c);
        let c2 = chord_from_string(&s).expect("round-trip");
        prop_assert_eq!(c.code, c2.code);
        prop_assert_eq!(c.mods.bits(), c2.mods.bits());
    }
}

#[test]
fn malformed_chord_string_returns_err() {
    assert!(chord_from_string("").is_err());
    assert!(chord_from_string("no-bar").is_err());
    assert!(chord_from_string("Junk|0").is_err());
    assert!(chord_from_string("Char('q')|999").is_err()); // bits don't fit u8
}
```

- [ ] **Step 4: Wire load/save against `Store`**

Add helpers on top of `Store` (extension trait or free functions):

```rust
pub fn load_keybind_profile(store: &dyn Store, name: &str)
    -> Result<Option<KeybindMap>, SidError>
{
    let Some(p) = store.get_keybind_profile(name)? else { return Ok(None); };
    let entries: Vec<ProfileEntry> = p.bindings.into_iter()
        .map(|e| ProfileEntry { chord: e.chord, action: e.action })
        .collect();
    Ok(Some(to_map(&entries)))
}

pub fn save_keybind_profile(store: &dyn Store, name: &str, map: &KeybindMap)
    -> Result<(), SidError>
{
    let bindings = from_map(map).into_iter()
        .map(|e| KeybindEntry { chord: e.chord, action: e.action })
        .collect();
    store.upsert_keybind_profile(&KeybindProfile { name: name.into(), bindings })
}
```

These free functions live in `sid-store::keybind_load` and the `sid-widgets` settings sub-views call them. The widget code still names `sid-core`'s `KeybindMap` and `sid-store`'s `Store` only — no external-crate leak.

- [ ] **Step 5: Doc tests** on every pub fn.

- [ ] **Step 6: Commit**

```bash
git add crates/sid-core crates/sid-store
git commit -m "feat(core,store): KeybindMap <-> KeybindProfile postcard round-trip"
```

---

### Task 9: `KeybindCaptureState` state machine

**Files:**
- Create: `crates/sid-core/src/keybind_capture.rs`
- Modify: `crates/sid-core/src/lib.rs`

The keybind editor enters capture mode when the user presses `Enter` on an action row. The state machine:

```
            Enter
   Idle ─────────────▶ Waiting
    ▲                    │
    │ Esc                │ KeyChord captured
    │                    ▼
    │                Captured(chord)
    │                    │
    │                    │ conflict? ──Yes──▶ ConfirmOverwrite(chord, conflicting_action)
    │ Esc                │                          │
    │                    │ No                       │ y → Apply
    │                    ▼                          │ n / Esc → return to Waiting
    └──────────────── Apply ◀───────────────────────┘
```

- [ ] **Step 1: Define the enum + transitions**

```rust
use crate::action::ActionId;
use crate::event::KeyChord;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaptureState {
    Idle,
    Waiting { for_action: ActionId },
    Captured { for_action: ActionId, chord: KeyChord },
    ConfirmOverwrite {
        for_action: ActionId,
        chord: KeyChord,
        conflicting_action: ActionId,
    },
    Apply { for_action: ActionId, chord: KeyChord },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaptureInput {
    EnterCaptureFor(ActionId),
    ChordPressed(KeyChord),
    ConflictResolved { conflicting_action: ActionId },
    NoConflict,
    ConfirmYes,
    ConfirmNo,
    Cancel,
    Reset,
}

impl CaptureState {
    pub fn new() -> Self { Self::Idle }
    pub fn step(self, input: CaptureInput) -> Self {
        use CaptureInput::*;
        use CaptureState::*;
        match (self, input) {
            (Idle, EnterCaptureFor(a)) => Waiting { for_action: a },
            (Waiting { for_action }, ChordPressed(c)) => Captured { for_action, chord: c },
            (Waiting { .. }, Cancel) => Idle,

            (Captured { for_action, chord }, NoConflict) => Apply { for_action, chord },
            (Captured { for_action, chord }, ConflictResolved { conflicting_action }) =>
                ConfirmOverwrite { for_action, chord, conflicting_action },
            (Captured { .. }, Cancel) => Idle,

            (ConfirmOverwrite { for_action, chord, .. }, ConfirmYes) =>
                Apply { for_action, chord },
            (ConfirmOverwrite { for_action, .. }, ConfirmNo) =>
                Waiting { for_action },
            (ConfirmOverwrite { .. }, Cancel) => Idle,

            (Apply { .. }, Reset) => Idle,
            (Apply { .. }, _) => Idle, // any input post-apply resets
            (state, _) => state, // any other (state, input) pair is a no-op
        }
    }
}

impl Default for CaptureState { fn default() -> Self { Self::Idle } }
```

- [ ] **Step 2: Failing tests — full transition table**

In a `#[cfg(test)] mod tests` block, test every valid transition AND every invalid transition (verifies state machine doesn't accept invalid inputs):

```rust
use super::*;
use crossterm::event::{KeyCode, KeyModifiers};
use crate::event::KeyChord;
use crate::action::ActionId;

fn chord() -> KeyChord { KeyChord::new(KeyCode::Char('x'), KeyModifiers::CONTROL) }
fn action() -> ActionId { ActionId::new("test.action") }

#[test]
fn idle_enter_capture_goes_to_waiting() {
    let s = CaptureState::Idle;
    let s = s.step(CaptureInput::EnterCaptureFor(action()));
    assert!(matches!(s, CaptureState::Waiting { .. }));
}

#[test]
fn waiting_chord_pressed_goes_to_captured() {
    let s = CaptureState::Waiting { for_action: action() };
    let s = s.step(CaptureInput::ChordPressed(chord()));
    assert!(matches!(s, CaptureState::Captured { .. }));
}

#[test]
fn captured_no_conflict_applies() {
    let s = CaptureState::Captured { for_action: action(), chord: chord() };
    let s = s.step(CaptureInput::NoConflict);
    assert!(matches!(s, CaptureState::Apply { .. }));
}

#[test]
fn captured_with_conflict_goes_to_confirm() {
    let s = CaptureState::Captured { for_action: action(), chord: chord() };
    let s = s.step(CaptureInput::ConflictResolved { conflicting_action: ActionId::new("other") });
    assert!(matches!(s, CaptureState::ConfirmOverwrite { .. }));
}

#[test]
fn confirm_yes_applies() {
    let s = CaptureState::ConfirmOverwrite {
        for_action: action(), chord: chord(),
        conflicting_action: ActionId::new("other"),
    };
    let s = s.step(CaptureInput::ConfirmYes);
    assert!(matches!(s, CaptureState::Apply { .. }));
}

#[test]
fn confirm_no_returns_to_waiting() {
    let s = CaptureState::ConfirmOverwrite {
        for_action: action(), chord: chord(),
        conflicting_action: ActionId::new("other"),
    };
    let s = s.step(CaptureInput::ConfirmNo);
    assert!(matches!(s, CaptureState::Waiting { .. }));
}

#[test]
fn cancel_from_any_state_returns_to_idle() {
    for s in [
        CaptureState::Waiting { for_action: action() },
        CaptureState::Captured { for_action: action(), chord: chord() },
        CaptureState::ConfirmOverwrite {
            for_action: action(), chord: chord(),
            conflicting_action: ActionId::new("o"),
        },
    ] {
        let s = s.step(CaptureInput::Cancel);
        assert!(matches!(s, CaptureState::Idle), "expected Idle, got {s:?}");
    }
}

// Invalid transitions (state, input) pairs that must NOT change state:
#[test]
fn idle_chord_pressed_is_noop() {
    let s = CaptureState::Idle;
    let s2 = s.clone().step(CaptureInput::ChordPressed(chord()));
    assert_eq!(s, s2);
}

#[test]
fn idle_confirm_yes_is_noop() {
    let s = CaptureState::Idle;
    let s2 = s.clone().step(CaptureInput::ConfirmYes);
    assert_eq!(s, s2);
}

#[test]
fn waiting_confirm_yes_is_noop() {
    let s = CaptureState::Waiting { for_action: action() };
    let s2 = s.clone().step(CaptureInput::ConfirmYes);
    assert_eq!(s, s2);
}

#[test]
fn apply_any_input_resets_to_idle() {
    let s = CaptureState::Apply { for_action: action(), chord: chord() };
    let s = s.step(CaptureInput::Reset);
    assert_eq!(s, CaptureState::Idle);
}
```

- [ ] **Step 3: Property test — total function**

```rust
proptest! {
    #[test]
    fn prop_step_is_total(
        state_pick in 0u8..5,
        input_pick in 0u8..8,
    ) {
        let s = match state_pick {
            0 => CaptureState::Idle,
            1 => CaptureState::Waiting { for_action: action() },
            2 => CaptureState::Captured { for_action: action(), chord: chord() },
            3 => CaptureState::ConfirmOverwrite {
                for_action: action(), chord: chord(),
                conflicting_action: ActionId::new("o"),
            },
            _ => CaptureState::Apply { for_action: action(), chord: chord() },
        };
        let i = match input_pick {
            0 => CaptureInput::EnterCaptureFor(action()),
            1 => CaptureInput::ChordPressed(chord()),
            2 => CaptureInput::NoConflict,
            3 => CaptureInput::ConflictResolved { conflicting_action: ActionId::new("o") },
            4 => CaptureInput::ConfirmYes,
            5 => CaptureInput::ConfirmNo,
            6 => CaptureInput::Cancel,
            _ => CaptureInput::Reset,
        };
        // The transition must not panic for any (state, input) pair.
        let _ = s.step(i);
    }
}
```

- [ ] **Step 4: Doc tests** on `CaptureState`, `CaptureInput`, `CaptureState::step`, `CaptureState::new`.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-core
git commit -m "feat(core): keybind capture state machine + total-function property"
```

---

### Task 10: `KeybindEditorView` — action list + chord display + capture entry

**Files:**
- Create: `crates/sid-widgets/src/settings/keybind_editor.rs`

- [ ] **Step 1: Define the view state**

```rust
use sid_core::action::{ActionId, ActionRegistry};
use sid_core::event::KeyChord;
use sid_core::keybind::KeybindMap;
use sid_core::keybind_capture::{CaptureInput, CaptureState};

pub struct KeybindEditorView {
    actions: Vec<ActionId>,
    /// Local mutable copy of the binding map; saved back to the store on apply.
    map: KeybindMap,
    focused: usize,
    capture: CaptureState,
}

impl KeybindEditorView {
    pub fn new(registry: &ActionRegistry, map: KeybindMap) -> Self {
        let actions: Vec<_> = registry.all().map(|a| a.id.clone()).collect();
        Self { actions, map, focused: 0, capture: CaptureState::new() }
    }
    pub fn focused_action(&self) -> Option<&ActionId> { self.actions.get(self.focused) }
    pub fn binding_for(&self, action: &ActionId) -> Option<KeyChord> {
        // KeybindMap::iter from Task 8 lets us inspect.
        self.map.iter().find(|(_, a)| *a == action).map(|(c, _)| c)
    }
    pub fn next(&mut self) {
        if !self.actions.is_empty() { self.focused = (self.focused + 1) % self.actions.len(); }
    }
    pub fn prev(&mut self) {
        if !self.actions.is_empty() {
            self.focused = if self.focused == 0 { self.actions.len() - 1 } else { self.focused - 1 };
        }
    }
    pub fn capture_state(&self) -> &CaptureState { &self.capture }
    pub fn enter_capture(&mut self) {
        if let Some(a) = self.actions.get(self.focused).cloned() {
            self.capture = std::mem::take(&mut self.capture).step(CaptureInput::EnterCaptureFor(a));
        }
    }
    pub fn cancel_capture(&mut self) {
        self.capture = std::mem::take(&mut self.capture).step(CaptureInput::Cancel);
    }
    pub fn map(&self) -> &KeybindMap { &self.map }
}
```

- [ ] **Step 2: Tests**

- Construct with default registry from Plan 1 (the action_ids list). Focused starts at 0.
- `next` cycles; `prev` cycles.
- `enter_capture` from Idle puts us in `Waiting { for_action: actions[0] }`.
- `enter_capture` from non-Idle is a no-op (matches state machine).
- `cancel_capture` from any state returns to Idle.
- `binding_for(quit_action)` returns the `Ctrl+Q` chord under the cosmos default.

- [ ] **Step 3: Adversarial coverage**

- Empty `ActionRegistry` — view constructs; `enter_capture` is a no-op (no focused action).
- 1000-action registry — `next` cycles cleanly.
- A bound action whose chord is not lookupable via `binding_for` (because chord was removed from map post-construction) returns `None`.

- [ ] **Step 4: Doc tests** on every pub fn.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): KeybindEditorView with capture-mode entry/cancel"
```

---

### Task 11: Conflict detection + confirm-overwrite flow

**Files:**
- Modify: `crates/sid-widgets/src/settings/keybind_editor.rs`

- [ ] **Step 1: Add chord-pressed handler**

```rust
impl KeybindEditorView {
    /// Called once a chord is captured while in Waiting state. Detects conflicts.
    pub fn on_chord_captured(&mut self, chord: KeyChord) {
        self.capture = std::mem::take(&mut self.capture).step(CaptureInput::ChordPressed(chord));
        // Detect conflict.
        let CaptureState::Captured { for_action, chord } = self.capture.clone() else { return; };
        match self.map.lookup(&chord) {
            Some(existing) if existing != &for_action => {
                let conflicting = existing.clone();
                self.capture = std::mem::take(&mut self.capture)
                    .step(CaptureInput::ConflictResolved { conflicting_action: conflicting });
            }
            _ => {
                self.capture = std::mem::take(&mut self.capture).step(CaptureInput::NoConflict);
                self.apply_if_ready();
            }
        }
    }

    pub fn confirm_overwrite_yes(&mut self) {
        self.capture = std::mem::take(&mut self.capture).step(CaptureInput::ConfirmYes);
        self.apply_if_ready();
    }
    pub fn confirm_overwrite_no(&mut self) {
        self.capture = std::mem::take(&mut self.capture).step(CaptureInput::ConfirmNo);
    }

    fn apply_if_ready(&mut self) {
        if let CaptureState::Apply { for_action, chord } = self.capture.clone() {
            // Remove any existing binding for this chord, then bind the new one.
            self.map.bind(sid_core::keybind::KeyBinding { chord, action: for_action });
            self.capture = std::mem::take(&mut self.capture).step(CaptureInput::Reset);
        }
    }
}
```

- [ ] **Step 2: Tests**

- Bind `Ctrl+X` (unbound chord) to focused action → no conflict, applied.
- Bind `Ctrl+Q` (already bound to `app.quit`) to a different action → ConfirmOverwrite shown.
- `confirm_overwrite_yes` re-binds and applies; old binding is gone.
- `confirm_overwrite_no` returns to Waiting (user picks a different chord).
- Bind a chord to its own current action (no actual change) → NoConflict path, idempotent.

- [ ] **Step 3: Adversarial coverage**

- Unbinding the quit chord (`Ctrl+Q`) — should be allowed (per the prompt's directive "should warn but allow"). The widget renders a warning in the toast bar but still allows the apply. Test: trigger a toast on quit-chord rebind; assert the binding is applied. Add a `pub fn dangerous_action_warnings(&self) -> Vec<&'static str>` returning a list of human-readable warning strings to show in the UI; test it returns a warning when unbinding/overwriting `app.quit`'s chord.
- Re-binding to the same chord it already has → NoConflict, no-op map mutation.
- Very long action labels (256 chars) render correctly (snapshot test).
- Unicode in action IDs — `binding_for` finds them.

- [ ] **Step 4: Insta snapshot**

```rust
#[test]
fn snapshot_confirm_overwrite_render() {
    let view = … ; // construct in ConfirmOverwrite state
    let s = render_keybind_editor(&view);
    insta::assert_snapshot!(s);
}
```

- [ ] **Step 5: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): keybind conflict detection + confirm-overwrite flow"
```

---

## Phase D — Behavior toggles

### Task 12: `BehaviorTogglesView` — (label, value) list

**Files:**
- Create: `crates/sid-widgets/src/settings/behavior_toggles.rs`

- [ ] **Step 1: Define the toggle list**

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToggleValue {
    Bool(bool),
    Choice { options: Vec<String>, selected: usize },
    U64 { value: u64, min: u64, max: u64 },
    String(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Toggle {
    pub key: &'static str,
    pub label: &'static str,
    pub value: ToggleValue,
}

pub struct BehaviorTogglesView {
    toggles: Vec<Toggle>,
    focused: usize,
    /// Set of keys modified since load — flushed on save.
    dirty: std::collections::BTreeSet<&'static str>,
}

impl BehaviorTogglesView {
    pub fn defaults() -> Self {
        use sid_store::settings_keys::*;
        Self {
            toggles: vec![
                Toggle { key: AUTO_RESTORE_SESSION, label: "Auto-restore session",
                    value: ToggleValue::Choice {
                        options: vec!["yes".into(), "ask".into(), "no".into()],
                        selected: 1,
                    }},
                Toggle { key: AUTO_SCAN_WORKSPACES, label: "Auto-scan workspace roots on startup",
                    value: ToggleValue::Bool(true) },
                Toggle { key: PERSIST_DEBOUNCE_MS, label: "State persist debounce (ms)",
                    value: ToggleValue::U64 { value: 250, min: 50, max: 5000 } },
                Toggle { key: HEARTBEAT_INTERVAL_SECS, label: "Session heartbeat interval (s)",
                    value: ToggleValue::U64 { value: 5, min: 1, max: 300 } },
                Toggle { key: DEFAULT_TAB, label: "Default tab on launch",
                    value: ToggleValue::Choice {
                        options: vec![
                            "workspaces".into(), "ssh".into(), "database".into(),
                            "network".into(), "system".into(), "settings".into(),
                        ],
                        selected: 0,
                    }},
            ],
            focused: 0,
            dirty: Default::default(),
        }
    }
    pub fn toggles(&self) -> &[Toggle] { &self.toggles }
    pub fn focused_index(&self) -> usize { self.focused }
    pub fn next(&mut self) { /* cycle */ }
    pub fn prev(&mut self) { /* cycle */ }
    pub fn cycle_focused_value(&mut self, dir: i32) { /* dir = +1 or -1 */ }
    pub fn dirty_keys(&self) -> impl Iterator<Item = &&'static str> { self.dirty.iter() }
    pub fn clear_dirty(&mut self) { self.dirty.clear(); }
}
```

`cycle_focused_value(+1)` mutates the focused toggle: bool flips, choice advances, u64 increments by a sensible step (10 for ms, 1 for secs), clamps to [min, max]. Records the key in `dirty`.

- [ ] **Step 2: Failing tests**

- Default constructor has 5 toggles.
- `cycle_focused_value(+1)` on Bool toggles the value and adds key to dirty.
- `cycle_focused_value(+1)` on Choice advances; wraps from last to first.
- `cycle_focused_value(+1)` on U64 increments by step, clamps at max.
- `cycle_focused_value(-1)` symmetric.
- `dirty_keys()` returns only modified keys.

- [ ] **Step 3: Adversarial**

- Calling `cycle_focused_value` 100,000 times on a Choice does not produce out-of-bound `selected`.
- U64 with `max == min` stays put under `cycle_focused_value(+1)` and `-1`.
- Property: focused index always in `[0, toggles.len())` after any number of `next`/`prev`.

- [ ] **Step 4: Doc tests** on every pub fn.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): BehaviorTogglesView with cycle-value semantics"
```

---

### Task 13: Persistence + reload hooks

**Files:**
- Modify: `crates/sid-widgets/src/settings/behavior_toggles.rs`

- [ ] **Step 1: Add `load_from_store` and `flush_dirty`**

```rust
impl BehaviorTogglesView {
    pub fn load_from_store(&mut self, store: &dyn sid_store::Store)
        -> Result<(), sid_core::SidError>
    {
        use sid_store::TypedSettings;
        for t in self.toggles.iter_mut() {
            match &mut t.value {
                ToggleValue::Bool(b) => {
                    if let Some(v) = store.get_bool(t.key)? { *b = v; }
                }
                ToggleValue::U64 { value, .. } => {
                    if let Some(v) = store.get_u64(t.key)? { *value = v; }
                }
                ToggleValue::Choice { options, selected } => {
                    if let Some(s) = store.get_string(t.key)? {
                        if let Some(idx) = options.iter().position(|o| o == &s) {
                            *selected = idx;
                        }
                    }
                }
                ToggleValue::String(s) => {
                    if let Some(v) = store.get_string(t.key)? { *s = v; }
                }
            }
        }
        Ok(())
    }
    pub fn flush_dirty(&mut self, store: &dyn sid_store::Store)
        -> Result<usize, sid_core::SidError>
    {
        use sid_store::TypedSettings;
        let dirty: Vec<&'static str> = self.dirty.iter().copied().collect();
        let mut wrote = 0;
        for key in dirty {
            let t = self.toggles.iter().find(|t| t.key == key).unwrap();
            match &t.value {
                ToggleValue::Bool(b) => store.put_bool(key, *b)?,
                ToggleValue::U64 { value, .. } => store.put_u64(key, *value)?,
                ToggleValue::Choice { options, selected } => store.put_string(key, &options[*selected])?,
                ToggleValue::String(s) => store.put_string(key, s)?,
            }
            wrote += 1;
        }
        self.dirty.clear();
        Ok(wrote)
    }
}
```

- [ ] **Step 2: Tests**

- Round-trip: defaults → modify → flush → reload → values match.
- A toggle's choice with an unknown stored value falls back to current selection without error.
- Flush is idempotent (second flush writes 0).

- [ ] **Step 3: Adversarial**

- Stored u64 above max is loaded but clamped on first cycle (document this behaviour with a test).
- Invalid bool in DB returns Err from `load_from_store` — recover by writing default and continuing.

- [ ] **Step 4: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): behavior toggles load/flush against Store"
```

---

## Phase E — Workspace roots

### Task 14: `WorkspaceRootsView` — list + add (with validation) + remove

**Files:**
- Create: `crates/sid-widgets/src/settings/workspace_roots.rs`

- [ ] **Step 1: Define the view**

```rust
use std::path::PathBuf;

pub struct WorkspaceRootsView {
    roots: Vec<PathBuf>,
    focused: usize,
    /// Input mode for adding a new root.
    input: Option<String>,
    /// Most recent validation error to display.
    last_error: Option<String>,
}

impl WorkspaceRootsView {
    pub fn new(roots: Vec<PathBuf>) -> Self {
        Self { roots, focused: 0, input: None, last_error: None }
    }
    pub fn roots(&self) -> &[PathBuf] { &self.roots }
    pub fn next(&mut self) { /* cycle */ }
    pub fn prev(&mut self) { /* cycle */ }
    pub fn begin_add(&mut self) { self.input = Some(String::new()); self.last_error = None; }
    pub fn type_char(&mut self, c: char) {
        if let Some(s) = self.input.as_mut() { s.push(c); }
    }
    pub fn backspace(&mut self) {
        if let Some(s) = self.input.as_mut() { s.pop(); }
    }
    pub fn cancel_add(&mut self) { self.input = None; self.last_error = None; }
    pub fn commit_add(&mut self) -> Result<&PathBuf, String> {
        let Some(raw) = self.input.take() else { return Err("not in add mode".into()); };
        let p = PathBuf::from(shellexpand::tilde(&raw).into_owned());
        if !p.exists() {
            let err = format!("path does not exist: {}", p.display());
            self.last_error = Some(err.clone());
            self.input = Some(raw);
            return Err(err);
        }
        if !p.is_dir() {
            let err = format!("not a directory: {}", p.display());
            self.last_error = Some(err.clone());
            self.input = Some(raw);
            return Err(err);
        }
        let abs = std::fs::canonicalize(&p).map_err(|e| e.to_string())?;
        if self.roots.iter().any(|r| r == &abs) {
            let err = format!("already registered: {}", abs.display());
            self.last_error = Some(err.clone());
            return Err(err);
        }
        self.roots.push(abs);
        Ok(self.roots.last().unwrap())
    }
    pub fn remove_focused(&mut self) -> Option<PathBuf> {
        if self.roots.is_empty() { return None; }
        let r = self.roots.remove(self.focused);
        if self.focused >= self.roots.len() && !self.roots.is_empty() {
            self.focused = self.roots.len() - 1;
        }
        Some(r)
    }
}
```

`shellexpand` is not currently in deps — add it as a `[dev-dependencies]` if only used in this task, OR vendor a tiny tilde-expander since the use is one-shot. The plan recommends **vendoring** since adding a workspace dep just for tilde expansion is overkill; vendor inline:

```rust
fn expand_tilde(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return format!("{}/{}", home.to_string_lossy(), rest);
        }
    }
    s.to_string()
}
```

- [ ] **Step 2: Tests**

- Add `~/vcs` (auto-expand to `$HOME/vcs`) when that dir exists in a tempdir-overridden HOME — succeeds.
- Add `/no/such/dir` — returns Err, input preserved.
- Add a path that exists but is a file — returns Err.
- Add a duplicate path — returns Err.
- `remove_focused` on empty roots returns None.
- After remove, `focused` clamps to last valid index.
- `cancel_add` clears input + last_error.

- [ ] **Step 3: Adversarial**

- Add a symlink that points to a real directory — should canonicalize and accept.
- Add a path with embedded NULs (`"/tmp/foo\0bar"`) — Err, not panic.
- Add a very long path (4 KiB) — handled without panic.

- [ ] **Step 4: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): WorkspaceRootsView with path validation + add/remove"
```

---

### Task 15: Workspace roots persistence (JSON-blob in settings)

**Files:**
- Modify: `crates/sid-widgets/src/settings/workspace_roots.rs`

- [ ] **Step 1: Serde the Vec<PathBuf> as a JSON array under `settings:workspace_roots`**

```rust
impl WorkspaceRootsView {
    pub fn save(&self, store: &dyn sid_store::Store) -> Result<(), sid_core::SidError> {
        use sid_store::settings_keys::WORKSPACE_ROOTS;
        let json = serde_json::to_string(&self.roots)
            .map_err(|e| sid_core::SidError::Storage(e.to_string()))?;
        store.put_setting(WORKSPACE_ROOTS,
            &sid_store::SettingValue(json.into_bytes()))
    }
    pub fn load(store: &dyn sid_store::Store) -> Result<Self, sid_core::SidError> {
        use sid_store::settings_keys::WORKSPACE_ROOTS;
        let roots = match store.get_setting(WORKSPACE_ROOTS)? {
            None => default_roots(),
            Some(v) => serde_json::from_slice(&v.0)
                .map_err(|e| sid_core::SidError::Storage(e.to_string()))?,
        };
        Ok(Self::new(roots))
    }
}

fn default_roots() -> Vec<PathBuf> {
    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home).join("vcs");
        if p.exists() { return vec![p]; }
    }
    vec![]
}
```

`serde_json` is in workspace deps (Plan 1 uses it for the command palette transcripts). If not, add it now — small commit body note.

- [ ] **Step 2: Tests**

- Round-trip: save then load → same vec.
- Missing setting yields `default_roots()` (which may be empty in test env).
- Malformed JSON in the setting returns Err on load.

- [ ] **Step 3: Adversarial**

- Save a vec with 1000 paths — round-trips without truncation.
- Setting contains valid JSON but wrong type (`"not-an-array"`) — Err on load.
- Setting contains JSON with non-UTF8 path representation — Err on load.

- [ ] **Step 4: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): persist workspace roots as JSON in settings table"
```

---

## Phase F — Quick actions editor

### Task 16: `QuickActionsView` — list + add/edit/remove

**Files:**
- Create: `crates/sid-widgets/src/settings/quick_actions.rs`

- [ ] **Step 1: Define the view**

```rust
use sid_store::{QuickAction, QuickActionScope};

pub struct QuickActionsView {
    actions: Vec<QuickAction>,
    focused: usize,
    /// Edit-in-place buffer for the focused action.
    edit_buffer: Option<EditBuffer>,
}

pub struct EditBuffer {
    pub id: String,
    pub label: String,
    pub cmd: String,
    pub keybind: Option<String>,
    pub scope: QuickActionScope,
    pub is_new: bool,
}

impl QuickActionsView {
    pub fn new(actions: Vec<QuickAction>) -> Self {
        Self { actions, focused: 0, edit_buffer: None }
    }
    pub fn actions(&self) -> &[QuickAction] { &self.actions }
    pub fn next(&mut self) { /* cycle */ }
    pub fn prev(&mut self) { /* cycle */ }
    pub fn begin_add(&mut self) {
        self.edit_buffer = Some(EditBuffer {
            id: String::new(), label: String::new(), cmd: String::new(),
            keybind: None, scope: QuickActionScope::Global, is_new: true,
        });
    }
    pub fn begin_edit_focused(&mut self) {
        if let Some(a) = self.actions.get(self.focused) {
            self.edit_buffer = Some(EditBuffer {
                id: a.id.clone(), label: a.label.clone(), cmd: a.cmd.clone(),
                keybind: a.keybind.clone(), scope: a.scope, is_new: false,
            });
        }
    }
    pub fn cancel_edit(&mut self) { self.edit_buffer = None; }
    pub fn commit_edit(&mut self) -> Result<&QuickAction, String> {
        let Some(buf) = self.edit_buffer.take() else { return Err("not editing".into()); };
        if buf.id.is_empty() { return Err("id required".into()); }
        if buf.cmd.is_empty() { return Err("cmd required".into()); }
        let action = QuickAction {
            id: buf.id, label: buf.label, cmd: buf.cmd,
            keybind: buf.keybind, scope: buf.scope,
        };
        // Insert-or-replace by id.
        if let Some(idx) = self.actions.iter().position(|a| a.id == action.id) {
            self.actions[idx] = action;
            Ok(&self.actions[idx])
        } else {
            self.actions.push(action);
            Ok(self.actions.last().unwrap())
        }
    }
    pub fn remove_focused(&mut self) -> Option<QuickAction> {
        if self.actions.is_empty() { return None; }
        Some(self.actions.remove(self.focused))
    }
}
```

- [ ] **Step 2: Failing tests**

- Add a new action with all required fields → success, appears in list.
- Add with empty id → Err.
- Add with empty cmd → Err.
- Edit existing action's label → replaces in place.
- Remove focused → list shrinks.
- Cancel edit → buffer discarded, list unchanged.

- [ ] **Step 3: Adversarial**

- Add two actions with the same id — second replaces first (idempotent).
- Unicode in label, cmd, keybind.
- Very long cmd (16 KiB) — round-trips.

- [ ] **Step 4: Doc tests** on every pub fn.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): QuickActionsView with add/edit/remove"
```

---

### Task 17: Quick action persistence + keybind validation

**Files:**
- Modify: `crates/sid-widgets/src/settings/quick_actions.rs`

- [ ] **Step 1: Save/load against `quick_actions` table**

```rust
impl QuickActionsView {
    pub fn load(store: &dyn sid_store::Store) -> Result<Self, sid_core::SidError> {
        Ok(Self::new(store.list_quick_actions()?))
    }
    pub fn save_all(&self, store: &dyn sid_store::Store) -> Result<(), sid_core::SidError> {
        // For v1, replace-all semantics: list current, delete any not in self.actions,
        // then upsert all in self.actions.
        let existing = store.list_quick_actions()?;
        for old in existing {
            if !self.actions.iter().any(|a| a.id == old.id) {
                store.remove_quick_action(&old.id)?;
            }
        }
        for a in &self.actions {
            store.upsert_quick_action(a)?;
        }
        Ok(())
    }
}
```

- [ ] **Step 2: Validate keybind strings**

```rust
impl EditBuffer {
    /// Validate a keybind string using the same parser as `keybind_profile::chord_from_string`.
    pub fn validate_keybind(s: &str) -> Result<(), String> {
        sid_core::keybind_profile::chord_from_string(s).map(|_| ())
    }
}
```

In `commit_edit`, validate `buf.keybind` if `Some`; reject with a useful error if invalid.

- [ ] **Step 3: Tests**

- Save then load → same list.
- Save replaces an existing id without leaving duplicates.
- `validate_keybind("Char('q')|2")` → Ok.
- `validate_keybind("garbage")` → Err.
- `commit_edit` rejects a buffer with malformed keybind.

- [ ] **Step 4: Adversarial**

- Save with 1000 actions — round-trips.
- Save then concurrent reader (`list_quick_actions` from a separate `RedbStore::open` on the same path) sees the saved list.
- A quick action whose `keybind` is the bytes of a control character (`"\x00"`) — Err on validation.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): persist quick actions + validate keybinds"
```

---

## Phase G — DB path override

### Task 18: `DbPathView` — display current + edit writes sid.toml

**Files:**
- Create: `crates/sid-widgets/src/settings/db_path.rs`

- [ ] **Step 1: View**

```rust
use std::path::{Path, PathBuf};
use sid_store::sid_toml::{read_sid_toml, write_sid_toml, SidToml};

pub struct DbPathView {
    /// The path sid is currently using.
    active_path: PathBuf,
    /// The contents of sid.toml (may differ from active if user edited but hasn't restarted).
    sid_toml_path: PathBuf,
    cfg: SidToml,
    /// Edit buffer.
    input: Option<String>,
    /// Last error to display.
    last_error: Option<String>,
}

impl DbPathView {
    pub fn open(active_path: PathBuf, sid_toml_path: PathBuf)
        -> Result<Self, sid_store::sid_toml::SidTomlError>
    {
        let cfg = read_sid_toml(&sid_toml_path)?;
        Ok(Self { active_path, sid_toml_path, cfg, input: None, last_error: None })
    }
    pub fn active_path(&self) -> &Path { &self.active_path }
    pub fn override_path(&self) -> Option<&Path> { self.cfg.db_path_override.as_deref() }
    pub fn begin_edit(&mut self) {
        let initial = self.cfg.db_path_override.as_ref()
            .map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        self.input = Some(initial);
        self.last_error = None;
    }
    pub fn cancel_edit(&mut self) { self.input = None; }
    pub fn commit_edit(&mut self) -> Result<RestartNotice, String> {
        let Some(raw) = self.input.take() else { return Err("not editing".into()); };
        let new = if raw.is_empty() { None } else { Some(PathBuf::from(raw)) };
        self.cfg.db_path_override = new;
        write_sid_toml(&self.sid_toml_path, &self.cfg).map_err(|e| {
            let s = e.to_string();
            self.last_error = Some(s.clone());
            s
        })?;
        Ok(RestartNotice { sid_toml_path: self.sid_toml_path.clone() })
    }
}

#[derive(Debug)]
pub struct RestartNotice { pub sid_toml_path: PathBuf }
```

- [ ] **Step 2: Tests**

- `open` with a missing sid.toml constructs with `override_path == None`.
- `begin_edit` populates input from current override.
- `commit_edit("")` clears override, writes empty TOML, returns RestartNotice.
- `commit_edit("/custom")` sets override, writes TOML, returns RestartNotice.
- `cancel_edit` discards input without writing.

- [ ] **Step 3: Adversarial**

- `commit_edit` with a path in a read-only directory returns Err and stashes message in `last_error`.
- Whitespace-only input is treated as empty (i.e. clears the override) — explicit policy with a test.
- A path containing a tilde (`~/data/sid.redb`) is *not* expanded — sid.toml stores the raw string and the binary expands it on the next launch. (Document this in the doc test.)

- [ ] **Step 4: Doc tests** on every pub fn.

- [ ] **Step 5: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): DbPathView reads/writes sid.toml with restart notice"
```

---

## Phase H — Reset to defaults

### Task 19: `ResetView` — confirm modal + factory-list reset

**Files:**
- Create: `crates/sid-widgets/src/settings/reset.rs`

- [ ] **Step 1: Define the factory key list**

```rust
use sid_store::{settings_keys, Store};

/// Keys cleared by "Reset to defaults". On next read, code paths fall back to
/// their compiled-in defaults.
pub const FACTORY_KEYS: &[&str] = &[
    settings_keys::THEME_NAME,
    settings_keys::KEYBIND_PROFILE_NAME,
    settings_keys::WORKSPACE_ROOTS,
    settings_keys::PERSIST_DEBOUNCE_MS,
    settings_keys::HEARTBEAT_INTERVAL_SECS,
    settings_keys::AUTO_RESTORE_SESSION,
    settings_keys::AUTO_SCAN_WORKSPACES,
    settings_keys::DEFAULT_TAB,
];

pub struct ResetView { confirm_open: bool }

impl ResetView {
    pub fn new() -> Self { Self { confirm_open: false } }
    pub fn is_confirming(&self) -> bool { self.confirm_open }
    pub fn open_confirm(&mut self) { self.confirm_open = true; }
    pub fn cancel(&mut self) { self.confirm_open = false; }
    pub fn confirm(&mut self, store: &dyn Store) -> Result<usize, sid_core::SidError> {
        if !self.confirm_open { return Ok(0); }
        self.confirm_open = false;
        let mut cleared = 0;
        for k in FACTORY_KEYS {
            // Best-effort delete; redb's `remove` is no-op on missing keys.
            // Need a `delete_setting` on Store — adds in Step 2.
            if store.delete_setting(k)? { cleared += 1; }
        }
        Ok(cleared)
    }
}

impl Default for ResetView { fn default() -> Self { Self::new() } }
```

- [ ] **Step 2: Add `Store::delete_setting`**

```rust
fn delete_setting(&self, key: &str) -> Result<bool, SidError>;
```

Implement on `RedbStore` and on the MemStore in the doc test of `lib.rs`. Returns `Ok(true)` if a value was removed, `Ok(false)` if the key did not exist.

- [ ] **Step 3: Tests**

- Reset clears each factory key; subsequent `get_string(key)` returns None.
- Reset is idempotent — second call returns `cleared == 0`.
- Reset does **not** clear non-factory keys (e.g. a custom user key written by the user).
- Reset does **not** delete records in `themes`, `keybinds`, `quick_actions`, or `workspaces` — only setting keys. Explicit test asserting these tables are untouched.
- `cancel()` clears `confirm_open`.

- [ ] **Step 4: Adversarial**

- Calling `confirm` when not in confirming state is a no-op returning `Ok(0)`.
- Reset on a store with a transaction in flight (concurrent reader on the same path) does not deadlock (Plan 1 already guarantees this; assert in test).

- [ ] **Step 5: Doc tests** on every pub fn.

- [ ] **Step 6: Commit**

```bash
git add crates/sid-store crates/sid-widgets
git commit -m "feat(widgets,store): reset-to-defaults flow + Store::delete_setting"
```

---

## Phase I — `SettingsWidget` composer

### Task 20: Compose sub-views with left/right pane

**Files:**
- Modify: `crates/sid-widgets/src/settings.rs` (full rewrite)
- Modify: `crates/sid-widgets/src/settings/mod.rs`

- [ ] **Step 1: Define the category enum**

```rust
pub enum SettingsCategory {
    Theme(ThemePickerView),
    Keybinds(KeybindEditorView),
    Behavior(BehaviorTogglesView),
    WorkspaceRoots(WorkspaceRootsView),
    QuickActions(QuickActionsView),
    DbPath(DbPathView),
    Reset(ResetView),
}

impl SettingsCategory {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Theme(_) => "Theme",
            Self::Keybinds(_) => "Keybinds",
            Self::Behavior(_) => "Behavior",
            Self::WorkspaceRoots(_) => "Workspace roots",
            Self::QuickActions(_) => "Quick actions",
            Self::DbPath(_) => "DB path",
            Self::Reset(_) => "Reset to defaults",
        }
    }
}
```

- [ ] **Step 2: Widget skeleton**

```rust
pub struct SettingsWidget {
    id: WidgetId,
    categories: Vec<SettingsCategory>,
    focused_category: usize,
    store: Arc<dyn Store>,
    theme_registry: Arc<ThemeRegistry>,
    action_registry: Arc<ActionRegistry>,
}

impl SettingsWidget {
    pub fn new(
        store: Arc<dyn Store>,
        theme_registry: Arc<ThemeRegistry>,
        action_registry: Arc<ActionRegistry>,
        keybind_map: KeybindMap,
        applied_theme: &str,
        sid_toml_path: PathBuf,
        active_db_path: PathBuf,
    ) -> Result<Self, SidError> {
        let theme = ThemePickerView::new(&theme_registry, applied_theme);
        let keybinds = KeybindEditorView::new(&action_registry, keybind_map);
        let mut behavior = BehaviorTogglesView::defaults();
        behavior.load_from_store(&*store)?;
        let roots = WorkspaceRootsView::load(&*store)?;
        let quick = QuickActionsView::load(&*store)?;
        let db_path = DbPathView::open(active_db_path, sid_toml_path)
            .map_err(|e| SidError::Storage(e.to_string()))?;
        let reset = ResetView::new();
        Ok(Self {
            id: WidgetId::new("settings.root"),
            categories: vec![
                SettingsCategory::Theme(theme),
                SettingsCategory::Keybinds(keybinds),
                SettingsCategory::Behavior(behavior),
                SettingsCategory::WorkspaceRoots(roots),
                SettingsCategory::QuickActions(quick),
                SettingsCategory::DbPath(db_path),
                SettingsCategory::Reset(reset),
            ],
            focused_category: 0,
            store, theme_registry, action_registry,
        })
    }
    pub fn category_labels(&self) -> Vec<&'static str> {
        self.categories.iter().map(|c| c.label()).collect()
    }
    pub fn focused_category_index(&self) -> usize { self.focused_category }
}
```

- [ ] **Step 3: Implement `Widget` trait**

`render(target)` paints a 2-pane layout: left column lists `category_labels()` with the focused row highlighted via `accent_primary`; right column dispatches to the focused category's render. `handle_event(ev, ctx)` checks for category-switching chords (Tab/Shift+Tab); otherwise routes to the focused category's `handle_event`.

- [ ] **Step 4: Failing tests**

- Construct with default registries and a fresh tempdir store; assert 7 categories present.
- Tab cycles focused_category forward; Shift+Tab cycles backward.
- A category's individual events (e.g. arrow keys) reach the category, not the composer.
- `save_state` / `load_state` round-trip preserves the focused category index.

- [ ] **Step 5: Insta snapshot of full render at default state**

```rust
#[test]
fn snapshot_default_settings_render() {
    let w = SettingsWidget::new(…);
    let s = render_to_string(&w);
    insta::assert_snapshot!(s);
}
```

- [ ] **Step 6: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): compose SettingsWidget with left/right pane"
```

---

### Task 21: Tab/Shift+Tab cycling + persist focused category

**Files:**
- Modify: `crates/sid-widgets/src/settings.rs`

- [ ] **Step 1: Persist focused category to `widget_state`**

```rust
impl Widget for SettingsWidget {
    fn save_state(&self) -> Vec<u8> {
        postcard::to_allocvec(&SettingsState {
            focused_category: self.focused_category as u8,
        }).unwrap_or_default()
    }
    fn load_state(&mut self, bytes: &[u8]) {
        if let Ok(s) = postcard::from_bytes::<SettingsState>(bytes) {
            if (s.focused_category as usize) < self.categories.len() {
                self.focused_category = s.focused_category as usize;
            }
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SettingsState { focused_category: u8 }
```

- [ ] **Step 2: Tests**

- After cycling to "Keybinds", save_state then load_state preserves selection.
- Garbage bytes passed to load_state are silently ignored (focused stays at 0).
- An out-of-range focused index in the blob is clamped to 0.

- [ ] **Step 3: Proptest**

```rust
proptest! {
    #[test]
    fn prop_save_load_state_round_trip(idx in 0u8..7) {
        let mut w = …;
        w.focused_category = idx as usize;
        let bytes = w.save_state();
        w.focused_category = 0;
        w.load_state(&bytes);
        prop_assert_eq!(w.focused_category, idx as usize);
    }
}
```

- [ ] **Step 4: Commit**

```bash
git add crates/sid-widgets
git commit -m "feat(widgets): persist SettingsWidget focused category across launches"
```

---

## Phase J — CLI

### Task 22: `sid settings get <key>`

**Files:**
- Modify: `crates/sid/src/main.rs`

- [ ] **Step 1: Extend the `Cmd` enum**

```rust
#[derive(clap::Subcommand, Debug)]
enum Cmd {
    Workspace { #[command(subcommand)] op: WorkspaceOp },
    Settings { #[command(subcommand)] op: SettingsOp },
}

#[derive(clap::Subcommand, Debug)]
enum SettingsOp {
    /// Print a setting value by key.
    Get { key: String },
    /// Set a setting value.
    Set { key: String, value: String },
    /// List all known setting keys with their values.
    List,
}
```

- [ ] **Step 2: Dispatch in `main`**

```rust
if let Some(Cmd::Settings { op }) = cli.cmd {
    match op {
        SettingsOp::Get { key } => {
            match store.get_setting(&key)? {
                Some(v) => {
                    // Print as UTF-8 if valid, else as bytes-as-hex.
                    match std::str::from_utf8(&v.0) {
                        Ok(s) => println!("{s}"),
                        Err(_) => println!("0x{}", hex_encode(&v.0)),
                    }
                }
                None => {
                    eprintln!("setting '{}' not set", key);
                    std::process::exit(1);
                }
            }
        }
        SettingsOp::Set { key, value } => { /* Task 23 */ }
        SettingsOp::List => {
            for k in sid_store::settings_keys::ALL { // helper constant added in this task
                let val = store.get_string(k)?.unwrap_or_else(|| "<unset>".into());
                println!("{:<32} {}", k, val);
            }
        }
    }
    return Ok(());
}
```

Add `pub const ALL: &[&str] = &[…];` to the `settings_keys` module in this task.

- [ ] **Step 3: Tests**

Create `crates/sid/tests/settings_cli.rs`:

```rust
use std::process::Command;
use tempfile::tempdir;

#[test]
fn settings_get_unset_key_exits_nonzero() {
    let d = tempdir().unwrap();
    let db = d.path().join("s.redb");
    let bin = env!("CARGO_BIN_EXE_sid");
    let out = Command::new(bin)
        .args(["--db", db.to_str().unwrap(), "settings", "get", "theme_name"])
        .output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not set"));
}

#[test]
fn settings_list_emits_all_keys() {
    let d = tempdir().unwrap();
    let db = d.path().join("s.redb");
    let bin = env!("CARGO_BIN_EXE_sid");
    let out = Command::new(bin)
        .args(["--db", db.to_str().unwrap(), "settings", "list"])
        .output().unwrap();
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("theme_name"));
    assert!(s.contains("workspace_roots"));
    assert!(s.contains("default_tab"));
}
```

- [ ] **Step 4: Commit**

```bash
git add crates/sid crates/sid-store
git commit -m "feat(bin): add `sid settings get` and `sid settings list`"
```

---

### Task 23: `sid settings set <key> <value>`

**Files:**
- Modify: `crates/sid/src/main.rs`

- [ ] **Step 1: Dispatch + validation**

```rust
SettingsOp::Set { key, value } => {
    // Validate against the type registry of known keys.
    validate_setting_value(&key, &value)?;
    store.put_string(&key, &value)?;
    println!("set {} = {}", key, value);
}
```

Define `validate_setting_value`:

```rust
fn validate_setting_value(key: &str, value: &str) -> anyhow::Result<()> {
    use sid_store::settings_keys::*;
    match key {
        THEME_NAME => Ok(()), // any string; theme registry resolves at runtime
        KEYBIND_PROFILE_NAME => Ok(()),
        DEFAULT_TAB => {
            const VALID: &[&str] = &["workspaces","ssh","database","network","system","settings"];
            if VALID.contains(&value) { Ok(()) } else {
                anyhow::bail!("default_tab must be one of {VALID:?}")
            }
        }
        AUTO_RESTORE_SESSION => {
            const VALID: &[&str] = &["yes","ask","no"];
            if VALID.contains(&value) { Ok(()) } else {
                anyhow::bail!("auto_restore_session must be one of {VALID:?}")
            }
        }
        AUTO_SCAN_WORKSPACES => match value {
            "true" | "false" => Ok(()),
            _ => anyhow::bail!("auto_scan_workspaces must be true or false"),
        },
        PERSIST_DEBOUNCE_MS | HEARTBEAT_INTERVAL_SECS => {
            value.parse::<u64>().map(|_| ()).map_err(|e| anyhow::anyhow!("{e}"))
        }
        WORKSPACE_ROOTS => {
            // Must be a valid JSON array of strings.
            let _: Vec<String> = serde_json::from_str(value)?;
            Ok(())
        }
        _ => {
            // Unknown key — allow (forward-compat), but warn.
            eprintln!("warning: setting '{}' is not in the known-key registry", key);
            Ok(())
        }
    }
}
```

- [ ] **Step 2: Tests**

```rust
#[test]
fn settings_set_then_get_round_trip() {
    let d = tempdir().unwrap();
    let db = d.path().join("s.redb");
    let bin = env!("CARGO_BIN_EXE_sid");
    let set = Command::new(bin).args(["--db", db.to_str().unwrap(),
        "settings", "set", "theme_name", "void"]).output().unwrap();
    assert!(set.status.success());
    let get = Command::new(bin).args(["--db", db.to_str().unwrap(),
        "settings", "get", "theme_name"]).output().unwrap();
    assert!(get.status.success());
    assert_eq!(String::from_utf8_lossy(&get.stdout).trim(), "void");
}

#[test]
fn settings_set_invalid_default_tab_fails() {
    let d = tempdir().unwrap();
    let db = d.path().join("s.redb");
    let bin = env!("CARGO_BIN_EXE_sid");
    let out = Command::new(bin).args(["--db", db.to_str().unwrap(),
        "settings", "set", "default_tab", "nonsense"]).output().unwrap();
    assert!(!out.status.success());
}

#[test]
fn settings_set_invalid_bool_fails() {
    let d = tempdir().unwrap();
    let db = d.path().join("s.redb");
    let bin = env!("CARGO_BIN_EXE_sid");
    let out = Command::new(bin).args(["--db", db.to_str().unwrap(),
        "settings", "set", "auto_scan_workspaces", "maybe"]).output().unwrap();
    assert!(!out.status.success());
}

#[test]
fn settings_set_invalid_u64_fails() {
    let d = tempdir().unwrap();
    let db = d.path().join("s.redb");
    let bin = env!("CARGO_BIN_EXE_sid");
    let out = Command::new(bin).args(["--db", db.to_str().unwrap(),
        "settings", "set", "persist_debounce_ms", "not-a-number"]).output().unwrap();
    assert!(!out.status.success());
}
```

- [ ] **Step 3: Adversarial**

- `sid settings set workspace_roots '["/tmp"]'` — succeeds (valid JSON array).
- `sid settings set workspace_roots '"not-an-array"'` — fails.
- `sid settings set unknown_key foo` — succeeds with a stderr warning.
- Setting an empty value (`sid settings set theme_name ""`) — succeeds; `get` returns empty string.

- [ ] **Step 4: Commit**

```bash
git add crates/sid
git commit -m "feat(bin): add `sid settings set` with per-key validation"
```

---

## Phase K — Integration + docs

### Task 24: Wire theme registry + keybind profile load on startup

**Files:**
- Modify: `crates/sid/src/wire.rs`

The binary's startup path currently loads `KeybindMap::cosmos_default()`. After Plan 7, it loads from the DB:

1. Read `settings: keybind_profile_name` (default `"cosmos"`).
2. Call `sid_store::load_keybind_profile(&store, &name)?`.
3. If `None`, fall back to `KeybindMap::cosmos_default()` and save it under that name (first-run seeding).

Similarly for theme:

1. Read `settings: theme_name` (default `"cosmos"`).
2. Resolve from `ThemeRegistry::with_builtins()` + user themes via `store.list_themes()`.
3. If theme is missing, fall back to `cosmos()` and log a warning.

- [ ] **Step 1: Add `wire::load_active_theme(&store) -> (Theme, ThemeRegistry)`**

```rust
pub fn load_active_theme(store: &dyn Store) -> (Theme, ThemeRegistry) {
    use sid_store::TypedSettings;
    let mut registry = ThemeRegistry::with_builtins();
    // Merge in user themes from the store.
    if let Ok(user_themes) = store.list_themes() {
        for spec in user_themes {
            registry.register(theme_spec_to_theme(spec));
        }
    }
    let name = store.get_string(settings_keys::THEME_NAME).ok().flatten()
        .unwrap_or_else(|| "cosmos".to_string());
    let theme = registry.get(&name).cloned().unwrap_or_else(|| {
        tracing::warn!(theme = %name, "theme not found, falling back to cosmos");
        cosmos()
    });
    (theme, registry)
}
```

- [ ] **Step 2: Add `wire::load_active_keybinds(&store) -> KeybindMap`**

```rust
pub fn load_active_keybinds(store: &dyn Store) -> KeybindMap {
    use sid_store::TypedSettings;
    let name = store.get_string(settings_keys::KEYBIND_PROFILE_NAME).ok().flatten()
        .unwrap_or_else(|| "cosmos".to_string());
    match sid_store::load_keybind_profile(store, &name) {
        Ok(Some(map)) => map,
        _ => {
            let m = KeybindMap::cosmos_default();
            let _ = sid_store::save_keybind_profile(store, "cosmos", &m);
            m
        }
    }
}
```

- [ ] **Step 3: Tests**

- First-run: empty store → theme = cosmos, keybinds = cosmos default; cosmos keybind profile is now persisted.
- Set theme_name to "dusk" via CLI, then load → theme is dusk.
- Set keybind_profile_name to "missing" → falls back without panicking.
- A user-registered theme appears in the registry.

- [ ] **Step 4: Commit**

```bash
git add crates/sid
git commit -m "feat(bin): load theme + keybind profile from store at startup"
```

---

### Task 25: Integration test — full settings round-trip across processes

**Files:**
- Create: `crates/sid/tests/settings_round_trip.rs`

End-to-end: launch `sid settings set` in one subprocess, then launch a second `sid settings get` subprocess (same DB), assert the value persists. Also assert the binary respects `sid.toml`:

```rust
use std::fs;
use std::process::Command;
use tempfile::tempdir;

#[test]
fn settings_persist_across_processes() {
    let d = tempdir().unwrap();
    let db = d.path().join("s.redb");
    let bin = env!("CARGO_BIN_EXE_sid");

    // Process 1: set theme_name to "void"
    let s = Command::new(bin).args(["--db", db.to_str().unwrap(),
        "settings", "set", "theme_name", "void"]).output().unwrap();
    assert!(s.status.success());

    // Process 2: read it back
    let g = Command::new(bin).args(["--db", db.to_str().unwrap(),
        "settings", "get", "theme_name"]).output().unwrap();
    assert!(g.status.success());
    assert_eq!(String::from_utf8_lossy(&g.stdout).trim(), "void");
}

#[test]
fn sid_toml_overrides_db_path() {
    let d = tempdir().unwrap();
    let custom_db = d.path().join("custom.redb");
    let toml_dir = d.path().join("config");
    fs::create_dir(&toml_dir).unwrap();
    let toml_path = toml_dir.join("sid.toml");
    fs::write(&toml_path,
        format!("db_path_override = \"{}\"\n", custom_db.display())).unwrap();

    let bin = env!("CARGO_BIN_EXE_sid");
    // Set XDG_CONFIG_HOME so sid reads our toml.
    let out = Command::new(bin)
        .env("XDG_CONFIG_HOME", &toml_dir.parent().unwrap())
        .args(["settings", "set", "theme_name", "dusk"])
        .output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    // The custom db file now exists.
    assert!(custom_db.exists());
}
```

- [ ] **Step 1: Implement, run, fix wire.rs to honour XDG_CONFIG_HOME for sid.toml lookup**

This requires `wire::db_path(cli.db, env)` to:

1. If `cli.db` is `Some`, use it.
2. Else, read `$XDG_CONFIG_HOME/sid/sid.toml` (or fall back to `$HOME/.config/sid/sid.toml`).
3. If `db_path_override` is present, use it.
4. Else, use the XDG_DATA_HOME default.

This is the canonical resolution order from the spec. Add unit tests of `db_path` covering each branch.

- [ ] **Step 2: Adversarial integration tests**

- `sid settings set` while another process holds a read transaction on the same DB does not deadlock or corrupt.
- `sid settings set theme_name <very-long-string>` (8 KiB) → succeeds, get retrieves intact.
- Concurrent `sid settings set foo bar` and `sid settings set foo baz` from two processes — last writer wins (redb single-writer guarantee).

- [ ] **Step 3: Commit**

```bash
git add crates/sid
git commit -m "test(bin): integration tests for settings round-trip + sid.toml override"
```

---

### Task 26: README + Settings.md docs

**Files:**
- Modify: `README.md`
- Create: `docs/Settings.md`

- [ ] **Step 1: README update**

Update the "What's inside (v1)" Settings row:

```markdown
| **Settings** | Theme picker (live preview), keybind editor (capture mode + conflict detection), behavior toggles, workspace roots, quick actions, DB path — all in-app |
```

Add a `Settings` block to the Quickstart section:

```markdown
# Settings (in-app)
Ctrl+, opens the Settings tab. Tab/Shift+Tab cycles categories.

# Settings (scripted)
sid settings list
sid settings get theme_name
sid settings set theme_name void
sid settings set default_tab workspaces
sid settings set workspace_roots '["~/vcs","~/work"]'
```

Update the "What works in this build" callout:

> Foundation + Workspaces + Settings tabs fully functional. Theme picker with live preview, keybind editor with capture-mode + conflict detection, behavior toggles, workspace roots editor, quick actions editor, DB path override (writes the one-line sid.toml), reset-to-defaults flow; `sid settings get/set/list` CLI for scripted access.

- [ ] **Step 2: `docs/Settings.md`**

Long-form reference: every setting key with its type, range, default, and effect; the Settings tab keybinds (Tab/Shift+Tab cycle, Enter activate, Esc cancel, j/k navigate within a category, r for reset); the CLI surface; the sid.toml format; and the table of factory-reset keys.

- [ ] **Step 3: Doc test the README block where possible**

If the README contains a fenced rust block (it doesn't currently), ensure it compiles. The shell-command block is uninspected.

- [ ] **Step 4: Commit**

```bash
git add README.md docs/Settings.md
git commit -m "docs: update README + add Settings.md reference"
```

---

## Done criteria for Plan 7

- [ ] `cargo build --workspace` succeeds with no errors or warnings
- [ ] `cargo test --all-features --workspace` passes
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` is clean
- [ ] `cargo fmt --check` is clean
- [ ] `cargo run -p sid` launches; pressing `Ctrl+,` focuses the Settings tab; Tab cycles between the 7 categories
- [ ] In the Theme picker: arrow keys preview each theme on the right; Enter applies; quit and relaunch — theme persists
- [ ] In the Keybind editor: Enter on `app.quit` starts capture; pressing a chord already bound prompts overwrite; Yes applies, No returns to capture; quit and relaunch — keybinds persist
- [ ] In the Behavior toggles: arrow keys cycle each toggle's value; quit and relaunch — values persist
- [ ] In the Workspace roots editor: `a` opens add-mode; typing a nonexistent path shows an inline error; typing a real one accepts; `d` removes; quit and relaunch — roots persist
- [ ] In the Quick actions editor: `a` opens add-mode with fields for id/label/cmd/keybind/scope; `e` edits focused; `d` removes; quit and relaunch — actions persist
- [ ] In the DB path view: the active path is shown; editing writes `~/.config/sid/sid.toml`; a restart notice is rendered
- [ ] In the Reset view: pressing Enter opens a confirm modal; Yes clears factory keys; non-factory tables are untouched
- [ ] `sid settings list/get/set` work as documented; validation rejects malformed values
- [ ] `sid.toml` with a `db_path_override` redirects the DB path on the next launch
- [ ] No regressions in Plans 1 and 2 (tabs, palette, workspaces, session restore)

---

## Self-review notes (run before requesting human review)

**1. Spec coverage.** Plan 7 covers the foundation spec's Settings tab in full: theme picker (with live preview, per the "re-render-on-hover" open-item resolution), keybind editor (with conflict detection), behavior toggles (auto-restore-session, auto-scan-workspaces, debounce intervals, default-tab), workspace roots editor, quick actions editor (shared with Plan 6's System tab via the `quick_actions` table), DB path override via the one-line `sid.toml`, and reset-to-defaults. Plus a scripting CLI surface (`sid settings get/set/list`).

**2. Items deferred to later plans (confirmed by future-features doc):**
   - In-app **theme editor** (per-color palette painting) — "Someday"
   - **Themable star animations** — "Someday"
   - **Vim-style modal keybind profile** — "Someday"
   - **Status bar customization** — "Someday"
   - **Notification center** — "Someday"
   - **Keyring secrets** (Plan 5 — `SecretStore` trait swap)
   - **User-configurable storage backend** (v2)

**3. Type consistency check.**
   - `SettingsWidget` lives in `sid-widgets`. It names traits from `sid-core` (`Widget`, `Store`, `KeybindMap`, `ActionRegistry`) and types from `sid-store` (`SidToml`, `QuickAction`, …). It does **not** name `redb`, `tokio`, or `ratatui` types in its public signature except where `sid-ui::Theme` (already canonical) appears.
   - `ThemeRegistry` lives in `sid-ui` because it owns `Theme`. The widget references it through `Arc<ThemeRegistry>` to keep the registry shareable.
   - Quick actions are persisted via the shared `quick_actions` redb table introduced here (Plan 6 will read but not own it).
   - `sid_toml.rs` lives in `sid-store` because (a) it shares serde + io paths with the rest of `sid-store`, (b) putting it in `sid-core` would force a `toml` dep on `sid-core` (which the spec forbids — `sid-core` stays runtime-free), and (c) the binary already depends on `sid-store`.

**4. Adapter pattern.** No new external crates. `toml`, `serde_json`, `serde`, and `postcard` are all already in workspace deps. Widget code names only traits + domain types — no `redb`, no `crossterm` color, no `tokio` types in widget signatures.

**5. Judgment call: storing typed settings as UTF-8 strings.** The plan stores values as UTF-8 (`b"250"`, `b"true"`, `b"cosmos"`) under the existing raw-bytes `settings` table, instead of versioned-postcard blobs. Reason: the values are short, human-inspectable, and editable from `sid settings set` without invoking postcard. Trade-off: each typed accessor performs UTF-8 parsing on read, which is a sub-microsecond cost. This is **inconsistent with the spec sentence** that says "settings: versioned-postcard `SettingValue`" — flagging here for human review. Alternative: use postcard everywhere and provide a `sid settings get --raw` mode. The plan takes the human-readable path; reverse if needed.

**6. Judgment call: KeybindMap iteration.** Plan 1 did not expose `KeybindMap::iter()`. Task 8 adds it. The implementation requires reshaping `KeybindMap` from `BTreeMap<ChordKey, ActionId>` to `BTreeMap<ChordKey, (KeyChord, ActionId)>` — a small backward-compat refactor. Plan 1's tests should still pass. **Flagged.**

**7. Judgment call: `app.quit` chord unbind.** The prompt requested "unbinding the quit key — should warn but allow". Task 11 implements this as a `dangerous_action_warnings()` API surfacing a toast-renderable warning while still applying the rebind. This errs on the side of "the user knows what they're doing"; an alternative is to refuse the unbind. **Flagged.**

**8. Judgment call: keybind chord string format.** Task 8 round-trips `KeyChord` through `format!("{:?}|{bits}", c.code)`. This is fragile against changes in `KeyCode`'s `Debug` impl. A more durable format would use an explicit enum-tag table. The plan accepts the `Debug`-based approach as v1 expedient since `crossterm`'s `KeyCode` is widely used and unlikely to change; a v2 hardening could introduce a stable serializer with a migration of stored profiles. **Flagged.**

**9. Placeholder scan.** No "TBD" or "fill in later" in task bodies. Three callouts:
   - Task 4's user-theme `Theme` <-> `ThemeSpec` conversion is sketched but the helper `theme_spec_to_theme` is implied — implementer writes it inline (trivial 9-field mapping).
   - Task 6's `render_preview` ASCII serializer is described but not spelled out byte-for-byte — implementer picks the exact buffer-to-string algorithm matching `sid-widgets/src/stub.rs`'s helper.
   - Task 25's `XDG_CONFIG_HOME` override path of `wire::db_path` requires modifying Plan 1's `wire.rs`; details left to the implementer.

**10. Scope check.** 26 tasks across 11 phases. Comparable to Plan 2's 33 tasks. Each phase produces working/testable software; the plan can stop at the end of any phase and the project is in a consistent state. Phases A, B, C, D, E, F, G, H, I, J, K map to the prompt's phase shape (A→K).

**11. CLAUDE.md compliance.** Every task includes doc tests on every new pub fn, property tests where invariants exist (settings round-trip, chord round-trip, capture state machine totality, theme registry sort), adversarial coverage (malformed TOML, garbage in the settings table, very long inputs, unicode, concurrent processes, quit-chord unbind, invalid chord strings, missing themes), and insta snapshots for live-preview rendering. Phase A and Phase C carry property tests; the live-preview gets a criterion bench (Task 6). The keybind capture state machine has full transition-table coverage (Task 9) including invalid-transition no-ops.

**12. Co-author trailer.** All commit subjects in this plan deliberately omit `Co-Authored-By: Claude…` trailers per the user's standing rule (`no-claude-coauthor-trailer`).
