use sid_core::sys::{Connection, ListeningPort, Pid, Protocol, SocketState, SysError};

/// Whether a TCP socket in `state` counts as "listening" for our purposes. Pulled out
/// as a pure function so the netstat2→`ListeningPort` state mapping is unit-testable
/// without constructing a full `ProtocolSocketInfo` (which needs OS-sourced data).
fn tcp_is_listen(state: netstat2::TcpState) -> bool {
    matches!(state, netstat2::TcpState::Listen)
}

/// Map a netstat2 TCP state to sid's coarse state plus a display label. Pure.
pub(crate) fn map_tcp_state(state: netstat2::TcpState) -> (SocketState, &'static str) {
    use netstat2::TcpState as T;
    match state {
        T::Established => (SocketState::Established, "ESTABLISHED"),
        T::Listen => (SocketState::Listen, "LISTEN"),
        T::SynSent => (SocketState::Other, "SYN_SENT"),
        T::SynReceived => (SocketState::Other, "SYN_RECV"),
        T::FinWait1 => (SocketState::Other, "FIN_WAIT1"),
        T::FinWait2 => (SocketState::Other, "FIN_WAIT2"),
        T::TimeWait => (SocketState::Other, "TIME_WAIT"),
        T::Closed => (SocketState::Other, "CLOSED"),
        T::CloseWait => (SocketState::Other, "CLOSE_WAIT"),
        T::LastAck => (SocketState::Other, "LAST_ACK"),
        T::Closing => (SocketState::Other, "CLOSING"),
        _ => (SocketState::Other, "UNKNOWN"),
    }
}

/// PID -> process name for `pids`, via a name-only refresh of just those PIDs (same
/// contract as [`resolve_commands`]: no dead-process pruning).
fn names_for(sys: &mut sysinfo::System, pids: &[Pid]) -> std::collections::HashMap<Pid, String> {
    let sp: Vec<sysinfo::Pid> = pids
        .iter()
        .map(|p| sysinfo::Pid::from_u32(p.as_u32()))
        .collect();
    if sp.is_empty() {
        return Default::default();
    }
    sys.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&sp),
        false,
        sysinfo::ProcessRefreshKind::nothing(),
    );
    pids.iter()
        .filter_map(|p| {
            sys.process(sysinfo::Pid::from_u32(p.as_u32()))
                .map(|pr| (*p, pr.name().to_string_lossy().into_owned()))
        })
        .collect()
}

/// List non-listening TCP connections with owning process names.
pub(crate) fn list_connections(sys: &mut sysinfo::System) -> Result<Vec<Connection>, SysError> {
    use netstat2::{AddressFamilyFlags, ProtocolFlags, ProtocolSocketInfo};

    let af = AddressFamilyFlags::IPV4 | AddressFamilyFlags::IPV6;
    let iter = netstat2::iterate_sockets_info(af, ProtocolFlags::TCP)
        .map_err(|e| SysError::Other(format!("netstat2: {e}")))?;

    let mut out = Vec::new();
    for entry in iter {
        let Ok(info) = entry else { continue };
        let ProtocolSocketInfo::Tcp(t) = info.protocol_socket_info else {
            continue;
        };
        if tcp_is_listen(t.state) {
            continue;
        }
        let (state, label) = map_tcp_state(t.state);
        out.push(Connection {
            protocol: Protocol::Tcp,
            state,
            state_label: label.to_string(),
            local_addr: t.local_addr.to_string(),
            local_port: t.local_port,
            remote_addr: t.remote_addr.to_string(),
            remote_port: t.remote_port,
            pid: info.associated_pids.into_iter().next().map(Pid::from_u32),
            command: String::new(),
        });
    }
    let pids = sid_core::sys::owner_pids(&out, |c| c.pid);
    let names = names_for(sys, &pids);
    for c in &mut out {
        c.command = c
            .pid
            .and_then(|p| names.get(&p).cloned())
            .unwrap_or_default();
    }
    out.sort_by(|a, b| {
        (a.local_port, a.remote_port, &a.remote_addr).cmp(&(
            b.local_port,
            b.remote_port,
            &b.remote_addr,
        ))
    });
    Ok(out)
}

/// CPU + memory for just `pids` (refreshes only those).
pub(crate) fn process_usage(
    sys: &mut sysinfo::System,
    pids: &[Pid],
) -> Result<std::collections::HashMap<Pid, sid_core::sys::ProcUsage>, SysError> {
    let sp: Vec<sysinfo::Pid> = pids
        .iter()
        .map(|p| sysinfo::Pid::from_u32(p.as_u32()))
        .collect();
    if sp.is_empty() {
        return Ok(Default::default());
    }
    sys.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&sp),
        false,
        sysinfo::ProcessRefreshKind::nothing()
            .with_cpu()
            .with_memory(),
    );
    Ok(pids
        .iter()
        .filter_map(|p| {
            sys.process(sysinfo::Pid::from_u32(p.as_u32())).map(|pr| {
                (
                    *p,
                    sid_core::sys::ProcUsage {
                        cpu_pct: pr.cpu_usage(),
                        rss_bytes: pr.memory(),
                    },
                )
            })
        })
        .collect())
}

/// Fill each row's `command` with its owning process's executable name.
///
/// Refreshes only the PIDs that own a socket, with `ProcessRefreshKind::nothing()`
/// (the name comes back regardless — it is basic process info, not an opt-in
/// field) and `remove_dead_processes: false`, so a caller that also lists
/// processes through the same handle does not have its cached process set pruned
/// down to the socket owners.
///
/// A row whose PID is `None`, or whose process exits between the socket
/// enumeration and this refresh, keeps the empty string — the render layer shows
/// `—` for that, which is the honest answer.
fn resolve_commands(sys: &mut sysinfo::System, rows: &mut [ListeningPort]) {
    let mut pids: Vec<sysinfo::Pid> = rows
        .iter()
        .filter_map(|row| row.pid)
        .map(|pid| sysinfo::Pid::from_u32(pid.as_u32()))
        .collect();
    pids.sort_unstable();
    pids.dedup();
    if pids.is_empty() {
        return;
    }
    sys.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&pids),
        false,
        sysinfo::ProcessRefreshKind::nothing(),
    );

    for row in rows.iter_mut() {
        row.command = row
            .pid
            .and_then(|pid| sys.process(sysinfo::Pid::from_u32(pid.as_u32())))
            .map(|proc| proc.name().to_string_lossy().into_owned())
            .unwrap_or_default();
    }
}

/// List sockets in LISTEN state via netstat2, each attributed to its owning
/// process name.
///
/// The PID → name join needs the owning processes to be present in `sys`. A
/// caller that never lists processes — the Network tab owns its own
/// `SysinfoProvider` and only ever asks for ports/interfaces — hands in a handle
/// that knows about no process at all, which is why every row's Process column
/// used to read `—` (bug-hunt 2026-07-26 #10). So refresh exactly the PIDs this
/// enumeration produced, after enumerating: a name-only refresh of a handful of
/// PIDs, not a full process sweep.
pub(crate) fn list_listening_ports(
    sys: &mut sysinfo::System,
) -> Result<Vec<ListeningPort>, SysError> {
    use netstat2::{AddressFamilyFlags, ProtocolFlags, ProtocolSocketInfo};

    let af = AddressFamilyFlags::IPV4 | AddressFamilyFlags::IPV6;
    let pf = ProtocolFlags::TCP | ProtocolFlags::UDP;
    let iter = netstat2::iterate_sockets_info(af, pf)
        .map_err(|e| SysError::Other(format!("netstat2: {e}")))?;

    let mut out = Vec::new();
    for entry in iter {
        let info = match entry {
            Ok(i) => i,
            Err(_) => continue, // skip rows we can't parse rather than failing the whole call
        };
        let (port, proto, local_addr, state, is_listen) = match info.protocol_socket_info {
            ProtocolSocketInfo::Tcp(t) => {
                let is_listen = tcp_is_listen(t.state);
                (
                    t.local_port,
                    Protocol::Tcp,
                    t.local_addr.to_string(),
                    SocketState::Listen,
                    is_listen,
                )
            }
            ProtocolSocketInfo::Udp(u) => (
                u.local_port,
                Protocol::Udp,
                u.local_addr.to_string(),
                SocketState::Listen,
                true, // UDP sockets in the list are de facto "bound and listening"
            ),
        };
        if !is_listen {
            continue;
        }

        let owning_pid = info.associated_pids.into_iter().next().map(Pid::from_u32);

        out.push(ListeningPort {
            port,
            pid: owning_pid,
            command: String::new(), // filled in below, once the PIDs are known
            protocol: proto,
            state,
            local_addr,
        });
    }

    resolve_commands(sys, &mut out);

    out.sort_by(|a, b| {
        a.port
            .cmp(&b.port)
            .then(format!("{:?}", a.protocol).cmp(&format!("{:?}", b.protocol)))
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bug-hunt 2026-07-26 #10: the Network tab's Ports table showed `—` in the
    /// Process column on every row, including the rows whose PID *was* resolved.
    /// The `System` handle the Network tab owns has never listed processes, so
    /// the PID→name join found nothing and `command` came back empty.
    ///
    /// Binds a real listening socket so the join has a PID we can predict: our
    /// own. `System::new()` reproduces the cold handle `SysinfoProvider::new()`
    /// hands in.
    #[test]
    fn owning_process_name_resolves_on_a_cold_system_handle() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let bound = listener.local_addr().unwrap().port();
        let mut sys = sysinfo::System::new().unwrap();

        let ports = list_listening_ports(&mut sys).unwrap();

        let row = ports
            .iter()
            .find(|p| p.port == bound && p.protocol == Protocol::Tcp)
            .expect("our own listening socket should be enumerated");
        assert_eq!(
            row.pid.map(|p| p.as_u32()),
            Some(std::process::id()),
            "the socket we just bound is owned by this test process"
        );
        assert!(
            !row.command.is_empty(),
            "a row with a resolved PID must carry the owning process name"
        );
    }

    #[test]
    fn tcp_state_mapping_labels() {
        assert_eq!(
            map_tcp_state(netstat2::TcpState::Established),
            (SocketState::Established, "ESTABLISHED")
        );
        assert_eq!(
            map_tcp_state(netstat2::TcpState::TimeWait).0,
            SocketState::Other
        );
        assert_eq!(map_tcp_state(netstat2::TcpState::CloseWait).1, "CLOSE_WAIT");
    }

    /// Open a real loopback connection; both ends are ours, so the connection list must
    /// contain it attributed to this process, and usage must resolve for our pid.
    #[test]
    fn established_loopback_connection_is_listed_with_owner_and_usage() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let c = std::net::TcpStream::connect(addr).unwrap();
        let (_srv, _) = l.accept().unwrap();
        let mut sys = sysinfo::System::new().unwrap();
        let conns = list_connections(&mut sys).unwrap();
        let me = std::process::id();
        let row = conns
            .iter()
            .find(|r| {
                r.local_port == c.local_addr().unwrap().port() && r.remote_port == addr.port()
            })
            .expect("our client end should be listed");
        assert_eq!(row.state, SocketState::Established);
        assert_eq!(row.pid.map(|p| p.as_u32()), Some(me));
        assert!(!row.command.is_empty());
        let u = process_usage(&mut sys, &[Pid::from_u32(me)]).unwrap();
        assert!(u[&Pid::from_u32(me)].rss_bytes > 0);
    }

    /// Load-bearing per the plan: the netstat→`ListeningPort` state mapping. Only
    /// `TcpState::Listen` should be surfaced as a listening port.
    #[test]
    fn tcp_listen_state_is_recognized() {
        assert!(tcp_is_listen(netstat2::TcpState::Listen));
    }

    #[test]
    fn other_tcp_states_are_not_listening() {
        assert!(!tcp_is_listen(netstat2::TcpState::Established));
        assert!(!tcp_is_listen(netstat2::TcpState::TimeWait));
        assert!(!tcp_is_listen(netstat2::TcpState::Closed));
    }

    /// The final row sort orders by (port, protocol debug label) — the display order
    /// the ports table relies on before any sortable-header work lands.
    #[test]
    fn ports_sort_by_port_then_protocol() {
        let mut ports = [
            ListeningPort {
                port: 80,
                pid: None,
                command: String::new(),
                protocol: Protocol::Udp,
                state: SocketState::Listen,
                local_addr: "0.0.0.0".into(),
            },
            ListeningPort {
                port: 22,
                pid: None,
                command: String::new(),
                protocol: Protocol::Tcp,
                state: SocketState::Listen,
                local_addr: "0.0.0.0".into(),
            },
            ListeningPort {
                port: 80,
                pid: None,
                command: String::new(),
                protocol: Protocol::Tcp,
                state: SocketState::Listen,
                local_addr: "0.0.0.0".into(),
            },
        ];
        ports.sort_by(|a, b| {
            a.port
                .cmp(&b.port)
                .then(format!("{:?}", a.protocol).cmp(&format!("{:?}", b.protocol)))
        });
        let got: Vec<(u16, Protocol)> = ports.iter().map(|p| (p.port, p.protocol)).collect();
        assert_eq!(
            got,
            vec![
                (22, Protocol::Tcp),
                (80, Protocol::Tcp),
                (80, Protocol::Udp)
            ]
        );
    }
}
