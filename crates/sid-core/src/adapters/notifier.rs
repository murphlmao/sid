//! Notifier — the only adapter with a v1 implementation, because toast
//! notifications are part of the foundation.

/// Severity level for a notification.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::notifier::NotifyLevel;
///
/// let level = NotifyLevel::Info;
/// let _cloned = level.clone();
/// ```
#[derive(Clone, Debug)]
pub enum NotifyLevel {
    /// Informational message.
    Info,
    /// Warning — something unexpected happened but the app can continue.
    Warn,
    /// Error — an operation failed.
    Error,
}

/// Trait for sending notifications to the user. Concrete impls might show
/// in-TUI toasts, write to a log, or push OS notifications.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::notifier::{NotifyLevel, Notifier};
///
/// struct LogNotifier;
///
/// impl Notifier for LogNotifier {
///     fn notify(&self, level: NotifyLevel, message: &str) {
///         eprintln!("[{level:?}] {message}");
///     }
/// }
///
/// let n = LogNotifier;
/// n.notify(NotifyLevel::Info, "hello from doc test");
/// ```
pub trait Notifier: Send + Sync {
    /// Emit a notification at the given level.
    fn notify(&self, level: NotifyLevel, message: &str);
}
