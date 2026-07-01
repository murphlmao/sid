//! P2.6 (critical path) — the secret boundary.
//!
//! Committed config carries only an opaque `secret_ref`; the real secret lives behind a
//! `SecretStore`. The two must never cross: no secret material in the TOML, and the ref
//! resolves back to the material only through the secret store.

use sid_secrets::{MemorySecretStore, SecretId, SecretStore};
use sid_store::{AuthMethod, Host, Scope, Store, ViewFilters, WorkspaceId, WorkspaceMeta};

fn setup() -> (tempfile::TempDir, Store, WorkspaceId) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("sid.redb")).unwrap();
    let ws_root = dir.path().join("acme");
    std::fs::create_dir_all(&ws_root).unwrap();
    let id = WorkspaceId::from_root(&ws_root);
    store
        .register_workspace(&WorkspaceMeta {
            id: id.clone(),
            root: ws_root,
            name: "acme".into(),
        })
        .unwrap();
    (dir, store, id)
}

const KEY_MATERIAL: &[u8] = b"-----BEGIN OPENSSH PRIVATE KEY-----\nabc123\n-----END-----";

#[test]
fn committed_config_holds_ref_never_secret() {
    let (_d, store, id) = setup();
    let secrets = MemorySecretStore::new();

    // Real key material goes to the secret store, keyed by an opaque ref.
    let secret_ref = "ssh.prod.key";
    secrets
        .put(&SecretId::new(secret_ref), KEY_MATERIAL)
        .unwrap();

    // The host config carries only the ref.
    let host = Host {
        alias: "prod".into(),
        user: "deploy".into(),
        host: "prod.acme-api.internal".into(),
        port: 22,
        secret_ref: Some(secret_ref.into()),
        auth: AuthMethod::default(),
    };
    store
        .write_host(&host, &Scope::Workspace(id.clone()))
        .unwrap();

    // The committed TOML contains the ref but NONE of the key material.
    let root = store
        .global()
        .get_workspace(id.as_str())
        .unwrap()
        .unwrap()
        .root;
    let toml = std::fs::read_to_string(root.join(".sid").join("config.toml")).unwrap();
    assert!(toml.contains("ssh.prod.key"), "the ref is committed");
    assert!(
        !toml.contains("BEGIN OPENSSH"),
        "the secret must NOT be committed"
    );
    assert!(
        !toml.contains("abc123"),
        "no secret material anywhere in the file"
    );
}

#[test]
fn ref_resolves_to_material_only_via_secret_store() {
    let (_d, store, id) = setup();
    let secrets = MemorySecretStore::new();
    secrets
        .put(&SecretId::new("ssh.prod.key"), KEY_MATERIAL)
        .unwrap();

    store
        .write_host(
            &Host {
                alias: "prod".into(),
                user: "deploy".into(),
                host: "h".into(),
                port: 22,
                secret_ref: Some("ssh.prod.key".into()),
                auth: AuthMethod::default(),
            },
            &Scope::Workspace(id.clone()),
        )
        .unwrap();

    // Read the host back; follow its ref through the secret store to the material.
    let view = store
        .read_hosts(&Scope::Workspace(id), ViewFilters::default())
        .unwrap();
    let r = view[0].item.secret_ref.clone().expect("host keeps its ref");
    let material = secrets.get(&SecretId::new(r)).unwrap().unwrap();
    assert_eq!(material, KEY_MATERIAL);
}

#[test]
fn secret_store_keys_are_refs_not_config() {
    // The secret store is oblivious to config: it only ever holds refs → bytes.
    let secrets = MemorySecretStore::new();
    secrets
        .put(&SecretId::new("db.acme-pg.pw"), b"hunter2")
        .unwrap();
    assert_eq!(
        secrets.list_ids().unwrap(),
        vec![SecretId::new("db.acme-pg.pw")]
    );
    secrets.delete(&SecretId::new("db.acme-pg.pw")).unwrap();
    assert!(
        secrets
            .get(&SecretId::new("db.acme-pg.pw"))
            .unwrap()
            .is_none()
    );
}
