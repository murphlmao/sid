use proptest::prelude::*;
use sid_core::tab::TabId;
use sid_store::{OpenStore, RedbStore, SessionRecord, Store, now_epoch};
use tempfile::tempdir;

fn make_session(id: &str, active_tab: &str) -> SessionRecord {
    SessionRecord {
        id: id.into(),
        started_at: now_epoch(),
        last_active: now_epoch(),
        ended_at: None,
        active_tab: Some(TabId::new(active_tab)),
        open_tabs: vec![TabId::new(active_tab)],
    }
}

fn make_session_at(id: &str, started_at: u64) -> SessionRecord {
    SessionRecord {
        id: id.into(),
        started_at,
        last_active: started_at,
        ended_at: None,
        active_tab: None,
        open_tabs: vec![],
    }
}

// ── Happy-path tests (plan minimums) ─────────────────────────────────────────

#[test]
fn upsert_and_current_session() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    let s = make_session("abc", "workspaces");
    store.upsert_session(&s).unwrap();
    let got = store.current_session().unwrap().unwrap();
    assert_eq!(got.id, "abc");
    assert_eq!(got.active_tab.as_ref().unwrap().as_str(), "workspaces");
}

#[test]
fn list_sessions_returns_all() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store
        .upsert_session(&make_session("a", "workspaces"))
        .unwrap();
    store.upsert_session(&make_session("b", "ssh")).unwrap();
    let all = store.list_sessions().unwrap();
    assert_eq!(all.len(), 2);
}

#[test]
fn end_session_marks_ended_at() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store
        .upsert_session(&make_session("a", "workspaces"))
        .unwrap();
    store.end_session("a", 12345).unwrap();
    let got = store.list_sessions().unwrap();
    let session = got.iter().find(|s| s.id == "a").unwrap();
    assert_eq!(session.ended_at, Some(12345));
}

// ── Adversarial tests ─────────────────────────────────────────────────────────

#[test]
fn end_session_on_nonexistent_id_is_noop() {
    // Calling end_session for an id that was never upserted must not error.
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    let result = store.end_session("never-existed", 999);
    assert!(result.is_ok(), "end_session on nonexistent id must be Ok");
}

#[test]
fn upsert_overwrites_current_session_pointer() {
    // Each upsert updates the "current" pointer to the new session id.
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store
        .upsert_session(&make_session("first", "workspaces"))
        .unwrap();
    assert_eq!(store.current_session().unwrap().unwrap().id, "first");
    store
        .upsert_session(&make_session("second", "ssh"))
        .unwrap();
    assert_eq!(store.current_session().unwrap().unwrap().id, "second");
}

#[test]
fn list_sessions_preserves_all_fields() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    let tabs = vec![TabId::new("workspaces"), TabId::new("ssh")];
    let s = SessionRecord {
        id: "full".into(),
        started_at: 100,
        last_active: 200,
        ended_at: Some(300),
        active_tab: Some(TabId::new("ssh")),
        open_tabs: tabs.clone(),
    };
    store.upsert_session(&s).unwrap();
    let all = store.list_sessions().unwrap();
    let got = all.iter().find(|r| r.id == "full").unwrap();
    assert_eq!(got.started_at, 100);
    assert_eq!(got.last_active, 200);
    assert_eq!(got.ended_at, Some(300));
    assert_eq!(got.open_tabs, tabs);
}

#[test]
fn upsert_updates_existing_session() {
    // Upserting a session with the same id replaces its data.
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    let mut s = make_session("update-me", "workspaces");
    store.upsert_session(&s).unwrap();
    s.last_active = 999_999;
    s.active_tab = Some(TabId::new("ssh"));
    store.upsert_session(&s).unwrap();
    let got = store.current_session().unwrap().unwrap();
    assert_eq!(got.last_active, 999_999);
    assert_eq!(got.active_tab.as_ref().unwrap().as_str(), "ssh");
}

#[test]
fn restart_and_resume_integration() {
    // Simulates: open → upsert → close (drop) → reopen → current_session
    let dir = tempdir().unwrap();
    let path = dir.path().join("sid.redb");
    {
        let store = RedbStore::open(&path).unwrap();
        let s = make_session("persist-me", "database");
        store.upsert_session(&s).unwrap();
    }
    // Reopen — process restart simulation.
    let store2 = RedbStore::open(&path).unwrap();
    let got = store2.current_session().unwrap().unwrap();
    assert_eq!(got.id, "persist-me");
    assert_eq!(got.active_tab.as_ref().unwrap().as_str(), "database");
}

#[test]
fn no_sessions_returns_empty_list_and_no_current() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    assert!(store.current_session().unwrap().is_none());
    assert!(store.list_sessions().unwrap().is_empty());
}

#[test]
fn many_sessions_all_listed() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    for i in 0..20 {
        store
            .upsert_session(&make_session_at(&format!("s{i}"), i as u64 * 1000))
            .unwrap();
    }
    let all = store.list_sessions().unwrap();
    assert_eq!(all.len(), 20);
}

#[test]
fn end_session_does_not_affect_other_sessions() {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    store
        .upsert_session(&make_session("alpha", "workspaces"))
        .unwrap();
    store.upsert_session(&make_session("beta", "ssh")).unwrap();
    store.end_session("alpha", 42).unwrap();

    let all = store.list_sessions().unwrap();
    let alpha = all.iter().find(|s| s.id == "alpha").unwrap();
    let beta = all.iter().find(|s| s.id == "beta").unwrap();
    assert_eq!(alpha.ended_at, Some(42));
    assert!(beta.ended_at.is_none(), "beta must be unaffected");
}

// ── Property tests: relational invariants ─────────────────────────────────────

proptest! {
    /// upsert_session then current_session is identity for the latest session.
    #[test]
    fn proptest_upsert_then_current_is_identity(
        id in "[a-z]{4,16}",
        started_at in 0u64..1_000_000_000u64,
    ) {
        let dir = tempdir().unwrap();
        let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
        let s = SessionRecord {
            id: id.clone(),
            started_at,
            last_active: started_at + 1,
            ended_at: None,
            active_tab: None,
            open_tabs: vec![],
        };
        store.upsert_session(&s).unwrap();
        let got = store.current_session().unwrap().unwrap();
        prop_assert_eq!(got.id, id);
        prop_assert_eq!(got.started_at, started_at);
        prop_assert_eq!(got.ended_at, None);
    }

    /// list_sessions includes every upserted session.
    #[test]
    fn proptest_list_sessions_includes_all_upserted(
        ids in proptest::collection::vec("[a-z]{4,8}", 1..=10),
    ) {
        // Deduplicate to avoid overwrite noise with same ids.
        let mut unique_ids = ids.clone();
        unique_ids.sort();
        unique_ids.dedup();

        let dir = tempdir().unwrap();
        let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
        for id in &unique_ids {
            let s = SessionRecord {
                id: id.clone(),
                started_at: 0,
                last_active: 0,
                ended_at: None,
                active_tab: None,
                open_tabs: vec![],
            };
            store.upsert_session(&s).unwrap();
        }
        let all = store.list_sessions().unwrap();
        prop_assert_eq!(all.len(), unique_ids.len());
        for id in &unique_ids {
            prop_assert!(
                all.iter().any(|s| &s.id == id),
                "session {id} missing from list_sessions"
            );
        }
    }

    /// end_session only affects the specified session; others are unchanged.
    #[test]
    fn proptest_end_session_only_affects_target(
        id_to_end in "[a-z]{4,8}",
        other_id in "[a-z]{4,8}",
        ended_at in 1u64..999_999u64,
    ) {
        // Ensure distinct ids.
        let id_to_end = format!("end-{id_to_end}");
        let other_id = format!("other-{other_id}");
        prop_assume!(id_to_end != other_id);

        let dir = tempdir().unwrap();
        let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
        for id in &[&id_to_end, &other_id] {
            store.upsert_session(&SessionRecord {
                id: (*id).clone(),
                started_at: 0,
                last_active: 0,
                ended_at: None,
                active_tab: None,
                open_tabs: vec![],
            }).unwrap();
        }
        store.end_session(&id_to_end, ended_at).unwrap();
        let all = store.list_sessions().unwrap();
        let ended = all.iter().find(|s| s.id == id_to_end).unwrap();
        let untouched = all.iter().find(|s| s.id == other_id).unwrap();
        prop_assert_eq!(ended.ended_at, Some(ended_at));
        prop_assert!(untouched.ended_at.is_none(), "other session must be unaffected");
    }
}
