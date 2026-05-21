//! SshClient — filled out in Plan 3 (SSH + SFTP).

/// Trait for SSH connections. Concrete impl (`RusshClient`) lives in `sid-ssh`.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::ssh::SshClient;
///
/// struct NoopSsh;
/// impl SshClient for NoopSsh {}
///
/// fn accepts_client(_c: &dyn SshClient) {}
/// accepts_client(&NoopSsh);
/// ```
pub trait SshClient: Send + Sync {}
