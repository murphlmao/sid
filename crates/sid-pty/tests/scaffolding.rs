use sid_pty::{PortablePtyProvider, Vt100Screen};

#[test]
fn provider_constructs() {
    let _ = PortablePtyProvider::new();
    let _: PortablePtyProvider = Default::default();
}

#[test]
fn screen_constructs() {
    let _ = Vt100Screen::new(24, 80);
}
