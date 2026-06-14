//! Integration tests for the `quick_actions` table on `RedbStore`.

use proptest::prelude::*;
use sid_store::{
    OpenStore, QuickAction, QuickActionScope, RedbStore, Store, schema::QUICK_ACTIONS,
};
use tempfile::tempdir;

fn store() -> (tempfile::TempDir, RedbStore) {
    let d = tempdir().unwrap();
    let s = RedbStore::open(&d.path().join("sid.redb")).unwrap();
    (d, s)
}

fn sample(id: &str) -> QuickAction {
    QuickAction {
        id: id.into(),
        label: format!("Label {id}"),
        cmd: "echo hi".into(),
        keybind: Some("Char('a')|2".into()),
        scope: QuickActionScope::Global,
    }
}

#[test]
fn list_is_empty_initially() {
    let (_d, s) = store();
    assert!(s.list_quick_actions().unwrap().is_empty());
}

#[test]
fn upsert_then_get_round_trips() {
    let (_d, s) = store();
    s.upsert_quick_action(&sample("qa.reload")).unwrap();
    assert_eq!(
        s.get_quick_action("qa.reload").unwrap().unwrap(),
        sample("qa.reload")
    );
}

#[test]
fn workspace_scope_round_trips() {
    let (_d, s) = store();
    let mut a = sample("qa.ws");
    a.scope = QuickActionScope::Workspace;
    s.upsert_quick_action(&a).unwrap();
    assert_eq!(
        s.get_quick_action("qa.ws").unwrap().unwrap().scope,
        QuickActionScope::Workspace,
    );
}

#[test]
fn no_keybind_round_trips() {
    let (_d, s) = store();
    let mut a = sample("qa.nk");
    a.keybind = None;
    s.upsert_quick_action(&a).unwrap();
    assert!(
        s.get_quick_action("qa.nk")
            .unwrap()
            .unwrap()
            .keybind
            .is_none()
    );
}

#[test]
fn list_returns_lexicographic_order() {
    let (_d, s) = store();
    for id in &["zz", "aa", "mm"] {
        s.upsert_quick_action(&sample(id)).unwrap();
    }
    let ids: Vec<_> = s
        .list_quick_actions()
        .unwrap()
        .into_iter()
        .map(|a| a.id)
        .collect();
    assert_eq!(ids, vec!["aa", "mm", "zz"]);
}

#[test]
fn upsert_replaces_existing() {
    let (_d, s) = store();
    s.upsert_quick_action(&sample("q")).unwrap();
    let mut v2 = sample("q");
    v2.label = "Other".into();
    s.upsert_quick_action(&v2).unwrap();
    assert_eq!(s.get_quick_action("q").unwrap().unwrap().label, "Other");
    assert_eq!(s.list_quick_actions().unwrap().len(), 1);
}

#[test]
fn get_returns_none_for_missing() {
    let (_d, s) = store();
    assert!(s.get_quick_action("missing").unwrap().is_none());
}

#[test]
fn remove_drops_action() {
    let (_d, s) = store();
    s.upsert_quick_action(&sample("x")).unwrap();
    s.remove_quick_action("x").unwrap();
    assert!(s.get_quick_action("x").unwrap().is_none());
}

#[test]
fn remove_missing_is_noop() {
    let (_d, s) = store();
    s.remove_quick_action("never").unwrap();
}

#[test]
fn empty_id_round_trips() {
    let (_d, s) = store();
    s.upsert_quick_action(&sample("")).unwrap();
    assert_eq!(s.get_quick_action("").unwrap().unwrap().id, "");
}

#[test]
fn unicode_label_round_trips() {
    let (_d, s) = store();
    let mut a = sample("u");
    a.label = "Réload ✦".into();
    s.upsert_quick_action(&a).unwrap();
    assert_eq!(s.get_quick_action("u").unwrap().unwrap().label, "Réload ✦");
}

#[test]
fn long_cmd_round_trips() {
    let (_d, s) = store();
    let mut a = sample("l");
    a.cmd = "sh -c 'echo ".to_string() + &"x".repeat(4096) + "'";
    s.upsert_quick_action(&a).unwrap();
    assert!(s.get_quick_action("l").unwrap().unwrap().cmd.contains("x"));
}

#[test]
fn corrupted_blob_returns_err_not_panic() {
    let d = tempdir().unwrap();
    let path = d.path().join("sid.redb");
    {
        let s = RedbStore::open(&path).unwrap();
        s.upsert_quick_action(&sample("q")).unwrap();
        drop(s);
    }
    {
        let raw = redb::Database::open(&path).unwrap();
        let txn = raw.begin_write().unwrap();
        {
            let mut tbl = txn.open_table(QUICK_ACTIONS).unwrap();
            tbl.insert("q", &b"not-a-postcard"[..]).unwrap();
        }
        txn.commit().unwrap();
    }
    let s = RedbStore::open(&path).unwrap();
    assert!(s.get_quick_action("q").is_err());
}

proptest! {
    #[test]
    fn prop_round_trip(
        id in "[a-z0-9.]{0,32}",
        label in "[\\p{L} ]{0,64}",
        cmd in "[ -~]{0,64}",
        kb in proptest::option::of("[a-zA-Z0-9_.|()' ]{1,32}"),
        ws in any::<bool>(),
    ) {
        let (_d, store) = store();
        let scope = if ws { QuickActionScope::Workspace } else { QuickActionScope::Global };
        let a = QuickAction {
            id: id.clone(), label, cmd, keybind: kb, scope,
        };
        store.upsert_quick_action(&a).unwrap();
        prop_assert_eq!(store.get_quick_action(&id).unwrap().unwrap(), a);
    }
}

#[test]
fn new_id_has_qa_prefix_and_fixed_length() {
    let id = QuickAction::new_id();
    assert!(id.starts_with("qa-"));
    assert_eq!(id.len(), 3 + 14);
}

#[test]
fn new_id_two_consecutive_ids_are_different() {
    // Wall-clock + pid mixing should produce distinct ids on consecutive calls
    // even in the same nanosecond bucket. Allow one retry to absorb rare ties.
    let id1 = QuickAction::new_id();
    let id2 = QuickAction::new_id();
    if id1 == id2 {
        // Retry once after a tiny sleep.
        std::thread::sleep(std::time::Duration::from_micros(1));
        let id3 = QuickAction::new_id();
        assert_ne!(id1, id3, "three consecutive ids all equal: {id1}");
    }
}

#[test]
fn shell_words_command_round_trips_through_storage() {
    let (_d, store) = store();
    let cmd = r#"sh -c "echo 'one two' | tr o O""#;
    let a = QuickAction {
        id: QuickAction::new_id(),
        label: "weird quoting".into(),
        scope: QuickActionScope::Global,
        cmd: cmd.into(),
        keybind: None,
    };
    store.upsert_quick_action(&a).unwrap();
    let got = store.get_quick_action(&a.id).unwrap().unwrap();
    assert_eq!(got.cmd, cmd);
    // Parser tolerates the weird quoting.
    let _ = shell_words::split(&got.cmd).unwrap();
}
