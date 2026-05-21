//! State-machine tests for the System tab sub-panel models.

use std::path::PathBuf;

use sid_core::adapters::systemctl::{JournalEntry, SystemUnit, UnitBus, UnitState};
use sid_store::{PinnedConfig, QuickAction, QuickActionScope};
use sid_widgets::system::{
    JournalTailState, PinnedConfigsModal, PinnedConfigsState, QuickActionsModal, QuickActionsState,
    ServicesAction, ServicesState, SystemPane, SystemState, parse_quick_action_cmd,
};

// ─── SystemState ────────────────────────────────────────────────────────────

#[test]
fn initial_focus_is_pinned_configs() {
    let s = SystemState::new();
    assert_eq!(s.focused_pane(), SystemPane::PinnedConfigs);
}

#[test]
fn tab_cycles_focus_through_three_panes() {
    let mut s = SystemState::new();
    s.cycle_focus_forward();
    assert_eq!(s.focused_pane(), SystemPane::Services);
    s.cycle_focus_forward();
    assert_eq!(s.focused_pane(), SystemPane::QuickActions);
    s.cycle_focus_forward();
    assert_eq!(s.focused_pane(), SystemPane::PinnedConfigs);
}

#[test]
fn shift_tab_cycles_backward() {
    let mut s = SystemState::new();
    s.cycle_focus_backward();
    assert_eq!(s.focused_pane(), SystemPane::QuickActions);
    s.cycle_focus_backward();
    assert_eq!(s.focused_pane(), SystemPane::Services);
}

#[test]
fn filter_substring_is_per_pane() {
    let mut s = SystemState::new();
    s.set_filter("nginx".into());
    assert_eq!(s.filter(), Some("nginx"));
    s.cycle_focus_forward();
    assert_eq!(s.filter(), None);
}

#[test]
fn many_forward_cycles_do_not_panic() {
    let mut s = SystemState::new();
    for _ in 0..1000 {
        s.cycle_focus_forward();
    }
    // 1000 % 3 == 1 → starting from PinnedConfigs we land on Services.
    assert_eq!(s.focused_pane(), SystemPane::Services);
}

#[test]
fn very_long_filter_substring_does_not_panic() {
    let mut s = SystemState::new();
    s.set_filter("x".repeat(100_000));
    assert_eq!(s.filter().unwrap().len(), 100_000);
}

#[test]
fn clear_filter_drops_it() {
    let mut s = SystemState::new();
    s.set_filter("x".into());
    s.clear_filter();
    assert!(s.filter().is_none());
}

// ─── PinnedConfigsState ─────────────────────────────────────────────────────

fn pc(p: &str, l: &str) -> PinnedConfig {
    PinnedConfig {
        path: PathBuf::from(p),
        label: l.into(),
        opener_cmd: None,
        created_at: 0,
    }
}

#[test]
fn pinned_configs_state_holds_and_selects() {
    let s = PinnedConfigsState::new(vec![pc("/a", "a"), pc("/b", "b")]);
    assert_eq!(s.selected().unwrap().label, "a");
}

#[test]
fn select_next_and_prev_cycle() {
    let mut s = PinnedConfigsState::new(vec![pc("/a", "a"), pc("/b", "b")]);
    s.select_next();
    assert_eq!(s.selected().unwrap().label, "b");
    s.select_next();
    assert_eq!(s.selected().unwrap().label, "a");
    s.select_prev();
    assert_eq!(s.selected().unwrap().label, "b");
}

#[test]
fn modal_opens_for_add_and_returns_new_record() {
    let s = PinnedConfigsState::new(vec![]);
    let m = s.begin_add();
    assert!(matches!(m, PinnedConfigsModal::Add { .. }));
}

#[test]
fn modal_begins_edit_of_selected() {
    let s = PinnedConfigsState::new(vec![pc("/etc/x", "x")]);
    let m = s.begin_edit_selected().unwrap();
    if let PinnedConfigsModal::Edit { original, .. } = m {
        assert_eq!(original.label, "x");
    } else {
        panic!("expected Edit modal");
    }
}

#[test]
fn modal_returns_none_on_edit_when_empty() {
    let s = PinnedConfigsState::new(vec![]);
    assert!(s.begin_edit_selected().is_none());
}

#[test]
fn filter_narrows_visible_list() {
    let s = PinnedConfigsState::new(vec![
        pc("/etc/nginx.conf", "nginx"),
        pc("/etc/sshd.conf", "ssh"),
    ]);
    let filtered = s.visible(Some("ngi"));
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].label, "nginx");
}

#[test]
fn select_next_on_empty_is_noop() {
    let mut s = PinnedConfigsState::new(vec![]);
    s.select_next();
    s.select_prev();
    assert!(s.selected().is_none());
}

#[test]
fn replace_items_clamps_selected_index() {
    let mut s = PinnedConfigsState::new(vec![pc("/a", "a"), pc("/b", "b"), pc("/c", "c")]);
    s.select_next();
    s.select_next();
    s.replace_items(vec![pc("/x", "x")]);
    assert_eq!(s.selected().unwrap().label, "x");
}

#[test]
fn confirm_delete_returns_target() {
    let s = PinnedConfigsState::new(vec![pc("/a", "a")]);
    let m = s.begin_confirm_delete().unwrap();
    assert!(matches!(m, PinnedConfigsModal::ConfirmDelete { .. }));
}

// ─── ServicesState ──────────────────────────────────────────────────────────

fn unit(name: &str, state: UnitState) -> SystemUnit {
    SystemUnit {
        name: name.into(),
        bus: UnitBus::User,
        state,
        sub_state: "x".into(),
        description: "x".into(),
        load_state: "loaded".into(),
    }
}

#[test]
fn services_state_filters_by_name() {
    let s = ServicesState::new(vec![
        unit("nginx.service", UnitState::Active),
        unit("sshd.service", UnitState::Active),
    ]);
    let v = s.visible(Some("ngi"), None);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].name, "nginx.service");
}

#[test]
fn services_state_filters_by_state() {
    let s = ServicesState::new(vec![
        unit("a", UnitState::Active),
        unit("b", UnitState::Failed),
    ]);
    let v = s.visible(None, Some(UnitState::Failed));
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].name, "b");
}

#[test]
fn open_menu_returns_actions() {
    let mut s = ServicesState::new(vec![unit("x.service", UnitState::Active)]);
    s.open_menu();
    assert!(s.menu_open());
    let actions = ServicesState::menu_actions();
    assert!(actions.contains(&ServicesAction::Start));
    assert!(actions.contains(&ServicesAction::Stop));
    assert!(actions.contains(&ServicesAction::Restart));
    assert!(actions.contains(&ServicesAction::JournalTail));
}

#[test]
fn menu_closes_on_escape() {
    let mut s = ServicesState::new(vec![unit("x", UnitState::Active)]);
    s.open_menu();
    s.close_menu();
    assert!(!s.menu_open());
}

#[test]
fn services_with_200_units_filters_correctly() {
    let mut units = Vec::new();
    for i in 0..200 {
        units.push(unit(
            &format!("svc-{i}.service"),
            if i % 5 == 0 {
                UnitState::Failed
            } else {
                UnitState::Active
            },
        ));
    }
    let s = ServicesState::new(units);
    let failed = s.visible(None, Some(UnitState::Failed));
    assert_eq!(failed.len(), 40);
}

#[test]
fn very_long_unit_name_does_not_panic() {
    let s = ServicesState::new(vec![unit(&"x".repeat(2000), UnitState::Active)]);
    assert_eq!(s.units()[0].name.len(), 2000);
}

#[test]
fn unit_name_with_spaces_is_handled() {
    let s = ServicesState::new(vec![unit("svc with spaces.service", UnitState::Active)]);
    let v = s.visible(Some("with spaces"), None);
    assert_eq!(v.len(), 1);
}

#[test]
fn services_select_next_and_prev_cycle() {
    let mut s = ServicesState::new(vec![
        unit("a", UnitState::Active),
        unit("b", UnitState::Active),
    ]);
    s.select_next();
    assert_eq!(s.selected().unwrap().name, "b");
    s.select_prev();
    assert_eq!(s.selected().unwrap().name, "a");
}

#[test]
fn services_replace_units_clamps_selected_idx() {
    let mut s = ServicesState::new(vec![
        unit("a", UnitState::Active),
        unit("b", UnitState::Active),
        unit("c", UnitState::Active),
    ]);
    s.select_next();
    s.select_next();
    s.replace_units(vec![unit("z", UnitState::Active)]);
    assert_eq!(s.selected().unwrap().name, "z");
}

// ─── JournalTailState ───────────────────────────────────────────────────────

fn je(secs: i64, msg: &str) -> JournalEntry {
    JournalEntry {
        timestamp_secs: secs,
        hostname: "host".into(),
        source: "src".into(),
        message: msg.into(),
    }
}

#[test]
fn journal_tail_initial_state() {
    let s = JournalTailState::new("nginx.service".into(), UnitBus::System);
    assert_eq!(s.unit_name(), "nginx.service");
    assert_eq!(s.bus(), UnitBus::System);
    assert!(!s.is_following());
    assert!(s.entries().is_empty());
}

#[test]
fn journal_tail_replaces_entries_on_reload() {
    let mut s = JournalTailState::new("x".into(), UnitBus::User);
    s.set_entries(vec![je(1, "a"), je(2, "b")]);
    assert_eq!(s.entries().len(), 2);
    s.set_entries(vec![je(3, "c")]);
    assert_eq!(s.entries().len(), 1);
}

#[test]
fn journal_tail_append_in_follow_mode_caps_at_1000() {
    let mut s = JournalTailState::new("x".into(), UnitBus::User);
    s.start_follow();
    assert!(s.is_following());
    for i in 0..1500 {
        s.push_followed(je(i, &format!("msg-{i}")));
    }
    assert!(s.entries().len() <= 1000);
    assert!(!s.entries().iter().any(|e| e.message == "msg-0"));
}

#[test]
fn stop_follow_clears_following_flag() {
    let mut s = JournalTailState::new("x".into(), UnitBus::User);
    s.start_follow();
    s.stop_follow();
    assert!(!s.is_following());
}

#[test]
fn journal_tail_format_snapshot() {
    let mut s = JournalTailState::new("nginx.service".into(), UnitBus::System);
    s.set_entries(vec![
        je(1_748_000_000, "starting"),
        je(1_748_000_005, "ready"),
    ]);
    let rendered: Vec<String> = s
        .entries()
        .iter()
        .map(|e| format!("{:>10}  {}", e.timestamp_secs, e.message))
        .collect();
    insta::assert_debug_snapshot!(rendered);
}

// ─── QuickActionsState ──────────────────────────────────────────────────────

fn qa(label: &str, cmd: &str) -> QuickAction {
    QuickAction {
        id: QuickAction::new_id(),
        label: label.into(),
        scope: QuickActionScope::Global,
        cmd: cmd.into(),
        keybind: None,
    }
}

#[test]
fn quick_actions_state_holds_and_selects() {
    let s = QuickActionsState::new(vec![qa("k", "kill x"), qa("l", "ls")]);
    assert_eq!(s.selected().unwrap().label, "k");
}

#[test]
fn quick_action_select_next_and_prev_cycle() {
    let mut s = QuickActionsState::new(vec![qa("a", "x"), qa("b", "y")]);
    s.select_next();
    assert_eq!(s.selected().unwrap().label, "b");
    s.select_prev();
    assert_eq!(s.selected().unwrap().label, "a");
}

#[test]
fn parse_quick_action_cmd_splits_correctly() {
    let v = parse_quick_action_cmd("fuser -k 5432/tcp").unwrap();
    assert_eq!(v, vec!["fuser", "-k", "5432/tcp"]);
}

#[test]
fn parse_quick_action_cmd_handles_quotes() {
    let v = parse_quick_action_cmd(r#"sh -c "echo 'one two'""#).unwrap();
    assert_eq!(v, vec!["sh", "-c", "echo 'one two'"]);
}

#[test]
fn parse_quick_action_cmd_rejects_malformed_quoting() {
    let r = parse_quick_action_cmd(r#"echo "unclosed"#);
    assert!(r.is_err());
}

#[test]
fn quick_actions_filter_by_label() {
    let s = QuickActionsState::new(vec![
        qa("kill port 5432", "fuser -k 5432/tcp"),
        qa("open scripts", "cd ~/scripts"),
    ]);
    let v = s.visible(Some("port"));
    assert_eq!(v.len(), 1);
}

#[test]
fn quick_actions_modal_add() {
    let s = QuickActionsState::new(vec![]);
    let m = s.begin_add();
    assert!(matches!(m, QuickActionsModal::Add { .. }));
}

#[test]
fn quick_actions_modal_begins_edit_of_selected() {
    let s = QuickActionsState::new(vec![qa("foo", "echo")]);
    let m = s.begin_edit_selected().unwrap();
    if let QuickActionsModal::Edit { original, .. } = m {
        assert_eq!(original.label, "foo");
    } else {
        panic!("expected Edit modal");
    }
}

#[test]
fn quick_action_with_empty_cmd_can_be_added_but_parses_empty() {
    let s = QuickActionsState::new(vec![qa("noop", "")]);
    assert!(
        parse_quick_action_cmd(&s.items()[0].cmd)
            .unwrap()
            .is_empty()
    );
}

use proptest::prelude::*;

proptest! {
    #[test]
    fn parse_quick_action_cmd_never_panics(s in ".*") {
        let _ = parse_quick_action_cmd(&s);
    }
}
