//! PtyProvider — filled out in Plan 3 (PTY backbone).

/// Trait for pseudo-terminal operations. Concrete impl lives in `sid-pty`.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::pty::PtyProvider;
///
/// struct NoopPty;
/// impl PtyProvider for NoopPty {}
///
/// fn accepts_pty(_p: &dyn PtyProvider) {}
/// accepts_pty(&NoopPty);
/// ```
pub trait PtyProvider: Send + Sync {}
