use sid_ssh::read_ssh_config;
use std::fs;
use tempfile::tempdir;

#[test]
fn parses_simple_host_block() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("config");
    fs::write(
        &p,
        "Host jp46-dev\n    HostName 10.1.40.102\n    User pi\n    Port 2222\n    IdentityFile ~/.ssh/id_jp46\n",
    )
    .unwrap();
    let entries = read_ssh_config(&p).unwrap();
    assert_eq!(entries.len(), 1);
    let e = &entries[0];
    assert_eq!(e.host, "jp46-dev");
    assert_eq!(e.hostname.as_deref(), Some("10.1.40.102"));
    assert_eq!(e.user.as_deref(), Some("pi"));
    assert_eq!(e.port, Some(2222));
    assert_eq!(e.identity_file.as_deref(), Some("~/.ssh/id_jp46"));
}

#[test]
fn parses_multiple_host_blocks() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("config");
    fs::write(
        &p,
        "Host a\n    HostName ahost\n\nHost b\n    HostName bhost\n",
    )
    .unwrap();
    let entries = read_ssh_config(&p).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].host, "a");
    assert_eq!(entries[1].host, "b");
}

#[test]
fn parses_proxy_jump() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("config");
    fs::write(
        &p,
        "Host internal\n    HostName 10.0.0.5\n    ProxyJump bastion\n",
    )
    .unwrap();
    let entries = read_ssh_config(&p).unwrap();
    assert_eq!(entries[0].proxy_jump.as_deref(), Some("bastion"));
}

#[test]
fn missing_file_returns_empty() {
    let entries = read_ssh_config(std::path::Path::new("/nonexistent/ssh-config")).unwrap();
    assert!(entries.is_empty());
}

#[test]
fn skips_comments_and_blank_lines() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("config");
    fs::write(
        &p,
        "# top comment\n\nHost real\n    # inline comment\n    HostName r\n",
    )
    .unwrap();
    let entries = read_ssh_config(&p).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].host, "real");
}

#[test]
fn glob_host_patterns_are_kept_as_is() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("config");
    fs::write(&p, "Host *.dev\n    User dev\n").unwrap();
    let entries = read_ssh_config(&p).unwrap();
    assert_eq!(entries[0].host, "*.dev");
    assert_eq!(entries[0].user.as_deref(), Some("dev"));
}

#[test]
fn handles_tabs_and_extra_whitespace() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("config");
    fs::write(&p, "Host\t\ttabbed\n\t  HostName\t10.0.0.1\n").unwrap();
    let entries = read_ssh_config(&p).unwrap();
    assert_eq!(entries[0].host, "tabbed");
    assert_eq!(entries[0].hostname.as_deref(), Some("10.0.0.1"));
}

#[test]
fn malformed_port_is_silently_dropped() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("config");
    fs::write(&p, "Host a\n    Port not-a-number\n").unwrap();
    let entries = read_ssh_config(&p).unwrap();
    assert_eq!(entries[0].host, "a");
    assert_eq!(entries[0].port, None);
}

#[test]
fn unicode_host_names() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("config");
    fs::write(&p, "Host emo-dev\n    HostName real.example.com\n").unwrap();
    let entries = read_ssh_config(&p).unwrap();
    assert_eq!(entries[0].host, "emo-dev");
}

use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_parser_does_not_panic_on_arbitrary_input(s in ".{0,2000}") {
        let dir = tempdir().unwrap();
        let p = dir.path().join("config");
        let _ = fs::write(&p, s);
        let _ = read_ssh_config(&p);
    }
}
