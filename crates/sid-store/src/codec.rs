//! Versioned postcard codec for schema evolution.
//!
//! Every persisted blob is prefixed with a 1-byte version number. This allows
//! future schema migrations: callers inspect the version and dispatch to the
//! correct deserialization path.
//!
//! # Wire format
//!
//! ```text
//! [ version: u8 ][ postcard-encoded payload: ... ]
//! ```
//!
//! # Examples
//!
//! ```
//! use serde::{Deserialize, Serialize};
//! use sid_store::codec::{decode_versioned, encode_versioned};
//!
//! #[derive(Debug, Serialize, Deserialize, PartialEq)]
//! struct MyData { x: u32 }
//!
//! let data = MyData { x: 42 };
//! let encoded = encode_versioned(1, &data).unwrap();
//! assert_eq!(encoded[0], 1); // version byte is first
//!
//! let (version, decoded) = decode_versioned::<MyData>(&encoded).unwrap();
//! assert_eq!(version, 1);
//! assert_eq!(decoded, data);
//! ```

use serde::{Deserialize, Serialize};
use sid_core::SidError;

/// Wrap a struct in a 1-byte version prefix + postcard payload.
///
/// The output format is `[version: u8][postcard bytes...]`.
///
/// # Errors
///
/// Returns `SidError::Storage` if postcard serialization fails.
///
/// # Examples
///
/// ```
/// use serde::{Deserialize, Serialize};
/// use sid_store::codec::encode_versioned;
///
/// #[derive(Serialize, Deserialize)]
/// struct Foo { n: u32 }
///
/// let bytes = encode_versioned(1, &Foo { n: 7 }).unwrap();
/// assert_eq!(bytes[0], 1u8);
/// assert!(bytes.len() > 1);
/// ```
pub fn encode_versioned<T: Serialize>(version: u8, value: &T) -> Result<Vec<u8>, SidError> {
    let body = postcard::to_allocvec(value)
        .map_err(|e| SidError::Storage(format!("postcard encode: {e}")))?;
    let mut out = Vec::with_capacity(1 + body.len());
    out.push(version);
    out.extend_from_slice(&body);
    Ok(out)
}

/// Decode a versioned payload. Returns `(version, value)`.
///
/// The `version` byte is returned to the caller, who decides whether to
/// accept it or fall back to a migration path.
///
/// # Errors
///
/// Returns `SidError::Storage` if the slice is empty or the postcard payload
/// cannot be decoded into `T`.
///
/// # Examples
///
/// ```
/// use serde::{Deserialize, Serialize};
/// use sid_store::codec::{decode_versioned, encode_versioned};
///
/// #[derive(Debug, Serialize, Deserialize, PartialEq)]
/// struct Bar { s: String }
///
/// let encoded = encode_versioned(2, &Bar { s: "hello".into() }).unwrap();
/// let (ver, val) = decode_versioned::<Bar>(&encoded).unwrap();
/// assert_eq!(ver, 2);
/// assert_eq!(val.s, "hello");
/// ```
pub fn decode_versioned<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<(u8, T), SidError> {
    let (&v, rest) = bytes
        .split_first()
        .ok_or_else(|| SidError::Storage("empty payload".into()))?;
    let value: T = postcard::from_bytes(rest)
        .map_err(|e| SidError::Storage(format!("postcard decode v{v}: {e}")))?;
    Ok((v, value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct S {
        n: u32,
        s: String,
    }

    #[test]
    fn encode_produces_version_first_byte() {
        let s = S { n: 1, s: "a".into() };
        let b = encode_versioned(7, &s).unwrap();
        assert_eq!(b[0], 7);
    }

    #[test]
    fn decode_empty_is_err() {
        let r: Result<(u8, S), _> = decode_versioned(&[]);
        assert!(r.is_err());
    }

    #[test]
    fn round_trip_preserves_all_fields() {
        let orig = S { n: 9999, s: "codec test".into() };
        let bytes = encode_versioned(1, &orig).unwrap();
        let (v, decoded) = decode_versioned::<S>(&bytes).unwrap();
        assert_eq!(v, 1);
        assert_eq!(decoded, orig);
    }

    #[test]
    fn empty_string_payload_round_trips() {
        let orig = S { n: 0, s: String::new() };
        let bytes = encode_versioned(0, &orig).unwrap();
        let (v, decoded) = decode_versioned::<S>(&bytes).unwrap();
        assert_eq!(v, 0);
        assert_eq!(decoded, orig);
    }
}
