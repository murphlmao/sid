//! Tests for `ProcessesTableState` — sort across all five columns, NaN-safe
//! CPU sort, and wrap-around selection.

use proptest::prelude::*;
use sid_core::adapters::sys::{Pid, ProcessInfo};
use sid_widgets::network::ports_table::SortDir;
use sid_widgets::network::processes_table::{ProcessesSortBy, ProcessesTableState};

fn p(pid: u32, name: &str, cpu: f32, rss: u64, started: i64) -> ProcessInfo {
    ProcessInfo {
        pid: Pid::from_u32(pid),
        name: name.into(),
        cmd: name.into(),
        cpu_pct: cpu,
        rss_bytes: rss,
        started_unix_secs: started,
        parent: None,
        user: None,
    }
}

fn sample() -> Vec<ProcessInfo> {
    vec![
        p(100, "zsh", 0.5, 8_000_000, 1_700_000_000),
        p(2, "init", 0.0, 4_000_000, 1_600_000_000),
        p(42, "sid", 5.2, 50_000_000, 1_750_000_000),
    ]
}

#[test]
fn sort_by_pid_ascending() {
    let mut s = ProcessesTableState::new();
    s.set_data(sample());
    s.set_sort(ProcessesSortBy::Pid, SortDir::Asc);
    assert_eq!(
        s.rows().iter().map(|r| r.pid.as_u32()).collect::<Vec<_>>(),
        vec![2, 42, 100]
    );
}

#[test]
fn sort_by_cpu_descending_floats() {
    let mut s = ProcessesTableState::new();
    s.set_data(sample());
    s.set_sort(ProcessesSortBy::Cpu, SortDir::Desc);
    let cpus: Vec<f32> = s.rows().iter().map(|r| r.cpu_pct).collect();
    assert!(cpus[0] >= cpus[1] && cpus[1] >= cpus[2], "{:?}", cpus);
}

#[test]
fn sort_by_name_ascending() {
    let mut s = ProcessesTableState::new();
    s.set_data(sample());
    s.set_sort(ProcessesSortBy::Name, SortDir::Asc);
    let names: Vec<&str> = s.rows().iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, vec!["init", "sid", "zsh"]);
}

#[test]
fn sort_by_rss_ascending() {
    let mut s = ProcessesTableState::new();
    s.set_data(sample());
    s.set_sort(ProcessesSortBy::Rss, SortDir::Asc);
    let rss: Vec<u64> = s.rows().iter().map(|r| r.rss_bytes).collect();
    assert_eq!(rss, vec![4_000_000, 8_000_000, 50_000_000]);
}

#[test]
fn sort_by_started_ascending() {
    let mut s = ProcessesTableState::new();
    s.set_data(sample());
    s.set_sort(ProcessesSortBy::Started, SortDir::Asc);
    let st: Vec<i64> = s.rows().iter().map(|r| r.started_unix_secs).collect();
    assert_eq!(st, vec![1_600_000_000, 1_700_000_000, 1_750_000_000]);
}

#[test]
fn nan_cpu_rows_do_not_panic_on_sort() {
    let mut rows = sample();
    rows.push(p(7, "nanproc", f32::NAN, 0, 0));
    let mut s = ProcessesTableState::new();
    s.set_data(rows);
    s.set_sort(ProcessesSortBy::Cpu, SortDir::Desc);
    assert_eq!(s.rows().len(), 4);
}

#[test]
fn select_next_wraps_at_end() {
    let mut s = ProcessesTableState::new();
    s.set_data(sample());
    s.select_next();
    s.select_next();
    s.select_next();
    assert_eq!(s.selected_index(), 0);
}

#[test]
fn select_prev_wraps_at_start() {
    let mut s = ProcessesTableState::new();
    s.set_data(sample());
    s.select_prev();
    assert_eq!(s.selected_index(), 2);
}

#[test]
fn empty_data_handles_navigation_without_panic() {
    let mut s = ProcessesTableState::new();
    s.set_data(vec![]);
    s.select_next();
    s.select_prev();
    assert!(s.selected_row().is_none());
}

proptest! {
    /// Property: arbitrary NaN/finite mixed CPU values never panic.
    #[test]
    fn prop_arbitrary_cpu_does_not_panic(
        cpus in proptest::collection::vec(
            prop_oneof![
                Just(f32::NAN),
                -1000.0f32..1000.0f32,
            ],
            0..20,
        )
    ) {
        let rows: Vec<_> = cpus.iter().enumerate().map(|(i, c)| {
            p(i as u32 + 1, "x", *c, 0, 0)
        }).collect();
        let mut s = ProcessesTableState::new();
        s.set_data(rows);
        s.set_sort(ProcessesSortBy::Cpu, SortDir::Desc);
        // Just need to not panic; count preserved.
        prop_assert_eq!(s.rows().len(), cpus.len());
    }

    /// Property: selection index always stays within bounds across arbitrary navigation.
    #[test]
    fn prop_selection_in_bounds(actions in proptest::collection::vec(0u8..2, 0..50)) {
        let rows: Vec<_> = (0..5).map(|i| p(i + 1, "x", 0.0, 0, 0)).collect();
        let mut s = ProcessesTableState::new();
        s.set_data(rows);
        for a in actions {
            if a == 0 { s.select_next(); } else { s.select_prev(); }
            prop_assert!(s.selected_index() < 5);
        }
    }
}
