//! Criterion bench for `WorkspaceDetailWidget` first-frame render.
//!
//! 16 ms wall budget at 5 sub-repos per the interaction spec — that's a
//! 60 Hz frame. Anything slower hitches when the tab opens. The scan
//! itself is off-thread; this benches only the render path the user
//! sees immediately.

use std::{hint::black_box, path::PathBuf};

use criterion::{Criterion, criterion_group, criterion_main};
use ratatui::{Terminal, backend::TestBackend};
use sid_core::workspace_metadata::WorkspaceKind;
use sid_store::Workspace;
use sid_ui::themes::cosmos;
use sid_widgets::{RepoGit, SatelliteRow, workspace_detail::WorkspaceDetailWidget};

fn make_satellites(n: usize) -> Vec<SatelliteRow> {
    (0..n)
        .map(|i| SatelliteRow {
            path: PathBuf::from(format!("/vcs/x/repo_{i}")),
            name: format!("repo_{i}"),
            is_umbrella: false,
            git: RepoGit::loaded("main".into(), 0, 0, 0),
        })
        .collect()
}

fn bench_open_and_render(c: &mut Criterion) {
    let ws = Workspace {
        path: PathBuf::from("/vcs/eggsight-stack"),
        name: "eggsight-stack".into(),
        kind: WorkspaceKind::Umbrella,
        manifest_hash: 0,
        last_seen: 0,
        parent: None,
    };
    let theme = cosmos();
    c.bench_function("workspace_detail_open_5_subrepos_first_frame", |b| {
        b.iter(|| {
            let mut w = WorkspaceDetailWidget::new(ws.clone(), None);
            w.apply_satellites(make_satellites(5));
            let backend = TestBackend::new(120, 40);
            let mut term = Terminal::new(backend).unwrap();
            term.draw(|f| w.render_into_frame(f, f.area(), &theme))
                .unwrap();
            black_box(())
        });
    });
}

criterion_group!(benches, bench_open_and_render);
criterion_main!(benches);
