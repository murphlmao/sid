//! Integration tests for `PlainStore` — the file-backed `SecretStore` impl.

use std::sync::Arc;

use proptest::prelude::*;
use sid_core::adapters::secrets::{SecretId, SecretStore};
use sid_secrets::PlainStore;
use sid_store::{OpenStore, RedbStore, Store};
use tempfile::tempdir;

fn fresh() -> (tempfile::TempDir, PlainStore) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("sid.redb");
    let inner: Arc<dyn Store> = Arc::new(RedbStore::open(&path).unwrap());
    (dir, PlainStore::new(inner))
}

// ---------------------------------------------------------------------------
// Basic round-trip
// ---------------------------------------------------------------------------

#[test]
fn put_then_get_returns_value() {
    let (_d, s) = fresh();
    let id = SecretId::new("ssh.key.id_ed25519");
    s.put(&id, b"passphrase").unwrap();
    assert_eq!(s.get(&id).unwrap().unwrap(), b"passphrase".to_vec());
}

#[test]
fn get_missing_returns_none() {
    let (_d, s) = fresh();
    assert!(s.get(&SecretId::new("missing.id")).unwrap().is_none());
}

#[test]
fn put_overwrites_previous_value() {
    let (_d, s) = fresh();
    let id = SecretId::new("api.token");
    s.put(&id, b"v1").unwrap();
    s.put(&id, b"v2").unwrap();
    assert_eq!(s.get(&id).unwrap().unwrap(), b"v2".to_vec());
}

#[test]
fn delete_removes_value() {
    let (_d, s) = fresh();
    let id = SecretId::new("doomed");
    s.put(&id, b"bye").unwrap();
    s.delete(&id).unwrap();
    assert!(s.get(&id).unwrap().is_none());
}

#[test]
fn delete_missing_id_is_noop() {
    let (_d, s) = fresh();
    s.delete(&SecretId::new("never.was")).unwrap();
}

#[test]
fn list_ids_empty_for_fresh_store() {
    let (_d, s) = fresh();
    assert!(s.list_ids().unwrap().is_empty());
}

#[test]
fn list_ids_returns_all_keys() {
    let (_d, s) = fresh();
    for k in &["a", "b", "c"] {
        s.put(&SecretId::new(*k), b"v").unwrap();
    }
    let mut ids: Vec<String> =
        s.list_ids().unwrap().into_iter().map(|i| i.as_str().to_string()).collect();
    ids.sort();
    assert_eq!(ids, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
}

#[test]
fn list_ids_excludes_deleted_keys() {
    let (_d, s) = fresh();
    s.put(&SecretId::new("a"), b"1").unwrap();
    s.put(&SecretId::new("b"), b"2").unwrap();
    s.delete(&SecretId::new("a")).unwrap();
    let ids: Vec<String> =
        s.list_ids().unwrap().into_iter().map(|i| i.as_str().to_string()).collect();
    assert_eq!(ids, vec!["b".to_string()]);
}

// ---------------------------------------------------------------------------
// Adversarial
// ---------------------------------------------------------------------------

#[test]
fn empty_value_is_storable_and_retrievable() {
    let (_d, s) = fresh();
    let id = SecretId::new("empty");
    s.put(&id, b"").unwrap();
    assert_eq!(s.get(&id).unwrap().unwrap(), Vec::<u8>::new());
}

#[test]
fn very_long_key_works() {
    let (_d, s) = fresh();
    let key: String = "k".repeat(10_000);
    let id = SecretId::new(key);
    s.put(&id, b"x").unwrap();
    assert_eq!(s.get(&id).unwrap().unwrap(), b"x".to_vec());
}

#[test]
fn one_megabyte_value_roundtrips() {
    let (_d, s) = fresh();
    let id = SecretId::new("big");
    let value = vec![0xABu8; 1_000_000];
    s.put(&id, &value).unwrap();
    assert_eq!(s.get(&id).unwrap().unwrap(), value);
}

#[test]
fn non_ascii_id_and_value_roundtrip() {
    let (_d, s) = fresh();
    let id = SecretId::new("ключ.секрет");
    let value = "пароль🔑".as_bytes();
    s.put(&id, value).unwrap();
    assert_eq!(s.get(&id).unwrap().unwrap(), value.to_vec());
}

#[test]
fn put_then_delete_then_get_is_none() {
    let (_d, s) = fresh();
    let id = SecretId::new("doomed");
    s.put(&id, b"value").unwrap();
    s.delete(&id).unwrap();
    assert!(s.get(&id).unwrap().is_none());
}

#[test]
fn many_secrets_all_retrievable() {
    let (_d, s) = fresh();
    for i in 0..200 {
        let id = SecretId::new(format!("k.{i}"));
        let v = format!("v.{i}");
        s.put(&id, v.as_bytes()).unwrap();
    }
    for i in 0..200 {
        let id = SecretId::new(format!("k.{i}"));
        let v = format!("v.{i}");
        assert_eq!(s.get(&id).unwrap().unwrap(), v.into_bytes());
    }
    assert_eq!(s.list_ids().unwrap().len(), 200);
}

// ---------------------------------------------------------------------------
// Send + Sync
// ---------------------------------------------------------------------------

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn plain_store_is_send_sync() {
    assert_send_sync::<PlainStore>();
    assert_send_sync::<&dyn SecretStore>();
}

// ---------------------------------------------------------------------------
// Property tests
// ---------------------------------------------------------------------------

proptest! {
    /// `put(id, value)` followed by `get(id)` always returns `Some(value)`.
    #[test]
    fn prop_put_get_roundtrip(
        id_str in "[a-zA-Z0-9_.\\-]{1,64}",
        value in proptest::collection::vec(any::<u8>(), 0..4096),
    ) {
        let (_d, s) = fresh();
        let id = SecretId::new(id_str);
        s.put(&id, &value).unwrap();
        prop_assert_eq!(s.get(&id).unwrap().unwrap(), value);
    }

    /// `put(id, _); delete(id); get(id)` returns `None`.
    #[test]
    fn prop_delete_removes(
        id_str in "[a-zA-Z0-9_.\\-]{1,64}",
        value in proptest::collection::vec(any::<u8>(), 0..256),
    ) {
        let (_d, s) = fresh();
        let id = SecretId::new(id_str);
        s.put(&id, &value).unwrap();
        s.delete(&id).unwrap();
        prop_assert!(s.get(&id).unwrap().is_none());
    }

    /// Last write wins: the most recent `put` for a given id is what `get` returns.
    #[test]
    fn prop_last_write_wins(
        id_str in "[a-zA-Z0-9_.\\-]{1,64}",
        v1 in proptest::collection::vec(any::<u8>(), 0..128),
        v2 in proptest::collection::vec(any::<u8>(), 0..128),
    ) {
        let (_d, s) = fresh();
        let id = SecretId::new(id_str);
        s.put(&id, &v1).unwrap();
        s.put(&id, &v2).unwrap();
        prop_assert_eq!(s.get(&id).unwrap().unwrap(), v2);
    }
}
