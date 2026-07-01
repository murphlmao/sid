//! P2.4 (critical path) — attributive composition: union, dedup-default, hide-global,
//! provenance, and the invariant that composing never mutates its inputs.

use sid_store::composer::{ViewFilters, compose};
use sid_store::{AuthMethod, Host, Scope, WorkspaceId};

fn h(alias: &str, user: &str) -> Host {
    Host {
        alias: alias.into(),
        user: user.into(),
        host: "h".into(),
        port: 22,
        secret_ref: None,
        auth: AuthMethod::default(),
    }
}

fn wsid() -> WorkspaceId {
    WorkspaceId("/x/acme".into())
}

#[test]
fn union_keeps_both_copies_when_not_collapsing() {
    let global = vec![h("A", "g"), h("B", "g")];
    let ws = vec![h("B", "w"), h("C", "w")];
    let id = wsid();
    let view = compose(
        &global,
        Some((&id, &ws)),
        ViewFilters {
            collapse_duplicates: false,
            hide_global: false,
        },
    );
    assert_eq!(view.len(), 4, "nothing is dropped");
    let bs: Vec<_> = view.iter().filter(|a| a.item.alias == "B").collect();
    assert_eq!(bs.len(), 2, "both B records coexist");
    assert!(bs.iter().all(|a| a.duplicate), "both flagged as duplicates");
}

#[test]
fn dedup_default_keeps_workspace_copy() {
    let global = vec![h("A", "g"), h("B", "g")];
    let ws = vec![h("B", "w"), h("C", "w")];
    let id = wsid();
    let view = compose(&global, Some((&id, &ws)), ViewFilters::default());

    assert_eq!(view.len(), 3, "the global B is collapsed away");
    assert_eq!(
        view.iter().filter(|a| a.item.alias == "B").count(),
        1,
        "exactly one B survives"
    );
    let b = view.iter().find(|a| a.item.alias == "B").unwrap();
    assert_eq!(
        b.origin,
        Scope::Workspace(id.clone()),
        "workspace copy wins"
    );
    assert_eq!(b.item.user, "w");
    assert!(b.duplicate, "surviving copy still flagged, for badging");
}

#[test]
fn provenance_and_non_duplicate_flags() {
    let global = vec![h("A", "g")];
    let ws = vec![h("C", "w")];
    let id = wsid();
    let view = compose(&global, Some((&id, &ws)), ViewFilters::default());

    let a = view.iter().find(|x| x.item.alias == "A").unwrap();
    let c = view.iter().find(|x| x.item.alias == "C").unwrap();
    assert_eq!(a.origin, Scope::Global);
    assert!(!a.duplicate);
    assert_eq!(c.origin, Scope::Workspace(id));
    assert!(!c.duplicate);
}

#[test]
fn hide_global_yields_workspace_only() {
    let global = vec![h("A", "g"), h("B", "g")];
    let ws = vec![h("B", "w"), h("C", "w")];
    let id = wsid();
    let view = compose(
        &global,
        Some((&id, &ws)),
        ViewFilters {
            collapse_duplicates: true,
            hide_global: true,
        },
    );
    assert_eq!(view.len(), 2);
    assert!(view.iter().all(|a| matches!(a.origin, Scope::Workspace(_))));
}

#[test]
fn global_scope_read_shows_global_only() {
    let global = vec![h("A", "g"), h("B", "g")];
    let view = compose(&global, None, ViewFilters::default());
    assert_eq!(view.len(), 2);
    assert!(
        view.iter()
            .all(|a| a.origin == Scope::Global && !a.duplicate)
    );
}

#[test]
fn compose_never_mutates_inputs() {
    let global = vec![h("A", "g"), h("B", "g")];
    let ws = vec![h("B", "w")];
    let id = wsid();
    let _ = compose(&global, Some((&id, &ws)), ViewFilters::default());
    // The global B "lost" the view, but the stored records are untouched.
    assert_eq!(global.len(), 2);
    assert_eq!(global[1].user, "g");
    assert_eq!(ws.len(), 1);
}

#[test]
fn hide_global_is_a_noop_at_global_scope() {
    let global = vec![h("A", "g"), h("B", "g")];
    // No workspace focused: hide_global has nothing to hide *toward*, so it's ignored
    // rather than yielding a confusing empty view.
    let view = compose(
        &global,
        None,
        ViewFilters {
            collapse_duplicates: true,
            hide_global: true,
        },
    );
    assert_eq!(view.len(), 2);
}
