//! Composition — the attributive union that *never overrides*.
//!
//! A read merges the always-present global layer with (optionally) a focused workspace
//! layer. Every item is returned, tagged with its [`Scope`] origin; nothing is shadowed
//! at the storage level. Two view filters sit on top:
//! - `collapse_duplicates` (default **on**): when the same [`Identity`] exists in both
//!   layers, show only the **workspace** copy (workspace-primary);
//! - `hide_global` (default **off**): drop all global-origin items (workspace-only view).
//!
//! These are *view* choices — the inputs are borrowed and never mutated.

use std::collections::HashSet;

use crate::entities::Identity;
use crate::scope::{Attributed, Scope, WorkspaceId};

/// The two view checkboxes over a lossless store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewFilters {
    /// Collapse same-identity duplicates, keeping the workspace copy.
    pub collapse_duplicates: bool,
    /// Hide all global-origin items — a workspace-only view.
    pub hide_global: bool,
}

impl Default for ViewFilters {
    fn default() -> Self {
        Self {
            collapse_duplicates: true,
            hide_global: false,
        }
    }
}

/// Compose the global layer with an optional focused workspace layer.
///
/// `workspace = None` is a `Global`-scope read (global items only). Returns clones tagged
/// by origin; the inputs are never mutated. A returned item's `duplicate` flag is `true`
/// whenever the *other* layer also holds a same-identity item — even after a collapse, so
/// the surviving workspace copy can still be badged as "also in global".
pub fn compose<T: Identity + Clone>(
    global: &[T],
    workspace: Option<(&WorkspaceId, &[T])>,
    filters: ViewFilters,
) -> Vec<Attributed<T>> {
    let ws_items: &[T] = workspace.map(|(_, items)| items).unwrap_or(&[]);
    let ws_ids: HashSet<&str> = ws_items.iter().map(Identity::identity).collect();
    let global_ids: HashSet<&str> = global.iter().map(Identity::identity).collect();

    let mut out = Vec::new();

    if !filters.hide_global {
        for g in global {
            let duplicate = ws_ids.contains(g.identity());
            if duplicate && filters.collapse_duplicates {
                // Workspace is primary: drop the global twin from the *view* only.
                continue;
            }
            out.push(Attributed {
                item: g.clone(),
                origin: Scope::Global,
                duplicate,
            });
        }
    }

    if let Some((id, items)) = workspace {
        for w in items {
            let duplicate = global_ids.contains(w.identity());
            out.push(Attributed {
                item: w.clone(),
                origin: Scope::Workspace(id.clone()),
                duplicate,
            });
        }
    }

    out
}
