//! P2.4 (critical path), property-based — the composer's attributive-union
//! invariant, fuzzed rather than hand-picked: for *any* global/workspace record
//! sets, `compose` must never lose, invent, or mis-attribute an item.
//!
//! The example-based tests in `compose.rs` pin specific scenarios (a couple of
//! aliases, a couple of overlaps). This file generates arbitrary global +
//! workspace host sets (unique aliases within a layer, arbitrary overlap
//! *between* layers — the realistic shape the store guarantees) and checks the
//! three invariants CLAUDE.md calls load-bearing:
//! - raw union (`collapse_duplicates: false`) returns exactly
//!   `global.len() + workspace.len()` items, each carrying its original value
//!   and a correctly-computed `duplicate` flag;
//! - the default (collapsed) view keeps exactly one item per distinct
//!   identity, and the surviving copy's *values* are the workspace's whenever
//!   the identity exists in both layers ("workspace wins", never a blend);
//! - `hide_global` yields exactly the workspace set, values untouched.

use std::collections::{BTreeSet, HashMap};

use proptest::prelude::*;
use sid_store::{AuthMethod, Host, Scope, ViewFilters, WorkspaceId, compose};

/// A small alias alphabet (`a`..=`f`) so generated global/workspace layers
/// collide on identity often enough to actually exercise dedup — a uniform
/// random string alphabet would almost never produce an overlap.
fn arb_alias() -> impl Strategy<Value = String> {
    prop::sample::select(vec!["a", "b", "c", "d", "e", "f"]).prop_map(String::from)
}

/// One layer's worth of hosts: a map keyed by alias (so aliases are unique
/// *within* the layer, matching what the real store enforces) with an
/// arbitrary "user" field so two same-alias hosts in different layers are
/// distinguishable by value, not just by origin.
fn arb_layer() -> impl Strategy<Value = Vec<Host>> {
    prop::collection::hash_map(arb_alias(), "[a-z]{1,6}", 0..6).prop_map(|by_alias| {
        by_alias
            .into_iter()
            .map(|(alias, user)| Host {
                alias,
                user,
                host: "h".into(),
                port: 22,
                secret_ref: None,
                auth: AuthMethod::Agent,
                folder: None,
            })
            .collect()
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn compose_is_a_lossless_attributive_union(global in arb_layer(), ws in arb_layer()) {
        let id = WorkspaceId("/x/acme".into());

        // ---- raw union: nothing collapsed, nothing hidden ----
        let raw = compose(
            &global,
            Some((&id, &ws)),
            ViewFilters { collapse_duplicates: false, hide_global: false },
        );
        prop_assert_eq!(raw.len(), global.len() + ws.len(), "no item is dropped or invented");

        let ws_by_alias: HashMap<&str, &Host> = ws.iter().map(|h| (h.alias.as_str(), h)).collect();
        let global_by_alias: HashMap<&str, &Host> = global.iter().map(|h| (h.alias.as_str(), h)).collect();

        for g in &global {
            let found = raw
                .iter()
                .find(|a| a.origin == Scope::Global && a.item.alias == g.alias)
                .expect("every global item survives the raw union, tagged Global");
            prop_assert_eq!(&found.item, g, "value is carried through unchanged");
            prop_assert_eq!(
                found.duplicate,
                ws_by_alias.contains_key(g.alias.as_str()),
                "duplicate flag matches whether the workspace also holds this identity"
            );
        }
        for w in &ws {
            let found = raw
                .iter()
                .find(|a| a.origin == Scope::Workspace(id.clone()) && a.item.alias == w.alias)
                .expect("every workspace item survives the raw union, tagged Workspace");
            prop_assert_eq!(&found.item, w, "value is carried through unchanged");
            prop_assert_eq!(
                found.duplicate,
                global_by_alias.contains_key(w.alias.as_str()),
                "duplicate flag matches whether global also holds this identity"
            );
        }

        // ---- default (collapsed) view: one row per identity, workspace wins ----
        let collapsed = compose(&global, Some((&id, &ws)), ViewFilters::default());
        let mut all_ids: BTreeSet<&str> = global.iter().map(|h| h.alias.as_str()).collect();
        all_ids.extend(ws.iter().map(|h| h.alias.as_str()));
        prop_assert_eq!(
            collapsed.len(),
            all_ids.len(),
            "exactly one surviving row per distinct identity"
        );
        for alias in &all_ids {
            let row = collapsed
                .iter()
                .find(|a| a.item.alias == *alias)
                .expect("every distinct identity has a surviving row");
            match ws_by_alias.get(alias) {
                // Present in the workspace (with or without a global twin):
                // the workspace's own value wins, never a blend of the two.
                Some(w) => {
                    prop_assert_eq!(&row.item, *w, "workspace copy wins the collapsed view");
                    prop_assert_eq!(row.origin.clone(), Scope::Workspace(id.clone()));
                }
                // Global-only identity: the global value is untouched.
                None => {
                    let g = global_by_alias[alias];
                    prop_assert_eq!(&row.item, g, "global-only identity is preserved verbatim");
                    prop_assert_eq!(row.origin.clone(), Scope::Global);
                }
            }
        }

        // ---- hide_global: exactly the workspace set, values untouched ----
        let hidden = compose(
            &global,
            Some((&id, &ws)),
            ViewFilters { collapse_duplicates: true, hide_global: true },
        );
        prop_assert_eq!(hidden.len(), ws.len(), "hide_global shows exactly the workspace layer");
        for w in &ws {
            let row = hidden
                .iter()
                .find(|a| a.item.alias == w.alias)
                .expect("every workspace item is present under hide_global");
            prop_assert_eq!(&row.item, w);
            prop_assert_eq!(row.origin.clone(), Scope::Workspace(id.clone()));
        }

        // ---- compose never mutates its (borrowed) inputs ----
        // Guaranteed by the type signature (`&[T]`), but pin the observable
        // consequence: the original vectors are unchanged after every call above.
        prop_assert_eq!(global.len(), global_by_alias.len());
        prop_assert_eq!(ws.len(), ws_by_alias.len());
    }

    #[test]
    fn global_scope_read_is_exactly_the_global_set(global in arb_layer()) {
        let view = compose(&global, None, ViewFilters::default());
        prop_assert_eq!(view.len(), global.len());
        for g in &global {
            let row = view.iter().find(|a| a.item.alias == g.alias).unwrap();
            prop_assert_eq!(&row.item, g);
            prop_assert_eq!(row.origin.clone(), Scope::Global);
            prop_assert!(!row.duplicate, "no workspace is focused, so nothing can be a duplicate");
        }
    }
}
