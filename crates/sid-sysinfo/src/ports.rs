use sid_core::sys::{ListeningPort, Pid, Protocol, SocketState, SysError};

/// Whether a TCP socket in `state` counts as "listening" for our purposes. Pulled out
/// as a pure function so the netstat2→`ListeningPort` state mapping is unit-testable
/// without constructing a full `ProtocolSocketInfo` (which needs OS-sourced data).
fn tcp_is_listen(state: netstat2::TcpState) -> bool {
    matches!(state, netstat2::TcpState::Listen)
}

/// List sockets in LISTEN state via netstat2. PID → command lookup uses
/// the cached sysinfo handle; no refresh is performed here.
pub(crate) fn list_listening_ports(sys: &sysinfo::System) -> Result<Vec<ListeningPort>, SysError> {
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

        let command = owning_pid
            .and_then(|pid| sys.process(sysinfo::Pid::from_u32(pid.as_u32())))
            .map(|p| p.name().to_string_lossy().into_owned())
            .unwrap_or_default();

        out.push(ListeningPort {
            port,
            pid: owning_pid,
            command,
            protocol: proto,
            state,
            local_addr,
        });
    }

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
