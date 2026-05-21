use std::path::PathBuf;

use sid_core::adapters::terminal_spawner::{SpawnRequest, SpawnerError, TerminalSpawner};

struct MockSpawner;

impl TerminalSpawner for MockSpawner {
    fn spawn(&self, _req: SpawnRequest) -> Result<(), SpawnerError> {
        Ok(())
    }
    fn name(&self) -> &'static str {
        "mock"
    }
}

#[test]
fn spawner_is_dyn_compatible() {
    let s: Box<dyn TerminalSpawner> = Box::new(MockSpawner);
    s.spawn(SpawnRequest {
        cwd: PathBuf::from("/tmp"),
        cmd: "echo hi".into(),
    })
    .unwrap();
    assert_eq!(s.name(), "mock");
}

#[test]
fn spawner_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Box<dyn TerminalSpawner>>();
}

#[test]
fn spawn_request_construction() {
    let r = SpawnRequest {
        cwd: PathBuf::from("/home/u"),
        cmd: "vim a.txt".into(),
    };
    assert_eq!(r.cwd.to_string_lossy(), "/home/u");
    assert_eq!(r.cmd, "vim a.txt");
}

#[test]
fn spawner_error_variants_format() {
    assert!(format!("{}", SpawnerError::TerminalMissing("kitty".into())).contains("kitty"));
    assert!(format!("{}", SpawnerError::EditorMissing).contains("EDITOR"));
    assert!(format!("{}", SpawnerError::Io("nope".into())).contains("nope"));
    assert!(format!("{}", SpawnerError::Other("x".into())).contains("x"));
}
