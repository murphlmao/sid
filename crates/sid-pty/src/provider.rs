//! `PortablePtyProvider` — opens portable-pty master/slave pairs and spawns a
//! child process on the slave end. Filled in over Tasks 12–14.

use sid_core::adapters::pty::PtyProvider;

/// Stateless provider; per-PTY handles are produced by `open_pty`.
///
/// # Examples
///
/// ```
/// use sid_pty::PortablePtyProvider;
/// let _p = PortablePtyProvider::new();
/// ```
pub struct PortablePtyProvider {
    _placeholder: (),
}

impl PortablePtyProvider {
    /// Construct a new provider. Cheap; no I/O.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_pty::PortablePtyProvider;
    /// let _p = PortablePtyProvider::new();
    /// ```
    pub fn new() -> Self {
        Self { _placeholder: () }
    }
}

impl Default for PortablePtyProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl PtyProvider for PortablePtyProvider {
    // Methods filled in over Tasks 12-14.
}
