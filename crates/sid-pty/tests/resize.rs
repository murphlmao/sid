use sid_core::adapters::pty::{PtyProvider, PtySize, PtySpawn};
use sid_pty::PortablePtyProvider;

#[test]
fn resize_updates_handle_size() {
    let p = PortablePtyProvider::new();
    let mut h = match p.open_pty(&PtySpawn::command("cat", &[])) {
        Ok(h) => h,
        Err(_) => return,
    };
    assert_eq!(h.size(), PtySize::new(24, 80));
    h.resize(PtySize::new(48, 160)).unwrap();
    assert_eq!(h.size(), PtySize::new(48, 160));
    h.kill().unwrap();
}

#[test]
fn resize_to_zero_does_not_panic() {
    let p = PortablePtyProvider::new();
    let mut h = match p.open_pty(&PtySpawn::command("cat", &[])) {
        Ok(h) => h,
        Err(_) => return,
    };
    let _ = h.resize(PtySize::new(0, 0));
    h.kill().unwrap();
}
