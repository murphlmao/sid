//! Two-state filter input: `Inactive` until `/` is pressed, then
//! `Editing(query)` while the user types. Submission keeps the query
//! active so the underlying tables stay filtered until the user cancels
//! with Esc.
//!
//! Filter predicates are case-insensitive substring matches across the
//! salient fields of each row type; they live alongside the state because
//! the widget assembly calls them per row per frame.

use sid_core::adapters::sys::{ListeningPort, NetInterface, ProcessInfo};

/// Editor mode for the filter input.
///
/// # Examples
///
/// ```
/// use sid_widgets::network::filter_input::FilterMode;
/// assert_eq!(FilterMode::default(), FilterMode::Inactive);
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum FilterMode {
    /// No filter is being entered or active.
    #[default]
    Inactive,
    /// User is typing into the filter; `query` accumulates characters.
    Editing,
    /// User submitted the filter; rows continue to be matched against
    /// `query` until cancelled.
    Active,
}

/// Filter input state.
///
/// # Examples
///
/// ```
/// use sid_widgets::network::filter_input::{FilterInputState, FilterMode};
///
/// let mut f = FilterInputState::new();
/// assert_eq!(f.mode(), &FilterMode::Inactive);
/// f.enter_filter();
/// f.push_char('h');
/// f.push_char('i');
/// assert_eq!(f.query(), "hi");
/// f.submit();
/// assert_eq!(f.mode(), &FilterMode::Active);
/// ```
#[derive(Clone, Debug, Default)]
pub struct FilterInputState {
    mode: FilterMode,
    query: String,
}

impl FilterInputState {
    /// Construct a fresh, inactive filter input.
    pub fn new() -> Self {
        Self::default()
    }

    /// Current editor mode.
    pub fn mode(&self) -> &FilterMode {
        &self.mode
    }

    /// Current query string. Empty when inactive or freshly opened.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Whether a filter is currently constraining the displayed rows
    /// (i.e., the input is `Editing` or `Active` with a non-empty query).
    pub fn is_filtering(&self) -> bool {
        matches!(self.mode, FilterMode::Editing | FilterMode::Active) && !self.query.is_empty()
    }

    /// Open the filter input for editing. Clears any prior query.
    pub fn enter_filter(&mut self) {
        self.mode = FilterMode::Editing;
        self.query.clear();
    }

    /// Cancel the filter; clears the query and returns to the inactive state.
    pub fn cancel(&mut self) {
        self.mode = FilterMode::Inactive;
        self.query.clear();
    }

    /// Append a character to the query. No-op when not in `Editing` mode.
    pub fn push_char(&mut self, c: char) {
        if matches!(self.mode, FilterMode::Editing) {
            self.query.push(c);
        }
    }

    /// Remove the last character from the query. No-op on an empty query
    /// or when not in `Editing` mode.
    pub fn pop_char(&mut self) {
        if matches!(self.mode, FilterMode::Editing) {
            self.query.pop();
        }
    }

    /// Submit the filter — transitions from `Editing` to `Active`. The
    /// query is retained so callers can keep filtering rows.
    pub fn submit(&mut self) {
        if matches!(self.mode, FilterMode::Editing) {
            self.mode = FilterMode::Active;
        }
    }
}

// ---------------------------------------------------------------------------
// Match predicates
// ---------------------------------------------------------------------------

fn matches_any(query: &str, fields: &[&str]) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    fields.iter().any(|f| f.to_lowercase().contains(&q))
}

/// Return true iff the listening-port row matches `query`. Matches on port
/// number, command name, and local address. An empty query matches every
/// row.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::sys::{ListeningPort, Pid, Protocol, SocketState};
/// use sid_widgets::network::filter_input::match_listening_port;
///
/// let row = ListeningPort {
///     port: 22,
///     pid: Some(Pid::from_u32(1)),
///     command: "sshd".into(),
///     protocol: Protocol::Tcp,
///     state: SocketState::Listen,
///     local_addr: "0.0.0.0".into(),
/// };
/// assert!(match_listening_port("ssh", &row));
/// assert!(match_listening_port("22", &row));
/// assert!(!match_listening_port("foo", &row));
/// assert!(match_listening_port("", &row));
/// ```
pub fn match_listening_port(query: &str, row: &ListeningPort) -> bool {
    let port_s = row.port.to_string();
    matches_any(query, &[&port_s, &row.command, &row.local_addr])
}

/// Return true iff the process row matches `query`. Matches on PID, short
/// name, full command, and user.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::sys::{Pid, ProcessInfo};
/// use sid_widgets::network::filter_input::match_process;
///
/// let row = ProcessInfo {
///     pid: Pid::from_u32(1234),
///     name: "sid".into(),
///     cmd: "sid --start-tab=network".into(),
///     cpu_pct: 0.0,
///     rss_bytes: 0,
///     started_unix_secs: 0,
///     parent: None,
///     user: Some("1000".into()),
/// };
/// assert!(match_process("sid", &row));
/// assert!(match_process("1234", &row));
/// assert!(match_process("1000", &row));
/// assert!(!match_process("nope", &row));
/// ```
pub fn match_process(query: &str, row: &ProcessInfo) -> bool {
    let pid_s = row.pid.as_u32().to_string();
    let user_s = row.user.as_deref().unwrap_or("");
    matches_any(query, &[&pid_s, &row.name, &row.cmd, user_s])
}

/// Return true iff the interface row matches `query`. Matches on name and
/// any bound address.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::sys::NetInterface;
/// use sid_widgets::network::filter_input::match_interface;
///
/// let row = NetInterface {
///     name: "eth0".into(),
///     addrs: vec!["192.168.1.10".into()],
///     rx_bytes: 0, tx_bytes: 0, is_up: true,
/// };
/// assert!(match_interface("eth", &row));
/// assert!(match_interface("192.168", &row));
/// assert!(!match_interface("wlan", &row));
/// ```
pub fn match_interface(query: &str, row: &NetInterface) -> bool {
    if matches_any(query, &[&row.name]) {
        return true;
    }
    // Empty query short-circuited above; fall through to address match.
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    row.addrs.iter().any(|a| a.to_lowercase().contains(&q))
}
