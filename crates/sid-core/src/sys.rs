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
///     mem_cached: 128,
///     mem_buffers: 0,
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
    /// Page-cache RAM (reclaimable), in bytes; 0 where the platform doesn't report it.
    pub mem_cached: u64,
    /// Buffer RAM, in bytes; 0 where the platform doesn't report it.
    pub mem_buffers: u64,
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
///             mem_cached: 0,
///             mem_buffers: 0,
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

    /// List non-listening TCP connections (established, closing, ...), each attributed
    /// to its owning process. Default: none, so existing impls compile unchanged.
    fn list_connections(&mut self) -> Result<Vec<Connection>, SysError> {
        Ok(Vec::new())
    }

    /// Current CPU/memory for just `pids` - for callers (the Network tab) that never list
    /// the full process table. CPU is a delta, so the first call reads `0.0` and later
    /// calls on the same provider are meaningful. Default: empty.
    fn process_usage(
        &mut self,
        _pids: &[Pid],
    ) -> Result<std::collections::HashMap<Pid, ProcUsage>, SysError> {
        Ok(std::collections::HashMap::new())
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
///     cmd_is_fallback: false,
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
    /// Full command line (argv joined by spaces), or — when that's empty/unavailable
    /// (another user's process without permission to read `/proc/<pid>/cmdline`, or a
    /// kernel thread with no argv at all) — `name` standing in for it, so the column
    /// still carries information instead of a bare dash. See [`Self::cmd_is_fallback`].
    pub cmd: String,
    /// True when [`Self::cmd`] is the process name, not a real command line. UI cells
    /// key their ink off this so a stand-in name reads as dimmer than an actual argv.
    pub cmd_is_fallback: bool,
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

/// One non-listening TCP connection row (established, closing, ...).
///
/// UDP is not listed: `netstat2` has no remote endpoint for UDP sockets, so a "connection"
/// row would be a bound-socket row that `list_listening_ports` already covers.
///
/// # Examples
///
/// ```
/// use sid_core::sys::{Connection, Protocol, SocketState};
/// let c = Connection {
///     protocol: Protocol::Tcp,
///     state: SocketState::Established,
///     state_label: "ESTABLISHED".into(),
///     local_addr: "10.0.0.2".into(),
///     local_port: 50000,
///     remote_addr: "93.184.216.34".into(),
///     remote_port: 443,
///     pid: None,
///     command: String::new(),
/// };
/// assert_eq!(c.remote_port, 443);
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Connection {
    /// Transport protocol (always TCP today).
    pub protocol: Protocol,
    /// Coarse state; `Established` or `Other` (a listener is never a connection row).
    pub state: SocketState,
    /// Precise TCP state for display ("ESTABLISHED", "TIME_WAIT", "CLOSE_WAIT", ...).
    pub state_label: String,
    /// Local address as a printable string.
    pub local_addr: String,
    /// Local port.
    pub local_port: u16,
    /// Remote address as a printable string.
    pub remote_addr: String,
    /// Remote port.
    pub remote_port: u16,
    /// Owning PID, when attributable.
    pub pid: Option<Pid>,
    /// Owning process name; empty when `pid` is `None` or the lookup failed.
    pub command: String,
}

/// A process's current CPU and memory, joined onto socket rows.
///
/// # Examples
///
/// ```
/// use sid_core::sys::ProcUsage;
/// let u = ProcUsage { cpu_pct: 1.5, rss_bytes: 4096 };
/// assert_eq!(u.rss_bytes, 4096);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProcUsage {
    /// CPU percent (delta since the previous refresh; `0.0` on the first sample).
    pub cpu_pct: f32,
    /// Resident set size in bytes.
    pub rss_bytes: u64,
}

/// A socket row plus its owning process's usage (`None` when the PID is unknown or the
/// process is gone / unreadable).
#[derive(Clone, Debug, PartialEq)]
pub struct WithUsage<T> {
    /// The socket row.
    pub row: T,
    /// Owner's CPU/memory, if resolved.
    pub usage: Option<ProcUsage>,
}

/// Index a process snapshot by PID.
///
/// # Examples
///
/// ```
/// use sid_core::sys::{usage_by_pid, Pid, ProcessInfo};
/// let p = ProcessInfo {
///     pid: Pid::from_u32(7), name: "x".into(), cmd: "x".into(), cmd_is_fallback: false,
///     cpu_pct: 2.0, rss_bytes: 10, started_unix_secs: 0, parent: None, user: None,
/// };
/// assert_eq!(usage_by_pid(&[p])[&Pid::from_u32(7)].rss_bytes, 10);
/// ```
pub fn usage_by_pid(procs: &[ProcessInfo]) -> std::collections::HashMap<Pid, ProcUsage> {
    procs
        .iter()
        .map(|p| {
            (
                p.pid,
                ProcUsage {
                    cpu_pct: p.cpu_pct,
                    rss_bytes: p.rss_bytes,
                },
            )
        })
        .collect()
}

/// Join per-process usage onto any rows, keyed by the row's PID. Order is preserved.
pub fn join_usage<T: Clone>(
    rows: &[T],
    pid_of: impl Fn(&T) -> Option<Pid>,
    usage: &std::collections::HashMap<Pid, ProcUsage>,
) -> Vec<WithUsage<T>> {
    rows.iter()
        .map(|r| WithUsage {
            usage: pid_of(r).and_then(|p| usage.get(&p).copied()),
            row: r.clone(),
        })
        .collect()
}

/// Join usage onto listening-port rows.
pub fn ports_with_usage(
    ports: &[ListeningPort],
    usage: &std::collections::HashMap<Pid, ProcUsage>,
) -> Vec<WithUsage<ListeningPort>> {
    join_usage(ports, |p| p.pid, usage)
}

/// Join usage onto connection rows.
pub fn connections_with_usage(
    conns: &[Connection],
    usage: &std::collections::HashMap<Pid, ProcUsage>,
) -> Vec<WithUsage<Connection>> {
    join_usage(conns, |c| c.pid, usage)
}

/// The distinct PIDs owning any of `rows`, sorted - what to hand `process_usage`.
pub fn owner_pids<T>(rows: &[T], pid_of: impl Fn(&T) -> Option<Pid>) -> Vec<Pid> {
    let mut v: Vec<Pid> = rows.iter().filter_map(pid_of).collect();
    v.sort_unstable();
    v.dedup();
    v
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
                    mem_cached: 0,
                    mem_buffers: 0,
                })
            }
        }
        let mut p = Bare;
        assert_eq!(p.default_route_iface_name().unwrap(), None);
    }

    fn lp(port: u16, pid: Option<u32>) -> ListeningPort {
        ListeningPort {
            port,
            pid: pid.map(Pid::from_u32),
            command: String::new(),
            protocol: Protocol::Tcp,
            state: SocketState::Listen,
            local_addr: "0.0.0.0".into(),
        }
    }

    fn proc(pid: u32, cpu: f32, rss: u64) -> ProcessInfo {
        ProcessInfo {
            pid: Pid::from_u32(pid),
            name: "p".into(),
            cmd: "p".into(),
            cmd_is_fallback: false,
            cpu_pct: cpu,
            rss_bytes: rss,
            started_unix_secs: 0,
            parent: None,
            user: None,
        }
    }

    #[test]
    fn usage_joins_onto_ports_by_pid_and_keeps_order() {
        let usage = usage_by_pid(&[proc(1, 5.0, 100), proc(2, 0.5, 200)]);
        let rows = ports_with_usage(
            &[
                lp(80, Some(2)),
                lp(22, Some(1)),
                lp(9, None),
                lp(7, Some(99)),
            ],
            &usage,
        );
        let got: Vec<(u16, Option<u64>)> = rows
            .iter()
            .map(|r| (r.row.port, r.usage.map(|u| u.rss_bytes)))
            .collect();
        assert_eq!(
            got,
            [(80, Some(200)), (22, Some(100)), (9, None), (7, None)]
        );
        assert_eq!(rows[1].usage.unwrap().cpu_pct, 5.0);
    }

    #[test]
    fn usage_joins_onto_connections() {
        let usage = usage_by_pid(&[proc(3, 1.0, 9)]);
        let c = Connection {
            protocol: Protocol::Tcp,
            state: SocketState::Established,
            state_label: "ESTABLISHED".into(),
            local_addr: "1.1.1.1".into(),
            local_port: 1,
            remote_addr: "2.2.2.2".into(),
            remote_port: 2,
            pid: Some(Pid::from_u32(3)),
            command: String::new(),
        };
        let out = connections_with_usage(&[c], &usage);
        assert_eq!(out[0].usage.map(|u| u.rss_bytes), Some(9));
    }

    #[test]
    fn owner_pids_are_sorted_and_distinct() {
        let rows = [lp(1, Some(5)), lp(2, None), lp(3, Some(2)), lp(4, Some(5))];
        assert_eq!(
            owner_pids(&rows, |r| r.pid),
            [Pid::from_u32(2), Pid::from_u32(5)]
        );
    }
}
