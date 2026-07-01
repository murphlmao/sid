//! Versioned postcard codec for redb values.
//!
//! Wire format: `[ version: u8 ][ postcard-encoded payload ]`. Lifted from the sid-poc
//! store (proven, unchanged shape) and adapted to the local [`StoreError`]. Storing a
//! leading version byte lets a value's schema evolve: a decoder can branch on `version`
//! and migrate old layouts forward.

use serde::{Serialize, de::DeserializeOwned};

use crate::error::{Result, StoreError};

/// Encode `value` as `[version][postcard(value)]`.
pub fn encode_versioned<T: Serialize>(version: u8, value: &T) -> Result<Vec<u8>> {
    let body = postcard::to_allocvec(value).map_err(|e| StoreError::Encode(e.to_string()))?;
    let mut out = Vec::with_capacity(1 + body.len());
    out.push(version);
    out.extend_from_slice(&body);
    Ok(out)
}

/// Decode a versioned value, returning `(version, value)`.
pub fn decode_versioned<T: DeserializeOwned>(bytes: &[u8]) -> Result<(u8, T)> {
    let (&version, rest) = bytes.split_first().ok_or_else(|| StoreError::Decode {
        version: 0,
        msg: "empty payload".into(),
    })?;
    let value = postcard::from_bytes(rest).map_err(|e| StoreError::Decode {
        version,
        msg: e.to_string(),
    })?;
    Ok((version, value))
}
