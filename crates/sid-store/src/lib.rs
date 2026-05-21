//! Domain-shaped storage trait. `RedbStore` is the v1 implementation.
//! Domain types here; impl details in `redb_impl.rs`.
//!
//! # Examples
//!
//! Opening and using the store (requires a filesystem path — see the
//! integration tests in `crates/sid-store/tests/` for runnable examples).

use std::path::Path;
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
/// use std::sync::Mutex;
/// use sid_core::SidError;
/// use sid_core::tab::TabId;
/// use sid_core::widget::WidgetId;
/// use sid_store::{Epoch, SessionRecord, SettingValue, Store, WidgetState};
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
/// }
/// ```
pub trait Store: Send + Sync {
    /// Retrieve a setting value by key. Returns `None` if not set.
    fn get_setting(&self, key: &str) -> Result<Option<SettingValue>, SidError>;
    /// Persist a setting value. Overwrites any existing value for the key.
    fn put_setting(&self, key: &str, val: &SettingValue) -> Result<(), SidError>;

    /// Retrieve the most recently active session, if any.
    fn current_session(&self) -> Result<Option<SessionRecord>, SidError>;
    /// Create or update a session record. Also updates the "current" pointer.
    fn upsert_session(&self, s: &SessionRecord) -> Result<(), SidError>;
    /// Mark a session as ended at the given epoch timestamp.
    fn end_session(&self, id: &str, ended_at: Epoch) -> Result<(), SidError>;
    /// Return all stored sessions.
    fn list_sessions(&self) -> Result<Vec<SessionRecord>, SidError>;

    /// Persist widget UI state blob for the given `(tab_id, widget_id)` pair.
    fn save_widget_state(&self, s: &WidgetState) -> Result<(), SidError>;
    /// Load widget UI state blob. Returns `None` if never saved.
    fn load_widget_state(
        &self,
        tab: &TabId,
        widget: &WidgetId,
    ) -> Result<Option<Vec<u8>>, SidError>;
}

/// Trait for opening a store from a filesystem path.
///
/// Separate from `Store` so the open path (which creates/migrates the DB) is
/// not confused with the read/write operations.
pub trait OpenStore {
    /// Open (or create) the store at the given path.
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
