//! Criterion bench for `WorkspacesState::visible_workspaces`.
//!
//! Called multiple times per frame from the render path (tree pane, selection
//! lookup, hint string). 100 µs budget at n=500 per the interaction spec.

use std::hint::black_box;
use std::path::PathBuf;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use sid_core::workspace_metadata::WorkspaceKind;
use sid_store::Workspace;
use sid_widgets::workspaces::WorkspacesState;

fn make_workspaces(n: usize) -> Vec<Workspace> {
    (0..n)
        .map(|i| Workspace {
            path: PathBuf::from(format!("/vcs/repo_{i}")),
            name: format!("repo_{i}"),
            kind: WorkspaceKind::Repo,
            manifest_hash: 0,
            last_seen: 0,
            parent: None,
        })
        .collect()
}

fn bench_visible_workspaces(c: &mut Criterion) {
    let mut group = c.benchmark_group("visible_workspaces");
    for n in [5usize, 50, 500] {
        let ws = make_workspaces(n);
        let state = WorkspacesState::new(ws);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let v = state.visible_workspaces();
                black_box(v.len())
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_visible_workspaces);
criterion_main!(benches);
