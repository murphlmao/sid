use std::path::PathBuf;

/// The top-level error type for `sid`.
///
/// Every variant carries enough context to produce a human-readable message.
/// Use [`Result`] as a convenience alias.
///
/// # Examples
///
/// ```
/// use sid_core::SidError;
///
/// fn might_fail(flag: bool) -> sid_core::Result<()> {
///     if flag {
///         Err(SidError::Other("something went wrong".into()))
///     } else {
///         Ok(())
///     }
/// }
///
/// assert!(might_fail(true).is_err());
/// assert!(might_fail(false).is_ok());
/// ```
#[derive(Debug, thiserror::Error)]
pub enum SidError {
    /// A storage-layer operation failed.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::SidError;
    ///
    /// let e = SidError::Storage("db write failed".into());
    /// let msg = format!("{e}");
    /// assert!(msg.contains("storage error"));
    /// assert!(msg.contains("db write failed"));
    /// ```
    #[error("storage error: {0}")]
    Storage(String),

    /// An I/O error, bound to the path that was being accessed.
    ///
    /// The `source` field is chained via [`std::error::Error::source`], so
    /// callers can inspect the underlying [`std::io::Error`].
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use sid_core::SidError;
    ///
    /// let inner = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
    /// let e = SidError::Io { path: PathBuf::from("/tmp/test"), source: inner };
    /// let msg = format!("{e}");
    /// assert!(msg.contains("io error reading"));
    /// assert!(msg.contains("/tmp/test"));
    /// ```
    #[error("io error reading {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A widget ID was referenced that has not been registered.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::SidError;
    ///
    /// let e = SidError::UnknownWidget("git-log".into());
    /// let msg = format!("{e}");
    /// assert!(msg.contains("git-log"));
    /// assert!(msg.contains("not registered"));
    /// ```
    #[error("widget '{0}' not registered")]
    UnknownWidget(String),

    /// An action ID was referenced that has not been registered.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::SidError;
    ///
    /// let e = SidError::UnknownAction("open-workspace".into());
    /// let msg = format!("{e}");
    /// assert!(msg.contains("open-workspace"));
    /// assert!(msg.contains("not registered"));
    /// ```
    #[error("action '{0}' not registered")]
    UnknownAction(String),

    /// A keybind string could not be parsed.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::SidError;
    ///
    /// let e = SidError::InvalidKeybind("ctrl+???".into());
    /// let msg = format!("{e}");
    /// assert!(msg.contains("invalid keybind"));
    /// assert!(msg.contains("ctrl+???"));
    /// ```
    #[error("invalid keybind: {0}")]
    InvalidKeybind(String),

    /// A catch-all for errors that don't fit another variant.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::SidError;
    ///
    /// let e = SidError::Other("unexpected condition".into());
    /// let msg = format!("{e}");
    /// assert!(msg.contains("unexpected condition"));
    /// ```
    #[error("{0}")]
    Other(String),
}

/// Convenience `Result` alias with [`SidError`] as the default error type.
///
/// # Examples
///
/// ```
/// use sid_core::{Result, SidError};
///
/// fn ok_path() -> Result<u32> { Ok(42) }
/// fn err_path() -> Result<u32> { Err(SidError::Other("oops".into())) }
///
/// assert_eq!(ok_path().unwrap(), 42);
/// assert!(err_path().is_err());
/// ```
pub type Result<T, E = SidError> = std::result::Result<T, E>;
