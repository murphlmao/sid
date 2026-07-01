//! P2.5 (critical path) — the Store facade: scoped writes, composition, promote/demote.

use sid_store::{Host, Scope, Store, StoreError, ViewFilters, WorkspaceId, WorkspaceMeta};

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

fn host(alias: &str, user: &str) -> Host {
    Host {
        alias: alias.into(),
        user: user.into(),
        host: "h".into(),
        port: 22,
        secret_ref: None,
    }
}

#[test]
fn write_lands_in_the_named_layer_only() {
    let (_d, s, id) = setup();
    s.write_host(&host("g1", "u"), &Scope::Global).unwrap();
    s.write_host(&host("w1", "u"), &Scope::Workspace(id.clone()))
        .unwrap();

    let g = s
        .read_hosts(&Scope::Global, ViewFilters::default())
        .unwrap();
    assert_eq!(g.len(), 1, "global scope sees only the global host");
    assert_eq!(g[0].item.alias, "g1");

    let w = s
        .read_hosts(&Scope::Workspace(id), ViewFilters::default())
        .unwrap();
    assert_eq!(w.len(), 2, "workspace scope sees the union");
}

#[test]
fn read_composes_with_dedup_default() {
    let (_d, s, id) = setup();
    s.write_host(&host("dup", "global"), &Scope::Global)
        .unwrap();
    s.write_host(&host("dup", "workspace"), &Scope::Workspace(id.clone()))
        .unwrap();

    let w = s
        .read_hosts(&Scope::Workspace(id), ViewFilters::default())
        .unwrap();
    assert_eq!(w.iter().filter(|a| a.item.alias == "dup").count(), 1);
    let d = w.iter().find(|a| a.item.alias == "dup").unwrap();
    assert_eq!(d.item.user, "workspace", "workspace wins the default view");
}

#[test]
fn promote_moves_workspace_host_to_global() {
    let (_d, s, id) = setup();
    s.write_host(&host("h", "u"), &Scope::Workspace(id.clone()))
        .unwrap();
    s.promote_host("h", &id).unwrap();

    assert!(s.global().get_host("h").unwrap().is_some(), "now global");
    // Without collapsing, the only "h" is the global copy (removed from the workspace).
    let w = s
        .read_hosts(
            &Scope::Workspace(id),
            ViewFilters {
                collapse_duplicates: false,
                hide_global: false,
            },
        )
        .unwrap();
    let hs: Vec<_> = w.iter().filter(|a| a.item.alias == "h").collect();
    assert_eq!(hs.len(), 1);
    assert_eq!(hs[0].origin, Scope::Global);
}

#[test]
fn demote_moves_global_host_to_workspace() {
    let (_d, s, id) = setup();
    s.write_host(&host("h", "u"), &Scope::Global).unwrap();
    s.demote_host("h", &id).unwrap();

    assert!(
        s.global().get_host("h").unwrap().is_none(),
        "gone from global"
    );
    let w = s
        .read_hosts(&Scope::Workspace(id.clone()), ViewFilters::default())
        .unwrap();
    let hh = w.iter().find(|a| a.item.alias == "h").unwrap();
    assert_eq!(hh.origin, Scope::Workspace(id));
}

#[test]
fn writing_to_an_unregistered_workspace_errors() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&dir.path().join("sid.redb")).unwrap();
    let ghost = WorkspaceId("/nonexistent".into());
    assert!(
        s.write_host(&host("x", "u"), &Scope::Workspace(ghost))
            .is_err()
    );
}

#[test]
fn promote_refuses_when_global_already_has_the_alias() {
    let (_d, s, id) = setup();
    // A legitimate cross-layer duplicate: same alias in both layers, different values.
    s.write_host(&host("prod", "global"), &Scope::Global)
        .unwrap();
    s.write_host(&host("prod", "workspace"), &Scope::Workspace(id.clone()))
        .unwrap();

    // Promote must refuse rather than destroy the global copy.
    assert!(matches!(
        s.promote_host("prod", &id),
        Err(StoreError::Conflict(_))
    ));

    // Nothing lost: both copies still exist.
    assert_eq!(s.global().get_host("prod").unwrap().unwrap().user, "global");
    let w = s
        .read_hosts(
            &Scope::Workspace(id),
            ViewFilters {
                collapse_duplicates: false,
                hide_global: false,
            },
        )
        .unwrap();
    assert_eq!(w.iter().filter(|a| a.item.alias == "prod").count(), 2);
}

#[test]
fn demote_refuses_when_workspace_already_has_the_alias() {
    let (_d, s, id) = setup();
    s.write_host(&host("prod", "global"), &Scope::Global)
        .unwrap();
    s.write_host(&host("prod", "workspace"), &Scope::Workspace(id.clone()))
        .unwrap();

    assert!(matches!(
        s.demote_host("prod", &id),
        Err(StoreError::Conflict(_))
    ));

    // Workspace copy is intact (the "workspace wins" values are preserved).
    let w = s
        .read_hosts(&Scope::Workspace(id), ViewFilters::default())
        .unwrap();
    assert_eq!(
        w.iter().find(|a| a.item.alias == "prod").unwrap().item.user,
        "workspace"
    );
}
