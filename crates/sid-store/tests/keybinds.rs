//! Integration tests for the `keybinds` table on `RedbStore`.

use proptest::prelude::*;
use sid_store::{KeybindEntry, KeybindProfile, OpenStore, RedbStore, Store, schema::KEYBINDS};
use tempfile::tempdir;

fn store() -> (tempfile::TempDir, RedbStore) {
    let d = tempdir().unwrap();
    let s = RedbStore::open(&d.path().join("sid.redb")).unwrap();
    (d, s)
}

fn sample(name: &str) -> KeybindProfile {
    KeybindProfile {
        name: name.into(),
        bindings: vec![
            KeybindEntry {
                chord: "Char('q')|0".into(),
                action: "app.quit".into(),
            },
            KeybindEntry {
                chord: "Char('?')|0".into(),
                action: "app.help".into(),
            },
        ],
    }
}

#[test]
fn list_is_empty_initially() {
    let (_d, s) = store();
    assert!(s.list_keybind_profiles().unwrap().is_empty());
}

#[test]
fn upsert_then_get_round_trips() {
    let (_d, s) = store();
    s.upsert_keybind_profile(&sample("default")).unwrap();
    assert_eq!(
        s.get_keybind_profile("default").unwrap().unwrap(),
        sample("default")
    );
}

#[test]
fn upsert_replaces_existing() {
    let (_d, s) = store();
    s.upsert_keybind_profile(&sample("p")).unwrap();
    let mut v2 = sample("p");
    v2.bindings.clear();
    s.upsert_keybind_profile(&v2).unwrap();
    assert!(
        s.get_keybind_profile("p")
            .unwrap()
            .unwrap()
            .bindings
            .is_empty()
    );
    assert_eq!(s.list_keybind_profiles().unwrap().len(), 1);
}

#[test]
fn list_returns_lexicographic_order() {
    let (_d, s) = store();
    for n in &["zeta", "alpha", "mu"] {
        s.upsert_keybind_profile(&sample(n)).unwrap();
    }
    let names: Vec<_> = s
        .list_keybind_profiles()
        .unwrap()
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(names, vec!["alpha", "mu", "zeta"]);
}

#[test]
fn get_returns_none_for_missing() {
    let (_d, s) = store();
    assert!(s.get_keybind_profile("missing").unwrap().is_none());
}

#[test]
fn remove_drops_profile() {
    let (_d, s) = store();
    s.upsert_keybind_profile(&sample("k")).unwrap();
    s.remove_keybind_profile("k").unwrap();
    assert!(s.get_keybind_profile("k").unwrap().is_none());
}

#[test]
fn remove_missing_is_noop() {
    let (_d, s) = store();
    s.remove_keybind_profile("never").unwrap();
}

#[test]
fn empty_name_round_trips() {
    let (_d, s) = store();
    s.upsert_keybind_profile(&sample("")).unwrap();
    assert_eq!(s.get_keybind_profile("").unwrap().unwrap().name, "");
}

#[test]
fn long_name_round_trips() {
    let (_d, s) = store();
    let name = "k".repeat(4096);
    s.upsert_keybind_profile(&sample(&name)).unwrap();
    assert_eq!(
        s.get_keybind_profile(&name).unwrap().unwrap().name.len(),
        4096
    );
}

#[test]
fn unicode_in_chord_string_round_trips() {
    let (_d, s) = store();
    let p = KeybindProfile {
        name: "u".into(),
        bindings: vec![KeybindEntry {
            chord: "Char('é')|0".into(),
            action: "app.unicode".into(),
        }],
    };
    s.upsert_keybind_profile(&p).unwrap();
    assert_eq!(s.get_keybind_profile("u").unwrap().unwrap(), p);
}

#[test]
fn empty_bindings_round_trip() {
    let (_d, s) = store();
    let p = KeybindProfile {
        name: "empty".into(),
        bindings: vec![],
    };
    s.upsert_keybind_profile(&p).unwrap();
    assert!(
        s.get_keybind_profile("empty")
            .unwrap()
            .unwrap()
            .bindings
            .is_empty()
    );
}

#[test]
fn corrupted_blob_returns_err_not_panic() {
    let d = tempdir().unwrap();
    let path = d.path().join("sid.redb");
    {
        let s = RedbStore::open(&path).unwrap();
        s.upsert_keybind_profile(&sample("k")).unwrap();
        // Drop the RedbStore so the database file is no longer held open.
        drop(s);
    }
    {
        let raw = redb::Database::open(&path).unwrap();
        let txn = raw.begin_write().unwrap();
        {
            let mut tbl = txn.open_table(KEYBINDS).unwrap();
            tbl.insert("k", &b"garbage"[..]).unwrap();
        }
        txn.commit().unwrap();
    }
    let s = RedbStore::open(&path).unwrap();
    assert!(s.get_keybind_profile("k").is_err());
}

proptest! {
    #[test]
    fn prop_round_trip(
        name in "[a-z]{0,32}",
        chords in proptest::collection::vec(("[a-zA-Z0-9_.()'|]{1,32}", "[a-z0-9.]{1,32}"), 0..16),
    ) {
        let (_d, store) = store();
        let bindings: Vec<KeybindEntry> = chords
            .into_iter()
            .map(|(c, a)| KeybindEntry { chord: c, action: a })
            .collect();
        let p = KeybindProfile { name: name.clone(), bindings };
        store.upsert_keybind_profile(&p).unwrap();
        let got = store.get_keybind_profile(&name).unwrap().unwrap();
        prop_assert_eq!(got, p);
    }
}
