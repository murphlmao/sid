use sid_core::adapters::sys::{ListeningPort, Pid, Protocol, SocketState, SysError};

/// List sockets in LISTEN state via netstat2. PID → command lookup uses
/// the cached sysinfo handle; no refresh is performed here.
pub(crate) fn list_listening_ports(sys: &sysinfo::System) -> Result<Vec<ListeningPort>, SysError> {
    use netstat2::{AddressFamilyFlags, ProtocolFlags, ProtocolSocketInfo, TcpState};

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
                let is_listen = matches!(t.state, TcpState::Listen);
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
