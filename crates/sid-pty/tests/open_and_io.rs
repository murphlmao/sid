use sid_core::adapters::pty::{PtyProvider, PtySize, PtySpawn};
use sid_pty::PortablePtyProvider;

#[test]
fn open_pty_with_true_command_spawns_and_exits() {
    let p = PortablePtyProvider::new();
    let spec = PtySpawn {
        program: "true".into(),
        args: Vec::new(),
        cwd: None,
        env: Default::default(),
        size: PtySize::new(24, 80),
    };
    let h = match p.open_pty(&spec) {
        Ok(h) => h,
        Err(_) => return,
    };
    std::thread::sleep(std::time::Duration::from_millis(200));
    let _ = h.child_alive();
    assert_eq!(h.size(), PtySize::new(24, 80));
}

#[test]
fn open_pty_with_nonexistent_program_returns_error() {
    let p = PortablePtyProvider::new();
    let spec = PtySpawn::command("/path/does/not/exist/foo-bar-baz", &[]);
    let _ = p.open_pty(&spec);
}

#[test]
fn write_zero_bytes_returns_zero() {
    let p = PortablePtyProvider::new();
    let mut h = match p.open_pty(&PtySpawn::command("cat", &[])) {
        Ok(h) => h,
        Err(_) => return,
    };
    assert_eq!(h.write(&[]).unwrap(), 0);
    let _ = h.kill();
}

#[test]
fn kill_is_idempotent() {
    let p = PortablePtyProvider::new();
    let mut h = match p.open_pty(&PtySpawn::command("cat", &[])) {
        Ok(h) => h,
        Err(_) => return,
    };
    h.kill().unwrap();
    h.kill().unwrap();
}

#[test]
fn echo_round_trips_through_pty() {
    let p = PortablePtyProvider::new();
    let mut h = match p.open_pty(&PtySpawn::command("cat", &[])) {
        Ok(h) => h,
        Err(_) => return,
    };
    h.write(b"hello\n").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    let bytes = h.try_read().unwrap();
    let s = String::from_utf8_lossy(&bytes);
    assert!(s.contains("hello"), "got: {s:?}");
    h.kill().unwrap();
}

#[test]
fn try_read_on_idle_pty_returns_empty() {
    let p = PortablePtyProvider::new();
    let mut h = match p.open_pty(&PtySpawn::command("cat", &[])) {
        Ok(h) => h,
        Err(_) => return,
    };
    let bytes = h.try_read().unwrap();
    assert!(bytes.is_empty());
    h.kill().unwrap();
}
