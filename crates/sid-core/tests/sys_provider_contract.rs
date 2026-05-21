//! Verifies the SysProvider trait is dyn-compatible (Box<dyn SysProvider> works)
//! and that a no-op MockProvider can implement every method.

use sid_core::adapters::sys::{
    ListeningPort, NetInterface, Pid, ProcessInfo, Protocol, Signal, SocketState, SysError,
    SysProvider,
};

struct MockProvider;

impl SysProvider for MockProvider {
    fn list_processes(&mut self) -> Result<Vec<ProcessInfo>, SysError> {
        Ok(vec![])
    }
    fn list_listening_ports(&mut self) -> Result<Vec<ListeningPort>, SysError> {
        Ok(vec![])
    }
    fn list_interfaces(&mut self) -> Result<Vec<NetInterface>, SysError> {
        Ok(vec![])
    }
    fn kill_process(&mut self, _pid: Pid, _sig: Signal) -> Result<(), SysError> {
        Ok(())
    }
}

#[test]
fn provider_is_dyn_compatible() {
    let mut p: Box<dyn SysProvider> = Box::new(MockProvider);
    assert!(p.list_processes().unwrap().is_empty());
    assert!(p.list_listening_ports().unwrap().is_empty());
    assert!(p.list_interfaces().unwrap().is_empty());
}

#[test]
fn provider_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Box<dyn SysProvider>>();
}

#[test]
fn protocol_variants_exist() {
    let _ = Protocol::Tcp;
    let _ = Protocol::Udp;
}

#[test]
fn socket_state_variants_exist() {
    let _ = SocketState::Listen;
    let _ = SocketState::Established;
    let _ = SocketState::Other;
}

#[test]
fn signal_variants_exist() {
    let _ = Signal::Term;
    let _ = Signal::Kill;
    let _ = Signal::Int;
    let _ = Signal::Hup;
}

#[test]
fn pid_is_constructable() {
    let p = Pid::from_u32(1234);
    assert_eq!(p.as_u32(), 1234);
}

#[test]
fn process_info_construction() {
    let pi = ProcessInfo {
        pid: Pid::from_u32(42),
        name: "sid".into(),
        cmd: "sid".into(),
        cpu_pct: 0.0,
        rss_bytes: 0,
        started_unix_secs: 0,
        parent: None,
        user: None,
    };
    assert_eq!(pi.pid.as_u32(), 42);
}

#[test]
fn syserror_variants_exist() {
    let _ = SysError::PermissionDenied("kill".into());
    let _ = SysError::NotFound("pid 999".into());
    let _ = SysError::Other("oops".into());
}

#[test]
fn pid_ordering() {
    let a = Pid::from_u32(1);
    let b = Pid::from_u32(2);
    assert!(a < b);
}

#[test]
fn listening_port_eq() {
    let a = ListeningPort {
        port: 80,
        pid: None,
        command: String::new(),
        protocol: Protocol::Tcp,
        state: SocketState::Listen,
        local_addr: "0.0.0.0".into(),
    };
    assert_eq!(a, a.clone());
}

#[test]
fn net_interface_eq() {
    let a = NetInterface {
        name: "lo".into(),
        addrs: vec!["127.0.0.1".into()],
        rx_bytes: 0,
        tx_bytes: 0,
        is_up: true,
    };
    assert_eq!(a, a.clone());
}
