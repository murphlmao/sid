//! SysProvider — filled out in Plan 5 (Network tab).

/// Trait for system metrics and network info. Concrete impl lives in `sid-sys`.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::sys::SysProvider;
///
/// struct NoopSys;
/// impl SysProvider for NoopSys {}
///
/// fn accepts_sys(_s: &dyn SysProvider) {}
/// accepts_sys(&NoopSys);
/// ```
pub trait SysProvider: Send + Sync {}
