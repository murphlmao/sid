//! `RusshClient` core — connect/disconnect/exec. Filled in over Tasks 6–8.

use sid_core::adapters::ssh::SshClient;

/// Stateless factory; per-host clients are produced by `connect`.
///
/// # Examples
///
/// ```
/// use sid_ssh::RusshClientFactory;
/// let _f = RusshClientFactory::new();
/// ```
pub struct RusshClientFactory;

impl RusshClientFactory {
    /// Construct a new factory. Cheap; no I/O.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_ssh::RusshClientFactory;
    /// let _f = RusshClientFactory::new();
    /// ```
    pub fn new() -> Self {
        Self
    }
}

impl Default for RusshClientFactory {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-host SSH client. Constructed by [`RusshClientFactory::connect`] in
/// Task 6. Holds the russh `Handle` plus a tokio task that pumps the channel.
///
/// # Examples
///
/// ```
/// // Construction details land in Task 6; this exists for type wiring.
/// ```
pub struct RusshClient {
    // Filled in by Task 6.
    pub(crate) _placeholder: (),
}

impl SshClient for RusshClient {
    // Methods filled in over Tasks 6-10.
}
