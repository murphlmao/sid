//! Bridge helpers between `sid_store::KeybindProfile` (the postcard wire
//! shape) and `sid_core::KeybindMap` (the in-memory shape).
//!
//! `sid-core` and `sid-store` agree only via [`crate::Store`] — this module is
//! where the two type vocabularies meet.
//!
//! # Examples
//!
//! ```
//! use sid_core::keybind::KeybindMap;
//! use sid_store::keybind_load::{load_keybind_profile, save_keybind_profile};
//! use sid_store::{OpenStore, RedbStore};
//! use tempfile::tempdir;
//!
//! let d = tempdir().unwrap();
//! let store = RedbStore::open(&d.path().join("s.redb")).unwrap();
//! let m = KeybindMap::cosmos_default();
//! save_keybind_profile(&store, "default", &m).unwrap();
//! let loaded = load_keybind_profile(&store, "default").unwrap().unwrap();
//! assert_eq!(
//!     sid_core::keybind_profile::from_map(&loaded),
//!     sid_core::keybind_profile::from_map(&m),
//! );
//! ```

use sid_core::{
    SidError,
    keybind::KeybindMap,
    keybind_profile::{ProfileEntry, from_map, to_map},
};

use crate::{KeybindEntry, KeybindProfile, Store};

/// Load and decode the keybind profile named `name` from `store`. Returns
/// `Ok(None)` if no profile is stored under that name.
///
/// # Errors
///
/// Propagates `SidError::Storage` from the underlying store on read failure.
///
/// # Examples
///
/// ```
/// use sid_store::keybind_load::load_keybind_profile;
/// use sid_store::{OpenStore, RedbStore};
/// use tempfile::tempdir;
///
/// let d = tempdir().unwrap();
/// let store = RedbStore::open(&d.path().join("s.redb")).unwrap();
/// // Missing profile returns None.
/// assert!(load_keybind_profile(&store, "absent").unwrap().is_none());
/// ```
pub fn load_keybind_profile(store: &dyn Store, name: &str) -> Result<Option<KeybindMap>, SidError> {
    let Some(p) = store.get_keybind_profile(name)? else {
        return Ok(None);
    };
    let entries: Vec<ProfileEntry> = p
        .bindings
        .into_iter()
        .map(|e| ProfileEntry {
            chord: e.chord,
            action: e.action,
        })
        .collect();
    Ok(Some(to_map(&entries)))
}

/// Persist `map` under `name` in `store`. Replaces any existing profile with
/// the same name.
///
/// # Errors
///
/// Propagates `SidError::Storage` from the underlying store on write failure.
///
/// # Examples
///
/// ```
/// use sid_core::keybind::KeybindMap;
/// use sid_store::keybind_load::save_keybind_profile;
/// use sid_store::{OpenStore, RedbStore, Store};
/// use tempfile::tempdir;
///
/// let d = tempdir().unwrap();
/// let store = RedbStore::open(&d.path().join("s.redb")).unwrap();
/// save_keybind_profile(&store, "default", &KeybindMap::cosmos_default()).unwrap();
/// assert!(store.get_keybind_profile("default").unwrap().is_some());
/// ```
pub fn save_keybind_profile(
    store: &dyn Store,
    name: &str,
    map: &KeybindMap,
) -> Result<(), SidError> {
    let bindings: Vec<KeybindEntry> = from_map(map)
        .into_iter()
        .map(|e| KeybindEntry {
            chord: e.chord,
            action: e.action,
        })
        .collect();
    store.upsert_keybind_profile(&KeybindProfile {
        name: name.into(),
        bindings,
    })
}
