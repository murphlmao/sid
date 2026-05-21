//! Read/write the tiny `~/.config/sid/sid.toml` file.
//!
//! The *only* setting that lives in this file is `db_path_override` — every
//! other knob lives in the redb database. Keeping the surface this small means
//! the file is genuinely optional: if it does not exist, sid uses the default
//! XDG location for `sid.redb`.
//!
//! # Examples
//!
//! Round-trip the config through a tempdir.
//!
//! ```
//! use std::path::PathBuf;
//! use sid_store::sid_toml::{read_sid_toml, write_sid_toml, SidToml};
//! use tempfile::tempdir;
//!
//! let dir = tempdir().unwrap();
//! let path = dir.path().join("sid.toml");
//! let cfg = SidToml {
//!     db_path_override: Some(PathBuf::from("/custom/sid.redb")),
//! };
//! write_sid_toml(&path, &cfg).unwrap();
//! let got = read_sid_toml(&path).unwrap();
//! assert_eq!(
//!     got.db_path_override.as_deref().and_then(|p| p.to_str()),
//!     Some("/custom/sid.redb"),
//! );
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The (small) shape of `sid.toml`. Unknown fields are silently ignored so the
/// file stays forward-compatible.
///
/// # Examples
///
/// ```
/// use sid_store::sid_toml::SidToml;
/// let cfg = SidToml::default();
/// assert!(cfg.db_path_override.is_none());
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SidToml {
    /// Optional override for the redb database path. When `Some`, sid opens the
    /// database at this path instead of the XDG default.
    pub db_path_override: Option<PathBuf>,
}

/// Errors returned by [`read_sid_toml`] / [`write_sid_toml`].
#[derive(Debug, thiserror::Error)]
pub enum SidTomlError {
    /// Filesystem I/O failure (other than a "file not found" on read, which
    /// is treated as a default config).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// TOML parse/serialise failure.
    #[error("parse: {0}")]
    Parse(String),
}

/// Read `sid.toml` from `path`. Returns `SidToml::default()` if the file does
/// not exist. Unknown TOML keys are silently ignored.
///
/// # Errors
///
/// Returns [`SidTomlError::Io`] for any I/O error other than "not found", and
/// [`SidTomlError::Parse`] for malformed TOML or type mismatches.
///
/// # Examples
///
/// ```
/// use sid_store::sid_toml::read_sid_toml;
/// use tempfile::tempdir;
///
/// let dir = tempdir().unwrap();
/// let cfg = read_sid_toml(&dir.path().join("absent.toml")).unwrap();
/// assert!(cfg.db_path_override.is_none());
/// ```
pub fn read_sid_toml(path: &Path) -> Result<SidToml, SidTomlError> {
    match std::fs::read_to_string(path) {
        Ok(s) => toml::from_str(&s).map_err(|e| SidTomlError::Parse(e.to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(SidToml::default()),
        Err(e) => Err(SidTomlError::Io(e)),
    }
}

/// Write `cfg` to `path` as pretty-printed TOML. Creates parent directories if
/// they do not exist.
///
/// # Errors
///
/// Returns [`SidTomlError::Io`] if directory creation or the file write fails,
/// and [`SidTomlError::Parse`] if `cfg` cannot be serialised (essentially
/// impossible with the current shape, but mapped for completeness).
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use sid_store::sid_toml::{write_sid_toml, SidToml};
/// use tempfile::tempdir;
///
/// let dir = tempdir().unwrap();
/// let path = dir.path().join("nested/sid.toml");
/// write_sid_toml(
///     &path,
///     &SidToml { db_path_override: Some(PathBuf::from("/x")) },
/// ).unwrap();
/// assert!(path.exists());
/// ```
pub fn write_sid_toml(path: &Path, cfg: &SidToml) -> Result<(), SidTomlError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let s = toml::to_string_pretty(cfg).map_err(|e| SidTomlError::Parse(e.to_string()))?;
    std::fs::write(path, s)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_no_override() {
        assert!(SidToml::default().db_path_override.is_none());
    }

    #[test]
    fn serialize_then_deserialize_round_trips() {
        let cfg = SidToml {
            db_path_override: Some(PathBuf::from("/x/y.redb")),
        };
        let s = toml::to_string_pretty(&cfg).unwrap();
        let back: SidToml = toml::from_str(&s).unwrap();
        assert_eq!(back, cfg);
    }
}
