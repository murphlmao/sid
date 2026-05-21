//! Debounced dirty-marker for state persistence.
//!
//! `StatePersister` tracks whether application state has changed since the last
//! flush. A debounce window prevents writing too frequently; only after
//! `debounce` time has elapsed since the _first_ dirty mark does
//! [`should_flush`][StatePersister::should_flush] return `true`.
//!
//! The persister is intentionally free of I/O. The binary crate holds the
//! `Store` and calls `should_flush` on each event-loop tick; if `true`, it
//! writes the current state and the dirty marker is automatically consumed.

use std::time::{Duration, Instant};

/// Tracks whether application state needs to be written to disk, with
/// debouncing to avoid excessive flushes.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use sid_core::persister::StatePersister;
///
/// let mut p = StatePersister::new(Duration::from_millis(100));
/// assert!(!p.is_dirty());
/// p.mark_dirty();
/// assert!(p.is_dirty());
/// // Within the debounce window, should_flush is false.
/// assert!(!p.should_flush());
/// ```
pub struct StatePersister {
    debounce: Duration,
    dirty_since: Option<Instant>,
}

impl StatePersister {
    /// Create a new `StatePersister` with the given debounce window.
    ///
    /// A `Duration::ZERO` debounce means flush is due immediately after
    /// `mark_dirty` is called.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use sid_core::persister::StatePersister;
    ///
    /// let p = StatePersister::new(Duration::from_millis(500));
    /// assert!(!p.is_dirty());
    /// ```
    pub fn new(debounce: Duration) -> Self {
        Self {
            debounce,
            dirty_since: None,
        }
    }

    /// Mark application state as dirty. Only the first call after a flush
    /// starts the debounce timer — subsequent calls before the next flush are
    /// no-ops and do **not** reset the timer.
    ///
    /// This means the debounce measures time since the _first_ dirty
    /// notification in a batch, not since the most recent one.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use sid_core::persister::StatePersister;
    ///
    /// let mut p = StatePersister::new(Duration::from_millis(100));
    /// assert!(!p.is_dirty());
    /// p.mark_dirty();
    /// assert!(p.is_dirty());
    /// // Calling again is a no-op — does not reset the timer.
    /// p.mark_dirty();
    /// assert!(p.is_dirty());
    /// ```
    pub fn mark_dirty(&mut self) {
        if self.dirty_since.is_none() {
            self.dirty_since = Some(Instant::now());
        }
    }

    /// Returns `true` if the debounce window has elapsed since the first
    /// `mark_dirty` call. Consuming: the dirty marker is cleared so a
    /// subsequent call returns `false` until `mark_dirty` is called again.
    ///
    /// Returns `false` immediately if nothing has been marked dirty.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use sid_core::persister::StatePersister;
    ///
    /// let mut p = StatePersister::new(Duration::ZERO);
    /// assert!(!p.should_flush());   // nothing dirty
    /// p.mark_dirty();
    /// assert!(p.should_flush());    // zero debounce → immediately due
    /// assert!(!p.should_flush());   // marker consumed
    /// ```
    pub fn should_flush(&mut self) -> bool {
        match self.dirty_since {
            Some(t) if t.elapsed() >= self.debounce => {
                self.dirty_since = None;
                true
            }
            _ => false,
        }
    }

    /// Return `true` if the state is marked dirty (flush not yet due or
    /// debounce not yet elapsed).
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use sid_core::persister::StatePersister;
    ///
    /// let mut p = StatePersister::new(Duration::from_millis(100));
    /// assert!(!p.is_dirty());
    /// p.mark_dirty();
    /// assert!(p.is_dirty());
    /// ```
    pub fn is_dirty(&self) -> bool {
        self.dirty_since.is_some()
    }
}
