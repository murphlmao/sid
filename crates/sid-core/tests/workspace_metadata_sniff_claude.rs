//! Additional tests for the CLAUDE.md sniffer — Task 12 adversarial coverage.

use std::fs;

use sid_core::workspace_metadata::sniff_claude_md;
use tempfile::tempdir;

#[test]
fn multiple_tables_in_same_file() {
    let dir = tempdir().unwrap();
    let content = r#"
## Section 1

| Alias | Details |
|---|---|
| `host-a` | 10.0.0.1 |

## Section 2

| Alias | Details |
|---|---|
| `host-b` | 10.0.0.2 |
"#;
    fs::write(dir.path().join("CLAUDE.md"), content).unwrap();
    let s = sniff_claude_md(dir.path()).unwrap().unwrap();
    assert!(s.ssh_aliases.contains(&"host-a".to_string()));
    assert!(s.ssh_aliases.contains(&"host-b".to_string()));
}

#[test]
fn non_backticked_first_columns_are_ignored() {
    let dir = tempdir().unwrap();
    let content = "| plain-host | x | y |\n";
    fs::write(dir.path().join("CLAUDE.md"), content).unwrap();
    let s = sniff_claude_md(dir.path()).unwrap().unwrap();
    // plain (non-backtick) first columns should be ignored
    assert!(s.ssh_aliases.is_empty());
}

#[test]
fn very_large_claude_md_does_not_panic() {
    let dir = tempdir().unwrap();
    let mut content = String::new();
    for i in 0..5000 {
        content.push_str(&format!(
            "| `host-{i}` | 10.0.{}.{} | gen |\n",
            i / 256,
            i % 256
        ));
    }
    fs::write(dir.path().join("CLAUDE.md"), &content).unwrap();
    let s = sniff_claude_md(dir.path()).unwrap().unwrap();
    assert_eq!(s.ssh_aliases.len(), 5000);
}

#[test]
fn alias_with_only_dashes_is_filtered() {
    let dir = tempdir().unwrap();
    // A divider row that happens to be backtick-wrapped
    let content = "| `---` | header |\n";
    fs::write(dir.path().join("CLAUDE.md"), content).unwrap();
    let s = sniff_claude_md(dir.path()).unwrap().unwrap();
    assert!(!s.ssh_aliases.contains(&"---".to_string()));
}

#[test]
fn numeric_alias_is_filtered() {
    let dir = tempdir().unwrap();
    let content = "| `42` | a number |\n";
    fs::write(dir.path().join("CLAUDE.md"), content).unwrap();
    let s = sniff_claude_md(dir.path()).unwrap().unwrap();
    assert!(!s.ssh_aliases.contains(&"42".to_string()));
}
