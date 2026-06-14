//! In-process log ring sub-view for the Settings tab.
//!
//! [`LogsView`] maintains a fixed-capacity ring buffer of [`LogEntry`] records.
//! The oldest entries are evicted when the buffer reaches [`LOG_RING_CAP`].
//! Entries are stored and displayed newest-last (chronological order): the most
//! recent entry is at the bottom, matching the convention used by standard log
//! viewers.
//!
//! The view does **not** depend on any clock — callers supply `epoch` values
//! (UNIX-epoch nanoseconds) directly. The helper [`format_hms`] converts a
//! nanosecond epoch to an `HH:MM:SS` UTC string for display.
//!
//! # Examples
//!
//! ```
//! use sid_widgets::settings::logs::{LogEntry, LogLevel, LogsView};
//!
//! let mut v = LogsView::new();
//! assert!(v.is_empty());
//! v.push(LogEntry::new(0, LogLevel::Info, "hello"));
//! assert_eq!(v.len(), 1);
//! ```

use std::collections::VecDeque;

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph},
};
use sid_ui::Theme;

/// Severity of a [`LogEntry`].
///
/// # Examples
///
/// ```
/// use sid_widgets::settings::logs::LogLevel;
/// assert_eq!(format!("{:?}", LogLevel::Info), "Info");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// General informational message.
    Info,
    /// Operation completed successfully.
    Success,
    /// Error or failure.
    Error,
}

/// A single log record held in the [`LogsView`] ring buffer.
///
/// `epoch` is UNIX-epoch **nanoseconds**. Use [`format_hms`] to render the
/// hour/minute/second component in UTC.
///
/// # Examples
///
/// ```
/// use sid_widgets::settings::logs::{LogEntry, LogLevel};
///
/// let e = LogEntry::new(3_661_000_000_000, LogLevel::Success, "done");
/// assert_eq!(e.epoch, 3_661_000_000_000);
/// assert_eq!(e.level, LogLevel::Success);
/// assert_eq!(e.message, "done");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    /// UNIX-epoch nanoseconds at which the event occurred.
    pub epoch: u64,
    /// Severity level.
    pub level: LogLevel,
    /// Human-readable message.
    pub message: String,
}

impl LogEntry {
    /// Construct a new [`LogEntry`].
    ///
    /// `epoch` must be UNIX-epoch nanoseconds.  `message` accepts anything
    /// that converts to a [`String`].
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::settings::logs::{LogEntry, LogLevel};
    ///
    /// let e = LogEntry::new(0, LogLevel::Info, "started");
    /// assert_eq!(e.message, "started");
    /// ```
    pub fn new(epoch: u64, level: LogLevel, message: impl Into<String>) -> Self {
        Self {
            epoch,
            level,
            message: message.into(),
        }
    }
}

/// Maximum number of [`LogEntry`] records the ring buffer retains.
///
/// When a [`LogsView::push`] would exceed this cap, the oldest entry is
/// evicted before the new one is appended.  500 entries at roughly 200 bytes
/// each keeps the ring comfortably under 100 KiB.
pub const LOG_RING_CAP: usize = 500;

/// Outcome returned by [`LogsView::handle_event`].
///
/// Currently the view only consumes navigation keys and never triggers a
/// persistent settings change, so `None` is the only variant.  The shape
/// mirrors sibling sub-views for forward compatibility.
///
/// # Examples
///
/// ```
/// use sid_widgets::settings::logs::LogsOutcome;
/// assert!(matches!(LogsOutcome::None, LogsOutcome::None));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogsOutcome {
    /// Event consumed or not recognised — no persistent change.
    None,
}

/// Settings sub-view that displays the in-process log ring.
///
/// Entries are ordered oldest-first (newest at the bottom) so the view reads
/// like a scrollable terminal log.  The scroll offset is measured from the
/// *bottom*: offset `0` shows the newest entries; increasing the offset scrolls
/// towards older entries.
///
/// # Examples
///
/// ```
/// use sid_widgets::settings::logs::{LogEntry, LogLevel, LogsView, LOG_RING_CAP};
///
/// let mut v = LogsView::new();
/// // Overfilling evicts the oldest entry.
/// for i in 0..=LOG_RING_CAP {
///     v.push(LogEntry::new(i as u64, LogLevel::Info, format!("msg {i}")));
/// }
/// assert_eq!(v.len(), LOG_RING_CAP);
/// // The very first entry ("msg 0") was evicted.
/// assert_eq!(v.entries().front().unwrap().message, "msg 1");
/// ```
pub struct LogsView {
    entries: VecDeque<LogEntry>,
    /// Scroll offset from the bottom (0 = newest visible at bottom).
    scroll_offset: usize,
}

impl LogsView {
    /// Construct an empty [`LogsView`].
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::settings::logs::LogsView;
    /// let v = LogsView::new();
    /// assert!(v.is_empty());
    /// assert_eq!(v.len(), 0);
    /// ```
    pub fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            scroll_offset: 0,
        }
    }

    /// Append `entry` to the ring.  If the ring is already at [`LOG_RING_CAP`],
    /// the oldest entry is evicted first.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::settings::logs::{LogEntry, LogLevel, LogsView};
    ///
    /// let mut v = LogsView::new();
    /// v.push(LogEntry::new(1_000_000, LogLevel::Info, "first"));
    /// v.push(LogEntry::new(2_000_000, LogLevel::Error, "second"));
    /// assert_eq!(v.len(), 2);
    /// ```
    pub fn push(&mut self, entry: LogEntry) {
        if self.entries.len() >= LOG_RING_CAP {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    /// Borrow the full entry ring (oldest first, newest last).
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::settings::logs::{LogEntry, LogLevel, LogsView};
    ///
    /// let mut v = LogsView::new();
    /// v.push(LogEntry::new(0, LogLevel::Info, "a"));
    /// assert_eq!(v.entries().len(), 1);
    /// ```
    pub fn entries(&self) -> &VecDeque<LogEntry> {
        &self.entries
    }

    /// Number of entries currently in the ring.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::settings::logs::{LogEntry, LogLevel, LogsView};
    ///
    /// let mut v = LogsView::new();
    /// assert_eq!(v.len(), 0);
    /// v.push(LogEntry::new(0, LogLevel::Info, "x"));
    /// assert_eq!(v.len(), 1);
    /// ```
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` when the ring contains no entries.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::settings::logs::LogsView;
    /// assert!(LogsView::new().is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Current scroll offset (rows from the bottom; `0` = newest at bottom).
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Scroll up by `n` rows (towards older entries), clamped so the view
    /// never scrolls past the oldest entry.
    pub fn scroll_up(&mut self, n: usize) {
        let max = self.entries.len().saturating_sub(1);
        self.scroll_offset = self.scroll_offset.saturating_add(n).min(max);
    }

    /// Scroll down by `n` rows (towards newer entries), clamped at `0`.
    pub fn scroll_down(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    /// Jump to the oldest visible entry (top of the ring).
    pub fn scroll_home(&mut self) {
        self.scroll_offset = self.entries.len().saturating_sub(1);
    }

    /// Jump to the newest entry (bottom of the ring).
    pub fn scroll_end(&mut self) {
        self.scroll_offset = 0;
    }

    /// Route a navigation key into the scroll state. All other keys are
    /// silently ignored and return [`LogsOutcome::None`].
    ///
    /// Key bindings:
    /// - `Up` / `k` — scroll up 1 row.
    /// - `Down` / `j` — scroll down 1 row.
    /// - `PageUp` — scroll up `page_size` rows.
    /// - `PageDown` — scroll down `page_size` rows.
    /// - `Home` — jump to oldest entry.
    /// - `End` — jump to newest entry.
    ///
    /// # Examples
    ///
    /// ```
    /// use crossterm::event::{KeyCode, KeyModifiers};
    /// use sid_core::event::{Event, KeyChord};
    /// use sid_widgets::settings::logs::{LogEntry, LogLevel, LogsView};
    ///
    /// let mut v = LogsView::new();
    /// for i in 0..10u64 {
    ///     v.push(LogEntry::new(i, LogLevel::Info, format!("m{i}")));
    /// }
    /// let ev = Event::Key(KeyChord::new(KeyCode::Up, KeyModifiers::NONE));
    /// v.handle_event(&ev, 10);
    /// assert_eq!(v.scroll_offset(), 1);
    /// ```
    pub fn handle_event(&mut self, ev: &sid_core::event::Event, page_size: usize) -> LogsOutcome {
        use crossterm::event::{KeyCode, KeyModifiers};
        let sid_core::event::Event::Key(k) = ev else {
            return LogsOutcome::None;
        };
        let page = page_size.max(1);
        match (k.code, k.mods) {
            (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
                self.scroll_up(1);
            }
            (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
                self.scroll_down(1);
            }
            (KeyCode::PageUp, _) => {
                self.scroll_up(page);
            }
            (KeyCode::PageDown, _) => {
                self.scroll_down(page);
            }
            (KeyCode::Home, _) => {
                self.scroll_home();
            }
            (KeyCode::End, _) => {
                self.scroll_end();
            }
            _ => {}
        }
        LogsOutcome::None
    }

    /// Render the log ring into `area`.
    ///
    /// `focused` controls the outer border color (accent vs muted) and the
    /// title-bar bold modifier, consistent with sibling sub-views.
    ///
    /// Lines are displayed oldest-first (newest at the bottom). The scroll
    /// offset shifts the visible window towards older entries. Very long
    /// messages are truncated to fit the terminal width.
    ///
    /// Guard: returns immediately if `area.width == 0 || area.height == 0`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ratatui::Terminal;
    /// use ratatui::backend::TestBackend;
    /// use sid_ui::themes::cosmos;
    /// use sid_widgets::settings::logs::LogsView;
    ///
    /// let v = LogsView::new();
    /// let backend = TestBackend::new(60, 8);
    /// let mut term = Terminal::new(backend).unwrap();
    /// let theme = cosmos();
    /// term.draw(|f| v.render_into_frame(f, f.area(), &theme, false)).unwrap();
    /// ```
    pub fn render_into_frame(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        theme: &Theme,
        focused: bool,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let border_color = if focused {
            theme.accent_primary
        } else {
            theme.muted
        };
        let mut title_style = Style::default().fg(theme.foreground.into());
        if focused {
            title_style = title_style.add_modifier(Modifier::BOLD);
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color.into()))
            .title(" Logs ")
            .title_style(title_style);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        let max_msg_len = inner.width.saturating_sub(12) as usize; // 10 for "HH:MM:SS X " prefix
        let view_height = inner.height as usize;

        if self.entries.is_empty() {
            let placeholder =
                Line::from("(no log entries)").style(Style::default().fg(theme.muted.into()));
            frame.render_widget(Paragraph::new(vec![placeholder]), inner);
            return;
        }

        // Build the full list of renderable lines (oldest first).
        let lines: Vec<Line<'static>> = self
            .entries
            .iter()
            .map(|e| {
                let ts = format_hms(e.epoch);
                let (marker, color) = match e.level {
                    LogLevel::Info => ('I', theme.foreground),
                    LogLevel::Success => ('S', theme.accent_success),
                    LogLevel::Error => ('E', theme.accent_error),
                };
                // Truncate very long messages so the line fits in the terminal.
                let msg = if e.message.len() > max_msg_len {
                    format!("{}…", &e.message[..max_msg_len.saturating_sub(1)])
                } else {
                    e.message.clone()
                };
                Line::from(format!("{ts} {marker} {msg}")).style(Style::default().fg(color.into()))
            })
            .collect();

        // Apply scroll: offset measured from the bottom.
        let total = lines.len();
        // The first index we want to render (from the top of `lines`).
        let first = if total <= view_height {
            0
        } else {
            let bottom_start = total - view_height;
            bottom_start.saturating_sub(self.scroll_offset)
        };
        let visible: Vec<Line<'static>> = lines.into_iter().skip(first).take(view_height).collect();

        frame.render_widget(Paragraph::new(visible), inner);
    }

    /// Render the newest `n` entries as a compact tail strip for the log-tail
    /// area at the bottom of the Settings layout. Returns an empty vec if
    /// the view has no entries.
    ///
    /// Used by the Settings composer to show an always-visible snippet
    /// regardless of the focused category.
    pub fn tail_lines(&self, n: usize, theme: &Theme, max_width: u16) -> Vec<Line<'static>> {
        if self.entries.is_empty() || n == 0 {
            return Vec::new();
        }
        let max_msg_len = (max_width as usize).saturating_sub(12);
        let skip = self.entries.len().saturating_sub(n);
        self.entries
            .iter()
            .skip(skip)
            .map(|e| {
                let ts = format_hms(e.epoch);
                let (marker, color) = match e.level {
                    LogLevel::Info => ('I', theme.foreground),
                    LogLevel::Success => ('S', theme.accent_success),
                    LogLevel::Error => ('E', theme.accent_error),
                };
                let msg = if e.message.len() > max_msg_len {
                    format!("{}…", &e.message[..max_msg_len.saturating_sub(1)])
                } else {
                    e.message.clone()
                };
                Line::from(format!("{ts} {marker} {msg}")).style(Style::default().fg(color.into()))
            })
            .collect()
    }
}

impl Default for LogsView {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a UNIX-epoch nanosecond timestamp to an `HH:MM:SS` UTC string.
///
/// The computation is: `total_secs = epoch_nanos / 1_000_000_000`,
/// `secs_of_day = total_secs % 86_400`, then `h = secs_of_day / 3600`,
/// `m = (secs_of_day % 3600) / 60`, `s = secs_of_day % 60`. All values are
/// zero-padded to two digits.
///
/// This function relies only on integer arithmetic — no external crates.
/// The result is always in UTC because UNIX epoch is defined as UTC.
///
/// # Examples
///
/// ```
/// use sid_widgets::settings::logs::format_hms;
///
/// // Epoch zero is midnight UTC.
/// assert_eq!(format_hms(0), "00:00:00");
///
/// // 3661 seconds past epoch = 1h 1m 1s.
/// assert_eq!(format_hms(3_661_000_000_000), "01:01:01");
///
/// // 86399 seconds = 23:59:59.
/// assert_eq!(format_hms(86_399_000_000_000), "23:59:59");
///
/// // 86400 seconds wraps to 00:00:00 (second day).
/// assert_eq!(format_hms(86_400_000_000_000), "00:00:00");
/// ```
pub fn format_hms(epoch_nanos: u64) -> String {
    let total_secs = epoch_nanos / 1_000_000_000;
    let secs_of_day = total_secs % 86_400;
    let h = secs_of_day / 3600;
    let m = (secs_of_day % 3600) / 60;
    let s = secs_of_day % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // format_hms correctness
    // -------------------------------------------------------------------------

    #[test]
    fn format_hms_zero_is_midnight() {
        assert_eq!(format_hms(0), "00:00:00");
    }

    #[test]
    fn format_hms_one_hour_one_minute_one_second() {
        // 3661 seconds = 1h 1m 1s
        assert_eq!(format_hms(3_661_000_000_000), "01:01:01");
    }

    #[test]
    fn format_hms_end_of_day() {
        // 86399 seconds = 23:59:59
        assert_eq!(format_hms(86_399_000_000_000), "23:59:59");
    }

    #[test]
    fn format_hms_day_boundary_wraps() {
        // 86400 seconds = start of next day = 00:00:00
        assert_eq!(format_hms(86_400_000_000_000), "00:00:00");
    }

    #[test]
    fn format_hms_noon() {
        // 43200 seconds = 12:00:00
        assert_eq!(format_hms(43_200_000_000_000), "12:00:00");
    }

    #[test]
    fn format_hms_sub_second_nanos_truncated() {
        // 999_999_999 nanos < 1 second -> still 00:00:00
        assert_eq!(format_hms(999_999_999), "00:00:00");
    }

    // -------------------------------------------------------------------------
    // Ring-buffer eviction
    // -------------------------------------------------------------------------

    #[test]
    fn push_within_cap_does_not_evict() {
        let mut v = LogsView::new();
        for i in 0..10u64 {
            v.push(LogEntry::new(i, LogLevel::Info, format!("m{i}")));
        }
        assert_eq!(v.len(), 10);
        assert_eq!(v.entries().front().unwrap().message, "m0");
    }

    #[test]
    fn push_beyond_cap_evicts_oldest() {
        let mut v = LogsView::new();
        for i in 0..=(LOG_RING_CAP as u64) {
            v.push(LogEntry::new(i, LogLevel::Info, format!("msg {i}")));
        }
        assert_eq!(v.len(), LOG_RING_CAP);
        assert_eq!(v.entries().front().unwrap().message, "msg 1");
        assert_eq!(
            v.entries().back().unwrap().message,
            format!("msg {}", LOG_RING_CAP)
        );
    }

    #[test]
    fn push_many_beyond_cap_keeps_newest() {
        let mut v = LogsView::new();
        let total = LOG_RING_CAP * 2;
        for i in 0..total {
            v.push(LogEntry::new(i as u64, LogLevel::Info, format!("{i}")));
        }
        assert_eq!(v.len(), LOG_RING_CAP);
        // Oldest surviving entry is `total - LOG_RING_CAP`.
        let expected_oldest = format!("{}", total - LOG_RING_CAP);
        assert_eq!(v.entries().front().unwrap().message, expected_oldest);
    }

    // -------------------------------------------------------------------------
    // Accessors
    // -------------------------------------------------------------------------

    #[test]
    fn new_is_empty() {
        let v = LogsView::new();
        assert!(v.is_empty());
        assert_eq!(v.len(), 0);
    }

    #[test]
    fn len_matches_entries_len() {
        let mut v = LogsView::new();
        v.push(LogEntry::new(1, LogLevel::Success, "a"));
        v.push(LogEntry::new(2, LogLevel::Error, "b"));
        assert_eq!(v.len(), v.entries().len());
    }

    // -------------------------------------------------------------------------
    // Scroll bounds — must never panic
    // -------------------------------------------------------------------------

    #[test]
    fn scroll_up_on_empty_is_noop() {
        let mut v = LogsView::new();
        v.scroll_up(100);
        assert_eq!(v.scroll_offset(), 0);
    }

    #[test]
    fn scroll_down_at_zero_is_noop() {
        let mut v = LogsView::new();
        for i in 0..5u64 {
            v.push(LogEntry::new(i, LogLevel::Info, "x"));
        }
        v.scroll_down(999);
        assert_eq!(v.scroll_offset(), 0);
    }

    #[test]
    fn scroll_up_clamps_at_max() {
        let mut v = LogsView::new();
        for i in 0..5u64 {
            v.push(LogEntry::new(i, LogLevel::Info, "x"));
        }
        v.scroll_up(999);
        // Max offset = len - 1 = 4
        assert_eq!(v.scroll_offset(), 4);
    }

    #[test]
    fn scroll_home_jumps_to_max_offset() {
        let mut v = LogsView::new();
        for i in 0..10u64 {
            v.push(LogEntry::new(i, LogLevel::Info, "x"));
        }
        v.scroll_home();
        assert_eq!(v.scroll_offset(), 9);
    }

    #[test]
    fn scroll_end_resets_to_zero() {
        let mut v = LogsView::new();
        for i in 0..10u64 {
            v.push(LogEntry::new(i, LogLevel::Info, "x"));
        }
        v.scroll_home();
        v.scroll_end();
        assert_eq!(v.scroll_offset(), 0);
    }

    #[test]
    fn scroll_home_on_single_entry_clamps_to_zero() {
        let mut v = LogsView::new();
        v.push(LogEntry::new(0, LogLevel::Info, "only"));
        v.scroll_home();
        assert_eq!(v.scroll_offset(), 0);
    }

    // -------------------------------------------------------------------------
    // handle_event key bindings
    // -------------------------------------------------------------------------

    fn key(code: crossterm::event::KeyCode) -> sid_core::event::Event {
        use crossterm::event::KeyModifiers;
        use sid_core::event::KeyChord;
        sid_core::event::Event::Key(KeyChord::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn handle_event_up_scrolls_up() {
        use crossterm::event::KeyCode;
        let mut v = LogsView::new();
        for i in 0..5u64 {
            v.push(LogEntry::new(i, LogLevel::Info, "x"));
        }
        v.handle_event(&key(KeyCode::Up), 10);
        assert_eq!(v.scroll_offset(), 1);
    }

    #[test]
    fn handle_event_down_scrolls_down() {
        use crossterm::event::KeyCode;
        let mut v = LogsView::new();
        for i in 0..5u64 {
            v.push(LogEntry::new(i, LogLevel::Info, "x"));
        }
        v.scroll_home();
        v.handle_event(&key(KeyCode::Down), 10);
        assert_eq!(v.scroll_offset(), v.len() - 2);
    }

    #[test]
    fn handle_event_page_up_scrolls_by_page() {
        use crossterm::event::KeyCode;
        let mut v = LogsView::new();
        for i in 0..50u64 {
            v.push(LogEntry::new(i, LogLevel::Info, "x"));
        }
        v.handle_event(&key(KeyCode::PageUp), 10);
        assert_eq!(v.scroll_offset(), 10);
    }

    #[test]
    fn handle_event_page_down_scrolls_towards_end() {
        use crossterm::event::KeyCode;
        let mut v = LogsView::new();
        for i in 0..50u64 {
            v.push(LogEntry::new(i, LogLevel::Info, "x"));
        }
        v.scroll_home();
        let before = v.scroll_offset();
        v.handle_event(&key(KeyCode::PageDown), 10);
        assert!(v.scroll_offset() < before);
    }

    #[test]
    fn handle_event_home_jumps_to_oldest() {
        use crossterm::event::KeyCode;
        let mut v = LogsView::new();
        for i in 0..10u64 {
            v.push(LogEntry::new(i, LogLevel::Info, "x"));
        }
        v.handle_event(&key(KeyCode::Home), 10);
        assert_eq!(v.scroll_offset(), 9);
    }

    #[test]
    fn handle_event_end_jumps_to_newest() {
        use crossterm::event::KeyCode;
        let mut v = LogsView::new();
        for i in 0..10u64 {
            v.push(LogEntry::new(i, LogLevel::Info, "x"));
        }
        v.scroll_home();
        v.handle_event(&key(KeyCode::End), 10);
        assert_eq!(v.scroll_offset(), 0);
    }

    #[test]
    fn handle_event_unknown_key_is_noop() {
        use crossterm::event::KeyCode;
        let mut v = LogsView::new();
        for i in 0..5u64 {
            v.push(LogEntry::new(i, LogLevel::Info, "x"));
        }
        let before = v.scroll_offset();
        v.handle_event(&key(KeyCode::Char('z')), 10);
        assert_eq!(v.scroll_offset(), before);
    }

    // -------------------------------------------------------------------------
    // Render smoke tests — must not panic
    // -------------------------------------------------------------------------

    fn render(v: &LogsView, width: u16, height: u16, focused: bool) -> String {
        use ratatui::{Terminal, backend::TestBackend};
        use sid_ui::themes::cosmos;
        let backend = TestBackend::new(width, height);
        let mut term = Terminal::new(backend).unwrap();
        let theme = cosmos();
        term.draw(|f| v.render_into_frame(f, f.area(), &theme, focused))
            .unwrap();
        let buf = term.backend().buffer();
        let mut s = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                s.push_str(buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "));
            }
            s.push('\n');
        }
        s
    }

    #[test]
    fn render_empty_does_not_panic() {
        let v = LogsView::new();
        let s = render(&v, 60, 8, false);
        assert!(s.contains("no log entries") || s.contains("Logs"));
    }

    #[test]
    fn render_zero_height_does_not_panic() {
        // height == 0: render_into_frame must return early without panicking.
        let v = LogsView::new();
        render(&v, 60, 0, false);
    }

    #[test]
    fn render_one_entry_does_not_panic() {
        let mut v = LogsView::new();
        v.push(LogEntry::new(3_661_000_000_000, LogLevel::Info, "hello"));
        let s = render(&v, 60, 8, true);
        assert!(s.contains("01:01:01"));
    }

    #[test]
    fn render_long_message_is_truncated_no_panic() {
        let mut v = LogsView::new();
        let long = "x".repeat(500);
        v.push(LogEntry::new(0, LogLevel::Error, &long));
        // Must not panic even with very wide content.
        render(&v, 40, 8, false);
    }

    #[test]
    fn render_many_entries_does_not_panic() {
        let mut v = LogsView::new();
        for i in 0..LOG_RING_CAP {
            v.push(LogEntry::new(
                i as u64 * 1_000_000_000,
                LogLevel::Info,
                format!("e{i}"),
            ));
        }
        render(&v, 80, 24, false);
    }

    // -------------------------------------------------------------------------
    // tail_lines
    // -------------------------------------------------------------------------

    #[test]
    fn tail_lines_empty_returns_empty() {
        use sid_ui::themes::cosmos;
        let v = LogsView::new();
        let theme = cosmos();
        assert!(v.tail_lines(3, &theme, 80).is_empty());
    }

    #[test]
    fn tail_lines_returns_newest_n() {
        use sid_ui::themes::cosmos;
        let mut v = LogsView::new();
        for i in 0..10u64 {
            v.push(LogEntry::new(
                i * 1_000_000_000,
                LogLevel::Info,
                format!("m{i}"),
            ));
        }
        let theme = cosmos();
        let lines = v.tail_lines(3, &theme, 80);
        assert_eq!(lines.len(), 3);
        // Each line's text should contain m7, m8, m9 (the last 3).
        let texts: Vec<String> = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<Vec<_>>()
                    .concat()
            })
            .collect();
        assert!(
            texts[0].contains("m7"),
            "first tail line should be m7, got: {}",
            texts[0]
        );
        assert!(texts[1].contains("m8"));
        assert!(texts[2].contains("m9"));
    }

    #[test]
    fn tail_lines_capped_at_available_entries() {
        use sid_ui::themes::cosmos;
        let mut v = LogsView::new();
        v.push(LogEntry::new(0, LogLevel::Info, "only"));
        let theme = cosmos();
        let lines = v.tail_lines(10, &theme, 80);
        assert_eq!(lines.len(), 1);
    }

    // -------------------------------------------------------------------------
    // Ordering guarantee
    // -------------------------------------------------------------------------

    #[test]
    fn entries_are_oldest_first() {
        let mut v = LogsView::new();
        v.push(LogEntry::new(1_000_000_000, LogLevel::Info, "old"));
        v.push(LogEntry::new(2_000_000_000, LogLevel::Info, "new"));
        let e: Vec<_> = v.entries().iter().collect();
        assert_eq!(e[0].message, "old");
        assert_eq!(e[1].message, "new");
    }
}
