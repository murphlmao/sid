//! redb table definitions for sid-store.
//!
//! All tables use `&str` keys and `&[u8]` values (versioned-postcard blobs).
//!
//! # Table layout
//!
//! | Table | Key | Value |
//! |---|---|---|
//! | `settings` | setting key string | versioned-postcard `SettingValue` |
//! | `sessions` | session id string | versioned-postcard `SessionRecord` |
//! | `session_meta` | `"current"` | raw UTF-8 session id bytes |
//! | `widget_state` | `"tab_id\0widget_id"` | raw widget blob bytes |
//! | `workspaces` | absolute path string | versioned-postcard `Workspace` |
//! | `secrets` | secret id string | raw secret bytes |
//! | `themes` | theme name | versioned-postcard `ThemeSpec` |
//! | `keybinds` | profile name | versioned-postcard `KeybindProfile` |
//! | `quick_actions` | action id | versioned-postcard `QuickAction` |
//! | `pinned_configs` | absolute path | versioned-postcard `PinnedConfig` |
//!
//! # Examples
//!
//! ```
//! use redb::TableHandle;
//! use sid_store::schema::{
//!     KEYBINDS, PINNED_CONFIGS, QUICK_ACTIONS, SECRETS, SESSION_META, SESSIONS, SETTINGS,
//!     THEMES, WIDGET_STATE, WORKSPACES,
//! };
//!
//! // The table names are stable constants.
//! assert_eq!(SETTINGS.name(), "settings");
//! assert_eq!(SESSIONS.name(), "sessions");
//! assert_eq!(SESSION_META.name(), "session_meta");
//! assert_eq!(WIDGET_STATE.name(), "widget_state");
//! assert_eq!(WORKSPACES.name(), "workspaces");
//! assert_eq!(SECRETS.name(), "secrets");
//! assert_eq!(THEMES.name(), "themes");
//! assert_eq!(KEYBINDS.name(), "keybinds");
//! assert_eq!(QUICK_ACTIONS.name(), "quick_actions");
//! assert_eq!(PINNED_CONFIGS.name(), "pinned_configs");
//! ```

use redb::TableDefinition;

/// Settings KV table. Key: setting name. Value: raw bytes (settings are stored
/// as-is, not wrapped in the versioned codec, for simplicity).
pub const SETTINGS: TableDefinition<&str, &[u8]> = TableDefinition::new("settings");

/// Session records table. Key: session id. Value: versioned-postcard
/// `SessionRecord`.
pub const SESSIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("sessions");

/// Session metadata: single-row table storing the current session id as raw
/// UTF-8 bytes under the key `"current"`.
pub const SESSION_META: TableDefinition<&str, &[u8]> = TableDefinition::new("session_meta");

/// Widget state table. Key: composite `"{tab_id}\0{widget_id}"`. Value: raw
/// widget blob bytes as returned by `Widget::save_state`.
pub const WIDGET_STATE: TableDefinition<&str, &[u8]> = TableDefinition::new("widget_state");

/// Workspace registry table. Key: absolute path string (the workspace's
/// primary key). Value: versioned-postcard `Workspace`.
///
/// # Examples
///
/// ```
/// use redb::TableHandle;
/// use sid_store::schema::WORKSPACES;
///
/// assert_eq!(WORKSPACES.name(), "workspaces");
/// ```
pub const WORKSPACES: TableDefinition<&str, &[u8]> = TableDefinition::new("workspaces");

/// Secrets table. Key: secret id string (caller-defined, e.g.
/// `"ssh.key.id_ed25519"`). Value: raw secret bytes — no codec wrapping, since
/// the bytes are opaque to the store.
///
/// # Examples
///
/// ```
/// use redb::TableHandle;
/// use sid_store::schema::SECRETS;
///
/// assert_eq!(SECRETS.name(), "secrets");
/// ```
pub const SECRETS: TableDefinition<&str, &[u8]> = TableDefinition::new("secrets");

/// User-saved themes. Key: theme name. Value: versioned-postcard `ThemeSpec`.
///
/// # Examples
///
/// ```
/// use redb::TableHandle;
/// use sid_store::schema::THEMES;
///
/// assert_eq!(THEMES.name(), "themes");
/// ```
pub const THEMES: TableDefinition<&str, &[u8]> = TableDefinition::new("themes");

/// Keybind profiles. Key: profile name. Value: versioned-postcard
/// `KeybindProfile`.
///
/// # Examples
///
/// ```
/// use redb::TableHandle;
/// use sid_store::schema::KEYBINDS;
///
/// assert_eq!(KEYBINDS.name(), "keybinds");
/// ```
pub const KEYBINDS: TableDefinition<&str, &[u8]> = TableDefinition::new("keybinds");

/// Global quick-actions (System tab + Settings tab share this table). Key:
/// action id string. Value: versioned-postcard `QuickAction`.
///
/// # Examples
///
/// ```
/// use redb::TableHandle;
/// use sid_store::schema::QUICK_ACTIONS;
///
/// assert_eq!(QUICK_ACTIONS.name(), "quick_actions");
/// ```
pub const QUICK_ACTIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("quick_actions");

/// Pinned configuration files (Plan 6 / System tab). Key: absolute path string.
/// Value: versioned-postcard [`crate::PinnedConfig`].
///
/// # Examples
///
/// ```
/// use redb::TableHandle;
/// use sid_store::schema::PINNED_CONFIGS;
/// assert_eq!(PINNED_CONFIGS.name(), "pinned_configs");
/// ```
pub const PINNED_CONFIGS: TableDefinition<&str, &[u8]> = TableDefinition::new("pinned_configs");

/// DB connection registry (Plan 4). Key: connection id. Value: versioned-postcard
/// [`crate::DbConnection`].
///
/// # Examples
///
/// ```
/// use redb::TableHandle;
/// use sid_store::schema::DB_CONNECTIONS;
/// assert_eq!(DB_CONNECTIONS.name(), "db_connections");
/// ```
pub const DB_CONNECTIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("db_connections");

/// Per-connection query history (Plan 4). Composite key `(ts_ns, seq)` packed
/// into a big-endian 24-byte buffer (`u128` ts_ns followed by `u64` seq). Value:
/// versioned-postcard [`crate::QueryRecord`].
///
/// # Examples
///
/// ```
/// use redb::TableHandle;
/// use sid_store::schema::QUERY_HISTORY;
/// assert_eq!(QUERY_HISTORY.name(), "query_history");
/// ```
pub const QUERY_HISTORY: TableDefinition<&[u8], &[u8]> = TableDefinition::new("query_history");

#[cfg(test)]
mod tests {
    use redb::TableHandle;

    use super::*;

    #[test]
    fn table_names_are_stable() {
        assert_eq!(SETTINGS.name(), "settings");
        assert_eq!(SESSIONS.name(), "sessions");
        assert_eq!(SESSION_META.name(), "session_meta");
        assert_eq!(WIDGET_STATE.name(), "widget_state");
        assert_eq!(WORKSPACES.name(), "workspaces");
        assert_eq!(SECRETS.name(), "secrets");
        assert_eq!(THEMES.name(), "themes");
        assert_eq!(KEYBINDS.name(), "keybinds");
        assert_eq!(QUICK_ACTIONS.name(), "quick_actions");
        assert_eq!(PINNED_CONFIGS.name(), "pinned_configs");
        assert_eq!(DB_CONNECTIONS.name(), "db_connections");
        assert_eq!(QUERY_HISTORY.name(), "query_history");
    }
}
