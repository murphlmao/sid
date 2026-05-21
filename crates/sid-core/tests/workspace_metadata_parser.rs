use std::fs;

use sid_core::workspace_metadata::{
    parse_metadata_file, sniff_cargo_workspace, sniff_claude_md, sniff_package_json_workspaces,
    sniff_procfile, WorkspaceKind,
};
use tempfile::tempdir;

// ── Task 11: parse_metadata_file ──────────────────────────────────────────────

#[test]
fn parses_metadata_sid_with_full_content() {
    let dir = tempdir().unwrap();
    let sid_dir = dir.path().join(".sid");
    fs::create_dir(&sid_dir).unwrap();
    let content = r#"{
        "name": "eggsight-stack",
        "kind": "Umbrella",
        "actions": [
            {"label": "Clone all repos", "cmd": "./clone-repos.sh", "key": "c"}
        ],
        "children": ["../eggsight-core", "../eggsight-frontend"]
    }"#;
    fs::write(sid_dir.join("_metadata.sid"), content).unwrap();
    let m = parse_metadata_file(dir.path()).unwrap().unwrap();
    assert_eq!(m.name, "eggsight-stack");
    assert_eq!(m.kind, WorkspaceKind::Umbrella);
    assert_eq!(m.actions.len(), 1);
    assert_eq!(m.actions[0].key, Some('c'));
    assert_eq!(m.children.len(), 2);
}

#[test]
fn returns_none_when_file_missing() {
    let dir = tempdir().unwrap();
    let m = parse_metadata_file(dir.path()).unwrap();
    assert!(m.is_none());
}

#[test]
fn returns_err_on_malformed_json() {
    let dir = tempdir().unwrap();
    let sid_dir = dir.path().join(".sid");
    fs::create_dir(&sid_dir).unwrap();
    fs::write(sid_dir.join("_metadata.sid"), b"{ not valid json").unwrap();
    let err = parse_metadata_file(dir.path()).unwrap_err();
    let _ = format!("{err}");
}

#[test]
fn parses_empty_actions_and_children_when_omitted() {
    let dir = tempdir().unwrap();
    let sid_dir = dir.path().join(".sid");
    fs::create_dir(&sid_dir).unwrap();
    fs::write(sid_dir.join("_metadata.sid"), r#"{"name": "x", "kind": "Repo"}"#).unwrap();
    let m = parse_metadata_file(dir.path()).unwrap().unwrap();
    assert!(m.actions.is_empty());
    assert!(m.children.is_empty());
}

#[test]
fn handles_unicode_workspace_name() {
    let dir = tempdir().unwrap();
    let sid_dir = dir.path().join(".sid");
    fs::create_dir(&sid_dir).unwrap();
    fs::write(
        sid_dir.join("_metadata.sid"),
        r#"{"name": "工作区-🐕", "kind": "Repo"}"#,
    )
    .unwrap();
    let m = parse_metadata_file(dir.path()).unwrap().unwrap();
    assert_eq!(m.name, "工作区-🐕");
}

#[test]
fn handles_metadata_file_with_extra_unknown_fields() {
    let dir = tempdir().unwrap();
    let sid_dir = dir.path().join(".sid");
    fs::create_dir(&sid_dir).unwrap();
    fs::write(
        sid_dir.join("_metadata.sid"),
        r#"{"name": "x", "kind": "Repo", "future_field": 42}"#,
    )
    .unwrap();
    // Should ignore unknown fields (serde default behavior)
    let m = parse_metadata_file(dir.path()).unwrap().unwrap();
    assert_eq!(m.name, "x");
}

#[test]
fn empty_file_returns_err() {
    let dir = tempdir().unwrap();
    let sid_dir = dir.path().join(".sid");
    fs::create_dir(&sid_dir).unwrap();
    fs::write(sid_dir.join("_metadata.sid"), b"").unwrap();
    assert!(parse_metadata_file(dir.path()).is_err());
}

#[test]
fn handles_json_with_null_key() {
    let dir = tempdir().unwrap();
    let sid_dir = dir.path().join(".sid");
    fs::create_dir(&sid_dir).unwrap();
    fs::write(
        sid_dir.join("_metadata.sid"),
        r#"{"name": "x", "kind": "Repo", "actions": [{"label":"a","cmd":"b","key":null}]}"#,
    )
    .unwrap();
    let m = parse_metadata_file(dir.path()).unwrap().unwrap();
    assert_eq!(m.actions[0].key, None);
}

// ── Task 12: sniff_claude_md ──────────────────────────────────────────────────

#[test]
fn sniff_claude_md_extracts_ssh_aliases_table() {
    let dir = tempdir().unwrap();
    let content = r#"
# Project

## Devices — Quick Reference

| Alias | IP | Generation |
|---|---|---|
| `jp46-dev` | 10.1.40.102 | JP4.6 |
| `jp51-5.1` | 10.1.45.183 | JP5.1 |
"#;
    fs::write(dir.path().join("CLAUDE.md"), content).unwrap();
    let snippet = sniff_claude_md(dir.path()).unwrap().unwrap();
    assert!(snippet.ssh_aliases.contains(&"jp46-dev".to_string()));
    assert!(snippet.ssh_aliases.contains(&"jp51-5.1".to_string()));
}

#[test]
fn sniff_claude_md_returns_none_when_missing() {
    let dir = tempdir().unwrap();
    let r = sniff_claude_md(dir.path()).unwrap();
    assert!(r.is_none());
}

#[test]
fn sniff_handles_empty_claude_md() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("CLAUDE.md"), "").unwrap();
    let s = sniff_claude_md(dir.path()).unwrap().unwrap();
    assert!(s.ssh_aliases.is_empty());
}

#[test]
fn sniff_handles_unicode_aliases() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("CLAUDE.md"), "| `🐕-dev` | x | y |\n").unwrap();
    let s = sniff_claude_md(dir.path()).unwrap().unwrap();
    assert!(s.ssh_aliases.contains(&"🐕-dev".to_string()));
}

#[test]
fn sniff_ignores_non_table_lines() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("CLAUDE.md"),
        "# Header\n\nSome paragraph\n\n- list item\n",
    )
    .unwrap();
    let s = sniff_claude_md(dir.path()).unwrap().unwrap();
    assert!(s.ssh_aliases.is_empty());
}

#[test]
fn sniff_does_not_include_divider_rows() {
    let dir = tempdir().unwrap();
    let content = "| Alias | IP |\n|---|---|\n| `host-1` | 1.2.3.4 |\n";
    fs::write(dir.path().join("CLAUDE.md"), content).unwrap();
    let s = sniff_claude_md(dir.path()).unwrap().unwrap();
    // divider row `---` should not be included
    assert!(!s.ssh_aliases.contains(&"---".to_string()));
    assert!(s.ssh_aliases.contains(&"host-1".to_string()));
}

// ── Task 13: sniff_cargo_workspace, sniff_package_json_workspaces, sniff_procfile ──

#[test]
fn sniff_cargo_workspace_returns_members() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        r#"
[workspace]
members = ["crates/a", "crates/b"]
"#,
    )
    .unwrap();
    let m = sniff_cargo_workspace(dir.path()).unwrap().unwrap();
    assert_eq!(m, vec!["crates/a", "crates/b"]);
}

#[test]
fn sniff_package_json_workspaces_returns_list() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("package.json"),
        r#"{
        "name": "monorepo",
        "workspaces": ["packages/*", "apps/web"]
    }"#,
    )
    .unwrap();
    let m = sniff_package_json_workspaces(dir.path()).unwrap().unwrap();
    assert!(m.iter().any(|s| s == "packages/*"));
    assert!(m.iter().any(|s| s == "apps/web"));
}

#[test]
fn sniff_procfile_returns_process_names() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("Procfile"),
        "web: cargo run --bin web\nworker: cargo run --bin worker\n",
    )
    .unwrap();
    let p = sniff_procfile(dir.path()).unwrap().unwrap();
    assert!(p.contains(&"web".to_string()));
    assert!(p.contains(&"worker".to_string()));
}

#[test]
fn sniff_cargo_workspace_missing_section_returns_none() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
    let m = sniff_cargo_workspace(dir.path()).unwrap();
    assert!(m.is_none());
}

#[test]
fn sniff_package_json_workspaces_object_form() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("package.json"),
        r#"{"workspaces":{"packages":["pkgs/*"],"nohoist":[]}}"#,
    )
    .unwrap();
    let m = sniff_package_json_workspaces(dir.path()).unwrap().unwrap();
    assert_eq!(m, vec!["pkgs/*"]);
}

#[test]
fn sniff_procfile_skips_comments_and_empty() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("Procfile"), "# comment\n\nweb: x\n").unwrap();
    let p = sniff_procfile(dir.path()).unwrap().unwrap();
    assert_eq!(p, vec!["web".to_string()]);
}

#[test]
fn sniff_cargo_workspace_missing_file_returns_none() {
    let dir = tempdir().unwrap();
    let m = sniff_cargo_workspace(dir.path()).unwrap();
    assert!(m.is_none());
}

#[test]
fn sniff_package_json_workspaces_missing_file_returns_none() {
    let dir = tempdir().unwrap();
    let m = sniff_package_json_workspaces(dir.path()).unwrap();
    assert!(m.is_none());
}

#[test]
fn sniff_procfile_returns_none_when_missing() {
    let dir = tempdir().unwrap();
    let p = sniff_procfile(dir.path()).unwrap();
    assert!(p.is_none());
}

#[test]
fn sniff_procfile_dev_fallback() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("Procfile.dev"), "web: rails s\nworker: sidekiq\n").unwrap();
    let p = sniff_procfile(dir.path()).unwrap().unwrap();
    assert!(p.contains(&"web".to_string()));
}

#[test]
fn sniff_cargo_workspace_with_malformed_toml_returns_err() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("Cargo.toml"), "this is not valid toml !!!===").unwrap();
    assert!(sniff_cargo_workspace(dir.path()).is_err());
}

#[test]
fn sniff_package_json_malformed_returns_err() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("package.json"), b"{ not json").unwrap();
    assert!(sniff_package_json_workspaces(dir.path()).is_err());
}

// ── Task 14: read_workspace_metadata ─────────────────────────────────────────

use sid_core::workspace_metadata::read_workspace_metadata;

#[test]
fn read_uses_metadata_sid_when_present() {
    let dir = tempdir().unwrap();
    let sid_dir = dir.path().join(".sid");
    fs::create_dir(&sid_dir).unwrap();
    fs::write(
        sid_dir.join("_metadata.sid"),
        r#"{"name":"explicit","kind":"Umbrella"}"#,
    )
    .unwrap();
    let m = read_workspace_metadata(dir.path()).unwrap();
    assert_eq!(m.name, "explicit");
    assert_eq!(m.kind, sid_core::workspace_metadata::WorkspaceKind::Umbrella);
}

#[test]
fn read_falls_back_to_basename_when_nothing_present() {
    let dir = tempdir().unwrap();
    // Don't create .sid, CLAUDE.md, Cargo.toml, or package.json
    let m = read_workspace_metadata(dir.path()).unwrap();
    // Name comes from basename
    let expected_name = dir.path().file_name().unwrap().to_str().unwrap().to_string();
    assert_eq!(m.name, expected_name);
    assert_eq!(m.kind, sid_core::workspace_metadata::WorkspaceKind::Repo);
}

#[test]
fn read_infers_umbrella_from_cargo_workspace_members() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        r#"[workspace]
members = ["crates/a", "crates/b"]"#,
    )
    .unwrap();
    let m = read_workspace_metadata(dir.path()).unwrap();
    assert_eq!(m.kind, sid_core::workspace_metadata::WorkspaceKind::Umbrella);
    assert_eq!(m.children.len(), 2);
}

#[test]
fn read_infers_umbrella_from_package_json_workspaces() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("package.json"),
        r#"{"name":"mono","workspaces":["packages/a","packages/b","packages/c"]}"#,
    )
    .unwrap();
    let m = read_workspace_metadata(dir.path()).unwrap();
    assert_eq!(m.kind, sid_core::workspace_metadata::WorkspaceKind::Umbrella);
    assert_eq!(m.children.len(), 3);
}

#[test]
fn read_metadata_sid_overrides_cargo_workspace() {
    // Even if Cargo.toml says umbrella, explicit metadata wins
    let dir = tempdir().unwrap();
    let sid_dir = dir.path().join(".sid");
    fs::create_dir(&sid_dir).unwrap();
    fs::write(
        sid_dir.join("_metadata.sid"),
        r#"{"name":"override","kind":"Repo"}"#,
    )
    .unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        r#"[workspace]
members = ["a"]"#,
    )
    .unwrap();
    let m = read_workspace_metadata(dir.path()).unwrap();
    assert_eq!(m.name, "override");
    assert_eq!(m.kind, sid_core::workspace_metadata::WorkspaceKind::Repo);
}
