use std::path::{Path, PathBuf};

use sid_store::{OpenStore, PinnedConfig, RedbStore, Store, now_epoch};
use tempfile::tempdir;

fn pc(p: &str, label: &str) -> PinnedConfig {
    PinnedConfig {
        path: PathBuf::from(p),
        label: label.into(),
        opener_cmd: None,
        created_at: now_epoch(),
    }
}

#[test]
fn pinned_config_construction() {
    let p = pc("/etc/nginx/nginx.conf", "nginx");
    assert_eq!(p.label, "nginx");
    assert!(p.opener_cmd.is_none());
}

#[test]
fn upsert_then_list() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store.upsert_pinned_config(&pc("/etc/a.conf", "a")).unwrap();
    store.upsert_pinned_config(&pc("/etc/b.conf", "b")).unwrap();
    let all = store.list_pinned_configs().unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn get_returns_existing_and_none_for_missing() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store.upsert_pinned_config(&pc("/etc/x.conf", "x")).unwrap();
    assert!(
        store
            .get_pinned_config(Path::new("/etc/x.conf"))
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .get_pinned_config(Path::new("/etc/missing.conf"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn remove_drops_it() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store.upsert_pinned_config(&pc("/etc/a.conf", "a")).unwrap();
    store
        .remove_pinned_config(Path::new("/etc/a.conf"))
        .unwrap();
    assert!(store.list_pinned_configs().unwrap().is_empty());
}

#[test]
fn upsert_replaces_existing() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store
        .upsert_pinned_config(&pc("/etc/a.conf", "v1"))
        .unwrap();
    store
        .upsert_pinned_config(&pc("/etc/a.conf", "v2"))
        .unwrap();
    let got = store
        .get_pinned_config(Path::new("/etc/a.conf"))
        .unwrap()
        .unwrap();
    assert_eq!(got.label, "v2");
}

#[test]
fn remove_nonexistent_is_noop() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store.remove_pinned_config(Path::new("/never")).unwrap();
}

#[test]
fn list_with_100_pins_returns_all() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    for i in 0..100 {
        store
            .upsert_pinned_config(&pc(&format!("/etc/c{i}.conf"), &format!("l{i}")))
            .unwrap();
    }
    assert_eq!(store.list_pinned_configs().unwrap().len(), 100);
}

#[test]
fn unicode_in_label_round_trips() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store
        .upsert_pinned_config(&pc("/etc/a.conf", "✦ cosmos config 🐕"))
        .unwrap();
    let got = store
        .get_pinned_config(Path::new("/etc/a.conf"))
        .unwrap()
        .unwrap();
    assert_eq!(got.label, "✦ cosmos config 🐕");
}

#[test]
fn opener_cmd_round_trip() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    let p = PinnedConfig {
        path: PathBuf::from("/etc/x.conf"),
        label: "x".into(),
        opener_cmd: Some("zellij action edit /etc/x.conf".into()),
        created_at: now_epoch(),
    };
    store.upsert_pinned_config(&p).unwrap();
    let got = store
        .get_pinned_config(Path::new("/etc/x.conf"))
        .unwrap()
        .unwrap();
    assert_eq!(got.opener_cmd.as_deref(), Some("zellij action edit /etc/x.conf"));
}

use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_pinned_config_postcard_roundtrip(
        path in "/etc/[a-z]{1,8}/[a-z0-9_.-]{1,16}",
        label in "[a-zA-Z0-9 _.-]{1,40}",
        with_opener in proptest::bool::ANY,
    ) {
        let p = PinnedConfig {
            path: PathBuf::from(path),
            label,
            opener_cmd: with_opener.then(|| "vim".to_string()),
            created_at: now_epoch(),
        };
        let bytes = postcard::to_allocvec(&p).unwrap();
        let back: PinnedConfig = postcard::from_bytes(&bytes).unwrap();
        prop_assert_eq!(p, back);
    }
}
