//! Domain-shaped storage trait. `RedbStore` is the v1 implementation.
//! Domain types here; impl details in `redb_impl.rs`.
//!
//! # Examples
//!
//! Opening and using the store (requires a filesystem path — see the
//! integration tests in `crates/sid-store/tests/` for runnable examples).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sid_core::tab::TabId;
use sid_core::widget::WidgetId;
use sid_core::SidError;

pub mod codec;
pub mod redb_impl;
pub mod schema;

pub use redb_impl::RedbStore;

/// Wall-clock instant as nanoseconds since UNIX epoch. Used for ordering.
///
/// # Examples
///
/// ```
/// use sid_store::now_epoch;
/// let t = now_epoch();
/// // Epoch is always a positive value in normal conditions.
/// assert!(t > 0);
/// ```
pub type Epoch = u64;

/// Returns the current wall-clock time as nanoseconds since UNIX epoch.
///
/// Returns `0` only if the system clock is before the UNIX epoch (unlikely in
/// practice; treated as a safe fallback).
///
/// # Examples
///
/// ```
/// use sid_store::now_epoch;
/// let t1 = now_epoch();
/// let t2 = now_epoch();
/// // Time is monotonically non-decreasing.
/// assert!(t2 >= t1);
/// ```
pub fn now_epoch() -> Epoch {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// An opaque settings value stored as raw bytes.
///
/// # Examples
///
/// ```
/// use sid_store::SettingValue;
/// let v = SettingValue(b"cosmos".to_vec());
/// assert_eq!(v.0, b"cosmos");
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingValue(pub Vec<u8>);

/// A persisted session record.
///
/// # Examples
///
/// ```
/// use sid_store::{now_epoch, SessionRecord};
///
/// let r = SessionRecord {
///     id: "sess-1".into(),
///     started_at: now_epoch(),
///     last_active: now_epoch(),
///     ended_at: None,
///     active_tab: None,
///     open_tabs: vec![],
/// };
/// assert_eq!(r.id, "sess-1");
/// assert!(r.ended_at.is_none());
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: String,
    pub started_at: Epoch,
    pub last_active: Epoch,
    pub ended_at: Option<Epoch>,
    pub active_tab: Option<TabId>,
    pub open_tabs: Vec<TabId>,
}

/// Widget UI state blob keyed by `(tab_id, widget_id)`.
///
/// # Examples
///
/// ```
/// use sid_core::tab::TabId;
/// use sid_core::widget::WidgetId;
/// use sid_store::WidgetState;
///
/// let ws = WidgetState {
///     tab_id: TabId::new("workspaces"),
///     widget_id: WidgetId::new("workspaces.root"),
///     blob: vec![1, 2, 3],
/// };
/// assert_eq!(ws.blob.len(), 3);
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WidgetState {
    pub tab_id: TabId,
    pub widget_id: WidgetId,
    pub blob: Vec<u8>,
}

// ─── Workspace domain types ──────────────────────────────────────────────────

/// Re-export `WorkspaceKind` from `sid-core` for consumers of `sid-store`
/// who need the type without a direct `sid-core` dep.
///
/// # Examples
///
/// ```
/// use sid_store::WorkspaceKind;
///
/// let kind = WorkspaceKind::Repo;
/// assert_eq!(kind, WorkspaceKind::Repo);
/// assert_ne!(kind, WorkspaceKind::Umbrella);
/// ```
pub use sid_core::workspace_metadata::WorkspaceKind;

/// A workspace registered in the sid registry.
///
/// Workspaces are keyed by their absolute filesystem path. The `kind` field
/// classifies whether this is a plain git repo, an umbrella, or other.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use sid_store::{Workspace, WorkspaceKind, now_epoch};
///
/// let w = Workspace {
///     path: PathBuf::from("/home/user/vcs/myproject"),
///     name: "myproject".into(),
///     kind: WorkspaceKind::Repo,
///     manifest_hash: 0,
///     last_seen: now_epoch(),
///     parent: None,
/// };
/// assert_eq!(w.name, "myproject");
/// assert!(w.parent.is_none());
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    /// Absolute path. Acts as the primary key.
    pub path: PathBuf,
    /// Human-readable name (usually the directory basename).
    pub name: String,
    /// Classification of this workspace.
    pub kind: WorkspaceKind,
    /// Fast cache-invalidation hint for manifest files (computed via xxhash3).
    /// `0` means "not computed / unknown".
    pub manifest_hash: u64,
    /// Wall-clock nanoseconds when this workspace was last observed on disk.
    pub last_seen: Epoch,
    /// For child workspaces of an umbrella, the parent's absolute path.
    pub parent: Option<PathBuf>,
}

// ─────────────────────────────────────────────────────────────────────────────

/// The domain storage trait. `sid-store` is the only crate that provides an
/// implementation (`RedbStore`); other crates depend on this trait only.
///
/// All methods take `&self` (interior mutability lives inside the impl via
/// `redb`'s transaction model).
///
/// # Examples
///
/// Implementing the trait for a test double:
///
/// ```
/// use std::collections::HashMap;
/// use std::path::Path;
/// use std::sync::Mutex;
/// use sid_core::SidError;
/// use sid_core::tab::TabId;
/// use sid_core::widget::WidgetId;
/// use sid_store::{Epoch, SessionRecord, SettingValue, Store, Workspace, WidgetState};
///
/// struct MemStore {
///     settings: Mutex<HashMap<String, SettingValue>>,
/// }
///
/// impl Store for MemStore {
///     fn get_setting(&self, key: &str) -> Result<Option<SettingValue>, SidError> {
///         Ok(self.settings.lock().unwrap().get(key).cloned())
///     }
///     fn put_setting(&self, key: &str, val: &SettingValue) -> Result<(), SidError> {
///         self.settings.lock().unwrap().insert(key.to_string(), val.clone());
///         Ok(())
///     }
///     fn current_session(&self) -> Result<Option<SessionRecord>, SidError> { Ok(None) }
///     fn upsert_session(&self, _: &SessionRecord) -> Result<(), SidError> { Ok(()) }
///     fn end_session(&self, _: &str, _: Epoch) -> Result<(), SidError> { Ok(()) }
///     fn list_sessions(&self) -> Result<Vec<SessionRecord>, SidError> { Ok(vec![]) }
///     fn save_widget_state(&self, _: &WidgetState) -> Result<(), SidError> { Ok(()) }
///     fn load_widget_state(&self, _: &TabId, _: &WidgetId) -> Result<Option<Vec<u8>>, SidError> { Ok(None) }
///     fn list_workspaces(&self) -> Result<Vec<Workspace>, SidError> { Ok(vec![]) }
///     fn upsert_workspace(&self, _: &Workspace) -> Result<(), SidError> { Ok(()) }
///     fn get_workspace(&self, _: &Path) -> Result<Option<Workspace>, SidError> { Ok(None) }
///     fn remove_workspace(&self, _: &Path) -> Result<(), SidError> { Ok(()) }
/// }
/// ```
pub trait Store: Send + Sync {
    /// Retrieve a setting value by key. Returns `None` if not set.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, SettingValue, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// // Missing key returns None.
    /// assert!(store.get_setting("theme").unwrap().is_none());
    /// // After a put, the value is returned.
    /// store.put_setting("theme", &SettingValue(b"cosmos".to_vec())).unwrap();
    /// assert_eq!(store.get_setting("theme").unwrap().unwrap().0, b"cosmos");
    /// ```
    fn get_setting(&self, key: &str) -> Result<Option<SettingValue>, SidError>;

    /// Persist a setting value. Overwrites any existing value for the key.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, SettingValue, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// store.put_setting("key", &SettingValue(b"v1".to_vec())).unwrap();
    /// // Overwrite with a new value.
    /// store.put_setting("key", &SettingValue(b"v2".to_vec())).unwrap();
    /// assert_eq!(store.get_setting("key").unwrap().unwrap().0, b"v2");
    /// ```
    fn put_setting(&self, key: &str, val: &SettingValue) -> Result<(), SidError>;

    /// Retrieve the most recently active session, if any.
    ///
    /// Returns `None` if no session has ever been upserted.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{now_epoch, OpenStore, RedbStore, SessionRecord, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// assert!(store.current_session().unwrap().is_none());
    ///
    /// let s = SessionRecord {
    ///     id: "s1".into(),
    ///     started_at: now_epoch(),
    ///     last_active: now_epoch(),
    ///     ended_at: None,
    ///     active_tab: None,
    ///     open_tabs: vec![],
    /// };
    /// store.upsert_session(&s).unwrap();
    /// assert_eq!(store.current_session().unwrap().unwrap().id, "s1");
    /// ```
    fn current_session(&self) -> Result<Option<SessionRecord>, SidError>;

    /// Create or update a session record. Also updates the "current" pointer.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{now_epoch, OpenStore, RedbStore, SessionRecord, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// let s = SessionRecord {
    ///     id: "sess".into(),
    ///     started_at: now_epoch(),
    ///     last_active: now_epoch(),
    ///     ended_at: None,
    ///     active_tab: None,
    ///     open_tabs: vec![],
    /// };
    /// store.upsert_session(&s).unwrap();
    /// assert_eq!(store.list_sessions().unwrap().len(), 1);
    /// ```
    fn upsert_session(&self, s: &SessionRecord) -> Result<(), SidError>;

    /// Mark a session as ended at the given epoch timestamp.
    ///
    /// No-op if the session id does not exist.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{now_epoch, OpenStore, RedbStore, SessionRecord, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// let s = SessionRecord {
    ///     id: "s".into(),
    ///     started_at: 1,
    ///     last_active: 2,
    ///     ended_at: None,
    ///     active_tab: None,
    ///     open_tabs: vec![],
    /// };
    /// store.upsert_session(&s).unwrap();
    /// store.end_session("s", 999).unwrap();
    /// let sessions = store.list_sessions().unwrap();
    /// assert_eq!(sessions[0].ended_at, Some(999));
    /// // Calling on a nonexistent id is a no-op.
    /// store.end_session("no-such-id", 0).unwrap();
    /// ```
    fn end_session(&self, id: &str, ended_at: Epoch) -> Result<(), SidError>;

    /// Return all stored sessions.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{now_epoch, OpenStore, RedbStore, SessionRecord, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// assert!(store.list_sessions().unwrap().is_empty());
    ///
    /// for id in &["a", "b", "c"] {
    ///     store.upsert_session(&SessionRecord {
    ///         id: id.to_string(),
    ///         started_at: 0,
    ///         last_active: 0,
    ///         ended_at: None,
    ///         active_tab: None,
    ///         open_tabs: vec![],
    ///     }).unwrap();
    /// }
    /// assert_eq!(store.list_sessions().unwrap().len(), 3);
    /// ```
    fn list_sessions(&self) -> Result<Vec<SessionRecord>, SidError>;

    /// Persist widget UI state blob for the given `(tab_id, widget_id)` pair.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::tab::TabId;
    /// use sid_core::widget::WidgetId;
    /// use sid_store::{OpenStore, RedbStore, Store, WidgetState};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// let state = WidgetState {
    ///     tab_id: TabId::new("workspaces"),
    ///     widget_id: WidgetId::new("workspaces.root"),
    ///     blob: vec![1, 2, 3],
    /// };
    /// store.save_widget_state(&state).unwrap();
    /// ```
    fn save_widget_state(&self, s: &WidgetState) -> Result<(), SidError>;

    /// Load widget UI state blob. Returns `None` if never saved.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::tab::TabId;
    /// use sid_core::widget::WidgetId;
    /// use sid_store::{OpenStore, RedbStore, Store, WidgetState};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// let tab = TabId::new("ssh");
    /// let widget = WidgetId::new("ssh.root");
    /// // No state saved yet.
    /// assert!(store.load_widget_state(&tab, &widget).unwrap().is_none());
    ///
    /// store.save_widget_state(&WidgetState {
    ///     tab_id: tab.clone(),
    ///     widget_id: widget.clone(),
    ///     blob: vec![42, 43],
    /// }).unwrap();
    /// assert_eq!(
    ///     store.load_widget_state(&tab, &widget).unwrap().unwrap(),
    ///     vec![42, 43]
    /// );
    /// ```
    fn load_widget_state(
        &self,
        tab: &TabId,
        widget: &WidgetId,
    ) -> Result<Option<Vec<u8>>, SidError>;

    /// Return all registered workspaces.
    ///
    /// Order is implementation-defined (redb returns keys in lexicographic order).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// assert!(store.list_workspaces().unwrap().is_empty());
    /// ```
    fn list_workspaces(&self) -> Result<Vec<Workspace>, SidError>;

    /// Insert or replace the workspace record keyed by `w.path`.
    ///
    /// If a workspace with the same path already exists it is fully replaced.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use sid_store::{now_epoch, OpenStore, RedbStore, Store, Workspace, WorkspaceKind};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// let w = Workspace {
    ///     path: PathBuf::from("/tmp/myrepo"),
    ///     name: "myrepo".into(),
    ///     kind: WorkspaceKind::Repo,
    ///     manifest_hash: 0,
    ///     last_seen: now_epoch(),
    ///     parent: None,
    /// };
    /// store.upsert_workspace(&w).unwrap();
    /// assert_eq!(store.list_workspaces().unwrap().len(), 1);
    /// ```
    fn upsert_workspace(&self, w: &Workspace) -> Result<(), SidError>;

    /// Retrieve a workspace by its absolute path. Returns `None` if not registered.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use sid_store::{now_epoch, OpenStore, RedbStore, Store, Workspace, WorkspaceKind};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// assert!(store.get_workspace(&PathBuf::from("/missing")).unwrap().is_none());
    /// ```
    fn get_workspace(&self, path: &Path) -> Result<Option<Workspace>, SidError>;

    /// Remove the workspace at `path`. No-op if not registered.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use sid_store::{OpenStore, RedbStore, Store};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    /// // Removing a nonexistent path is always a no-op.
    /// store.remove_workspace(&PathBuf::from("/not-there")).unwrap();
    /// ```
    fn remove_workspace(&self, path: &Path) -> Result<(), SidError>;
}

/// Trait for opening a store from a filesystem path.
///
/// Separate from `Store` so the open path (which creates/migrates the DB) is
/// not confused with the read/write operations.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
/// use sid_store::{OpenStore, RedbStore};
///
/// // Open (or create) the store at a filesystem path.
/// let store = RedbStore::open(Path::new("/tmp/sid-example.redb")).unwrap();
/// ```
pub trait OpenStore {
    /// Open (or create) the store at the given path.
    ///
    /// # Errors
    ///
    /// Returns `SidError::Storage` if the path cannot be created or opened
    /// (e.g. the parent directory does not exist, permissions are denied, or
    /// the file is corrupted).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_store::{OpenStore, RedbStore};
    /// use tempfile::tempdir;
    ///
    /// let dir = tempdir().unwrap();
    /// let path = dir.path().join("sid.redb");
    /// let store = RedbStore::open(&path).unwrap();
    /// // The file is created on disk.
    /// assert!(path.exists());
    /// ```
    fn open(path: &Path) -> Result<Self, SidError>
    where
        Self: Sized;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_epoch_is_positive() {
        assert!(now_epoch() > 0);
    }

    #[test]
    fn now_epoch_is_non_decreasing() {
        let t1 = now_epoch();
        let t2 = now_epoch();
        assert!(t2 >= t1);
    }

    #[test]
    fn setting_value_stores_bytes() {
        let v = SettingValue(b"test".to_vec());
        assert_eq!(v.0, b"test");
    }

    #[test]
    fn setting_value_equality() {
        let a = SettingValue(b"x".to_vec());
        let b = SettingValue(b"x".to_vec());
        let c = SettingValue(b"y".to_vec());
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn session_record_constructor() {
        let r = SessionRecord {
            id: "s1".into(),
            started_at: 100,
            last_active: 200,
            ended_at: None,
            active_tab: None,
            open_tabs: vec![],
        };
        assert_eq!(r.id, "s1");
        assert_eq!(r.started_at, 100);
        assert!(r.ended_at.is_none());
        assert!(r.open_tabs.is_empty());
    }

    #[test]
    fn session_record_with_ended_at() {
        let r = SessionRecord {
            id: "s2".into(),
            started_at: 1,
            last_active: 2,
            ended_at: Some(3),
            active_tab: Some(TabId::new("workspaces")),
            open_tabs: vec![TabId::new("workspaces")],
        };
        assert_eq!(r.ended_at, Some(3));
        assert_eq!(r.active_tab.as_ref().unwrap().as_str(), "workspaces");
    }

    #[test]
    fn widget_state_constructor() {
        let ws = WidgetState {
            tab_id: TabId::new("ssh"),
            widget_id: WidgetId::new("ssh.root"),
            blob: vec![0xDE, 0xAD],
        };
        assert_eq!(ws.tab_id.as_str(), "ssh");
        assert_eq!(ws.widget_id.as_str(), "ssh.root");
        assert_eq!(ws.blob, vec![0xDE, 0xAD]);
    }

    /// Verify the Store trait can be object-safe and implemented by a MemStore.
    #[test]
    fn store_trait_can_be_implemented() {
        use std::collections::HashMap;
        use std::sync::Mutex;

        struct MemStore {
            settings: Mutex<HashMap<String, SettingValue>>,
        }

        impl Store for MemStore {
            fn get_setting(&self, key: &str) -> Result<Option<SettingValue>, SidError> {
                Ok(self.settings.lock().unwrap().get(key).cloned())
            }
            fn put_setting(&self, key: &str, val: &SettingValue) -> Result<(), SidError> {
                self.settings.lock().unwrap().insert(key.to_string(), val.clone());
                Ok(())
            }
            fn current_session(&self) -> Result<Option<SessionRecord>, SidError> {
                Ok(None)
            }
            fn upsert_session(&self, _: &SessionRecord) -> Result<(), SidError> {
                Ok(())
            }
            fn end_session(&self, _: &str, _: Epoch) -> Result<(), SidError> {
                Ok(())
            }
            fn list_sessions(&self) -> Result<Vec<SessionRecord>, SidError> {
                Ok(vec![])
            }
            fn save_widget_state(&self, _: &WidgetState) -> Result<(), SidError> {
                Ok(())
            }
            fn load_widget_state(
                &self,
                _: &TabId,
                _: &WidgetId,
            ) -> Result<Option<Vec<u8>>, SidError> {
                Ok(None)
            }
            fn list_workspaces(&self) -> Result<Vec<Workspace>, SidError> {
                Ok(vec![])
            }
            fn upsert_workspace(&self, _: &Workspace) -> Result<(), SidError> {
                Ok(())
            }
            fn get_workspace(&self, _: &Path) -> Result<Option<Workspace>, SidError> {
                Ok(None)
            }
            fn remove_workspace(&self, _: &Path) -> Result<(), SidError> {
                Ok(())
            }
        }

        let store = MemStore { settings: Mutex::new(HashMap::new()) };
        let key = "foo";
        let val = SettingValue(b"bar".to_vec());
        store.put_setting(key, &val).unwrap();
        let got = store.get_setting(key).unwrap().unwrap();
        assert_eq!(got, val);

        // Verify trait object works
        let _dyn_store: &dyn Store = &store;
    }
}
