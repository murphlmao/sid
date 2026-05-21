//! Integration tests for the `secrets` table on `RedbStore`.

use proptest::prelude::*;
use sid_store::{OpenStore, RedbStore, Store};
use tempfile::tempdir;

fn fresh() -> (tempfile::TempDir, RedbStore) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("sid.redb");
    let store = RedbStore::open(&path).unwrap();
    (dir, store)
}

// ---------------------------------------------------------------------------
// Basic round-trip
// ---------------------------------------------------------------------------

#[test]
fn secret_put_then_get_returns_value() {
    let (_d, s) = fresh();
    s.secret_put("ssh.key.foo", b"phrase").unwrap();
    assert_eq!(
        s.secret_get("ssh.key.foo").unwrap().unwrap(),
        b"phrase".to_vec()
    );
}

#[test]
fn secret_get_missing_returns_none() {
    let (_d, s) = fresh();
    assert!(s.secret_get("missing").unwrap().is_none());
}

#[test]
fn secret_put_overwrites() {
    let (_d, s) = fresh();
    s.secret_put("k", b"v1").unwrap();
    s.secret_put("k", b"v2").unwrap();
    assert_eq!(s.secret_get("k").unwrap().unwrap(), b"v2".to_vec());
}

#[test]
fn secret_delete_removes_value() {
    let (_d, s) = fresh();
    s.secret_put("k", b"v").unwrap();
    s.secret_delete("k").unwrap();
    assert!(s.secret_get("k").unwrap().is_none());
}

#[test]
fn secret_delete_missing_is_noop() {
    let (_d, s) = fresh();
    s.secret_delete("never.was").unwrap();
}

#[test]
fn list_secret_ids_empty_for_fresh_store() {
    let (_d, s) = fresh();
    assert!(s.list_secret_ids().unwrap().is_empty());
}

#[test]
fn list_secret_ids_returns_all_keys() {
    let (_d, s) = fresh();
    for k in &["a", "b", "c"] {
        s.secret_put(k, b"v").unwrap();
    }
    let mut ids = s.list_secret_ids().unwrap();
    ids.sort();
    assert_eq!(ids, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
}

// ---------------------------------------------------------------------------
// Adversarial
// ---------------------------------------------------------------------------

#[test]
fn empty_value_storable() {
    let (_d, s) = fresh();
    s.secret_put("empty", b"").unwrap();
    assert_eq!(s.secret_get("empty").unwrap().unwrap(), Vec::<u8>::new());
}

#[test]
fn one_megabyte_value_roundtrips() {
    let (_d, s) = fresh();
    let value = vec![0x55u8; 1_000_000];
    s.secret_put("big", &value).unwrap();
    assert_eq!(s.secret_get("big").unwrap().unwrap(), value);
}

#[test]
fn unicode_id_and_value_roundtrip() {
    let (_d, s) = fresh();
    s.secret_put("ключ", "пароль🔑".as_bytes()).unwrap();
    assert_eq!(
        s.secret_get("ключ").unwrap().unwrap(),
        "пароль🔑".as_bytes().to_vec()
    );
}

#[test]
fn secrets_persist_across_reopen() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("sid.redb");
    {
        let s = RedbStore::open(&path).unwrap();
        s.secret_put("k", b"persisted").unwrap();
    }
    let s2 = RedbStore::open(&path).unwrap();
    assert_eq!(s2.secret_get("k").unwrap().unwrap(), b"persisted".to_vec());
}

// ---------------------------------------------------------------------------
// Property tests
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn prop_put_get_roundtrip(
        id in "[a-zA-Z0-9_.\\-]{1,64}",
        value in proptest::collection::vec(any::<u8>(), 0..2048),
    ) {
        let (_d, s) = fresh();
        s.secret_put(&id, &value).unwrap();
        prop_assert_eq!(s.secret_get(&id).unwrap().unwrap(), value);
    }

    #[test]
    fn prop_delete_then_get_is_none(
        id in "[a-zA-Z0-9_.\\-]{1,64}",
        value in proptest::collection::vec(any::<u8>(), 0..256),
    ) {
        let (_d, s) = fresh();
        s.secret_put(&id, &value).unwrap();
        s.secret_delete(&id).unwrap();
        prop_assert!(s.secret_get(&id).unwrap().is_none());
    }
}
