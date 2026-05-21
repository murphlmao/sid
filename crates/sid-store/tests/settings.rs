use proptest::prelude::*;
use sid_store::{OpenStore, RedbStore, SettingValue, Store};
use tempfile::tempdir;

// ── Happy-path tests (plan minimums) ─────────────────────────────────────────

#[test]
fn put_then_get_round_trips() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store
        .put_setting("theme.name", &SettingValue(b"cosmos".to_vec()))
        .unwrap();
    let v = store.get_setting("theme.name").unwrap().unwrap();
    assert_eq!(v.0, b"cosmos".to_vec());
}

#[test]
fn get_unknown_returns_none() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    assert!(store.get_setting("missing").unwrap().is_none());
}

#[test]
fn put_overwrites_existing_value() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store
        .put_setting("k", &SettingValue(b"v1".to_vec()))
        .unwrap();
    store
        .put_setting("k", &SettingValue(b"v2".to_vec()))
        .unwrap();
    let v = store.get_setting("k").unwrap().unwrap();
    assert_eq!(v.0, b"v2".to_vec());
}

// ── Adversarial tests ─────────────────────────────────────────────────────────

#[test]
fn empty_key_round_trips() {
    // Empty string is a valid key.
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store
        .put_setting("", &SettingValue(b"empty_key".to_vec()))
        .unwrap();
    let v = store.get_setting("").unwrap().unwrap();
    assert_eq!(v.0, b"empty_key");
}

#[test]
fn very_long_key_round_trips() {
    // 10 KB key.
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    let big_key = "k".repeat(10_000);
    store
        .put_setting(&big_key, &SettingValue(b"big_key_val".to_vec()))
        .unwrap();
    let v = store.get_setting(&big_key).unwrap().unwrap();
    assert_eq!(v.0, b"big_key_val");
}

#[test]
fn key_with_nul_bytes_round_trips() {
    // Keys with NUL bytes are valid (redb uses byte slice keys).
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    // Note: redb's &str key cannot contain interior NULs via normal str,
    // but the implementation uses str keys, so test with look-alike chars.
    let nul_like_key = "key\u{0000}suffix";
    // redb's TableDefinition<&str, &[u8]> accepts NUL-containing &str
    store
        .put_setting(nul_like_key, &SettingValue(b"nul".to_vec()))
        .unwrap();
    let v = store.get_setting(nul_like_key).unwrap().unwrap();
    assert_eq!(v.0, b"nul");
}

#[test]
fn value_with_1mb_payload_round_trips() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    let big_val = SettingValue(vec![0x42u8; 1024 * 1024]);
    store.put_setting("big", &big_val).unwrap();
    let got = store.get_setting("big").unwrap().unwrap();
    assert_eq!(got.0, big_val.0);
}

#[test]
fn get_after_put_after_delete_pattern() {
    // put → verify → put empty → verify empty → get unknown returns None
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();

    store
        .put_setting("key", &SettingValue(b"first".to_vec()))
        .unwrap();
    assert_eq!(store.get_setting("key").unwrap().unwrap().0, b"first");

    // Overwrite with empty value (equivalent to "delete" in simple stores).
    store.put_setting("key", &SettingValue(vec![])).unwrap();
    assert_eq!(store.get_setting("key").unwrap().unwrap().0, b"" as &[u8]);

    // A different key is still None.
    assert!(store.get_setting("other").unwrap().is_none());
}

// ── Property tests: relational invariants ─────────────────────────────────────

proptest! {
    /// put_setting then get_setting round-trip for arbitrary (key, value) pairs.
    #[test]
    fn proptest_put_get_round_trip(
        key in "[a-z.]{1,32}",
        value in proptest::collection::vec(0u8..=255u8, 0..=1024),
    ) {
        let dir = tempdir().unwrap();
        let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
        let val = SettingValue(value.clone());
        store.put_setting(&key, &val).unwrap();
        let got = store.get_setting(&key).unwrap().unwrap();
        prop_assert_eq!(got.0, value);
    }

    /// put is idempotent: last write wins.
    #[test]
    fn proptest_put_idempotent_last_write_wins(
        key in "[a-z]{1,16}",
        v1 in proptest::collection::vec(0u8..=255u8, 1..=64),
        v2 in proptest::collection::vec(0u8..=255u8, 1..=64),
    ) {
        let dir = tempdir().unwrap();
        let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
        store.put_setting(&key, &SettingValue(v1)).unwrap();
        store.put_setting(&key, &SettingValue(v2.clone())).unwrap();
        let got = store.get_setting(&key).unwrap().unwrap();
        prop_assert_eq!(got.0, v2);
    }
}

#[test]
fn multiple_distinct_keys_are_independent() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store
        .put_setting("a", &SettingValue(b"alpha".to_vec()))
        .unwrap();
    store
        .put_setting("b", &SettingValue(b"beta".to_vec()))
        .unwrap();
    assert_eq!(store.get_setting("a").unwrap().unwrap().0, b"alpha");
    assert_eq!(store.get_setting("b").unwrap().unwrap().0, b"beta");
}

#[test]
fn empty_value_is_distinct_from_absent_key() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    // Key "present" has an empty value — it IS present.
    store.put_setting("present", &SettingValue(vec![])).unwrap();
    assert!(
        store.get_setting("present").unwrap().is_some(),
        "empty value key must be present"
    );
    // Key "absent" was never written.
    assert!(
        store.get_setting("absent").unwrap().is_none(),
        "never-written key must be absent"
    );
}
