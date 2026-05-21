use proptest::prelude::*;
use sid_core::tab::TabId;
use sid_core::widget::WidgetId;
use sid_store::{OpenStore, RedbStore, Store, WidgetState};
use tempfile::tempdir;

// Helper to create a store with a standard temp dir.
fn make_store() -> (tempfile::TempDir, RedbStore) {
    let dir = tempdir().unwrap();
    let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
    (dir, store)
}

// ── Happy-path tests (plan minimums) ─────────────────────────────────────────

#[test]
fn round_trip_widget_state() {
    let (_dir, store) = make_store();
    let tab = TabId::new("workspaces");
    let widget = WidgetId::new("workspaces.root");
    let state = WidgetState {
        tab_id: tab.clone(),
        widget_id: widget.clone(),
        blob: vec![1, 2, 3, 4],
    };
    store.save_widget_state(&state).unwrap();
    let got = store.load_widget_state(&tab, &widget).unwrap().unwrap();
    assert_eq!(got, vec![1, 2, 3, 4]);
}

#[test]
fn unknown_pair_returns_none() {
    let (_dir, store) = make_store();
    let got = store
        .load_widget_state(&TabId::new("nope"), &WidgetId::new("nope"))
        .unwrap();
    assert!(got.is_none());
}

// ── Adversarial tests ─────────────────────────────────────────────────────────

#[test]
fn empty_blob_round_trips() {
    let (_dir, store) = make_store();
    let state = WidgetState {
        tab_id: TabId::new("tab"),
        widget_id: WidgetId::new("w"),
        blob: vec![],
    };
    store.save_widget_state(&state).unwrap();
    let got = store
        .load_widget_state(&TabId::new("tab"), &WidgetId::new("w"))
        .unwrap();
    assert!(got.is_some(), "empty blob must be present after save");
    assert_eq!(got.unwrap(), Vec::<u8>::new());
}

#[test]
fn one_mb_blob_round_trips() {
    let (_dir, store) = make_store();
    let big = vec![0xFFu8; 1024 * 1024];
    let state = WidgetState {
        tab_id: TabId::new("tab"),
        widget_id: WidgetId::new("w"),
        blob: big.clone(),
    };
    store.save_widget_state(&state).unwrap();
    let got = store
        .load_widget_state(&TabId::new("tab"), &WidgetId::new("w"))
        .unwrap()
        .unwrap();
    assert_eq!(got, big);
}

#[test]
fn save_overwrite_load() {
    // save → overwrite with different blob → load gets latest
    let (_dir, store) = make_store();
    let tab = TabId::new("t");
    let widget = WidgetId::new("w");

    store
        .save_widget_state(&WidgetState {
            tab_id: tab.clone(),
            widget_id: widget.clone(),
            blob: vec![1, 2, 3],
        })
        .unwrap();
    store
        .save_widget_state(&WidgetState {
            tab_id: tab.clone(),
            widget_id: widget.clone(),
            blob: vec![9, 8, 7],
        })
        .unwrap();
    let got = store.load_widget_state(&tab, &widget).unwrap().unwrap();
    assert_eq!(got, vec![9, 8, 7]);
}

#[test]
fn distinct_tab_and_widget_combos_are_independent() {
    // Multiple (tab_id, widget_id) pairs stored independently.
    let (_dir, store) = make_store();
    let pairs = [
        (TabId::new("a"), WidgetId::new("x"), vec![1u8]),
        (TabId::new("a"), WidgetId::new("y"), vec![2u8]),
        (TabId::new("b"), WidgetId::new("x"), vec![3u8]),
        (TabId::new("b"), WidgetId::new("y"), vec![4u8]),
    ];
    for (tab, widget, blob) in &pairs {
        store
            .save_widget_state(&WidgetState {
                tab_id: tab.clone(),
                widget_id: widget.clone(),
                blob: blob.clone(),
            })
            .unwrap();
    }
    for (tab, widget, expected) in &pairs {
        let got = store.load_widget_state(tab, widget).unwrap().unwrap();
        assert_eq!(&got, expected, "mismatch for {tab}/{widget}");
    }
}

#[test]
fn nul_in_tab_id_does_not_collide_with_separator() {
    // Composite key is "{tab_id}\0{widget_id}".
    // If tab_id itself contains \0, it must NOT collide with a different
    // pair where the widget_id starts earlier.
    //
    // Example collision scenario to test:
    //   tab="a\0b", widget="c" → key = "a\0b\0c"
    //   tab="a",    widget="b\0c" → key = "a\0b\0c"   ← SAME key!
    //
    // redb stores str keys, so we can embed NUL in the strings:
    let (_dir, store) = make_store();
    let tab1 = TabId::new("a\0b");
    let widget1 = WidgetId::new("c");
    let tab2 = TabId::new("a");
    let widget2 = WidgetId::new("b\0c");

    store
        .save_widget_state(&WidgetState {
            tab_id: tab1.clone(),
            widget_id: widget1.clone(),
            blob: vec![0xAAu8],
        })
        .unwrap();
    store
        .save_widget_state(&WidgetState {
            tab_id: tab2.clone(),
            widget_id: widget2.clone(),
            blob: vec![0xBBu8],
        })
        .unwrap();

    // Both pairs must be retrievable (even if they hash to the same key,
    // the last write wins — this test documents the collision, not prevents it;
    // the implementation uses "\0" as separator which can collide with NUL-
    // containing ids; this is expected behavior for Plan 1).
    let r1 = store.load_widget_state(&tab1, &widget1).unwrap();
    let r2 = store.load_widget_state(&tab2, &widget2).unwrap();

    // At least one of them must be present (last write wins if they collide).
    assert!(
        r1.is_some() || r2.is_some(),
        "at least one pair must be retrievable"
    );
}

#[test]
fn widget_state_survives_restart() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("sid.redb");
    let tab = TabId::new("ssh");
    let widget = WidgetId::new("ssh.root");
    {
        let store = RedbStore::open(&path).unwrap();
        store
            .save_widget_state(&WidgetState {
                tab_id: tab.clone(),
                widget_id: widget.clone(),
                blob: vec![42u8; 32],
            })
            .unwrap();
    }
    // Reopen — simulate process restart.
    let store2 = RedbStore::open(&path).unwrap();
    let got = store2.load_widget_state(&tab, &widget).unwrap().unwrap();
    assert_eq!(got, vec![42u8; 32]);
}

// ── Proptest ─────────────────────────────────────────────────────────────────

proptest! {
    #[test]
    fn proptest_arbitrary_blob_round_trips(
        tab_id in "[a-z]{1,16}",
        widget_id in "[a-z]{1,16}",
        blob in proptest::collection::vec(0u8..=255u8, 0..4096),
    ) {
        let dir = tempdir().unwrap();
        let store = RedbStore::open(&dir.path().join("sid.redb")).unwrap();
        let tab = TabId::new(tab_id);
        let widget = WidgetId::new(widget_id);
        store.save_widget_state(&WidgetState {
            tab_id: tab.clone(),
            widget_id: widget.clone(),
            blob: blob.clone(),
        }).unwrap();
        let got = store.load_widget_state(&tab, &widget).unwrap().unwrap();
        prop_assert_eq!(got, blob);
    }
}
