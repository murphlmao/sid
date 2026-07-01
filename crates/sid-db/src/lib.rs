//! `sid-db` — `DbClient` implementations for the Database tab.
//!
//! Hosts:
//! - `PostgresClient` (`tokio-postgres`)
//! - `SqliteClient` (`rusqlite` + `spawn_blocking`)
//! - `redb_browse::RedbBrowseClient` — the read-only [`sid_core::db::DbKind::Redb`]
//!   pseudo-engine over an opened `sid-store` [`sid_store::GlobalStore`]
//! - `lexer` — a hand-rolled SQL lexer for syntax highlighting
//!
//! All public types route through the `sid_core::db` trait surface; nothing in
//! this crate is named directly by frontend crates. This crate is the one
//! permitted place `tokio-postgres` and `rusqlite` are named (see
//! `CLAUDE.md`'s adapter rule).

pub mod descriptor;
pub mod lexer;
pub mod postgres;
pub mod redb_browse;
pub mod sqlite;

pub use descriptor::{PostgresDescriptor, SqliteDescriptor};
pub use postgres::PostgresClient;
pub use redb_browse::RedbBrowseClient;
pub use sqlite::SqliteClient;
