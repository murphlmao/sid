//! `sid-db-clients` — DbClient implementations for the Database tab.
//!
//! Hosts:
//! - `PostgresClient` (tokio-postgres)
//! - `SqliteClient` (rusqlite + spawn_blocking)
//! - `lexer` — a hand-rolled SQL lexer for syntax highlighting
//!
//! All public types route through the `sid_core::adapters::db_client` trait
//! surface; nothing in this crate is named directly by `sid-widgets`.

pub mod lexer;
pub mod postgres;
pub mod sqlite;

pub use postgres::PostgresClient;
pub use sqlite::SqliteClient;
