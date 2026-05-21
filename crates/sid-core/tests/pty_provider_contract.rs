//! Verifies the PtyProvider trait is dyn-compatible and a MockPty can fill it.

use std::sync::Mutex;

use sid_core::adapters::pty::{PtyError, PtyHandle, PtyProvider, PtySize, PtySpawn};

struct MockPty {
    inbox: Mutex<Vec<u8>>,
    outbox: Mutex<Vec<u8>>,
    alive: bool,
    size: PtySize,
}

impl PtyHandle for MockPty {
    fn write(&mut self, bytes: &[u8]) -> Result<usize, PtyError> {
        self.inbox.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn try_read(&mut self) -> Result<Vec<u8>, PtyError> {
        let mut o = self.outbox.lock().unwrap();
        let v = o.clone();
        o.clear();
        Ok(v)
    }
    fn resize(&mut self, size: PtySize) -> Result<(), PtyError> {
        self.size = size;
        Ok(())
    }
    fn child_alive(&self) -> bool {
        self.alive
    }
    fn size(&self) -> PtySize {
        self.size
    }
    fn kill(&mut self) -> Result<(), PtyError> {
        self.alive = false;
        Ok(())
    }
}

struct MockProvider;

impl PtyProvider for MockProvider {
    fn open_pty(&self, _spec: &PtySpawn) -> Result<Box<dyn PtyHandle>, PtyError> {
        Ok(Box::new(MockPty {
            inbox: Mutex::new(Vec::new()),
            outbox: Mutex::new(Vec::new()),
            alive: true,
            size: PtySize { rows: 24, cols: 80 },
        }))
    }
}

#[test]
fn provider_is_dyn_compatible() {
    let p: Box<dyn PtyProvider> = Box::new(MockProvider);
    let mut h = p.open_pty(&PtySpawn::shell()).unwrap();
    let n = h.write(b"echo hi\n").unwrap();
    assert_eq!(n, 8);
    assert!(h.child_alive());
    h.kill().unwrap();
    assert!(!h.child_alive());
}

#[test]
fn provider_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Box<dyn PtyProvider>>();
    assert_send_sync::<Box<dyn PtyHandle>>();
}

#[test]
fn pty_size_construction_and_eq() {
    let a = PtySize { rows: 24, cols: 80 };
    let b = PtySize::new(24, 80);
    assert_eq!(a, b);
}

#[test]
fn pty_spawn_shell_uses_env_shell_when_set() {
    let s = PtySpawn::shell();
    assert!(!s.program.is_empty());
}

#[test]
fn pty_spawn_command_builder() {
    let s = PtySpawn::command("ls", &["-la", "/"]);
    assert_eq!(s.program, "ls");
    assert_eq!(s.args, vec!["-la".to_string(), "/".to_string()]);
}

#[test]
fn open_pty_with_zero_size_does_not_panic() {
    let p = MockProvider;
    let spec = PtySpawn {
        program: "true".into(),
        args: Vec::new(),
        cwd: None,
        env: Default::default(),
        size: PtySize { rows: 0, cols: 0 },
    };
    let _ = p.open_pty(&spec).unwrap();
}

#[test]
fn double_kill_is_idempotent() {
    let p = MockProvider;
    let mut h = p.open_pty(&PtySpawn::shell()).unwrap();
    h.kill().unwrap();
    h.kill().unwrap();
    assert!(!h.child_alive());
}

#[test]
fn write_zero_bytes_returns_zero() {
    let p = MockProvider;
    let mut h = p.open_pty(&PtySpawn::shell()).unwrap();
    assert_eq!(h.write(&[]).unwrap(), 0);
}
