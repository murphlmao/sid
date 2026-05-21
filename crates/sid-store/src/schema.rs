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
//!
//! # Examples
//!
//! ```
//! use redb::TableHandle;
//! use sid_store::schema::{SECRETS, SESSION_META, SESSIONS, SETTINGS, WIDGET_STATE, WORKSPACES};
//!
//! // The table names are stable constants.
//! assert_eq!(SETTINGS.name(), "settings");
//! assert_eq!(SESSIONS.name(), "sessions");
//! assert_eq!(SESSION_META.name(), "session_meta");
//! assert_eq!(WIDGET_STATE.name(), "widget_state");
//! assert_eq!(WORKSPACES.name(), "workspaces");
//! assert_eq!(SECRETS.name(), "secrets");
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
    }
}
