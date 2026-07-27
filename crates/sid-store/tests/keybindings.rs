//! The `keybindings` table — Settings → Keymap's user overrides.
//!
//! Overrides live in their own table (keyed by the action's stable id) rather than as
//! fields on [`sid_store::Settings`]: `Settings` is a single positional-postcard blob
//! with a V1→V4 migration chain mirrored in `sid-db`'s redb browse engine, and a
//! keybinding set is a *collection*, not a scalar preference. A new table is additive —
//! adding an action later needs no schema hop at all.
//!
//! Persistence across a restart is the acceptance property for the rebinding UI, so it
//! is asserted here directly: written, dropped, reopened, still there.

use sid_store::{KeyBinding, Store};

fn tmp_store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().expect("tmp dir");
    let store = Store::open(&dir.path().join("store.redb")).expect("open tmp store");
    (dir, store)
}

fn binding(action: &str, key: &str) -> KeyBinding {
    KeyBinding {
        action: action.into(),
        key: key.into(),
        ctrl: true,
        shift: false,
    }
}

#[test]
fn a_store_with_no_overrides_lists_none() {
    let (_dir, store) = tmp_store();
    assert!(store.keybindings().expect("read").is_empty());
}

#[test]
fn a_keybinding_round_trips_every_field() {
    let (_dir, store) = tmp_store();
    let b = KeyBinding {
        action: "command_palette".into(),
        key: "g".into(),
        ctrl: true,
        shift: true,
    };
    store.set_keybinding(&b).expect("write");
    assert_eq!(store.keybindings().expect("read"), vec![b]);
}

#[test]
fn rebinding_the_same_action_twice_replaces_rather_than_appends() {
    // The action id is the identity: a second rebind is an upsert, never a duplicate
    // row that would make the effective registry ambiguous.
    let (_dir, store) = tmp_store();
    store.set_keybinding(&binding("settings", "g")).expect("w1");
    store.set_keybinding(&binding("settings", "j")).expect("w2");
    let all = store.keybindings().expect("read");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].key, "j");
}

#[test]
fn clearing_one_override_leaves_the_others() {
    let (_dir, store) = tmp_store();
    store.set_keybinding(&binding("settings", "g")).expect("w1");
    store
        .set_keybinding(&binding("cheat_sheet", "j"))
        .expect("w2");
    assert!(store.clear_keybinding("settings").expect("clear"));
    let all = store.keybindings().expect("read");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].action, "cheat_sheet");
}

#[test]
fn clearing_an_action_that_was_never_rebound_is_not_an_error() {
    let (_dir, store) = tmp_store();
    assert!(!store.clear_keybinding("settings").expect("clear"));
}

#[test]
fn clear_all_empties_the_table() {
    let (_dir, store) = tmp_store();
    store.set_keybinding(&binding("settings", "g")).expect("w1");
    store
        .set_keybinding(&binding("cheat_sheet", "j"))
        .expect("w2");
    assert_eq!(store.clear_all_keybindings().expect("reset all"), 2);
    assert!(store.keybindings().expect("read").is_empty());
    // Idempotent: "reset all" on an already-default keymap is a no-op, not an error.
    assert_eq!(store.clear_all_keybindings().expect("again"), 0);
}

#[test]
fn an_override_survives_closing_and_reopening_the_store() {
    // The acceptance property behind the rebinding UI: a rebound chord is still
    // rebound after a restart. Same file, fresh `Store`.
    let dir = tempfile::tempdir().expect("tmp dir");
    let path = dir.path().join("store.redb");
    {
        let store = Store::open(&path).expect("open");
        store
            .set_keybinding(&KeyBinding {
                action: "command_palette".into(),
                key: "j".into(),
                ctrl: true,
                shift: false,
            })
            .expect("write");
    }
    let reopened = Store::open(&path).expect("reopen");
    let all = reopened.keybindings().expect("read");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].action, "command_palette");
    assert_eq!(all[0].key, "j");
    assert!(all[0].ctrl);
    assert!(!all[0].shift);
}

#[test]
fn keybindings_do_not_disturb_settings() {
    // The reason for a separate table: writing an override must not read-modify-write
    // the `Settings` blob (whose positional layout is mirrored in sid-db).
    let (_dir, store) = tmp_store();
    let mut settings = store.settings().expect("settings");
    settings.theme = "void".into();
    store.set_settings(&settings).expect("write settings");
    store.set_keybinding(&binding("settings", "g")).expect("kb");
    assert_eq!(store.settings().expect("re-read").theme, "void");
}
