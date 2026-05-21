//! Integration tests for `TypedSettings` extension trait on `Store` /
//! `RedbStore`. Includes round-trip property tests and adversarial inputs.

use proptest::prelude::*;
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
    assert_eq!(
        s.get_string(settings_keys::THEME_NAME).unwrap().as_deref(),
        Some("cosmos")
    );
}

#[test]
fn string_overwrite() {
    let (_d, s) = store();
    s.put_string("k", "v1").unwrap();
    s.put_string("k", "v2").unwrap();
    assert_eq!(s.get_string("k").unwrap().as_deref(), Some("v2"));
}

#[test]
fn u64_round_trip() {
    let (_d, s) = store();
    assert!(s.get_u64(settings_keys::PERSIST_DEBOUNCE_MS).unwrap().is_none());
    s.put_u64(settings_keys::PERSIST_DEBOUNCE_MS, 250).unwrap();
    assert_eq!(
        s.get_u64(settings_keys::PERSIST_DEBOUNCE_MS).unwrap(),
        Some(250)
    );
}

#[test]
fn u64_zero_round_trips() {
    let (_d, s) = store();
    s.put_u64("k", 0).unwrap();
    assert_eq!(s.get_u64("k").unwrap(), Some(0));
}

#[test]
fn u64_max_round_trips() {
    let (_d, s) = store();
    s.put_u64("k", u64::MAX).unwrap();
    assert_eq!(s.get_u64("k").unwrap(), Some(u64::MAX));
}

#[test]
fn bool_round_trip() {
    let (_d, s) = store();
    s.put_bool(settings_keys::AUTO_RESTORE_SESSION, true).unwrap();
    assert_eq!(
        s.get_bool(settings_keys::AUTO_RESTORE_SESSION).unwrap(),
        Some(true)
    );
    s.put_bool(settings_keys::AUTO_RESTORE_SESSION, false).unwrap();
    assert_eq!(
        s.get_bool(settings_keys::AUTO_RESTORE_SESSION).unwrap(),
        Some(false)
    );
}

#[test]
fn invalid_bool_returns_error() {
    let (_d, s) = store();
    s.put_setting(
        settings_keys::AUTO_RESTORE_SESSION,
        &SettingValue(b"maybe".to_vec()),
    )
    .unwrap();
    assert!(s.get_bool(settings_keys::AUTO_RESTORE_SESSION).is_err());
}

#[test]
fn invalid_u64_returns_error() {
    let (_d, s) = store();
    s.put_setting(
        settings_keys::PERSIST_DEBOUNCE_MS,
        &SettingValue(b"not-a-number".to_vec()),
    )
    .unwrap();
    assert!(s.get_u64(settings_keys::PERSIST_DEBOUNCE_MS).is_err());
}

#[test]
fn invalid_utf8_string_returns_error() {
    let (_d, s) = store();
    s.put_setting(settings_keys::THEME_NAME, &SettingValue(vec![0xFF, 0xFE]))
        .unwrap();
    assert!(s.get_string(settings_keys::THEME_NAME).is_err());
}

#[test]
fn invalid_utf8_u64_returns_error() {
    let (_d, s) = store();
    s.put_setting("k", &SettingValue(vec![0xFF, 0xFE])).unwrap();
    assert!(s.get_u64("k").is_err());
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
    assert_eq!(
        s.get_string("k").unwrap().as_deref(),
        Some("héllo · ✦ ★ 🐕")
    );
}

#[test]
fn settings_keys_are_distinct() {
    // Trivial sanity: no two canonical keys collide.
    let all = [
        settings_keys::THEME_NAME,
        settings_keys::KEYBIND_PROFILE_NAME,
        settings_keys::WORKSPACE_ROOTS,
        settings_keys::PERSIST_DEBOUNCE_MS,
        settings_keys::HEARTBEAT_INTERVAL_SECS,
        settings_keys::AUTO_RESTORE_SESSION,
        settings_keys::AUTO_SCAN_WORKSPACES,
        settings_keys::DEFAULT_TAB,
        settings_keys::SETTINGS_FOCUSED_CATEGORY,
    ];
    let mut sorted = all.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), all.len(), "duplicate setting key constant");
}

proptest! {
    #[test]
    fn prop_string_round_trip(s in "[\\p{L}\\p{N}_.\\- ]{0,128}") {
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
