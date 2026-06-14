use sid_store::{OpenStore, RedbStore, SshAuthKind, SshHost, SshHostSource, Store, now_epoch};
use tempfile::tempdir;

fn host(alias: &str, host: &str, user: &str) -> SshHost {
    SshHost {
        alias: alias.into(),
        host: host.into(),
        port: 22,
        user: user.into(),
        identity_file: None,
        source: SshHostSource::Manual,
        last_connected: now_epoch(),
        command_history: Vec::new(),
        last_sftp_path: None,
        auth_kind: SshAuthKind::Agent,
    }
}

#[test]
fn ssh_host_construction() {
    let h = host("jp46-dev", "10.1.40.102", "pi");
    assert_eq!(h.alias, "jp46-dev");
    assert_eq!(h.source, SshHostSource::Manual);
}

#[test]
fn now_epoch_is_positive() {
    assert!(now_epoch() > 0);
}

#[test]
fn opening_store_creates_ssh_hosts_table_without_error() {
    let dir = tempdir().unwrap();
    {
        let _store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    }
    let _store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
}

#[test]
fn upsert_then_list_returns_host() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store.upsert_ssh_host(&host("a", "10.0.0.1", "u")).unwrap();
    let all = store.list_ssh_hosts().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].alias, "a");
}

#[test]
fn get_ssh_host_returns_existing_and_none_for_missing() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store.upsert_ssh_host(&host("a", "h", "u")).unwrap();
    assert!(store.get_ssh_host("a").unwrap().is_some());
    assert!(store.get_ssh_host("missing").unwrap().is_none());
}

#[test]
fn remove_ssh_host_drops_it() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store.upsert_ssh_host(&host("a", "h", "u")).unwrap();
    store.remove_ssh_host("a").unwrap();
    assert!(store.list_ssh_hosts().unwrap().is_empty());
}

#[test]
fn upsert_replaces_existing() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store.upsert_ssh_host(&host("a", "v1", "u")).unwrap();
    store.upsert_ssh_host(&host("a", "v2", "u")).unwrap();
    let found = store.get_ssh_host("a").unwrap().unwrap();
    assert_eq!(found.host, "v2");
}

#[test]
fn remove_nonexistent_is_noop() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store.remove_ssh_host("never-added").unwrap();
}

#[test]
fn list_with_many_hosts_returns_all() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    for i in 0..200 {
        store
            .upsert_ssh_host(&SshHost {
                alias: format!("h{i}"),
                host: format!("10.0.0.{}", i % 256),
                port: 22,
                user: "u".into(),
                identity_file: None,
                source: SshHostSource::Manual,
                last_connected: 0,
                command_history: Vec::new(),
                last_sftp_path: None,
                auth_kind: SshAuthKind::Agent,
            })
            .unwrap();
    }
    assert_eq!(store.list_ssh_hosts().unwrap().len(), 200);
}

#[test]
fn long_command_history_round_trips() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    let h = SshHost {
        alias: "a".into(),
        host: "h".into(),
        port: 22,
        user: "u".into(),
        identity_file: None,
        source: SshHostSource::Manual,
        last_connected: 0,
        command_history: (0..500).map(|i| format!("cmd {i}")).collect(),
        last_sftp_path: None,
        auth_kind: SshAuthKind::Agent,
    };
    store.upsert_ssh_host(&h).unwrap();
    let back = store.get_ssh_host("a").unwrap().unwrap();
    assert_eq!(back.command_history.len(), 500);
}

// ── Schema migration: v1 (no last_sftp_path) → v2 ─────────────────────────────

/// A v1 record written before the `last_sftp_path` field was introduced must
/// decode cleanly, with `last_sftp_path == None`.
#[test]
fn decode_v1_blob_maps_to_v2_with_none_last_sftp_path() {
    use serde::{Deserialize, Serialize};
    use sid_store::{SshHostSource, codec::encode_versioned, decode_ssh_host};

    // Mirror of the pre-v2 wire shape.
    #[derive(Serialize, Deserialize)]
    struct OldSshHost {
        alias: String,
        host: String,
        port: u16,
        user: String,
        identity_file: Option<String>,
        source: SshHostSource,
        last_connected: i64,
        command_history: Vec<String>,
    }

    let old = OldSshHost {
        alias: "legacy".into(),
        host: "h".into(),
        port: 22,
        user: "u".into(),
        identity_file: None,
        source: SshHostSource::Manual,
        last_connected: 17,
        command_history: vec!["echo hi".into()],
    };
    let bytes = encode_versioned(1, &old).unwrap();
    let migrated = decode_ssh_host(&bytes).unwrap();
    assert_eq!(migrated.alias, "legacy");
    assert_eq!(migrated.command_history, vec!["echo hi".to_string()]);
    assert_eq!(migrated.last_sftp_path, None, "v1 → v2 sets None");
}

/// A v2 record written before `auth_kind` was added must decode cleanly,
/// with `auth_kind == Agent` (the migration default).
#[test]
fn decode_v2_blob_maps_to_v3_with_agent_auth_kind() {
    use serde::{Deserialize, Serialize};
    use sid_store::{SshHostSource, codec::encode_versioned, decode_ssh_host};

    // Mirror of the pre-v3 wire shape (v2 had `last_sftp_path` but no `auth_kind`).
    #[derive(Serialize, Deserialize)]
    struct OldSshHostV2 {
        alias: String,
        host: String,
        port: u16,
        user: String,
        identity_file: Option<String>,
        source: SshHostSource,
        last_connected: i64,
        command_history: Vec<String>,
        last_sftp_path: Option<String>,
    }

    let old = OldSshHostV2 {
        alias: "v2-host".into(),
        host: "h".into(),
        port: 22,
        user: "u".into(),
        identity_file: Some("~/.ssh/id_ed25519".into()),
        source: SshHostSource::Manual,
        last_connected: 17,
        command_history: vec!["ls".into()],
        last_sftp_path: Some("/var/log".into()),
    };
    let bytes = encode_versioned(2, &old).unwrap();
    let migrated = decode_ssh_host(&bytes).unwrap();
    assert_eq!(migrated.alias, "v2-host");
    assert_eq!(migrated.last_sftp_path.as_deref(), Some("/var/log"));
    assert_eq!(
        migrated.auth_kind,
        SshAuthKind::Agent,
        "v2 → v3 sets Agent as the most permissive default"
    );
}

/// New (v3) records round-trip through upsert/get with `auth_kind` intact.
#[test]
fn upsert_get_round_trips_auth_kind() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    let mut h = host("a", "h", "u");
    h.auth_kind = SshAuthKind::Key;
    store.upsert_ssh_host(&h).unwrap();
    let back = store.get_ssh_host("a").unwrap().unwrap();
    assert_eq!(back.auth_kind, SshAuthKind::Key);
}

/// New (v2) records round-trip through upsert/get with `last_sftp_path` intact.
#[test]
fn upsert_get_round_trips_last_sftp_path() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    let mut h = host("a", "h", "u");
    h.last_sftp_path = Some("/var/log".into());
    store.upsert_ssh_host(&h).unwrap();
    let back = store.get_ssh_host("a").unwrap().unwrap();
    assert_eq!(back.last_sftp_path.as_deref(), Some("/var/log"));
}

use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_upsert_get_round_trip(
        alias in "[a-zA-Z0-9_-]{1,16}",
        h in "[a-z0-9.]{1,40}",
    ) {
        let dir = tempdir().unwrap();
        let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
        let entry = SshHost {
            alias: alias.clone(),
            host: h.clone(),
            port: 22,
            user: "u".into(),
            identity_file: None,
            source: SshHostSource::Manual,
            last_connected: 0,
            command_history: Vec::new(),
            last_sftp_path: None,
            auth_kind: SshAuthKind::Agent,
        };
        store.upsert_ssh_host(&entry).unwrap();
        let back = store.get_ssh_host(&alias).unwrap().unwrap();
        prop_assert_eq!(entry, back);
    }
}
