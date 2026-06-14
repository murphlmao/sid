use std::{path::Path, sync::Mutex};

use sid_system::{env::resolve_editor, kitty::spawn_request_for_file};

// $EDITOR is process-global state — serialize mutation across tests.
static ENV_GUARD: Mutex<()> = Mutex::new(());

fn set_editor(v: Option<&str>) -> EditorRestore<'_> {
    let _g = ENV_GUARD.lock().unwrap();
    let prev = std::env::var("EDITOR").ok();
    let prev_visual = std::env::var("VISUAL").ok();
    unsafe {
        std::env::remove_var("VISUAL");
        match v {
            Some(s) => std::env::set_var("EDITOR", s),
            None => std::env::remove_var("EDITOR"),
        }
    }
    EditorRestore {
        prev,
        prev_visual,
        _guard: _g,
    }
}

struct EditorRestore<'a> {
    prev: Option<String>,
    prev_visual: Option<String>,
    _guard: std::sync::MutexGuard<'a, ()>,
}

impl Drop for EditorRestore<'_> {
    fn drop(&mut self) {
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var("EDITOR", v),
                None => std::env::remove_var("EDITOR"),
            }
            match &self.prev_visual {
                Some(v) => std::env::set_var("VISUAL", v),
                None => std::env::remove_var("VISUAL"),
            }
        }
    }
}

#[test]
fn editor_from_env_overrides_default() {
    let _r = set_editor(Some("nano"));
    let v = resolve_editor().unwrap();
    assert_eq!(v, "nano");
}

#[test]
fn editor_falls_back_to_vi_when_unset() {
    let _r = set_editor(None);
    let r = resolve_editor();
    match r {
        Ok(e) => assert_eq!(e, "vi"),
        Err(sid_core::adapters::terminal_spawner::SpawnerError::EditorMissing) => {}
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn spawn_request_uses_parent_dir_and_editor_cmd() {
    let _r = set_editor(Some("nvim"));
    let req = spawn_request_for_file(Path::new("/etc/nginx/nginx.conf"), None).unwrap();
    assert_eq!(req.cwd.to_string_lossy(), "/etc/nginx");
    assert!(req.cmd.contains("nvim"));
    assert!(req.cmd.contains("nginx.conf"));
}

#[test]
fn spawn_request_uses_explicit_opener_when_provided() {
    let req = spawn_request_for_file(
        Path::new("/etc/x.conf"),
        Some("zellij action edit /etc/x.conf"),
    )
    .unwrap();
    assert_eq!(req.cmd, "zellij action edit /etc/x.conf");
}

#[test]
fn spawn_request_handles_file_in_root() {
    let _r = set_editor(Some("vi"));
    let req = spawn_request_for_file(Path::new("/etc.conf"), None).unwrap();
    assert_eq!(req.cwd.to_string_lossy(), "/");
}

#[test]
fn spawn_request_quotes_filename_with_spaces() {
    let _r = set_editor(Some("vi"));
    let req = spawn_request_for_file(Path::new("/home/u/My Configs/my conf.toml"), None).unwrap();
    assert!(req.cmd.contains("'my conf.toml'") || req.cmd.contains("\"my conf.toml\""));
}

#[test]
fn spawn_request_with_unicode_path() {
    let _r = set_editor(Some("vi"));
    let req = spawn_request_for_file(Path::new("/home/u/🐕/conf.toml"), None).unwrap();
    assert!(req.cwd.to_string_lossy().contains("🐕"));
}
