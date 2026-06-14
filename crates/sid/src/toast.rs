//! Small, time-decayed toast messages surfaced in the lower-right corner of the
//! TUI.
//!
//! Used to report the outcome of modal submits (workspaces.new, ssh.new, ...),
//! async job completions (ssh-copy-id, ssh-keygen, ssh debug), and other
//! best-effort feedback. Toasts have a fixed default lifetime of 3 seconds and
//! self-evict via [`ToastQueue::drain_expired`] (called each event-loop pass).
//!
//! Render order: starfield → body → footer → toasts → modal/palette. The
//! modal/palette overlay deliberately covers the toast region; the user can
//! see the toast once the modal is dismissed if it has not yet expired.
//!
//! # Example
//!
//! ```
//! use sid::toast::{Toast, ToastKind, ToastQueue};
//!
//! let mut q = ToastQueue::new(4);
//! q.push(Toast::success("workspace 'foo' added"));
//! q.push(Toast::error("validation failed"));
//! q.push(Toast::info("running ssh-copy-id..."));
//!
//! assert_eq!(q.iter().count(), 3);
//! assert_eq!(q.iter().nth(0).unwrap().kind, ToastKind::Success);
//! ```

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// Severity / styling marker for a [`Toast`]. Determines the prefix glyph and
/// colour used by the renderer in `wire::render_toasts`.
///
/// # Example
///
/// ```
/// use sid::toast::ToastKind;
/// assert_ne!(ToastKind::Success, ToastKind::Error);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    /// Mutation succeeded — green check glyph.
    Success,
    /// Mutation or job failed — red cross glyph.
    Error,
    /// Informational, e.g., "running ssh-copy-id...". Muted dot glyph.
    Info,
}

/// Default per-toast lifetime; toasts older than this expire on the next
/// [`ToastQueue::drain_expired`].
pub const TOAST_LIFETIME: Duration = Duration::from_secs(3);

/// A single toast message: kind, body, and spawn timestamp.
///
/// Construct via [`Toast::success`] / [`Toast::error`] / [`Toast::info`] so
/// the spawn timestamp is set consistently.
///
/// # Example
///
/// ```
/// use sid::toast::{Toast, ToastKind};
/// let t = Toast::success("workspace 'foo' added");
/// assert_eq!(t.kind, ToastKind::Success);
/// assert_eq!(t.message, "workspace 'foo' added");
/// assert!(!t.is_expired());
/// ```
#[derive(Debug, Clone)]
pub struct Toast {
    /// The body shown to the right of the glyph prefix.
    pub message: String,
    /// Severity / styling marker.
    pub kind: ToastKind,
    /// Monotonic time the toast was constructed. Tests can mutate this to
    /// simulate aged toasts; production code never sets it directly.
    pub spawned_at: Instant,
}

impl Toast {
    /// Build a [`ToastKind::Success`] toast with the current time as
    /// `spawned_at`.
    ///
    /// # Example
    ///
    /// ```
    /// use sid::toast::{Toast, ToastKind};
    /// let t = Toast::success("ok");
    /// assert_eq!(t.kind, ToastKind::Success);
    /// ```
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: ToastKind::Success,
            spawned_at: Instant::now(),
        }
    }

    /// Build a [`ToastKind::Error`] toast with the current time as
    /// `spawned_at`.
    ///
    /// # Example
    ///
    /// ```
    /// use sid::toast::{Toast, ToastKind};
    /// let t = Toast::error("bad input");
    /// assert_eq!(t.kind, ToastKind::Error);
    /// ```
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: ToastKind::Error,
            spawned_at: Instant::now(),
        }
    }

    /// Build a [`ToastKind::Info`] toast with the current time as
    /// `spawned_at`.
    ///
    /// # Example
    ///
    /// ```
    /// use sid::toast::{Toast, ToastKind};
    /// let t = Toast::info("running...");
    /// assert_eq!(t.kind, ToastKind::Info);
    /// ```
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: ToastKind::Info,
            spawned_at: Instant::now(),
        }
    }

    /// True if this toast is older than [`TOAST_LIFETIME`].
    ///
    /// # Example
    ///
    /// ```
    /// use std::time::{Duration, Instant};
    /// use sid::toast::Toast;
    ///
    /// let mut t = Toast::success("x");
    /// assert!(!t.is_expired(), "fresh toast not expired");
    /// // Simulate ageing by 5s.
    /// t.spawned_at = Instant::now() - Duration::from_secs(5);
    /// assert!(t.is_expired(), "aged toast expired");
    /// ```
    pub fn is_expired(&self) -> bool {
        self.spawned_at.elapsed() > TOAST_LIFETIME
    }
}

/// Fixed-capacity ring of toasts: pushing beyond `cap` drops the oldest entry.
///
/// `iter` yields entries in insertion order (oldest first); the renderer is
/// free to reverse this for bottom-up stacking.
///
/// # Example
///
/// ```
/// use sid::toast::{Toast, ToastQueue};
/// let mut q = ToastQueue::new(2);
/// q.push(Toast::info("a"));
/// q.push(Toast::info("b"));
/// q.push(Toast::info("c"));
/// // Cap is 2 — oldest ('a') dropped.
/// let msgs: Vec<&str> = q.iter().map(|t| t.message.as_str()).collect();
/// assert_eq!(msgs, vec!["b", "c"]);
/// ```
pub struct ToastQueue {
    items: VecDeque<Toast>,
    cap: usize,
}

impl ToastQueue {
    /// Construct a new queue capped at `cap` entries. `cap == 0` is allowed
    /// and yields a queue that silently drops everything pushed — useful for
    /// tests that want to opt out of the toast surface entirely.
    ///
    /// # Example
    ///
    /// ```
    /// use sid::toast::ToastQueue;
    /// let q = ToastQueue::new(4);
    /// assert!(q.is_empty());
    /// ```
    pub fn new(cap: usize) -> Self {
        Self {
            items: VecDeque::with_capacity(cap),
            cap,
        }
    }

    /// Push a new toast. If the queue is already at capacity, the oldest
    /// (front) entry is dropped.
    ///
    /// # Example
    ///
    /// ```
    /// use sid::toast::{Toast, ToastQueue};
    /// let mut q = ToastQueue::new(1);
    /// q.push(Toast::info("a"));
    /// q.push(Toast::info("b"));
    /// // Cap 1 → only "b" remains.
    /// assert_eq!(q.iter().count(), 1);
    /// assert_eq!(q.iter().next().unwrap().message, "b");
    /// ```
    pub fn push(&mut self, t: Toast) {
        if self.cap == 0 {
            return;
        }
        while self.items.len() >= self.cap {
            self.items.pop_front();
        }
        self.items.push_back(t);
    }

    /// Evict any expired toasts. Called once per event-loop iteration.
    ///
    /// # Example
    ///
    /// ```
    /// use std::time::{Duration, Instant};
    /// use sid::toast::{Toast, ToastQueue};
    ///
    /// let mut q = ToastQueue::new(4);
    /// let mut t = Toast::info("old");
    /// t.spawned_at = Instant::now() - Duration::from_secs(5);
    /// q.push(t);
    /// q.drain_expired();
    /// assert!(q.is_empty());
    /// ```
    pub fn drain_expired(&mut self) {
        self.items.retain(|t| !t.is_expired());
    }

    /// Iterate toasts in insertion order (oldest first).
    ///
    /// # Example
    ///
    /// ```
    /// use sid::toast::{Toast, ToastQueue};
    /// let mut q = ToastQueue::new(2);
    /// q.push(Toast::info("a"));
    /// q.push(Toast::info("b"));
    /// let xs: Vec<&str> = q.iter().map(|t| t.message.as_str()).collect();
    /// assert_eq!(xs, vec!["a", "b"]);
    /// ```
    pub fn iter(&self) -> impl Iterator<Item = &Toast> {
        self.items.iter()
    }

    /// True iff the queue has no entries.
    ///
    /// # Example
    ///
    /// ```
    /// use sid::toast::ToastQueue;
    /// let q = ToastQueue::new(4);
    /// assert!(q.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Number of toasts currently held. Used by `render_toasts` to limit the
    /// visible count.
    ///
    /// # Example
    ///
    /// ```
    /// use sid::toast::{Toast, ToastQueue};
    /// let mut q = ToastQueue::new(4);
    /// q.push(Toast::info("a"));
    /// q.push(Toast::info("b"));
    /// assert_eq!(q.len(), 2);
    /// ```
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Configured capacity.
    ///
    /// # Example
    ///
    /// ```
    /// use sid::toast::ToastQueue;
    /// let q = ToastQueue::new(8);
    /// assert_eq!(q.cap(), 8);
    /// ```
    #[allow(dead_code)]
    pub fn cap(&self) -> usize {
        self.cap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ToastQueue` drops the oldest entry when pushing past capacity.
    #[test]
    fn toast_queue_caps_at_n() {
        let mut q = ToastQueue::new(4);
        for i in 0..10 {
            q.push(Toast::info(format!("msg-{i}")));
        }
        assert_eq!(q.len(), 4, "should cap at 4");
        let kept: Vec<&str> = q.iter().map(|t| t.message.as_str()).collect();
        // Newest 4 should be msg-6, msg-7, msg-8, msg-9.
        assert_eq!(kept, vec!["msg-6", "msg-7", "msg-8", "msg-9"]);
    }

    /// `drain_expired` removes only the aged toasts.
    #[test]
    fn toast_drain_expired_removes_old() {
        let mut q = ToastQueue::new(4);
        // Inject an old toast directly.
        let mut old = Toast::info("old");
        old.spawned_at = Instant::now() - Duration::from_secs(5);
        q.push(old);
        q.push(Toast::info("new"));
        q.drain_expired();
        assert_eq!(q.len(), 1, "only the fresh toast survives");
        assert_eq!(q.iter().next().unwrap().message, "new");
    }

    /// `drain_expired` on an empty queue is a no-op.
    #[test]
    fn drain_expired_empty_is_noop() {
        let mut q = ToastQueue::new(4);
        q.drain_expired();
        assert!(q.is_empty());
    }

    /// `is_expired` returns false for a freshly built toast.
    #[test]
    fn fresh_toast_not_expired() {
        let t = Toast::success("just now");
        assert!(!t.is_expired());
    }

    /// `is_expired` returns true once `spawned_at` is older than the lifetime.
    #[test]
    fn aged_toast_is_expired() {
        let mut t = Toast::error("ages ago");
        t.spawned_at = Instant::now() - Duration::from_secs(10);
        assert!(t.is_expired());
    }

    /// Constructor helpers tag the kind correctly.
    #[test]
    fn constructors_set_kind() {
        assert_eq!(Toast::success("a").kind, ToastKind::Success);
        assert_eq!(Toast::error("b").kind, ToastKind::Error);
        assert_eq!(Toast::info("c").kind, ToastKind::Info);
    }

    /// Capacity-0 queue silently drops pushes.
    #[test]
    fn capacity_zero_drops_pushes() {
        let mut q = ToastQueue::new(0);
        q.push(Toast::info("ignored"));
        assert!(q.is_empty());
        assert_eq!(q.cap(), 0);
    }

    /// Iterator order matches insertion order.
    #[test]
    fn iter_returns_oldest_first() {
        let mut q = ToastQueue::new(4);
        q.push(Toast::info("1"));
        q.push(Toast::info("2"));
        q.push(Toast::info("3"));
        let xs: Vec<&str> = q.iter().map(|t| t.message.as_str()).collect();
        assert_eq!(xs, vec!["1", "2", "3"]);
    }

    /// Mixed expired + fresh: only fresh survive `drain_expired`.
    #[test]
    fn drain_expired_preserves_order_of_survivors() {
        let mut q = ToastQueue::new(8);
        for i in 0..4 {
            let mut t = Toast::info(format!("o{i}"));
            t.spawned_at = Instant::now() - Duration::from_secs(10);
            q.push(t);
            q.push(Toast::info(format!("n{i}")));
        }
        q.drain_expired();
        let survivors: Vec<&str> = q.iter().map(|t| t.message.as_str()).collect();
        assert_eq!(survivors, vec!["n0", "n1", "n2", "n3"]);
    }

    /// `Toast::success` round-trips its message exactly.
    #[test]
    fn success_message_roundtrip() {
        let t = Toast::success("workspace 'foo' added");
        assert_eq!(t.message, "workspace 'foo' added");
    }

    /// Length is updated as entries are added and evicted.
    #[test]
    fn len_tracks_size_through_evictions() {
        let mut q = ToastQueue::new(2);
        assert_eq!(q.len(), 0);
        q.push(Toast::info("a"));
        assert_eq!(q.len(), 1);
        q.push(Toast::info("b"));
        assert_eq!(q.len(), 2);
        q.push(Toast::info("c"));
        // Over capacity: still at cap.
        assert_eq!(q.len(), 2);
    }
}
