//! `SystemctlClient` trait + supporting domain types.
//!
//! The trait is dyn-compatible (`Box<dyn SystemctlClient>`), `Send + Sync`,
//! with no generics in method position. Concrete implementations live in
//! sibling crates (e.g. `sid-system::SystemctlCmdClient`).

use serde::{Deserialize, Serialize};

/// Domain-shaped systemctl error. Concrete impls map their failure modes here.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::systemctl::SystemctlError;
/// let e = SystemctlError::SystemctlMissing;
/// let s = format!("{e}");
/// assert!(s.contains("systemctl"));
/// ```
#[derive(Debug, thiserror::Error)]
pub enum SystemctlError {
    /// `systemctl` binary not in PATH.
    #[error("systemctl binary not found in PATH")]
    SystemctlMissing,
    /// `journalctl` binary not in PATH.
    #[error("journalctl binary not found in PATH")]
    JournalctlMissing,
    /// Unit name not known to systemd.
    #[error("unit '{0}' not found")]
    UnitNotFound(String),
    /// Operation requires root and we couldn't escalate.
    #[error("operation requires root (system-bus write); re-run with sudo or via polkit")]
    SudoRequired,
    /// Subprocess exited non-zero.
    #[error("systemctl returned non-zero: {0}")]
    NonZeroExit(String),
    /// Output parser failed.
    #[error("output parser failure: {0}")]
    Parse(String),
    /// I/O error spawning or reading the subprocess.
    #[error("io: {0}")]
    Io(String),
    /// Catch-all for unexpected failure modes.
    #[error("other: {0}")]
    Other(String),
}

/// Which systemd bus the unit lives on.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::systemctl::UnitBus;
/// assert_ne!(UnitBus::User, UnitBus::System);
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum UnitBus {
    /// `systemctl --user` (per-user-session manager).
    #[default]
    User,
    /// `systemctl --system` (root-owned PID 1 manager).
    System,
}

/// Coarse-grained active state (`ActiveState` in systemd's vocabulary).
///
/// # Examples
///
/// ```
/// use sid_core::adapters::systemctl::UnitState;
/// let s = UnitState::Active;
/// assert_eq!(s, UnitState::Active);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum UnitState {
    Active,
    Reloading,
    Inactive,
    Failed,
    Activating,
    Deactivating,
    Unknown,
}

/// Display record for one unit row.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::systemctl::{SystemUnit, UnitBus, UnitState};
/// let u = SystemUnit {
///     name: "nginx.service".into(),
///     bus: UnitBus::System,
///     state: UnitState::Active,
///     sub_state: "running".into(),
///     description: "high performance web server".into(),
///     load_state: "loaded".into(),
/// };
/// assert_eq!(u.name, "nginx.service");
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SystemUnit {
    pub name: String,
    pub bus: UnitBus,
    pub state: UnitState,
    /// e.g. "running", "dead", "exited".
    pub sub_state: String,
    pub description: String,
    /// e.g. "loaded", "masked", "not-found".
    pub load_state: String,
}

/// Filter applied to [`SystemctlClient::list_units`].
///
/// # Examples
///
/// ```
/// use sid_core::adapters::systemctl::{UnitBus, UnitFilter};
/// let f = UnitFilter::default();
/// assert!(f.name_substring.is_none());
/// assert!(f.state.is_none());
/// assert_eq!(f.bus, UnitBus::User);
/// assert!(!f.bus_both);
/// ```
#[derive(Clone, Debug, Default)]
pub struct UnitFilter {
    /// Substring match against `name`. None = no name filter.
    pub name_substring: Option<String>,
    /// Match only units in this state. None = all states.
    pub state: Option<UnitState>,
    /// Which bus to query if `bus_both` is false. Default User.
    pub bus: UnitBus,
    /// If true, query both buses and merge the results. Overrides `bus`.
    pub bus_both: bool,
}

/// One line from `journalctl -n100 -u <unit>`.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::systemctl::JournalEntry;
/// let je = JournalEntry {
///     timestamp_secs: 1_700_000_000,
///     hostname: "host".into(),
///     source: "nginx[1234]".into(),
///     message: "started".into(),
/// };
/// assert_eq!(je.source, "nginx[1234]");
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Seconds since UNIX epoch.
    pub timestamp_secs: i64,
    /// Hostname column (often elided if --no-hostname; included for completeness).
    pub hostname: String,
    /// Process / unit identifier ("systemd[1]:", "nginx[1234]:").
    pub source: String,
    /// Free-text log message body.
    pub message: String,
}

/// Trait the System tab depends on. Implementations live in `sid-system`.
///
/// # Object safety
///
/// All methods take `&self` and use no generics in method position, so
/// `Box<dyn SystemctlClient>` works.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::systemctl::{
///     JournalEntry, SystemUnit, SystemctlClient, SystemctlError, UnitBus, UnitFilter,
/// };
///
/// struct Mock;
/// impl SystemctlClient for Mock {
///     fn list_units(&self, _f: UnitFilter) -> Result<Vec<SystemUnit>, SystemctlError> {
///         Ok(vec![])
///     }
///     fn status(&self, _b: UnitBus, _u: &str) -> Result<SystemUnit, SystemctlError> {
///         Err(SystemctlError::UnitNotFound(_u.to_string()))
///     }
///     fn start(&self, _b: UnitBus, _u: &str) -> Result<(), SystemctlError> { Ok(()) }
///     fn stop(&self, _b: UnitBus, _u: &str) -> Result<(), SystemctlError> { Ok(()) }
///     fn restart(&self, _b: UnitBus, _u: &str) -> Result<(), SystemctlError> { Ok(()) }
///     fn journal_tail(&self, _b: UnitBus, _u: &str, _n: usize)
///         -> Result<Vec<JournalEntry>, SystemctlError> { Ok(vec![]) }
/// }
///
/// let c: Box<dyn SystemctlClient> = Box::new(Mock);
/// assert!(c.list_units(UnitFilter::default()).unwrap().is_empty());
/// ```
pub trait SystemctlClient: Send + Sync {
    /// List units, applying `filter`.
    fn list_units(&self, filter: UnitFilter) -> Result<Vec<SystemUnit>, SystemctlError>;

    /// Inspect a single unit's status.
    fn status(&self, bus: UnitBus, unit: &str) -> Result<SystemUnit, SystemctlError>;

    /// Start a unit. May return [`SystemctlError::SudoRequired`] for the system bus.
    fn start(&self, bus: UnitBus, unit: &str) -> Result<(), SystemctlError>;

    /// Stop a unit. May return [`SystemctlError::SudoRequired`] for the system bus.
    fn stop(&self, bus: UnitBus, unit: &str) -> Result<(), SystemctlError>;

    /// Restart a unit. May return [`SystemctlError::SudoRequired`] for the system bus.
    fn restart(&self, bus: UnitBus, unit: &str) -> Result<(), SystemctlError>;

    /// Read the last `lines` journal entries for this unit. Bounded; never blocks indefinitely.
    fn journal_tail(
        &self,
        bus: UnitBus,
        unit: &str,
        lines: usize,
    ) -> Result<Vec<JournalEntry>, SystemctlError>;
}
