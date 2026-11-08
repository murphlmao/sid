//! Hostile-repo hardening for `.sid/config.toml`: symlinks, FIFOs, oversize files.
#![cfg(unix)]

use sid_store::entities::{AuthMethod, Host};
use sid_store::workspace::{WorkspaceConfig, WorkspaceStore};
use std::os::unix::fs::symlink;

fn host(alias: &str) -> Host {
    Host {
        alias: alias.into(),
        user: "u".into(),
        host: "h".into(),
        port: 22,
        secret_ref: None,
        auth: AuthMethod::default(),
        folder: None,
    }
}

#[test]
fn symlinked_tmp_is_never_followed_on_save() {
    let dir = tempfile::tempdir().unwrap();
    let victim = dir.path().join("victim.txt");
    std::fs::write(&victim, "SENTINEL").unwrap();
    let ws_root = dir.path().join("ws");
    std::fs::create_dir_all(ws_root.join(".sid")).unwrap();
    symlink(&victim, ws_root.join(".sid/config.toml.tmp")).unwrap();
    let ws = WorkspaceStore::new(&ws_root);
    ws.upsert_host(&host("a")).unwrap();
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "SENTINEL");
    assert_eq!(ws.load().unwrap().ssh.host.len(), 1);
}

#[test]
fn symlinked_config_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("t.toml");
    std::fs::write(&target, "version = 1\n").unwrap();
    let ws_root = dir.path().join("ws");
    std::fs::create_dir_all(ws_root.join(".sid")).unwrap();
    symlink(&target, ws_root.join(".sid/config.toml")).unwrap();
    let ws = WorkspaceStore::new(&ws_root);
    assert!(ws.load().is_err());
    assert!(ws.upsert_host(&host("a")).is_err());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "version = 1\n");
}

#[test]
fn symlinked_sid_dir_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real");
    std::fs::create_dir_all(&real).unwrap();
    let ws_root = dir.path().join("ws");
    std::fs::create_dir_all(&ws_root).unwrap();
    symlink(&real, ws_root.join(".sid")).unwrap();
    let ws = WorkspaceStore::new(&ws_root);
    assert!(ws.load().is_err());
    assert!(ws.save(&WorkspaceConfig::default()).is_err());
    assert!(!real.join("config.toml").exists());
}

#[test]
fn fifo_config_is_refused_without_hanging() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".sid")).unwrap();
    let p = dir.path().join(".sid/config.toml");
    let st = std::process::Command::new("mkfifo")
        .arg(&p)
        .status()
        .unwrap();
    assert!(st.success());
    assert!(WorkspaceStore::new(dir.path()).load().is_err());
}

#[test]
fn oversize_config_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".sid")).unwrap();
    let big = format!("version = 1\n# {}\n", "x".repeat(2 * 1024 * 1024));
    std::fs::write(dir.path().join(".sid/config.toml"), big).unwrap();
    assert!(WorkspaceStore::new(dir.path()).load().is_err());
}
