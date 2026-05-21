//! Integration tests for `sid_toml::read_sid_toml` / `write_sid_toml`.

use std::path::PathBuf;

use sid_store::sid_toml::{SidToml, read_sid_toml, write_sid_toml};
use tempfile::tempdir;

#[test]
fn read_returns_default_when_file_absent() {
    let d = tempdir().unwrap();
    let p = d.path().join("sid.toml");
    let got = read_sid_toml(&p).unwrap();
    assert!(got.db_path_override.is_none());
}

#[test]
fn write_then_read_round_trips() {
    let d = tempdir().unwrap();
    let p = d.path().join("sid.toml");
    let cfg = SidToml {
        db_path_override: Some(PathBuf::from("/custom/sid.redb")),
    };
    write_sid_toml(&p, &cfg).unwrap();
    let got = read_sid_toml(&p).unwrap();
    assert_eq!(
        got.db_path_override.as_deref().and_then(|p| p.to_str()),
        Some("/custom/sid.redb")
    );
}

#[test]
fn unknown_keys_are_ignored() {
    let d = tempdir().unwrap();
    let p = d.path().join("sid.toml");
    std::fs::write(&p, "db_path_override = \"/x\"\nunknown_key = \"y\"\n").unwrap();
    let got = read_sid_toml(&p).unwrap();
    assert_eq!(
        got.db_path_override.as_deref().and_then(|p| p.to_str()),
        Some("/x")
    );
}

#[test]
fn malformed_toml_returns_error() {
    let d = tempdir().unwrap();
    let p = d.path().join("sid.toml");
    std::fs::write(&p, "this is = = not valid toml [[[").unwrap();
    assert!(read_sid_toml(&p).is_err());
}

#[test]
fn write_creates_parent_dir() {
    let d = tempdir().unwrap();
    let p = d.path().join("nested/dir/sid.toml");
    let cfg = SidToml {
        db_path_override: Some(PathBuf::from("/x")),
    };
    write_sid_toml(&p, &cfg).unwrap();
    assert!(p.exists());
}

#[test]
fn empty_file_parses_as_default() {
    let d = tempdir().unwrap();
    let p = d.path().join("sid.toml");
    std::fs::write(&p, "").unwrap();
    let got = read_sid_toml(&p).unwrap();
    assert!(got.db_path_override.is_none());
}

#[test]
fn whitespace_only_file_parses_as_default() {
    let d = tempdir().unwrap();
    let p = d.path().join("sid.toml");
    std::fs::write(&p, "   \n\n\t\n").unwrap();
    let got = read_sid_toml(&p).unwrap();
    assert!(got.db_path_override.is_none());
}

#[test]
fn wrong_type_returns_parse_error() {
    let d = tempdir().unwrap();
    let p = d.path().join("sid.toml");
    std::fs::write(&p, "db_path_override = 42\n").unwrap();
    assert!(read_sid_toml(&p).is_err());
}

#[test]
fn long_file_with_one_valid_line_parses() {
    let d = tempdir().unwrap();
    let p = d.path().join("sid.toml");
    let mut body = String::with_capacity(64 * 1024);
    body.push_str("db_path_override = \"/q\"\n");
    for _ in 0..2048 {
        body.push_str("# this is a comment\n");
    }
    std::fs::write(&p, body).unwrap();
    let got = read_sid_toml(&p).unwrap();
    assert_eq!(
        got.db_path_override.as_deref().and_then(|p| p.to_str()),
        Some("/q")
    );
}

#[test]
fn explicit_no_override_round_trips() {
    let d = tempdir().unwrap();
    let p = d.path().join("sid.toml");
    let cfg = SidToml::default();
    write_sid_toml(&p, &cfg).unwrap();
    let got = read_sid_toml(&p).unwrap();
    assert_eq!(got, cfg);
}

#[test]
fn write_with_no_parent_uses_cwd() {
    // path with no parent (just a filename) — write should not blow up.
    let d = tempdir().unwrap();
    let cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(d.path()).unwrap();
    let res = write_sid_toml(
        std::path::Path::new("sid.toml"),
        &SidToml {
            db_path_override: Some(PathBuf::from("/z")),
        },
    );
    std::env::set_current_dir(cwd).unwrap();
    assert!(res.is_ok(), "got {res:?}");
}
