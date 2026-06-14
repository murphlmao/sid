//! Property test: random toggle/undo sequence preserves baseline against
//! a real RedbStore in a TempDir.
//!
//! v1 of branch #5 ships live-apply without an in-binary undo ring; the
//! undo affordance is filed as a follow-up. This test still verifies the
//! "undo" semantics manually (snapshot the previous value, replay it),
//! because that's the invariant any future undo implementation must
//! preserve.

use proptest::prelude::*;
use sid_store::{
    OpenStore, RedbStore, SettingValue, Store, TypedSettings, settings_keys::AUTO_RESTORE_SESSION,
};
use tempfile::TempDir;

#[derive(Clone, Debug)]
enum Op {
    Toggle,
    Undo,
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![Just(Op::Toggle), Just(Op::Undo)]
}

const CHOICES: &[&str] = &["yes", "ask", "no"];

proptest! {
    #[test]
    fn random_toggle_undo_sequence_preserves_baseline(
        ops in prop::collection::vec(op_strategy(), 0..30),
    ) {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("sid.redb");
        let store = RedbStore::open(&db).unwrap();
        // Baseline: "ask" (index 1).
        store.put_string(AUTO_RESTORE_SESSION, "ask").unwrap();

        // Snapshot prev → toggle forward → undo replays prev. Matches the
        // contract the future undo ring will satisfy.
        let mut undo_stack: Vec<SettingValue> = Vec::new();
        let mut current_idx = 1usize;

        for op in &ops {
            match op {
                Op::Toggle => {
                    let prev = store
                        .get_setting(AUTO_RESTORE_SESSION)
                        .unwrap()
                        .unwrap_or_else(|| SettingValue(Vec::new()));
                    current_idx = (current_idx + 1) % CHOICES.len();
                    store
                        .put_string(AUTO_RESTORE_SESSION, CHOICES[current_idx])
                        .unwrap();
                    undo_stack.push(prev);
                }
                Op::Undo => {
                    if let Some(prev) = undo_stack.pop() {
                        store.put_setting(AUTO_RESTORE_SESSION, &prev).unwrap();
                        let v = store.get_string(AUTO_RESTORE_SESSION).unwrap();
                        if let Some(s) = v {
                            current_idx = CHOICES
                                .iter()
                                .position(|c| *c == s)
                                .unwrap_or(1);
                        }
                    }
                }
            }
        }

        // Restore baseline: undo everything still on the stack.
        while let Some(prev) = undo_stack.pop() {
            store.put_setting(AUTO_RESTORE_SESSION, &prev).unwrap();
        }
        let final_val = store.get_string(AUTO_RESTORE_SESSION).unwrap();
        prop_assert_eq!(final_val.as_deref(), Some("ask"));
    }
}
