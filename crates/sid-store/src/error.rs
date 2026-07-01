//! Errors for the layered store.

use thiserror::Error;

/// Anything that can go wrong in the global (redb), workspace (TOML), or composition layers.
#[derive(Debug, Error)]
pub enum StoreError {
    /// A backing-store failure (redb open/txn, filesystem).
    #[error("storage: {0}")]
    Storage(String),
    /// postcard/TOML serialization failed.
    #[error("encode: {0}")]
    Encode(String),
    /// Decoding a stored value failed (version is the leading version byte, 0 if absent).
    #[error("decode (v{version}): {msg}")]
    Decode { version: u8, msg: String },
    /// A stored value carries a version this build does not understand.
    #[error("unsupported version {0}")]
    UnsupportedVersion(u8),
    /// An underlying I/O error.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Convenience alias for store results.
pub type Result<T> = std::result::Result<T, StoreError>;
