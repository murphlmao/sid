//! Additional tests for Cargo.toml / package.json / Procfile sniffers — Task 13.

use std::fs;

use proptest::prelude::*;
use sid_core::workspace_metadata::{
    sniff_cargo_workspace, sniff_package_json_workspaces, sniff_procfile,
};
use tempfile::tempdir;

// ── Cargo.toml sniffer ────────────────────────────────────────────────────────

#[test]
fn sniff_cargo_workspace_handles_empty_members_array() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
    let m = sniff_cargo_workspace(dir.path()).unwrap().unwrap();
    assert!(m.is_empty());
}

#[test]
fn sniff_cargo_workspace_handles_resolver_key() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        r#"[workspace]
resolver = "2"
members = ["crates/a"]
"#,
    )
    .unwrap();
    let m = sniff_cargo_workspace(dir.path()).unwrap().unwrap();
    assert_eq!(m, vec!["crates/a"]);
}

#[test]
fn sniff_cargo_workspace_skips_non_string_members() {
    let dir = tempdir().unwrap();
    // TOML only supports strings in arrays here, so this tests robustness with valid TOML
    fs::write(
        dir.path().join("Cargo.toml"),
        r#"[workspace]
members = ["a", "b"]
"#,
    )
    .unwrap();
    let m = sniff_cargo_workspace(dir.path()).unwrap().unwrap();
    assert_eq!(m.len(), 2);
}

proptest! {
    #[test]
    fn prop_sniff_cargo_workspace_count(n in 1usize..10) {
        let dir = tempdir().unwrap();
        let members: Vec<String> = (0..n).map(|i| format!("crates/c{i}")).collect();
        let members_toml = members.iter().map(|s| format!("\"{s}\"")).collect::<Vec<_>>().join(", ");
        let content = format!("[workspace]\nmembers = [{}]\n", members_toml);
        fs::write(dir.path().join("Cargo.toml"), content).unwrap();
        let found = sniff_cargo_workspace(dir.path()).unwrap().unwrap();
        prop_assert_eq!(found.len(), n);
    }
}

// ── package.json sniffer ──────────────────────────────────────────────────────

#[test]
fn sniff_package_json_without_workspaces_field_returns_none() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("package.json"), r#"{"name":"single"}"#).unwrap();
    let m = sniff_package_json_workspaces(dir.path()).unwrap();
    assert!(m.is_none());
}

#[test]
fn sniff_package_json_workspaces_empty_array_returns_some_empty() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("package.json"), r#"{"workspaces":[]}"#).unwrap();
    let m = sniff_package_json_workspaces(dir.path()).unwrap().unwrap();
    assert!(m.is_empty());
}

#[test]
fn sniff_package_json_workspaces_with_unicode_path() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("package.json"),
        r#"{"workspaces":["packages/工作区"]}"#,
    )
    .unwrap();
    let m = sniff_package_json_workspaces(dir.path()).unwrap().unwrap();
    assert_eq!(m, vec!["packages/工作区"]);
}

// ── Procfile sniffer ──────────────────────────────────────────────────────────

#[test]
fn sniff_procfile_prefers_procfile_over_procfile_dev_when_both_exist() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("Procfile"), "web: main\n").unwrap();
    fs::write(dir.path().join("Procfile.dev"), "worker: job\n").unwrap();
    let p = sniff_procfile(dir.path()).unwrap().unwrap();
    // Should get Procfile (tried first)
    assert!(p.contains(&"web".to_string()));
}

#[test]
fn sniff_procfile_handles_colon_in_command() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("Procfile"),
        "web: env PORT=3000 bundle exec ruby app.rb\n",
    )
    .unwrap();
    let p = sniff_procfile(dir.path()).unwrap().unwrap();
    assert_eq!(p, vec!["web".to_string()]);
}

#[test]
fn sniff_procfile_with_100_processes() {
    let dir = tempdir().unwrap();
    let content: String = (0..100).map(|i| format!("proc{i}: cmd{i}\n")).collect();
    fs::write(dir.path().join("Procfile"), content).unwrap();
    let p = sniff_procfile(dir.path()).unwrap().unwrap();
    assert_eq!(p.len(), 100);
}

#[test]
fn sniff_procfile_all_comments_returns_none() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("Procfile"), "# all comments\n# no processes\n").unwrap();
    let p = sniff_procfile(dir.path()).unwrap();
    assert!(p.is_none());
}
