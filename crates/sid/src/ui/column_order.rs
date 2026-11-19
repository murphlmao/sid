//! The column order a user dragged a table into, kept across restarts
//! (`Settings::column_order`: table id -> column keys).
//!
//! # Table ids
//!
//! `<tab>.<table>[.<variant>]`: the tab's `Tab::id()` slug, then the table, lowercase and
//! dot-separated. Every id lives here as a constant; a table never spells its own.
//! Renaming one orphans the orders users saved under it (harmless, an unknown id is never
//! read, but their layout is gone), so don't. Kubernetes tables carry the resource kind
//! as the variant (`network.kubernetes.pods`), so a future kind gets its own id.
//!
//! Not persisted: the Database results grid (its columns belong to a query).
//!
//! # The flow
//!
//! `AppState::new` caches `Settings::column_order`, because tables are built lazily from
//! `render`, which must not read the store. A table's builder calls
//! [`AppState::persist_column_order`] once on its delegate's `FillColumns`, before
//! `TableState::new`. That applies the saved order and subscribes the save to
//! `FillColumns::on_reorder`, which reports each drop after the store has followed it.
//! An entry exists only while a table is out of its declared order: dragging it back
//! removes it. Saved keys a table no longer has are ignored, and new columns follow in
//! declaration order (`FillColumns::apply_order`).

use std::collections::BTreeMap;

use gpui::Context;
use sid_store::Store;
use sid_ui::FillColumns;

use crate::app::AppState;

/// The System tab's process table.
pub(crate) const SYSTEM_PROCESSES: &str = "system.processes";
/// The System tab's Services sub-view (both systemd scopes share one order).
pub(crate) const SYSTEM_SERVICES: &str = "system.services";
/// The Network tab's Ports sub-view.
pub(crate) const NETWORK_PORTS: &str = "network.ports";
/// The Network tab's Connections sub-view.
pub(crate) const NETWORK_CONNECTIONS: &str = "network.connections";
/// The Network tab's Docker containers.
pub(crate) const NETWORK_DOCKER: &str = "network.docker";
/// The Network tab's Kubernetes pods (every context shares one order).
pub(crate) const NETWORK_KUBERNETES_PODS: &str = "network.kubernetes.pods";
/// The Workspaces tab's umbrella fleet table.
pub(crate) const WORKSPACES_FLEET: &str = "workspaces.fleet";

/// Record table `id`'s order in `map`; `None` (its declared order) removes the entry.
fn record(map: &mut BTreeMap<String, Vec<String>>, id: &str, order: Option<Vec<String>>) {
    match order {
        Some(order) => {
            map.insert(id.to_string(), order);
        }
        None => {
            map.remove(id);
        }
    }
}

/// Write table `id`'s order to the store (read-modify-write, like every other setting).
pub(crate) fn store_order(
    store: &Store,
    id: &str,
    order: Option<Vec<String>>,
) -> sid_store::Result<()> {
    let mut settings = store.settings()?;
    record(&mut settings.column_order, id, order);
    store.set_settings(&settings)
}

impl AppState {
    /// Give table `id` its saved column order and save every order the user drags it
    /// into. Call once, on the delegate's columns, before `TableState::new`.
    pub(crate) fn persist_column_order(
        &self,
        id: &'static str,
        columns: &mut FillColumns,
        cx: &mut Context<Self>,
    ) {
        if let Some(saved) = self.column_order.get(id) {
            columns.apply_order(saved);
        }
        let app = cx.weak_entity();
        columns.on_reorder(move |order, cx| {
            let _ = app.update(cx, |app, _| app.save_column_order(id, order));
        });
    }

    fn save_column_order(&mut self, id: &str, order: Option<Vec<String>>) {
        record(&mut self.column_order, id, order.clone());
        if let Err(e) = store_order(&self.store, id, order) {
            eprintln!("sid: could not save the {id} column order: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use sid_ui::ColumnWidth;
    use sid_ui::component::Column;

    use super::*;

    fn columns() -> FillColumns {
        FillColumns::new(["a", "b", "c"].map(|k| (Column::new(k, k), ColumnWidth::Fixed(80.))))
            .reorderable()
    }

    #[test]
    fn an_order_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.redb");
        let store = Store::open(&path).expect("store");
        let mut dragged = columns();
        dragged.apply_order(&["c".into(), "a".into()]);
        store_order(&store, SYSTEM_PROCESSES, dragged.saved_order()).unwrap();
        drop(store);

        let store = Store::open(&path).expect("reopen");
        let cached = store.settings().unwrap().column_order;
        let mut rebuilt = columns();
        rebuilt.apply_order(&cached[SYSTEM_PROCESSES]);
        assert_eq!(rebuilt.order(), ["c", "a", "b"]);
    }

    #[test]
    fn a_table_back_at_its_default_order_stores_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("store.redb")).expect("store");
        store_order(&store, WORKSPACES_FLEET, Some(vec!["b".into()])).unwrap();
        store_order(&store, SYSTEM_PROCESSES, Some(vec!["c".into()])).unwrap();
        store_order(&store, WORKSPACES_FLEET, columns().saved_order()).unwrap();
        let saved = store.settings().unwrap().column_order;
        assert!(!saved.contains_key(WORKSPACES_FLEET), "{saved:?}");
        assert!(
            saved.contains_key(SYSTEM_PROCESSES),
            "other tables untouched"
        );
    }

    #[test]
    fn a_stale_saved_order_is_read_leniently() {
        // Saved under an older build: a column since removed, and one since added.
        let mut cols = columns();
        cols.apply_order(&["gone".into(), "b".into(), "a".into()]);
        assert_eq!(cols.order(), ["b", "a", "c"]);
    }
}
