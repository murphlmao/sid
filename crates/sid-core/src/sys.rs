//! System probe trait + supporting domain types. Implementations live in `sid-sysinfo`.
//!
//! Ported from the `sid-poc` adapter (`sid_core::adapters::sys`), flattened into this
//! module so it sits beside `db.rs`/`ssh.rs`/`term.rs` rather than in a separate
//! `adapters` tree. The Network tab is **live/ephemeral** — nothing here is ever
//! persisted (no store, no scope, no secrets), so unlike the other `sid-core` domain
//! types these carry no `serde` derives.

/// Process identifier. Wraps a `u32` so widget/UI code never has to know whether the
/// underlying probe uses `i32`, `pid_t`, or `usize`.
///
/// # Examples
///
/// ```
/// use sid_core::sys::Pid;
/// let p = Pid::from_u32(42);
/// assert_eq!(p.as_u32(), 42);
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Pid(u32);

impl Pid {
    /// Construct a `Pid` from a raw `u32`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::sys::Pid;
    /// let p = Pid::from_u32(1);
    /// assert_eq!(p.as_u32(), 1);
    /// ```
    pub fn from_u32(v: u32) -> Self {
        Self(v)
    }

    /// Return the raw `u32` PID.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::sys::Pid;
    /// assert_eq!(Pid::from_u32(7).as_u32(), 7);
    /// ```
    pub fn as_u32(self) -> u32 {
        self.0
    }
}

/// Signal kinds accepted by `kill_process`. Keep this list small — anything beyond
/// these is out of scope for inc-1.
///
/// # Examples
///
/// ```
/// use sid_core::sys::Signal;
/// let s = Signal::Term;
/// assert_eq!(s, Signal::Term);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Signal {
    /// SIGTERM — graceful termination request.
    Term,
    /// SIGKILL — uncatchable kill.
    Kill,
    /// SIGINT — interactive interrupt.
    Int,
    /// SIGHUP — hangup, often used to reload config.
    Hup,
}

/// Transport-layer protocol of a listening socket.
///
/// # Examples
///
/// ```
/// use sid_core::sys::Protocol;
/// assert_ne!(Protocol::Tcp, Protocol::Udp);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Protocol {
    /// Transmission Control Protocol.
    Tcp,
    /// User Datagram Protocol.
    Udp,
}

/// State of a socket. Inc-1 lists only LISTEN sockets, but the type carries enough
/// variants to future-proof for "established connections" work.
///
/// # Examples
///
/// ```
/// use sid_core::sys::SocketState;
/// let s = SocketState::Listen;
/// assert_eq!(s, SocketState::Listen);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SocketState {
    /// Socket is in LISTEN state, accepting new connections.
    Listen,
    /// Socket is part of an established connection.
    Established,
    /// Any other transport-layer state.
    Other,
}

/// One listening port row.
///
/// # Examples
///
/// ```
/// use sid_core::sys::{ListeningPort, Protocol, SocketState};
/// let lp = ListeningPort {
///     port: 22,
///     pid: None,
///     command: String::new(),
///     protocol: Protocol::Tcp,
///     state: SocketState::Listen,
///     local_addr: "0.0.0.0".into(),
/// };
/// assert_eq!(lp.port, 22);
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListeningPort {
    /// Port number (1..=65535 in practice; type allows 0 for invalid input).
    pub port: u16,
    /// Owning PID, if attributable. On some platforms the netstat-style API cannot
    /// attribute a socket to a process — in that case this is `None`.
    pub pid: Option<Pid>,
    /// Display command (executable name). Empty string if `pid` is `None` or lookup
    /// failed.
    pub command: String,
    /// Transport protocol.
    pub protocol: Protocol,
    /// Socket state.
    pub state: SocketState,
    /// Local bind address as a printable string ("0.0.0.0", "::", "127.0.0.1").
    pub local_addr: String,
}

/// One network interface row.
///
/// # Examples
///
/// ```
/// use sid_core::sys::NetInterface;
/// let ni = NetInterface {
///     name: "lo".into(),
///     addrs: vec!["127.0.0.1".into()],
///     rx_bytes: 0,
///     tx_bytes: 0,
///     is_up: true,
/// };
/// assert!(ni.is_up);
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NetInterface {
    /// Interface name (e.g., "lo", "eth0").
    pub name: String,
    /// IPv4 + IPv6 addresses bound to this interface.
    pub addrs: Vec<String>,
    /// Bytes received since the system was probed for the first time.
    pub rx_bytes: u64,
    /// Bytes transmitted since the system was probed for the first time.
    pub tx_bytes: u64,
    /// Whether the OS reports the interface as up.
    pub is_up: bool,
}

/// A point-in-time snapshot of host identity plus CPU/memory/load metrics, for the
/// Systems tab's overview cards. Unlike [`ProcessInfo`] rows (a table refreshed
/// wholesale), this is a single struct returned by [`SysProvider::overview`].
///
/// # Examples
///
/// ```
/// use sid_core::sys::SystemOverview;
/// let ov = SystemOverview {
///     hostname: "box".into(),
///     kernel: "6.8.0".into(),
///     os: "Ubuntu 24.04".into(),
///     uptime_secs: 3_600,
///     load_avg: (0.1, 0.2, 0.3),
///     cpu_total_pct: 12.5,
///     cpu_per_core: vec![10.0, 15.0],
///     mem_total: 1_024,
///     mem_used: 512,
///     swap_total: 0,
///     swap_used: 0,
/// };
/// assert_eq!(ov.cpu_per_core.len(), 2);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct SystemOverview {
    /// The machine's hostname.
    pub hostname: String,
    /// Kernel version string (e.g. "6.8.0-48-generic").
    pub kernel: String,
    /// OS/distribution display string (e.g. "Ubuntu 24.04").
    pub os: String,
    /// Seconds since boot.
    pub uptime_secs: u64,
    /// 1/5/15-minute load averages, in that order.
    pub load_avg: (f64, f64, f64),
    /// Aggregate CPU percent across all cores (0..=100).
    pub cpu_total_pct: f32,
    /// Per-core CPU percent, one entry per logical core, in probe-reported order.
    pub cpu_per_core: Vec<f32>,
    /// Total RAM, in bytes.
    pub mem_total: u64,
    /// Used RAM, in bytes.
    pub mem_used: u64,
    /// Total swap, in bytes (0 if no swap is configured).
    pub swap_total: u64,
    /// Used swap, in bytes.
    pub swap_used: u64,
}

/// Domain-shaped system error. Concrete impls map their library errors into this.
///
/// # Examples
///
/// ```
/// use sid_core::sys::SysError;
/// let e = SysError::NotFound("pid 0".into());
/// assert!(format!("{e}").contains("not found"));
/// ```
#[derive(Debug, thiserror::Error)]
pub enum SysError {
    /// e.g., trying to kill a root-owned process as an unprivileged user.
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    /// e.g., the PID doesn't exist (ESRCH from `kill(2)`).
    #[error("not found: {0}")]
    NotFound(String),
    /// e.g., signal value isn't one of the supported variants on this platform.
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// Anything else mapped from the underlying library.
    #[error("system probe error: {0}")]
    Other(String),
}

/// System / network metrics needed by the Network tab. Implementations live in
/// `sid-sysinfo`.
///
/// # Refresh semantics
///
/// Each `list_*` method takes `&mut self` so impls can keep a cached `sysinfo::System`
/// (or similar handle) between calls and only re-refresh the kinds it needs.
/// Implementations MUST be safe to call repeatedly on the same instance and MUST NOT
/// leak file descriptors between calls.
///
/// # Object safety
///
/// All methods take `&mut self` and use no generics in method position, so
/// `Box<dyn SysProvider>` / `Arc<Mutex<dyn SysProvider>>` works.
///
/// # Examples
///
/// ```
/// use sid_core::sys::{
///     ListeningPort, NetInterface, Pid, ProcessInfo, Signal, SysError, SysProvider,
///     SystemOverview,
/// };
///
/// struct Noop;
/// impl SysProvider for Noop {
///     fn list_processes(&mut self) -> Result<Vec<ProcessInfo>, SysError> { Ok(vec![]) }
///     fn list_listening_ports(&mut self) -> Result<Vec<ListeningPort>, SysError> { Ok(vec![]) }
///     fn list_interfaces(&mut self) -> Result<Vec<NetInterface>, SysError> { Ok(vec![]) }
///     fn kill_process(&mut self, _: Pid, _: Signal) -> Result<(), SysError> { Ok(()) }
///     fn overview(&mut self) -> Result<SystemOverview, SysError> {
///         Ok(SystemOverview {
///             hostname: String::new(),
///             kernel: String::new(),
///             os: String::new(),
///             uptime_secs: 0,
///             load_avg: (0.0, 0.0, 0.0),
///             cpu_total_pct: 0.0,
///             cpu_per_core: vec![],
///             mem_total: 0,
///             mem_used: 0,
///             swap_total: 0,
///             swap_used: 0,
///         })
///     }
/// }
///
/// let mut n = Noop;
/// assert!(n.list_processes().unwrap().is_empty());
/// ```
pub trait SysProvider: Send + Sync {
    /// List all visible processes. On Linux, processes outside the caller's namespace
    /// or with restricted `/proc` permissions may be omitted.
    fn list_processes(&mut self) -> Result<Vec<ProcessInfo>, SysError>;

    /// List sockets in `LISTEN` state across TCP and UDP. PID attribution is
    /// best-effort and may be `None` on some platforms / for some sockets.
    fn list_listening_ports(&mut self) -> Result<Vec<ListeningPort>, SysError>;

    /// List network interfaces, including loopback. Addresses include both IPv4 and
    /// IPv6.
    fn list_interfaces(&mut self) -> Result<Vec<NetInterface>, SysError>;

    /// Snapshot host identity plus CPU/memory/load metrics for the Systems tab's
    /// overview cards (see [`SystemOverview`]).
    ///
    /// Like the `list_*` methods, implementations should keep a cached probe handle
    /// between calls. CPU percentages are computed as a delta since the previous
    /// refresh, so `cpu_total_pct`/`cpu_per_core` may read `0.0` on the very first
    /// call and become meaningful from the second call onward — callers on a
    /// recurring refresh loop (e.g. the Systems tab's 2s timer) see accurate values
    /// after the first tick.
    fn overview(&mut self) -> Result<SystemOverview, SysError>;

    /// Send `sig` to `pid`. Maps platform errors:
    /// - `EPERM`/`EACCES` → [`SysError::PermissionDenied`]
    /// - `ESRCH`           → [`SysError::NotFound`]
    /// - anything else     → [`SysError::Other`]
    fn kill_process(&mut self, pid: Pid, sig: Signal) -> Result<(), SysError>;

    /// Return the name of the network interface holding the default route, if one
    /// exists. Used by widgets to sort interfaces with the primary WAN first.
    ///
    /// The default implementation returns `Ok(None)` so existing impls compile
    /// unchanged. Concrete impls override this for their platform (Linux reads
    /// `/proc/net/route`; macOS shells out to `route`).
    fn default_route_iface_name(&mut self) -> Result<Option<String>, SysError> {
        Ok(None)
    }
}

/// One process row.
///
/// # Examples
///
/// ```
/// use sid_core::sys::{Pid, ProcessInfo};
/// let pi = ProcessInfo {
///     pid: Pid::from_u32(1),
///     name: "init".into(),
///     cmd: "/sbin/init".into(),
///     cpu_pct: 0.0,
///     rss_bytes: 0,
///     started_unix_secs: 0,
///     parent: None,
///     user: None,
/// };
/// assert_eq!(pi.name, "init");
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessInfo {
    /// Process identifier.
    pub pid: Pid,
    /// Short name (argv[0] basename).
    pub name: String,
    /// Full command line (argv joined by spaces).
    pub cmd: String,
    /// Aggregate CPU percent (0..=100 per core; >100 possible on multi-core).
    pub cpu_pct: f32,
    /// Resident set size in bytes.
    pub rss_bytes: u64,
    /// Process start time, seconds since UNIX epoch.
    pub started_unix_secs: i64,
    /// Parent process identifier, if known.
    pub parent: Option<Pid>,
    /// User identifier (stringified UID), if known.
    pub user: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Pid` derives `Ord`/`PartialOrd`/`Hash` — used to sort/dedup process lists by
    /// pid elsewhere in the tree. Pin that the derive actually orders by the
    /// wrapped value (not, say, by memory layout of some future added field).
    #[test]
    fn pid_ordering_follows_the_wrapped_value() {
        assert!(Pid::from_u32(1) < Pid::from_u32(2));
        assert!(Pid::from_u32(100) > Pid::from_u32(99));
        assert_eq!(Pid::from_u32(5), Pid::from_u32(5));

        let mut pids = vec![Pid::from_u32(30), Pid::from_u32(10), Pid::from_u32(20)];
        pids.sort();
        assert_eq!(
            pids,
            vec![Pid::from_u32(10), Pid::from_u32(20), Pid::from_u32(30)]
        );
    }

    /// `Pid` derives `Hash` — used as a `HashMap`/`HashSet` key (e.g. pid → command
    /// lookups). Confirm equal pids hash equally, the property a `Hash` impl must
    /// uphold for hash-map lookups to work at all.
    #[test]
    fn pid_equal_values_hash_equally() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        fn hash_of(p: Pid) -> u64 {
            let mut h = DefaultHasher::new();
            p.hash(&mut h);
            h.finish()
        }

        assert_eq!(hash_of(Pid::from_u32(42)), hash_of(Pid::from_u32(42)));
    }

    #[test]
    fn sys_error_messages_carry_their_detail() {
        assert!(
            SysError::InvalidInput("bad signal".into())
                .to_string()
                .contains("bad signal")
        );
        assert!(
            SysError::Other("netstat2: boom".into())
                .to_string()
                .contains("netstat2: boom")
        );
        assert!(
            SysError::InvalidInput(String::new())
                .to_string()
                .contains("invalid input")
        );
        assert!(
            SysError::Other(String::new())
                .to_string()
                .starts_with("system probe error")
        );
    }

    /// `SysProvider::default_route_iface_name` has a default trait-method body
    /// (`Ok(None)`) so pre-existing impls compile unchanged. Pin that default
    /// directly: an impl that doesn't override it must return `Ok(None)`, not
    /// panic or return an error.
    #[test]
    fn default_route_iface_name_default_impl_is_ok_none() {
        struct Bare;
        impl SysProvider for Bare {
            fn list_processes(&mut self) -> Result<Vec<ProcessInfo>, SysError> {
                Ok(vec![])
            }
            fn list_listening_ports(&mut self) -> Result<Vec<ListeningPort>, SysError> {
                Ok(vec![])
            }
            fn list_interfaces(&mut self) -> Result<Vec<NetInterface>, SysError> {
                Ok(vec![])
            }
            fn kill_process(&mut self, _: Pid, _: Signal) -> Result<(), SysError> {
                Ok(())
            }
            fn overview(&mut self) -> Result<SystemOverview, SysError> {
                Ok(SystemOverview {
                    hostname: String::new(),
                    kernel: String::new(),
                    os: String::new(),
                    uptime_secs: 0,
                    load_avg: (0.0, 0.0, 0.0),
                    cpu_total_pct: 0.0,
                    cpu_per_core: vec![],
                    mem_total: 0,
                    mem_used: 0,
                    swap_total: 0,
                    swap_used: 0,
                })
            }
        }
        let mut p = Bare;
        assert_eq!(p.default_route_iface_name().unwrap(), None);
    }
}
