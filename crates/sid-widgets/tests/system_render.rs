//! Insta snapshot tests for [`SystemWidget::render_into_frame`].
//!
//! Each test builds a deterministic widget state (pinned configs, services,
//! quick actions, journal entries) and renders into a fixed `TestBackend`
//! via [`render_to_string`]. The text body is snapshotted so future layout
//! changes surface as a visible diff.
//!
//! The current `SystemPane` enum has three variants (PinnedConfigs, Services,
//! QuickActions). `JournalTailState` is modeled as an overlay/modal surfaced
//! from the Services pane menu, not a fourth pane, so the journal snapshot
//! exercises that overlay path rather than a dedicated pane focus state.

use sid_core::adapters::systemctl::{JournalEntry, SystemUnit, UnitBus, UnitState};
use sid_store::{PinnedConfig, QuickAction, QuickActionScope};
use sid_widgets::{
    SystemWidget,
    system::{JournalTailState, SystemPane, render_to_string},
};

fn pinned(path: &str, label: &str) -> PinnedConfig {
    PinnedConfig {
        path: path.into(),
        label: label.into(),
        opener_cmd: None,
        created_at: 0,
    }
}

fn unit(name: &str, state: UnitState, sub: &str) -> SystemUnit {
    SystemUnit {
        name: name.into(),
        bus: UnitBus::System,
        state,
        sub_state: sub.into(),
        description: String::new(),
        load_state: "loaded".into(),
    }
}

fn quick(label: &str, cmd: &str, key: Option<&str>) -> QuickAction {
    QuickAction {
        // Stable id so the snapshot stays deterministic.
        id: format!("qa-{label}"),
        label: label.into(),
        cmd: cmd.into(),
        keybind: key.map(str::to_string),
        scope: QuickActionScope::Global,
    }
}

#[test]
fn snapshot_default_focus_empty_pinned() {
    let w = SystemWidget::new();
    let s = render_to_string(&w, 80, 12);
    insta::assert_snapshot!("system_default_empty_pinned", s);
}

#[test]
fn snapshot_pinned_configs_with_three_entries_selection() {
    let mut w = SystemWidget::new();
    w.pinned_configs_mut().replace_items(vec![
        pinned("/home/u/.zshrc", "zsh"),
        pinned("/home/u/.config/nvim/init.lua", "nvim"),
        pinned("/etc/hosts", "hosts"),
    ]);
    // Move selection to the second row to make the highlight visible.
    w.pinned_configs_mut().select_next();
    let s = render_to_string(&w, 80, 12);
    insta::assert_snapshot!("system_pinned_three_selected", s);
}

#[test]
fn snapshot_services_pane_focused_with_three_units() {
    let mut w = SystemWidget::new();
    w.state_mut().cycle_focus_forward();
    assert_eq!(w.state().focused_pane(), SystemPane::Services);
    w.services_mut().replace_units(vec![
        unit("nginx.service", UnitState::Active, "running"),
        unit("docker.service", UnitState::Inactive, "dead"),
        unit("postgres.service", UnitState::Failed, "failed"),
    ]);
    let s = render_to_string(&w, 80, 12);
    insta::assert_snapshot!("system_services_three_units", s);
}

#[test]
fn snapshot_journal_overlay_with_five_lines_following() {
    let mut w = SystemWidget::new();
    // Surface the Services pane to make context obvious in the snapshot.
    w.state_mut().cycle_focus_forward();
    w.services_mut()
        .replace_units(vec![unit("nginx.service", UnitState::Active, "running")]);
    let mut j = JournalTailState::new("nginx.service".into(), UnitBus::System);
    j.set_entries(vec![
        JournalEntry {
            timestamp_secs: 1_700_000_001,
            hostname: "h".into(),
            source: "nginx[1]".into(),
            message: "starting up".into(),
        },
        JournalEntry {
            timestamp_secs: 1_700_000_002,
            hostname: "h".into(),
            source: "nginx[1]".into(),
            message: "loaded config".into(),
        },
        JournalEntry {
            timestamp_secs: 1_700_000_003,
            hostname: "h".into(),
            source: "nginx[1]".into(),
            message: "bound :80".into(),
        },
        JournalEntry {
            timestamp_secs: 1_700_000_004,
            hostname: "h".into(),
            source: "nginx[1]".into(),
            message: "worker spawned".into(),
        },
        JournalEntry {
            timestamp_secs: 1_700_000_005,
            hostname: "h".into(),
            source: "nginx[1]".into(),
            message: "ready".into(),
        },
    ]);
    j.start_follow();
    w.set_journal(j);
    let s = render_to_string(&w, 80, 12);
    insta::assert_snapshot!("system_journal_overlay_following", s);
}

#[test]
fn snapshot_quick_actions_pane_focused_with_three_actions() {
    let mut w = SystemWidget::new();
    w.state_mut().cycle_focus_forward();
    w.state_mut().cycle_focus_forward();
    assert_eq!(w.state().focused_pane(), SystemPane::QuickActions);
    w.quick_actions_mut().replace_items(vec![
        quick(
            "Reload nginx",
            "sudo systemctl reload nginx",
            Some("Char('r')"),
        ),
        quick("Tail syslog", "journalctl -f", None),
        quick("Kill 8080", "fuser -k 8080/tcp", Some("Char('k')")),
    ]);
    let s = render_to_string(&w, 80, 12);
    insta::assert_snapshot!("system_quick_actions_three", s);
}

#[test]
fn snapshot_filter_bar_visible_with_active_query() {
    let mut w = SystemWidget::new();
    w.pinned_configs_mut().replace_items(vec![
        pinned("/etc/nginx.conf", "nginx"),
        pinned("/etc/sshd.conf", "sshd"),
    ]);
    w.state_mut().set_filter("ngi".into());
    let s = render_to_string(&w, 80, 12);
    insta::assert_snapshot!("system_filter_active", s);
}
