//! Insta snapshot tests for stable serialized formats.
//!
//! These tests fail loudly if any serialized format or schema constant changes.
//! That is their purpose: enforce stability at the byte level so format changes
//! are always intentional and reviewed.

use redb::TableHandle;
use sid_store::codec::encode_versioned;
use sid_store::schema::{SESSION_META, SESSIONS, SETTINGS, WIDGET_STATE};
use sid_store::SessionRecord;

/// A deterministic `SessionRecord` with fixed timestamps for snapshot stability.
fn canonical_session() -> SessionRecord {
    SessionRecord {
        id: "snap-sess-1".into(),
        started_at: 1_700_000_000_000_000_000,
        last_active: 1_700_000_000_100_000_000,
        ended_at: Some(1_700_000_001_000_000_000),
        active_tab: None,
        open_tabs: vec![],
    }
}

/// Snapshot the hex dump of a `SessionRecord` encoded via `encode_versioned`.
///
/// This test will fail if the postcard encoding or struct field layout changes.
/// That is the desired behavior: format changes must be explicit and reviewed.
#[test]
fn snapshot_session_record_postcard_hex() {
    let record = canonical_session();
    let bytes = encode_versioned(1, &record).unwrap();
    // Represent as uppercase hex for readability.
    let hex: String = bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ");
    insta::assert_snapshot!("session_record_postcard_hex", hex);
}

/// Snapshot the JSON representation of the same `SessionRecord`.
///
/// This test ensures the serde JSON serialization is stable. Any rename,
/// reorder, or type change in `SessionRecord` will fail this snapshot.
#[test]
fn snapshot_session_record_json() {
    let record = canonical_session();
    let json = serde_json::to_string_pretty(&record).unwrap();
    insta::assert_snapshot!("session_record_json", json);
}

/// Snapshot all schema table names.
///
/// If a table is renamed or a new table is added, this snapshot fails,
/// requiring a deliberate migration decision.
#[test]
fn snapshot_schema_table_names() {
    let names = [
        SETTINGS.name(),
        SESSIONS.name(),
        SESSION_META.name(),
        WIDGET_STATE.name(),
    ];
    let dump = names.join("\n");
    insta::assert_snapshot!("schema_table_names", dump);
}
