//! Clipboard — filled out in a later plan as needed.

/// Trait for clipboard access. Concrete impl will wrap a platform clipboard crate.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::clipboard::Clipboard;
///
/// struct NoopClipboard;
///
/// impl Clipboard for NoopClipboard {
///     fn copy(&self, _text: &str) {}
/// }
///
/// let c = NoopClipboard;
/// c.copy("hello clipboard");
/// ```
pub trait Clipboard: Send + Sync {
    /// Copy `text` to the system clipboard.
    fn copy(&self, text: &str);
}
