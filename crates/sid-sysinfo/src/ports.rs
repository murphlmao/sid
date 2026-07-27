use sid_core::sys::{ListeningPort, Pid, Protocol, SocketState, SysError};

/// Whether a TCP socket in `state` counts as "listening" for our purposes. Pulled out
/// as a pure function so the netstat2→`ListeningPort` state mapping is unit-testable
/// without constructing a full `ProtocolSocketInfo` (which needs OS-sourced data).
fn tcp_is_listen(state: netstat2::TcpState) -> bool {
    matches!(state, netstat2::TcpState::Listen)
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
        let mut sys = sysinfo::System::new();

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
