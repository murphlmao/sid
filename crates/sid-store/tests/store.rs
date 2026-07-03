//! P2.5 (critical path) — the Store facade: scoped writes, composition, promote/demote.

use sid_core::db::DbKind;
use sid_store::{
    AuthMethod, DbConnection, Host, Scope, Store, StoreError, ViewFilters, WorkspaceId,
    WorkspaceMeta,
};

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
        auth: AuthMethod::default(),
        folder: None,
    }
}

fn connection(id: &str, name: &str) -> DbConnection {
    DbConnection {
        id: id.into(),
        dsn: "postgres://x".into(),
        secret_ref: None,
        kind: DbKind::Postgres,
        name: name.into(),
        folder: None,
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

#[test]
fn delete_from_workspace_leaves_global_intact_and_unshadows_it() {
    let (_d, s, id) = setup();
    // A legitimate cross-layer duplicate: workspace shadows global in the collapsed view.
    s.write_host(&host("dup", "global"), &Scope::Global)
        .unwrap();
    s.write_host(&host("dup", "workspace"), &Scope::Workspace(id.clone()))
        .unwrap();

    assert!(
        s.delete_host("dup", &Scope::Workspace(id.clone())).unwrap(),
        "deleting the workspace copy reports it was present"
    );

    // Global copy untouched; deleting the workspace copy un-shadows it in the view.
    assert_eq!(s.global().get_host("dup").unwrap().unwrap().user, "global");
    let w = s
        .read_hosts(&Scope::Workspace(id), ViewFilters::default())
        .unwrap();
    let d = w.iter().find(|a| a.item.alias == "dup").unwrap();
    assert_eq!(d.origin, Scope::Global, "only the global copy remains");
    assert_eq!(d.item.user, "global");
}

#[test]
fn delete_from_global_leaves_workspace_intact() {
    let (_d, s, id) = setup();
    s.write_host(&host("dup", "global"), &Scope::Global)
        .unwrap();
    s.write_host(&host("dup", "workspace"), &Scope::Workspace(id.clone()))
        .unwrap();

    assert!(s.delete_host("dup", &Scope::Global).unwrap());

    // Global copy gone; workspace copy intact.
    assert!(s.global().get_host("dup").unwrap().is_none());
    let w = s
        .read_hosts(
            &Scope::Workspace(id.clone()),
            ViewFilters {
                collapse_duplicates: false,
                hide_global: false,
            },
        )
        .unwrap();
    let hs: Vec<_> = w.iter().filter(|a| a.item.alias == "dup").collect();
    assert_eq!(hs.len(), 1);
    assert_eq!(hs[0].origin, Scope::Workspace(id));
    assert_eq!(hs[0].item.user, "workspace");
}

#[test]
fn delete_missing_alias_is_ok_false() {
    let (_d, s, id) = setup();
    assert!(
        !s.delete_host("nope", &Scope::Global).unwrap(),
        "deleting an absent global host is Ok(false)"
    );
    assert!(
        !s.delete_host("nope", &Scope::Workspace(id)).unwrap(),
        "deleting an absent workspace host is Ok(false)"
    );
}

#[test]
fn delete_from_unregistered_workspace_errors() {
    let dir = tempfile::tempdir().unwrap();
    let s = Store::open(&dir.path().join("sid.redb")).unwrap();
    let ghost = WorkspaceId("/nonexistent".into());
    assert!(s.delete_host("x", &Scope::Workspace(ghost)).is_err());
}

// ---- connections (structural mirror of the host facade above) ----

#[test]
fn connection_write_lands_in_the_named_layer_only() {
    let (_d, s, id) = setup();
    s.write_connection(&connection("g1", "Global One"), &Scope::Global)
        .unwrap();
    s.write_connection(
        &connection("w1", "Workspace One"),
        &Scope::Workspace(id.clone()),
    )
    .unwrap();

    let g = s
        .read_connections(&Scope::Global, ViewFilters::default())
        .unwrap();
    assert_eq!(g.len(), 1, "global scope sees only the global connection");
    assert_eq!(g[0].item.id, "g1");

    let w = s
        .read_connections(&Scope::Workspace(id), ViewFilters::default())
        .unwrap();
    assert_eq!(w.len(), 2, "workspace scope sees the union");
}

#[test]
fn connection_read_composes_with_dedup_default() {
    let (_d, s, id) = setup();
    s.write_connection(&connection("dup", "Global"), &Scope::Global)
        .unwrap();
    s.write_connection(
        &connection("dup", "Workspace"),
        &Scope::Workspace(id.clone()),
    )
    .unwrap();

    let w = s
        .read_connections(&Scope::Workspace(id), ViewFilters::default())
        .unwrap();
    assert_eq!(w.iter().filter(|a| a.item.id == "dup").count(), 1);
    let d = w.iter().find(|a| a.item.id == "dup").unwrap();
    assert_eq!(
        d.item.name, "Workspace",
        "workspace copy wins the default view"
    );
}

#[test]
fn connection_delete_from_workspace_leaves_global_intact_and_unshadows_it() {
    let (_d, s, id) = setup();
    s.write_connection(&connection("dup", "Global"), &Scope::Global)
        .unwrap();
    s.write_connection(
        &connection("dup", "Workspace"),
        &Scope::Workspace(id.clone()),
    )
    .unwrap();

    assert!(
        s.delete_connection("dup", &Scope::Workspace(id.clone()))
            .unwrap(),
        "deleting the workspace copy reports it was present"
    );

    let w = s
        .read_connections(&Scope::Workspace(id), ViewFilters::default())
        .unwrap();
    let d = w.iter().find(|a| a.item.id == "dup").unwrap();
    assert_eq!(d.origin, Scope::Global, "only the global copy remains");
    assert_eq!(d.item.name, "Global");
}

#[test]
fn connection_promote_moves_workspace_to_global() {
    let (_d, s, id) = setup();
    s.write_connection(&connection("c", "Mine"), &Scope::Workspace(id.clone()))
        .unwrap();
    s.promote_connection("c", &id).unwrap();

    assert!(
        s.global().get_connection("c").unwrap().is_some(),
        "now global"
    );
    let w = s
        .read_connections(
            &Scope::Workspace(id),
            ViewFilters {
                collapse_duplicates: false,
                hide_global: false,
            },
        )
        .unwrap();
    let cs: Vec<_> = w.iter().filter(|a| a.item.id == "c").collect();
    assert_eq!(cs.len(), 1);
    assert_eq!(cs[0].origin, Scope::Global);
}

#[test]
fn connection_demote_moves_global_to_workspace() {
    let (_d, s, id) = setup();
    s.write_connection(&connection("c", "Mine"), &Scope::Global)
        .unwrap();
    s.demote_connection("c", &id).unwrap();

    assert!(
        s.global().get_connection("c").unwrap().is_none(),
        "gone from global"
    );
    let w = s
        .read_connections(&Scope::Workspace(id.clone()), ViewFilters::default())
        .unwrap();
    let cc = w.iter().find(|a| a.item.id == "c").unwrap();
    assert_eq!(cc.origin, Scope::Workspace(id));
}

#[test]
fn connection_promote_refuses_when_global_already_has_the_id() {
    let (_d, s, id) = setup();
    s.write_connection(&connection("prod", "Global"), &Scope::Global)
        .unwrap();
    s.write_connection(
        &connection("prod", "Workspace"),
        &Scope::Workspace(id.clone()),
    )
    .unwrap();

    assert!(matches!(
        s.promote_connection("prod", &id),
        Err(StoreError::Conflict(_))
    ));

    assert_eq!(
        s.global().get_connection("prod").unwrap().unwrap().name,
        "Global"
    );
}

#[test]
fn connection_demote_refuses_when_workspace_already_has_the_id() {
    let (_d, s, id) = setup();
    s.write_connection(&connection("prod", "Global"), &Scope::Global)
        .unwrap();
    s.write_connection(
        &connection("prod", "Workspace"),
        &Scope::Workspace(id.clone()),
    )
    .unwrap();

    assert!(matches!(
        s.demote_connection("prod", &id),
        Err(StoreError::Conflict(_))
    ));

    let w = s
        .read_connections(&Scope::Workspace(id), ViewFilters::default())
        .unwrap();
    assert_eq!(
        w.iter().find(|a| a.item.id == "prod").unwrap().item.name,
        "Workspace"
    );
}

// ---- rename + folder mutators ----

#[test]
fn rename_host_preserves_auth_secret_ref_and_folder() {
    let (_d, s, _id) = setup();
    let h = Host {
        alias: "old".into(),
        user: "u".into(),
        host: "h".into(),
        port: 22,
        secret_ref: Some("ssh.old.key".into()),
        auth: AuthMethod::Key { path: "/k".into() },
        folder: Some("prod".into()),
    };
    s.write_host(&h, &Scope::Global).unwrap();

    s.rename_host(&Scope::Global, "old", "new").unwrap();

    assert!(
        s.global().get_host("old").unwrap().is_none(),
        "old alias no longer present"
    );
    let renamed = s.global().get_host("new").unwrap().unwrap();
    assert_eq!(renamed.user, "u");
    assert_eq!(renamed.secret_ref.as_deref(), Some("ssh.old.key"));
    assert_eq!(renamed.auth, AuthMethod::Key { path: "/k".into() });
    assert_eq!(renamed.folder.as_deref(), Some("prod"));
}

#[test]
fn rename_host_refuses_when_target_alias_exists_in_same_layer() {
    let (_d, s, _id) = setup();
    s.write_host(&host("a", "u1"), &Scope::Global).unwrap();
    s.write_host(&host("b", "u2"), &Scope::Global).unwrap();

    assert!(matches!(
        s.rename_host(&Scope::Global, "a", "b"),
        Err(StoreError::Conflict(_))
    ));
    // Nothing lost or clobbered: both records survive, unchanged.
    assert_eq!(s.global().get_host("a").unwrap().unwrap().user, "u1");
    assert_eq!(s.global().get_host("b").unwrap().unwrap().user, "u2");
}

#[test]
fn rename_host_refuses_a_noop_rename() {
    let (_d, s, _id) = setup();
    s.write_host(&host("a", "u"), &Scope::Global).unwrap();
    assert!(matches!(
        s.rename_host(&Scope::Global, "a", "a"),
        Err(StoreError::Conflict(_))
    ));
}

#[test]
fn rename_host_errors_when_alias_missing_in_scope() {
    let (_d, s, id) = setup();
    assert!(s.rename_host(&Scope::Global, "nope", "new").is_err());
    assert!(s.rename_host(&Scope::Workspace(id), "nope", "new").is_err());
}

#[test]
fn rename_host_targets_only_its_own_layer() {
    let (_d, s, id) = setup();
    // A legitimate cross-layer duplicate: same alias, different values.
    s.write_host(&host("dup", "global"), &Scope::Global)
        .unwrap();
    s.write_host(&host("dup", "workspace"), &Scope::Workspace(id.clone()))
        .unwrap();

    s.rename_host(&Scope::Workspace(id.clone()), "dup", "dup2")
        .unwrap();

    // The workspace copy renamed...
    let w = s
        .read_hosts(
            &Scope::Workspace(id.clone()),
            ViewFilters {
                collapse_duplicates: false,
                hide_global: false,
            },
        )
        .unwrap();
    assert!(
        w.iter()
            .any(|a| a.item.alias == "dup2" && a.origin == Scope::Workspace(id.clone()))
    );
    // ...but the global "dup" is completely untouched — attributive, not shared identity.
    assert_eq!(s.global().get_host("dup").unwrap().unwrap().user, "global");
}

#[test]
fn rename_connection_updates_name_in_place() {
    let (_d, s, _id) = setup();
    s.write_connection(&connection("c1", "Old Name"), &Scope::Global)
        .unwrap();

    s.rename_connection(&Scope::Global, "c1", "New Name")
        .unwrap();

    let got = s.global().get_connection("c1").unwrap().unwrap();
    assert_eq!(got.id, "c1", "identity is unchanged");
    assert_eq!(got.name, "New Name");
}

#[test]
fn rename_connection_errors_when_id_missing_in_scope() {
    let (_d, s, id) = setup();
    assert!(s.rename_connection(&Scope::Global, "nope", "x").is_err());
    assert!(
        s.rename_connection(&Scope::Workspace(id), "nope", "x")
            .is_err()
    );
}

#[test]
fn rename_connection_targets_only_its_own_layer() {
    let (_d, s, id) = setup();
    s.write_connection(&connection("dup", "Global"), &Scope::Global)
        .unwrap();
    s.write_connection(
        &connection("dup", "Workspace"),
        &Scope::Workspace(id.clone()),
    )
    .unwrap();

    s.rename_connection(&Scope::Workspace(id.clone()), "dup", "Renamed")
        .unwrap();

    assert_eq!(
        s.global().get_connection("dup").unwrap().unwrap().name,
        "Global",
        "the global copy is untouched"
    );
    let w = s
        .read_connections(&Scope::Workspace(id), ViewFilters::default())
        .unwrap();
    assert_eq!(
        w.iter().find(|a| a.item.id == "dup").unwrap().item.name,
        "Renamed"
    );
}

#[test]
fn folder_set_and_clear_round_trips_through_redb_and_toml() {
    let (_d, s, id) = setup();
    s.write_host(&host("g", "u"), &Scope::Global).unwrap();
    s.write_host(&host("w", "u"), &Scope::Workspace(id.clone()))
        .unwrap();
    s.write_connection(&connection("c", "C"), &Scope::Workspace(id.clone()))
        .unwrap();

    // Set: global (redb) and workspace (TOML) both take the folder.
    s.set_host_folder(&Scope::Global, "g", Some("prod".into()))
        .unwrap();
    s.set_host_folder(&Scope::Workspace(id.clone()), "w", Some("staging".into()))
        .unwrap();
    s.set_connection_folder(&Scope::Workspace(id.clone()), "c", Some("analytics".into()))
        .unwrap();

    assert_eq!(
        s.global().get_host("g").unwrap().unwrap().folder.as_deref(),
        Some("prod")
    );
    let root = s.global().get_workspace(id.as_str()).unwrap().unwrap().root;
    let toml = std::fs::read_to_string(root.join(".sid").join("config.toml")).unwrap();
    assert!(toml.contains("folder = \"staging\""));
    assert!(toml.contains("folder = \"analytics\""));

    // Clear: folder goes back to `None`, both in redb and in the committed TOML — an
    // unset folder must not linger in the git-diffable file.
    s.set_host_folder(&Scope::Global, "g", None).unwrap();
    s.set_host_folder(&Scope::Workspace(id.clone()), "w", None)
        .unwrap();
    s.set_connection_folder(&Scope::Workspace(id.clone()), "c", None)
        .unwrap();

    assert_eq!(s.global().get_host("g").unwrap().unwrap().folder, None);
    let toml_after = std::fs::read_to_string(root.join(".sid").join("config.toml")).unwrap();
    assert!(
        !toml_after.contains("folder"),
        "an unset folder must not appear in the committed TOML"
    );
}

#[test]
fn set_folder_errors_when_record_missing_in_scope() {
    let (_d, s, id) = setup();
    assert!(
        s.set_host_folder(&Scope::Global, "nope", Some("x".into()))
            .is_err()
    );
    assert!(
        s.set_connection_folder(&Scope::Workspace(id), "nope", Some("x".into()))
            .is_err()
    );
}
