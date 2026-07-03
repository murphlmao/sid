//! `sid-store` — the layered, attributive store.
//!
//! Two layers behind one API:
//! - a machine-local **global** layer (redb), always present;
//! - a per-workspace layer, a committed **`.sid/config.toml`** that travels with the repo.
//!
//! Composition is an **attributive union** — never override. A read returns items from
//! both layers, each tagged by origin ([`Attributed`]); duplicate-collapse
//! (workspace-primary) and hide-global are *view filters* over a lossless store. Secrets
//! never live here — committed config holds only an opaque `secret_ref` into the OS
//! keyring.
//!
//! Design source of truth: `docs/design/2026-06-27-store-schema.html`.

pub mod codec;
pub mod composer;
pub mod entities;
pub mod error;
pub mod global;
pub mod scope;
pub mod store;
pub mod workspace;

pub use composer::{ViewFilters, compose};
pub use entities::{
    AuthMethod, DbConnection, DefaultScope, Host, Identity, PanelSide, QuickAction, Settings,
};
pub use error::{Result, StoreError};
pub use global::GlobalStore;
pub use scope::{Attributed, Scope, WorkspaceId, WorkspaceMeta};
pub use store::Store;
pub use workspace::{WorkspaceConfig, WorkspaceStore};
