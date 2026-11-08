//! Per-host SSH command history: cap, dedupe, ordering, persistence.

use sid_store::{GlobalStore, HOST_HISTORY_CAP, HostHistory, StoreError};

fn store() -> (tempfile::TempDir, GlobalStore) {
    let d = tempfile::tempdir().unwrap();
    let s = GlobalStore::open(&d.path().join("sid.redb")).unwrap();
    (d, s)
}

#[test]
fn newest_first_and_per_host_isolated() {
    let (_d, s) = store();
    s.record_host_command("web", "ls", 1).unwrap();
    s.record_host_command("web", "pwd", 2).unwrap();
    s.record_host_command("db", "psql", 3).unwrap();
    let web: Vec<_> = s
        .host_history("web", None, 10)
        .unwrap()
        .into_iter()
        .map(|e| e.command)
        .collect();
    assert_eq!(web, ["pwd", "ls"]);
    assert_eq!(s.host_history("db", None, 10).unwrap().len(), 1);
    assert!(s.host_history("nobody", None, 10).unwrap().is_empty());
}

#[test]
fn consecutive_duplicates_collapse_but_separated_ones_do_not() {
    let (_d, s) = store();
    assert!(s.record_host_command("h", "ls", 1).unwrap());
    assert!(!s.record_host_command("h", "ls", 2).unwrap());
    assert!(s.record_host_command("h", "pwd", 3).unwrap());
    assert!(s.record_host_command("h", "ls", 4).unwrap());
    let h = s.host_history("h", None, 10).unwrap();
    assert_eq!(h.len(), 3);
    let first_ls = h.last().unwrap();
    assert_eq!((first_ls.command.as_str(), first_ls.at_unix), ("ls", 2));
}

#[test]
fn blank_commands_are_ignored() {
    let (_d, s) = store();
    assert!(!s.record_host_command("h", "", 1).unwrap());
    assert!(!s.record_host_command("h", "   ", 1).unwrap());
    assert!(s.host_history("h", None, 10).unwrap().is_empty());
}

#[test]
fn cap_evicts_the_oldest() {
    let (_d, s) = store();
    for i in 0..(HOST_HISTORY_CAP + 25) {
        s.record_host_command("h", &format!("cmd {i}"), i as i64)
            .unwrap();
    }
    let all = s.host_history("h", None, usize::MAX).unwrap();
    assert_eq!(all.len(), HOST_HISTORY_CAP);
    assert_eq!(all[0].command, format!("cmd {}", HOST_HISTORY_CAP + 24));
    assert_eq!(all.last().unwrap().command, "cmd 25");
}

#[test]
fn prefix_search_and_limit() {
    let (_d, s) = store();
    for (i, c) in ["git status", "ls", "git log", "grep x", "git diff"]
        .iter()
        .enumerate()
    {
        s.record_host_command("h", c, i as i64).unwrap();
    }
    let g: Vec<_> = s
        .host_history("h", Some("git"), 10)
        .unwrap()
        .into_iter()
        .map(|e| e.command)
        .collect();
    assert_eq!(g, ["git diff", "git log", "git status"]);
    assert_eq!(s.host_history("h", Some("git"), 2).unwrap().len(), 2);
    assert_eq!(s.host_history("h", Some(""), 10).unwrap().len(), 5);
}

#[test]
fn survives_reopen_and_round_trips() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("sid.redb");
    {
        let s = GlobalStore::open(&path).unwrap();
        s.record_host_command("h", "uptime", 42).unwrap();
    }
    let s = GlobalStore::open(&path).unwrap();
    let h = s.host_history("h", None, 5).unwrap();
    assert_eq!((h[0].command.as_str(), h[0].at_unix), ("uptime", 42));
}

#[test]
fn clear_and_rename() {
    let (_d, s) = store();
    s.record_host_command("old", "ls", 1).unwrap();
    s.rename_host_history("old", "new").unwrap();
    assert!(s.host_history("old", None, 5).unwrap().is_empty());
    assert_eq!(s.host_history("new", None, 5).unwrap().len(), 1);

    s.record_host_command("other", "pwd", 2).unwrap();
    assert!(matches!(
        s.rename_host_history("new", "other"),
        Err(StoreError::Conflict(_))
    ));
    assert_eq!(
        s.host_history("new", None, 5).unwrap().len(),
        1,
        "a refused rename loses nothing"
    );
    s.rename_host_history("ghost", "x").unwrap();

    assert!(s.clear_host_history("new").unwrap());
    assert!(!s.clear_host_history("new").unwrap());
}

#[test]
fn pure_push_matches_store_semantics() {
    let mut h = HostHistory::default();
    assert!(h.push("a", 1));
    assert!(!h.push("a", 2));
    assert_eq!(h.entries.len(), 1);
}
