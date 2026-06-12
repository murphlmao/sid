# UX-v2 Branch 1 — Workspaces Rework Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or superpowers:executing-plans. Steps use checkbox syntax for tracking.

**Goal:** Rebuild the Workspaces surface around the gen4-stack umbrella/satellite model on the new UX-v2 substrate. The overview keeps inline tree expansion but Enter on any node (umbrella **or** satellite **or** single repo) pushes a *subrepo detail tab* with one consistent layout. The detail tab shows an umbrella git header (branch · dirty · outgoing), a left list (umbrella row + satellites) whose per-row `GitProvider` data loads off-thread via `sid-job`, and a right `SplitView` drill-in stack (ops → Outgoing / Log / Branches / Stash / Worktrees → commit list → scrollable diff; `←` pops). Registration moves to substrate `FormSpec`/`FormPane` side-pane forms: a create-new wizard with a feature checklist and an adopt-existing directory scan with a pre-checked multi-select confirm. Umbrella↔satellite links persist through the `Store` trait via the existing `Workspace.parent` field — no schema change.

**Architecture:** `sid-widgets` gains pure, testable detail state types (`DetailOp`, `DetailView`, `RepoGit`, `SatelliteRow`) plus a rewritten `WorkspaceDetailWidget` driven by `SplitView<DetailView>` and `ListCursor`. Git data is loaded by the binary off-thread (`sid-job`) and pushed in via `apply_*` setters — widgets never name `git2`/`russh`/`redb` (adapter rule). `sid-core` gains a pure umbrella-satellite scan helper (`scan_adoptable_repos`) reused by both the adopt wizard and discovery. The `sid` binary wires the new job outcomes, the two registration forms (via substrate `open_form`/`dispatch_form_submit`), and the per-row off-thread git loads. All `wire.rs` edits are additive (new functions + single registration call-sites) so the five parallel branches do not collide.

**Tech Stack:** Rust, ratatui (sid-widgets only), crossterm (sid-core only for `Event`), `sid-job` for off-thread git loads, `git2` confined to `sid-git`, insta snapshots, proptest for cursor/stack invariants.

---

## Substrate APIs assumed (from branch 0, merged before this branch runs)

- `sid_widgets::list_cursor::{ListCursor, CursorTarget}` — `ListCursor::new(len, add_new, pos)`, `.target()`, `.up()`, `.down()`, `.total()`.
- `sid_store::settings_keys::SHOW_ADD_NEW_ROW` + `sid::wire::load_show_add_new_row(&dyn Store) -> bool`.
- `sid_widgets::form::{FormSpec, FormSection, FormField, FormId, FormValues, SectionKind, Validate}`; `FormField::new`, `.with_validate`, `FormSpec::new`, `.with_reshape`, `.values()`, `.run_reshape()`, `.first_error()`. `FormValues = BTreeMap<String, String>`.
- `sid_widgets::form::{FormPane, FormEvent, PaneFocusState}` + `sid_widgets::render_form_pane`.
- `sid_widgets::split_view::{SplitView<V>, SplitFocus}` — `.push/.pop/.reroot/.reset/.top/.depth/.focus`.
- `sid_core::tab::TabManager::push_background(tab) -> Result<(), SidError>`.
- `sid_core::event::{KeyChord}` with `.strip_nav()`, `.is_background_open()`; kitty protocol enabled at startup.
- `sid::wire::open_form(&mut SidApp, FormSpec)`; `dispatch_form_submit(&mut SidApp, &str, FormValues)` (substrate ships only the wildcard arm; this branch adds arms); form hosting (`SidApp.form: Option<FormPane>`, `form_origin_tab: Option<TabId>`) + 40/60 split render + `?` help overlay already wired.

These are the only branch-0 symbols this plan leans on. Everything else is grounded in existing source quoted below.

---

### Task 1: Pure umbrella/satellite scan helper in `sid-core`

The adopt-existing wizard and the detail-tab satellite list both need "given an umbrella root, find the nested + symlinked git repos under it". Today `scan_workspace_root` (in `workspace_discovery.rs`) walks with `follow_links(false)` and skips hidden dirs, so it misses symlinked satellites — gen4-stack registers satellites as symlinks. Add a focused, pure helper that finds adoptable repos one level deep including symlinks, returning a stable, sorted result.

**Files:**
- Modify: `crates/sid-core/src/workspace_discovery.rs` (add after `scan_workspace_root`, which ends at line 127; `SKIP_DIRS` is at line 38, `is_umbrella_signal` at line 131)

- [x] **Step 1: Write the failing test first**

Append to the existing `#[cfg(test)] mod tests` block at the bottom of `workspace_discovery.rs` (there is no test mod yet in this file — add one):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn make_repo(dir: &std::path::Path) {
        std::fs::create_dir_all(dir.join(".git")).unwrap();
    }

    #[test]
    fn scan_adoptable_finds_direct_and_symlinked_repos() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        make_repo(&root.join("api"));
        make_repo(&root.join("web"));
        // a symlinked satellite living outside the umbrella, linked in
        let external = tmp.path().join("external-lib");
        make_repo(&external);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&external, root.join("lib")).unwrap();
        // a non-repo dir must be ignored
        std::fs::create_dir_all(root.join("docs")).unwrap();

        let found = scan_adoptable_repos(root);
        let names: Vec<&str> = found.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"api"));
        assert!(names.contains(&"web"));
        #[cfg(unix)]
        assert!(names.contains(&"lib"));
        assert!(!names.contains(&"docs"));
        // sorted by name for deterministic rendering
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
    }

    #[test]
    fn scan_adoptable_skips_the_umbrella_root_itself() {
        let tmp = tempfile::tempdir().unwrap();
        make_repo(tmp.path()); // root is itself a repo
        make_repo(&tmp.path().join("sub"));
        let found = scan_adoptable_repos(tmp.path());
        let names: Vec<&str> = found.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["sub"]);
    }

    #[test]
    fn scan_adoptable_on_missing_dir_is_empty() {
        assert!(scan_adoptable_repos(std::path::Path::new("/nonexistent-xyz")).is_empty());
    }
}
```

- [x] **Step 2: Run it (expect failure — symbol missing)**

Run: `cargo test -p sid-core workspace_discovery::tests::scan_adoptable`
Expected: compile error — `scan_adoptable_repos` not found.

- [x] **Step 3: Implement the helper**

Insert after `scan_workspace_root` (after line 127), before `is_umbrella_signal`:

```rust
/// One adoptable repository found directly under an umbrella root.
///
/// Unlike [`DiscoveredWorkspace`], this carries only the data the adopt-existing
/// wizard needs (display name + absolute path) and is produced by a one-level
/// scan that *does* resolve symlinks — gen4-style umbrellas register satellites
/// as symlinks, which [`scan_workspace_root`] deliberately skips.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use sid_core::workspace_discovery::AdoptableRepo;
///
/// let r = AdoptableRepo { name: "api".into(), path: PathBuf::from("/stack/api") };
/// assert_eq!(r.name, "api");
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptableRepo {
    /// Directory basename (the satellite's display name).
    pub name: String,
    /// Absolute path to the repo (symlink resolved to a real dir).
    pub path: PathBuf,
}

/// Find git repos exactly one level under `umbrella`, following symlinks.
///
/// Returns repos sorted by name for deterministic rendering. The umbrella root
/// itself is never included. A directory counts as a repo if it contains a
/// `.git` entry (directory or file — worktrees use a `.git` file). Best-effort:
/// an unreadable `umbrella` yields an empty vec rather than an error, because
/// the caller (a wizard pre-scan) treats "nothing found" and "couldn't read"
/// identically.
///
/// # Examples
///
/// ```no_run
/// use std::path::Path;
/// use sid_core::workspace_discovery::scan_adoptable_repos;
///
/// let repos = scan_adoptable_repos(Path::new("/home/user/vcs/gen4-stack"));
/// for r in &repos {
///     println!("{} -> {}", r.name, r.path.display());
/// }
/// ```
pub fn scan_adoptable_repos(umbrella: &Path) -> Vec<AdoptableRepo> {
    let mut out: Vec<AdoptableRepo> = Vec::new();
    let Ok(read) = umbrella.read_dir() else {
        return out;
    };
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        // Resolve symlinks: metadata() follows links; symlink targets that are
        // real dirs become candidates.
        let path = entry.path();
        let is_dir = std::fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false);
        if !is_dir {
            continue;
        }
        if path.join(".git").exists() {
            // Canonicalize so a symlinked satellite stores its real path (the
            // primary key for a Workspace record). Fall back to the link path
            // if canonicalization fails (e.g. permission).
            let real = std::fs::canonicalize(&path).unwrap_or(path);
            out.push(AdoptableRepo { name, path: real });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}
```

- [x] **Step 4: Re-run (expect pass)**

Run: `cargo test -p sid-core workspace_discovery::tests::scan_adoptable && cargo test -p sid-core --doc workspace_discovery`
Expected: 3 unit tests + doc tests pass.

- [x] **Step 5: Commit**

```bash
git add crates/sid-core/src/workspace_discovery.rs
git commit -m "feat(sid-core): scan_adoptable_repos — one-level symlink-following repo scan for adopt wizard"
```

---

### Task 2: `RepoGit` + `SatelliteRow` — pure per-row git snapshot state

The detail tab's left list is the umbrella row plus its satellites; each row carries a git snapshot loaded off-thread. Model the snapshot as a pure type the binary fills (so widgets never touch `git2`).

**Files:**
- Create: `crates/sid-widgets/src/workspace_detail_state.rs`
- Modify: `crates/sid-widgets/src/lib.rs` (add `pub mod workspace_detail_state;` after `pub mod workspace_detail;` at line 17, and a re-export line — see Step 4)

- [x] **Step 1: Write the type + failing tests**

```rust
//! Pure state for the Workspaces *detail* tab (UX-v2 rework).
//!
//! The detail tab shows an umbrella row plus its satellites in a left list,
//! each carrying a [`RepoGit`] snapshot loaded off-thread by the binary (so
//! widget code never names `git2`). The right side is a `SplitView` drill-in
//! stack over [`DetailView`]; see `workspace_detail.rs` for the widget that
//! drives these types.

use std::path::PathBuf;

use sid_core::adapters::git::{Branch, CommitInfo, DiffEntry};

/// A git snapshot for one repo row. `None`-ish defaults mean "not loaded yet";
/// the binary replaces this wholesale via the widget's `apply_*` setters once
/// the off-thread load completes.
///
/// # Examples
///
/// ```
/// use sid_widgets::workspace_detail_state::RepoGit;
///
/// let g = RepoGit::loading();
/// assert!(g.is_loading());
/// assert_eq!(g.branch, "…");
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RepoGit {
    /// Current branch name, or `"…"` while loading, `"?"` if detached/failed.
    pub branch: String,
    /// Files with uncommitted changes.
    pub dirty: u32,
    /// Commits ahead of upstream (the "outgoing" count shown in the header).
    pub outgoing: u32,
    /// Commits behind upstream.
    pub behind: u32,
    /// True until the off-thread load lands.
    loading: bool,
}

impl RepoGit {
    /// The pre-load placeholder snapshot.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::workspace_detail_state::RepoGit;
    /// assert!(RepoGit::loading().is_loading());
    /// ```
    pub fn loading() -> Self {
        Self {
            branch: "…".to_string(),
            dirty: 0,
            outgoing: 0,
            behind: 0,
            loading: true,
        }
    }

    /// Build a loaded snapshot.
    pub fn loaded(branch: String, dirty: u32, outgoing: u32, behind: u32) -> Self {
        Self {
            branch,
            dirty,
            outgoing,
            behind,
            loading: false,
        }
    }

    /// Whether the off-thread load is still pending.
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// One-line header summary: `main · 3 dirty · ↑2`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_widgets::workspace_detail_state::RepoGit;
    /// let g = RepoGit::loaded("main".into(), 3, 2, 0);
    /// assert_eq!(g.header_summary(), "main · 3 dirty · ↑2");
    /// let clean = RepoGit::loaded("main".into(), 0, 0, 0);
    /// assert_eq!(clean.header_summary(), "main · clean");
    /// ```
    pub fn header_summary(&self) -> String {
        let dirty = if self.dirty == 0 {
            "clean".to_string()
        } else {
            format!("{} dirty", self.dirty)
        };
        let mut s = format!("{} · {dirty}", self.branch);
        if self.outgoing > 0 {
            s.push_str(&format!(" · ↑{}", self.outgoing));
        }
        if self.behind > 0 {
            s.push_str(&format!(" · ↓{}", self.behind));
        }
        s
    }
}

/// One row in the detail tab's left list: a repo (umbrella or satellite) plus
/// its git snapshot.
///
/// # Examples
///
/// ```
/// use std::path::PathBuf;
/// use sid_widgets::workspace_detail_state::{RepoGit, SatelliteRow};
///
/// let row = SatelliteRow {
///     name: "api".into(),
///     path: PathBuf::from("/stack/api"),
///     is_umbrella: false,
///     git: RepoGit::loading(),
/// };
/// assert_eq!(row.name, "api");
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SatelliteRow {
    /// Display name (umbrella name or satellite basename).
    pub name: String,
    /// Absolute repo path — the key the binary loads git data against.
    pub path: PathBuf,
    /// True for the single umbrella row that heads the list.
    pub is_umbrella: bool,
    /// Off-thread git snapshot for this row.
    pub git: RepoGit,
}

/// Loaded git detail for the currently-drilled repo (Outgoing/Log/Branches/etc.).
/// The binary fills these via the widget's `apply_*` setters as the user drills.
///
/// # Examples
///
/// ```
/// use sid_widgets::workspace_detail_state::RepoDetail;
/// let d = RepoDetail::default();
/// assert!(d.branches.is_empty());
/// assert!(d.commits.is_empty());
/// ```
#[derive(Clone, Debug, Default)]
pub struct RepoDetail {
    /// Branches (Branches view).
    pub branches: Vec<Branch>,
    /// Commits — populated for both Outgoing and Log views.
    pub commits: Vec<CommitInfo>,
    /// Per-file diff entries for the diff view.
    pub diff: Vec<DiffEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loading_default_renders_ellipsis_branch() {
        let g = RepoGit::loading();
        assert!(g.is_loading());
        assert_eq!(g.branch, "…");
        assert_eq!(g.header_summary(), "… · clean");
    }

    #[test]
    fn header_summary_covers_all_arms() {
        assert_eq!(
            RepoGit::loaded("dev".into(), 0, 0, 0).header_summary(),
            "dev · clean"
        );
        assert_eq!(
            RepoGit::loaded("dev".into(), 1, 0, 0).header_summary(),
            "dev · 1 dirty"
        );
        assert_eq!(
            RepoGit::loaded("dev".into(), 0, 5, 2).header_summary(),
            "dev · clean · ↑5 · ↓2"
        );
    }

    #[test]
    fn default_repogit_is_not_loading() {
        // Default (derive) leaves loading=false; only `loading()` sets it true.
        assert!(!RepoGit::default().is_loading());
    }
}
```

- [x] **Step 2: Run (expect failure — module not declared)**

Run: `cargo test -p sid-widgets workspace_detail_state`
Expected: error — module not found.

- [x] **Step 3: Wire the module**

In `crates/sid-widgets/src/lib.rs`, after line 17 (`pub mod workspace_detail;`):

```rust
pub mod workspace_detail_state;
```

And after the existing `pub use workspace_detail::{CiStatus, RepoSummary, WorkspaceDetailWidget};` (line 26):

```rust
pub use workspace_detail_state::{RepoDetail, RepoGit, SatelliteRow};
```

- [x] **Step 4: Re-run (expect pass)**

Run: `cargo test -p sid-widgets workspace_detail_state && cargo test -p sid-widgets --doc workspace_detail_state`
Expected: 3 unit tests + 5 doc tests pass.

- [x] **Step 5: Commit**

```bash
git add crates/sid-widgets/src/workspace_detail_state.rs crates/sid-widgets/src/lib.rs
git commit -m "feat(sid-widgets): RepoGit/SatelliteRow/RepoDetail — pure off-thread git snapshot state for detail tab"
```

---

### Task 3: `DetailOp` + `DetailView` — the drill-in op set and view stack payload

The right pane's `SplitView<DetailView>` stack drills: ops menu → one of {Outgoing, Log, Branches, Stash, Worktrees} → commit list → diff. Model the op menu and the view-stack payload as pure enums.

**Files:**
- Modify: `crates/sid-widgets/src/workspace_detail_state.rs` (append to the module + test mod)

- [x] **Step 1: Failing tests first**

Append to the `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn detail_op_cycles_and_labels() {
        assert_eq!(DetailOp::ALL.len(), 5);
        assert_eq!(DetailOp::Outgoing.label(), "Outgoing");
        assert_eq!(DetailOp::Worktrees.label(), "Worktrees");
        // ALL is the stable render order
        assert_eq!(DetailOp::ALL[0], DetailOp::Outgoing);
        assert_eq!(DetailOp::ALL[2], DetailOp::Branches);
    }

    #[test]
    fn detail_view_from_op_wraps_the_op() {
        let v = DetailView::Op(DetailOp::Log);
        assert!(matches!(v, DetailView::Op(DetailOp::Log)));
    }
```

- [x] **Step 2: Run (expect failure)**

Run: `cargo test -p sid-widgets workspace_detail_state::tests::detail`
Expected: error — `DetailOp` / `DetailView` not found.

- [x] **Step 3: Implement (append to the module, above the test mod)**

```rust
/// The fixed set of git operations the detail drill-in offers, in render order.
///
/// # Examples
///
/// ```
/// use sid_widgets::workspace_detail_state::DetailOp;
/// assert_eq!(DetailOp::Branches.label(), "Branches");
/// assert_eq!(DetailOp::ALL.len(), 5);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DetailOp {
    /// Commits ahead of upstream (the "outgoing" list).
    Outgoing,
    /// Full commit log (paginated by the binary).
    Log,
    /// Branch list.
    Branches,
    /// Stash entries (read-only list in v1).
    Stash,
    /// Linked worktrees (read-only list in v1).
    Worktrees,
}

impl DetailOp {
    /// Every op, in the order the ops menu renders them.
    pub const ALL: [DetailOp; 5] = [
        DetailOp::Outgoing,
        DetailOp::Log,
        DetailOp::Branches,
        DetailOp::Stash,
        DetailOp::Worktrees,
    ];

    /// Human-readable menu label.
    pub fn label(self) -> &'static str {
        match self {
            DetailOp::Outgoing => "Outgoing",
            DetailOp::Log => "Log",
            DetailOp::Branches => "Branches",
            DetailOp::Stash => "Stash",
            DetailOp::Worktrees => "Worktrees",
        }
    }
}

/// One level of the right-pane drill-in stack (held by `SplitView<DetailView>`).
///
/// Stack shapes the detail tab pushes:
/// - `[Op(op)]` — the op's primary list (commits for Outgoing/Log, branches, …).
/// - `[Op(Outgoing|Log), Commits, Diff(idx)]` — drilled into a commit's diff.
///
/// # Examples
///
/// ```
/// use sid_widgets::workspace_detail_state::{DetailOp, DetailView};
/// let v = DetailView::Op(DetailOp::Outgoing);
/// assert!(matches!(v, DetailView::Op(_)));
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DetailView {
    /// An op's top-level list.
    Op(DetailOp),
    /// A scrollable diff for the commit at this index into `RepoDetail::commits`.
    Diff(usize),
}
```

- [x] **Step 4: Re-run (expect pass)**

Run: `cargo test -p sid-widgets workspace_detail_state && cargo test -p sid-widgets --doc workspace_detail_state`
Expected: all unit + doc tests pass.

- [x] **Step 5: Commit**

```bash
git add crates/sid-widgets/src/workspace_detail_state.rs
git commit -m "feat(sid-widgets): DetailOp/DetailView — drill-in op set and SplitView stack payload"
```

---

### Task 4: Rewrite `WorkspaceDetailWidget` state — SatelliteRow list + SplitView + cursors

Replace the v1 `WorkspaceDetailWidget` (sub-repo `RepoSummary` table + placeholder `RightPane`) with the umbrella/satellite model: a `Vec<SatelliteRow>`, a `ListCursor` over it, a `SplitView<DetailView>` drill-in, an inner `ListCursor` for the active pane list, a `RepoDetail`, and a scroll offset. Keep the existing `CiStatus`/`RepoSummary`/`format_age`/`render_to_string` items (Task 5 reuses `render_to_string`; `RepoSummary` stays a public type other branches' tests reference via the re-export). The current widget struct is at `workspace_detail.rs:102`; `new` at 141; `apply_scan_results` at 157; `Widget` impl at 344.

**Files:**
- Modify: `crates/sid-widgets/src/workspace_detail.rs` (replace the struct body + inherent impl + `Widget` impl; keep `CiStatus`, `RepoSummary`, `format_age`, `render_to_string`)

- [x] **Step 1: Failing tests first**

Add these to the `#[cfg(test)] mod tests` at the bottom of `workspace_detail.rs` (create the mod if absent — the file currently has none):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use sid_core::adapters::git::{Branch, CommitInfo};
    use sid_core::workspace_metadata::WorkspaceKind;
    use sid_store::Workspace;
    fn umbrella() -> Workspace {
        Workspace {
            path: std::path::PathBuf::from("/stack"),
            name: "gen4-stack".into(),
            kind: WorkspaceKind::Umbrella,
            manifest_hash: 0,
            last_seen: 0,
            parent: None,
        }
    }

    #[test]
    fn new_seeds_umbrella_row_and_is_scanning() {
        let w = WorkspaceDetailWidget::new(umbrella(), None);
        assert!(w.is_scanning());
        // exactly the umbrella row until satellites land
        assert_eq!(w.rows().len(), 1);
        assert!(w.rows()[0].is_umbrella);
        assert_eq!(w.rows()[0].name, "gen4-stack");
    }

    #[test]
    fn apply_satellites_appends_after_umbrella_row() {
        let mut w = WorkspaceDetailWidget::new(umbrella(), None);
        w.apply_satellites(vec![
            SatelliteRow { name: "api".into(), path: "/stack/api".into(), is_umbrella: false, git: RepoGit::loading() },
            SatelliteRow { name: "web".into(), path: "/stack/web".into(), is_umbrella: false, git: RepoGit::loading() },
        ]);
        assert!(!w.is_scanning());
        assert_eq!(w.rows().len(), 3);
        assert!(w.rows()[0].is_umbrella);
        assert_eq!(w.rows()[1].name, "api");
    }

    #[test]
    fn apply_row_git_updates_matching_path_only() {
        let mut w = WorkspaceDetailWidget::new(umbrella(), None);
        w.apply_satellites(vec![SatelliteRow {
            name: "api".into(),
            path: "/stack/api".into(),
            is_umbrella: false,
            git: RepoGit::loading(),
        }]);
        w.apply_row_git(std::path::Path::new("/stack/api"), RepoGit::loaded("main".into(), 2, 1, 0));
        let api = w.rows().iter().find(|r| r.name == "api").unwrap();
        assert!(!api.git.is_loading());
        assert_eq!(api.git.outgoing, 1);
        // unknown path is a no-op (no panic)
        w.apply_row_git(std::path::Path::new("/nope"), RepoGit::loaded("x".into(), 0, 0, 0));
    }

    #[test]
    fn list_navigation_wraps_via_cursor() {
        let mut w = WorkspaceDetailWidget::new(umbrella(), None);
        w.apply_satellites(vec![
            SatelliteRow { name: "api".into(), path: "/stack/api".into(), is_umbrella: false, git: RepoGit::loading() },
        ]);
        assert_eq!(w.selected_row().unwrap().name, "gen4-stack");
        w.select_next();
        assert_eq!(w.selected_row().unwrap().name, "api");
        w.select_next(); // saturates at bottom (ListCursor::down does not wrap)
        assert_eq!(w.selected_row().unwrap().name, "api");
        w.select_prev();
        assert_eq!(w.selected_row().unwrap().name, "gen4-stack");
    }

    #[test]
    fn enter_op_drills_into_pane_and_left_pops_back_to_list() {
        let mut w = WorkspaceDetailWidget::new(umbrella(), None);
        // start on the ops menu, focus list
        assert_eq!(w.split().focus(), sid_widgets::split_view::SplitFocus::List);
        w.enter_op(DetailOp::Outgoing); // push Op(Outgoing)
        assert_eq!(w.split().focus(), sid_widgets::split_view::SplitFocus::Pane);
        assert_eq!(w.split().top(), Some(&DetailView::Op(DetailOp::Outgoing)));
        // drill into a commit's diff
        w.apply_detail(RepoDetail {
            commits: vec![CommitInfo {
                oid: "abc".into(),
                summary: "s".into(),
                author_name: "a".into(),
                author_email: "a@b".into(),
                timestamp_secs: 0,
                parents: vec![],
            }],
            ..RepoDetail::default()
        });
        w.drill_into_commit();
        assert_eq!(w.split().top(), Some(&DetailView::Diff(0)));
        w.pop_view(); // back to Op(Outgoing)
        assert_eq!(w.split().top(), Some(&DetailView::Op(DetailOp::Outgoing)));
        w.pop_view(); // back to list
        assert_eq!(w.split().focus(), sid_widgets::split_view::SplitFocus::List);
    }

    #[test]
    fn selecting_a_new_row_resets_the_drill_in() {
        let mut w = WorkspaceDetailWidget::new(umbrella(), None);
        w.apply_satellites(vec![SatelliteRow {
            name: "api".into(),
            path: "/stack/api".into(),
            is_umbrella: false,
            git: RepoGit::loading(),
        }]);
        w.enter_op(DetailOp::Branches);
        assert_eq!(w.split().focus(), sid_widgets::split_view::SplitFocus::Pane);
        w.select_next(); // moving the list selection re-roots the right pane
        assert_eq!(w.split().focus(), sid_widgets::split_view::SplitFocus::List);
        assert_eq!(w.split().depth(), 0);
    }
}
```

Note: inside sid-widgets, reference these items via `crate::...`, not by the crate's own name.

- [x] **Step 2: Run (expect failure)**

Run: `cargo test -p sid-widgets workspace_detail::tests`
Expected: compile errors — `rows`, `apply_satellites`, `split`, etc. missing.

- [x] **Step 3: Replace the struct + inherent impl**

Replace the struct (lines 99–113) and the inherent `impl WorkspaceDetailWidget` block (lines 115–342) with:

```rust
use crate::list_cursor::{CursorTarget, ListCursor};
use crate::split_view::{SplitFocus, SplitView};
use crate::workspace_detail_state::{DetailOp, DetailView, RepoDetail, RepoGit, SatelliteRow};

/// Tab widget for the Workspaces detail view (UX-v2 rework).
///
/// Owns the umbrella workspace, the row list (umbrella + satellites), a list
/// cursor, the right-pane drill-in `SplitView`, an inner list cursor for the
/// active pane list, the loaded `RepoDetail`, and a diff scroll offset. Git
/// data is loaded off-thread by the binary and pushed in via the `apply_*`
/// setters; this type never names `git2`.
pub struct WorkspaceDetailWidget {
    id: WidgetId,
    workspace: Workspace,
    rows: Vec<SatelliteRow>,
    list: ListCursor,
    split: SplitView<DetailView>,
    /// Cursor over the active pane list (commits or branches).
    pane_list: ListCursor,
    /// Loaded detail for the currently-selected row.
    detail: RepoDetail,
    /// Scroll offset within the diff view.
    diff_scroll: usize,
    #[allow(dead_code)] // The binary opens providers itself; kept for symmetry.
    git_factory: Option<Arc<dyn GitProvider>>,
    /// True until the satellite scan lands.
    scanning: bool,
}

impl WorkspaceDetailWidget {
    /// Construct with the umbrella workspace. The list seeds with the single
    /// umbrella row (`is_umbrella = true`); satellites arrive via
    /// [`Self::apply_satellites`]. The right pane starts on the ops menu with
    /// list focus.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use sid_core::workspace_metadata::WorkspaceKind;
    /// use sid_store::Workspace;
    /// use sid_widgets::workspace_detail::WorkspaceDetailWidget;
    ///
    /// let ws = Workspace {
    ///     path: PathBuf::from("/stack"),
    ///     name: "gen4-stack".into(),
    ///     kind: WorkspaceKind::Umbrella,
    ///     manifest_hash: 0,
    ///     last_seen: 0,
    ///     parent: None,
    /// };
    /// let w = WorkspaceDetailWidget::new(ws, None);
    /// assert!(w.is_scanning());
    /// assert_eq!(w.rows().len(), 1);
    /// ```
    pub fn new(workspace: Workspace, git_factory: Option<Arc<dyn GitProvider>>) -> Self {
        let tab_id = format!("workspace_detail:{}", workspace.path.display());
        let umbrella_row = SatelliteRow {
            name: workspace.name.clone(),
            path: workspace.path.clone(),
            is_umbrella: true,
            git: RepoGit::loading(),
        };
        Self {
            id: WidgetId::new(tab_id),
            workspace,
            rows: vec![umbrella_row],
            list: ListCursor::new(1, false, 0),
            split: SplitView::default(),
            pane_list: ListCursor::new(0, false, 0),
            detail: RepoDetail::default(),
            diff_scroll: 0,
            git_factory,
            scanning: true,
        }
    }

    /// Append satellites after the umbrella row and clear the scanning flag.
    /// Re-clamps the list cursor.
    pub fn apply_satellites(&mut self, satellites: Vec<SatelliteRow>) {
        self.rows.truncate(1); // keep the umbrella row only
        self.rows.extend(satellites);
        self.scanning = false;
        self.list = ListCursor::new(self.rows.len(), false, self.list.pos);
    }

    /// Replace one row's git snapshot, matched by path. No-op if no row matches.
    pub fn apply_row_git(&mut self, path: &std::path::Path, git: RepoGit) {
        if let Some(row) = self.rows.iter_mut().find(|r| r.path == path) {
            row.git = git;
        }
    }

    /// Replace the loaded detail for the selected row and reset the pane cursor
    /// + diff scroll. Sizes the pane cursor to whichever list the active op shows.
    pub fn apply_detail(&mut self, detail: RepoDetail) {
        self.detail = detail;
        self.diff_scroll = 0;
        let len = self.active_pane_len();
        self.pane_list = ListCursor::new(len, false, 0);
    }

    /// Number of items in the currently-shown pane list.
    fn active_pane_len(&self) -> usize {
        match self.split.top() {
            Some(DetailView::Op(DetailOp::Branches)) => self.detail.branches.len(),
            Some(DetailView::Op(DetailOp::Outgoing | DetailOp::Log)) => self.detail.commits.len(),
            _ => 0,
        }
    }

    /// Whether the satellite scan is still running.
    pub fn is_scanning(&self) -> bool {
        self.scanning
    }

    /// The row list (umbrella first, then satellites).
    pub fn rows(&self) -> &[SatelliteRow] {
        &self.rows
    }

    /// The umbrella workspace this detail tab represents.
    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    /// The drill-in split state (focus + view stack).
    pub fn split(&self) -> &SplitView<DetailView> {
        &self.split
    }

    /// The currently-selected row, if any.
    pub fn selected_row(&self) -> Option<&SatelliteRow> {
        match self.list.target() {
            CursorTarget::Item(i) => self.rows.get(i),
            _ => None,
        }
    }

    /// Index into `detail.commits` the pane cursor points at (Outgoing/Log).
    pub fn selected_commit_index(&self) -> Option<usize> {
        match (self.split.top(), self.pane_list.target()) {
            (Some(DetailView::Op(DetailOp::Outgoing | DetailOp::Log)), CursorTarget::Item(i)) => {
                Some(i)
            }
            _ => None,
        }
    }

    /// Diff scroll offset.
    pub fn diff_scroll(&self) -> usize {
        self.diff_scroll
    }

    /// Move the list selection down; re-root the right pane (selecting a row
    /// resets the drill-in to that row's ops menu, list-focused).
    pub fn select_next(&mut self) {
        self.list.down();
        self.split.reset();
    }

    /// Move the list selection up; re-root the right pane.
    pub fn select_prev(&mut self) {
        self.list.up();
        self.split.reset();
    }

    /// Push an op view onto the stack (focuses the pane).
    pub fn enter_op(&mut self, op: DetailOp) {
        self.split.push(DetailView::Op(op));
        self.pane_list = ListCursor::new(self.active_pane_len(), false, 0);
        self.diff_scroll = 0;
    }

    /// From an Outgoing/Log commit list, drill into the selected commit's diff.
    pub fn drill_into_commit(&mut self) {
        if let Some(i) = self.selected_commit_index() {
            self.split.push(DetailView::Diff(i));
            self.diff_scroll = 0;
        }
    }

    /// Pop one drill-in level; when the stack empties, focus returns to the list.
    pub fn pop_view(&mut self) {
        self.split.pop();
        self.pane_list = ListCursor::new(self.active_pane_len(), false, 0);
        self.diff_scroll = 0;
    }

    /// Move the active pane list cursor down.
    pub fn pane_next(&mut self) {
        self.pane_list.down();
    }

    /// Move the active pane list cursor up.
    pub fn pane_prev(&mut self) {
        self.pane_list.up();
    }

    /// Scroll the diff view down one line.
    pub fn diff_scroll_down(&mut self) {
        self.diff_scroll = self.diff_scroll.saturating_add(1);
    }

    /// Scroll the diff view up one line.
    pub fn diff_scroll_up(&mut self) {
        self.diff_scroll = self.diff_scroll.saturating_sub(1);
    }

    /// Borrow the loaded detail (for the renderer).
    pub fn detail(&self) -> &RepoDetail {
        &self.detail
    }

    /// Pane cursor (for the renderer to highlight the selected list row).
    pub fn pane_cursor(&self) -> &ListCursor {
        &self.pane_list
    }
}
```

Note for the executor: the old `apply_scan_results(&mut self, Vec<RepoSummary>)` is gone — the binary now calls `apply_satellites`. Task 7 updates the wire-layer call site. Until then `cargo test -p sid` will not compile; that is expected and fixed in Task 7. Run only the scoped `-p sid-widgets` tests for Tasks 4–6.

- [x] **Step 4: Re-run (expect pass on sid-widgets only)**

Run: `cargo test -p sid-widgets workspace_detail::tests && cargo test -p sid-widgets --doc workspace_detail`
Expected: 6 unit tests + the `new` doc test pass. (The `render_to_string` doc test still asserts `s.contains("scanning")`; Task 5's renderer keeps a "scanning…" string, so it stays green — but if it goes red before Task 5 lands, that is acceptable interim state; Task 5 makes it green.)

- [x] **Step 5: Commit**

```bash
git add crates/sid-widgets/src/workspace_detail.rs
git commit -m "feat(sid-widgets): rewrite WorkspaceDetailWidget state — satellite rows, SplitView drill-in, pane cursors"
```

---

### Task 5: Render the rewritten detail tab — umbrella header, satellite list, drill-in pane

Rewrite the widget's rendering: an umbrella git header line, a left satellite list (40%) using the row `RepoGit` summaries, and a right pane (60%) that renders whatever `split.top()` says (ops menu / commit list / branch list / stash / worktrees / scrollable diff). Re-implement the `Widget` impl's `footer_hint` and `handle_event` to route through the new cursors and `SplitView`. Keep `format_age` and `render_to_string`.

**Files:**
- Modify: `crates/sid-widgets/src/workspace_detail.rs` (replace `render_into_frame`/`render_table`/`render_drilldown` and the `Widget` impl; keep `render_to_string` at the bottom)

- [x] **Step 1: Snapshot + behavior tests first**

Append to the `#[cfg(test)] mod tests` block:

```rust
    fn loaded_widget() -> WorkspaceDetailWidget {
        let mut w = WorkspaceDetailWidget::new(umbrella(), None);
        w.apply_row_git(std::path::Path::new("/stack"), RepoGit::loaded("main".into(), 2, 3, 0));
        w.apply_satellites(vec![
            SatelliteRow { name: "api".into(), path: "/stack/api".into(), is_umbrella: false, git: RepoGit::loaded("main".into(), 0, 0, 0) },
            SatelliteRow { name: "web".into(), path: "/stack/web".into(), is_umbrella: false, git: RepoGit::loaded("feat/x".into(), 5, 0, 1) },
        ]);
        w
    }

    #[test]
    fn snapshot_detail_list_and_ops_menu() {
        let w = loaded_widget();
        let s = render_to_string(&w, 100, 24);
        insta::assert_snapshot!("detail_list_and_ops_menu", s);
    }

    #[test]
    fn snapshot_detail_outgoing_commits() {
        let mut w = loaded_widget();
        w.enter_op(DetailOp::Outgoing);
        w.apply_detail(RepoDetail {
            commits: vec![
                CommitInfo { oid: "deadbeef0".into(), summary: "feat: thing".into(), author_name: "a".into(), author_email: "a@b".into(), timestamp_secs: 0, parents: vec![] },
                CommitInfo { oid: "cafebabe1".into(), summary: "fix: bug".into(), author_name: "a".into(), author_email: "a@b".into(), timestamp_secs: 0, parents: vec![] },
            ],
            ..RepoDetail::default()
        });
        let s = render_to_string(&w, 100, 24);
        insta::assert_snapshot!("detail_outgoing_commits", s);
    }

    #[test]
    fn header_shows_umbrella_git_summary() {
        let w = loaded_widget();
        let s = render_to_string(&w, 100, 24);
        // umbrella header carries branch · dirty · outgoing
        assert!(s.contains("main"));
        assert!(s.contains("↑3"));
    }

    #[test]
    fn handle_enter_on_ops_menu_drills_in() {
        use sid_core::context::WidgetCtx;
        use sid_core::event::Event;
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut w = loaded_widget();
        let mut reg = sid_core::action::ActionRegistry::new();
        let mut ctx = WidgetCtx::new(&mut reg);
        let ev = Event::Key(sid_core::event::KeyChord { code: KeyCode::Enter, mods: KeyModifiers::NONE });
        let _ = w.handle_event(&ev, &mut ctx);
        // Enter on the ops menu (default selection 0 = Outgoing) drills in
        assert_eq!(w.split().focus(), sid_widgets::split_view::SplitFocus::Pane);
    }
```

Note: confirm `WidgetCtx::new` / `ActionRegistry::new` signatures by reading how existing `workspaces.rs` tests construct a `WidgetCtx` (grep `WidgetCtx::new` under `crates/sid-core/src` and `crates/sid-widgets`); mirror exactly. If `WidgetCtx` needs a different constructor, use the same pattern the existing detail/workspaces tests use.

- [x] **Step 2: Run (expect failure — snapshots missing / behavior)**

Run: `cargo test -p sid-widgets workspace_detail::tests`
Expected: snapshot tests fail pending acceptance; `handle_enter_on_ops_menu_drills_in` fails until the new `handle_event` lands.

- [x] **Step 3: Implement the renderer + Widget impl**

Replace `render_into_frame` (line 211), `render_table` (220), `render_drilldown` (312), and the `Widget` impl (344–389). Layout: a top header line (1 row), then a horizontal 40/60 split below it. The left list renders each `SatelliteRow` with `git.header_summary()`; the umbrella row is marked with a leading glyph. The right pane switches on `split.top()`:

```rust
    /// Draw the detail tab: umbrella header row, then a 40/60 list/pane split.
    pub fn render_into_frame(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(area);
        self.render_header(frame, rows[0], theme);
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(rows[1]);
        self.render_list(frame, cols[0], theme);
        self.render_pane(frame, cols[1], theme);
    }

    fn render_header(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let umbrella_git = self
            .rows
            .first()
            .map(|r| r.git.header_summary())
            .unwrap_or_default();
        let line = Line::from(vec![
            Span::styled(
                format!(" {} ", self.workspace.name),
                Style::default()
                    .fg(theme.background.into())
                    .bg(theme.accent_primary.into())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(umbrella_git, Style::default().fg(theme.foreground.into())),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }

    fn render_list(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let focused = self.split.focus() == SplitFocus::List;
        let border_color = if focused {
            theme.accent_primary
        } else {
            theme.muted
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color.into()))
            .title(" Repos ");
        if self.scanning && self.rows.len() <= 1 {
            let body = Paragraph::new(Line::from(Span::styled(
                "  scanning for satellites…",
                Style::default().fg(theme.muted.into()),
            )))
            .block(block);
            frame.render_widget(body, area);
            return;
        }
        let sel = match self.list.target() {
            CursorTarget::Item(i) => Some(i),
            _ => None,
        };
        let lines: Vec<Line<'_>> = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let glyph = if r.is_umbrella { '▾' } else { '·' };
                let marker = if Some(i) == sel { '>' } else { ' ' };
                let label = format!("{marker} {glyph} {}  {}", r.name, r.git.header_summary());
                let style = if Some(i) == sel {
                    Style::default()
                        .fg(theme.background.into())
                        .bg(theme.accent_primary.into())
                } else {
                    Style::default().fg(theme.foreground.into())
                };
                Line::from(Span::styled(label, style))
            })
            .collect();
        frame.render_widget(Paragraph::new(lines).block(block), area);
    }

    fn render_pane(&self, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
        let focused = self.split.focus() == SplitFocus::Pane;
        let border_color = if focused {
            theme.accent_primary
        } else {
            theme.muted
        };
        let title = match self.split.top() {
            None => " Ops ".to_string(),
            Some(DetailView::Op(op)) => format!(" {} ", op.label()),
            Some(DetailView::Diff(_)) => " Diff ".to_string(),
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color.into()))
            .title(title);
        let body: Vec<Line<'_>> = match self.split.top() {
            None => DetailOp::ALL
                .iter()
                .map(|op| {
                    Line::from(Span::styled(
                        format!("  {}", op.label()),
                        Style::default().fg(theme.foreground.into()),
                    ))
                })
                .collect(),
            Some(DetailView::Op(DetailOp::Outgoing | DetailOp::Log)) => {
                self.render_commit_lines(theme)
            }
            Some(DetailView::Op(DetailOp::Branches)) => self.render_branch_lines(theme),
            Some(DetailView::Op(DetailOp::Stash)) => vec![Line::from(Span::styled(
                "  (no stash entries)",
                Style::default().fg(theme.muted.into()),
            ))],
            Some(DetailView::Op(DetailOp::Worktrees)) => vec![Line::from(Span::styled(
                "  (no linked worktrees)",
                Style::default().fg(theme.muted.into()),
            ))],
            Some(DetailView::Diff(idx)) => self.render_diff_lines(*idx, theme),
        };
        frame.render_widget(Paragraph::new(body).block(block), area);
    }

    fn render_commit_lines(&self, theme: &Theme) -> Vec<Line<'_>> {
        if self.detail.commits.is_empty() {
            return vec![Line::from(Span::styled(
                "  (no commits)",
                Style::default().fg(theme.muted.into()),
            ))];
        }
        let sel = match self.pane_list.target() {
            CursorTarget::Item(i) => Some(i),
            _ => None,
        };
        self.detail
            .commits
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let short: String = c.oid.chars().take(8).collect();
                let marker = if Some(i) == sel { '>' } else { ' ' };
                Line::from(vec![
                    Span::styled(
                        format!("{marker} {short}"),
                        Style::default().fg(theme.accent_warning.into()),
                    ),
                    Span::raw("  "),
                    Span::styled(c.summary.clone(), Style::default().fg(theme.foreground.into())),
                ])
            })
            .collect()
    }

    fn render_branch_lines(&self, theme: &Theme) -> Vec<Line<'_>> {
        if self.detail.branches.is_empty() {
            return vec![Line::from(Span::styled(
                "  (no branches loaded)",
                Style::default().fg(theme.muted.into()),
            ))];
        }
        let sel = match self.pane_list.target() {
            CursorTarget::Item(i) => Some(i),
            _ => None,
        };
        self.detail
            .branches
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let glyph = if b.is_current { '●' } else { '○' };
                let marker = if Some(i) == sel { '>' } else { ' ' };
                Line::from(Span::styled(
                    format!("{marker} {glyph} {}", b.name),
                    Style::default().fg(theme.foreground.into()),
                ))
            })
            .collect()
    }

    fn render_diff_lines(&self, idx: usize, theme: &Theme) -> Vec<Line<'_>> {
        // The binary loads diff entries for the drilled commit into detail.diff.
        const MAX: usize = 200;
        let _ = idx; // diff is per-commit; the binary fills detail.diff for the drilled commit
        if self.detail.diff.is_empty() {
            return vec![Line::from(Span::styled(
                "  (no diff loaded)",
                Style::default().fg(theme.muted.into()),
            ))];
        }
        self.detail
            .diff
            .iter()
            .flat_map(|e| e.patch.lines())
            .skip(self.diff_scroll)
            .take(MAX)
            .map(|l| Line::from(Span::raw(l.to_string())))
            .collect()
    }
```

And the `Widget` impl (key routing through `SplitView` focus — `←` pops, `→`/`Enter` drills, `j/k` move whichever cursor owns focus):

```rust
impl Widget for WorkspaceDetailWidget {
    fn id(&self) -> &WidgetId {
        &self.id
    }

    fn title(&self) -> &str {
        &self.workspace.name
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn render(&self, _: &mut dyn RenderTarget) {}

    fn footer_hint(&self) -> Vec<FooterHint> {
        vec![
            FooterHint::new("j/k", "select"),
            FooterHint::new("→/⏎", "drill in"),
            FooterHint::new("←", "back"),
            FooterHint::new("Ctrl+W", "close"),
        ]
    }

    fn handle_event(&mut self, ev: &Event, _ctx: &mut WidgetCtx) -> EventOutcome {
        use crossterm::event::{KeyCode, KeyModifiers};
        let Event::Key(chord) = ev else {
            return EventOutcome::Bubble;
        };
        match self.split.focus() {
            SplitFocus::List => match (chord.code, chord.mods) {
                (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
                    self.select_next();
                    EventOutcome::Consumed
                }
                (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
                    self.select_prev();
                    EventOutcome::Consumed
                }
                (KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter, KeyModifiers::NONE) => {
                    // Enter the ops menu: push the first op (Outgoing).
                    self.enter_op(DetailOp::Outgoing);
                    EventOutcome::Consumed
                }
                _ => EventOutcome::Bubble,
            },
            SplitFocus::Pane => match (chord.code, chord.mods) {
                (KeyCode::Char('h') | KeyCode::Left, KeyModifiers::NONE) => {
                    self.pop_view();
                    EventOutcome::Consumed
                }
                (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
                    self.pane_next();
                    if matches!(self.split.top(), Some(DetailView::Diff(_))) {
                        self.diff_scroll_down();
                    }
                    EventOutcome::Consumed
                }
                (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
                    self.pane_prev();
                    if matches!(self.split.top(), Some(DetailView::Diff(_))) {
                        self.diff_scroll_up();
                    }
                    EventOutcome::Consumed
                }
                (KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter, KeyModifiers::NONE) => {
                    // From a commit list, drill into the diff.
                    if matches!(
                        self.split.top(),
                        Some(DetailView::Op(DetailOp::Outgoing | DetailOp::Log))
                    ) {
                        self.drill_into_commit();
                    }
                    EventOutcome::Consumed
                }
                _ => EventOutcome::Bubble,
            },
        }
    }
}
```

Note for the executor: the ops menu currently always enters `Outgoing` on the first drill (`enter_op(DetailOp::Outgoing)`); selecting a *different* op from the menu is a pane-list interaction left for a follow-up — the master plan's binding requirement is the ops → list → commit → diff stack working with `←` popping, which this delivers. The `DetailOp::ALL` list still renders so the other ops are discoverable. If you want per-op selection in this branch, add an ops-menu `ListCursor` and gate `enter_op` on its target; keep it minimal.

- [x] **Step 4: Re-run + accept snapshots**

Run: `cargo test -p sid-widgets workspace_detail::tests`
Then: `cargo insta review` (accept `detail_list_and_ops_menu`, `detail_outgoing_commits`).
Re-run: `cargo test -p sid-widgets workspace_detail`
Expected: snapshot + behavior tests pass; the `render_to_string` doc test (`assert!(s.contains("scanning"))`) passes because the empty-satellite render still prints "scanning for satellites…".

- [x] **Step 5: Commit**

```bash
git add crates/sid-widgets/src/workspace_detail.rs crates/sid-widgets/src/snapshots/
git commit -m "feat(sid-widgets): render detail tab — umbrella header, satellite list, SplitView drill-in pane"
```

---

### Task 6: Overview Enter — open *every* node as a pushed subrepo tab

The overview (`workspaces.rs`) today opens a detail tab only for `Repo` leaves; `Umbrella` Enter toggles expansion. Per master-plan decision 8, Enter on **any** node (umbrella, satellite, or single repo) opens it as a pushed subrepo tab with the same layout; `→/↓/←` keep doing inline tree expansion. Keep the tree-expansion keys; change only the Enter handler so umbrellas *also* set `pending_open_detail`. Also support `Ctrl+Enter`/`O` background-open via the substrate chord helper.

**Files:**
- Modify: `crates/sid-widgets/src/workspaces.rs` (the Enter arm at lines 1952–1972; add a `pending_open_background: bool` flag next to `pending_open_detail` at line 1313; add a drain method)

- [x] **Step 1: Failing tests first**

Append to `workspaces.rs`'s `#[cfg(test)] mod tests`:

```rust
    fn umbrella_ws() -> Workspace {
        Workspace {
            path: std::path::PathBuf::from("/stack"),
            name: "gen4".into(),
            kind: WorkspaceKind::Umbrella,
            manifest_hash: 0,
            last_seen: 0,
            parent: None,
        }
    }

    #[test]
    fn enter_on_umbrella_now_opens_detail_not_just_expand() {
        use sid_core::context::WidgetCtx;
        use sid_core::event::{Event, KeyChord};
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut w = WorkspacesWidget::new(vec![umbrella_ws()], None);
        let mut reg = sid_core::action::ActionRegistry::new();
        let mut ctx = WidgetCtx::new(&mut reg);
        let ev = Event::Key(KeyChord { code: KeyCode::Enter, mods: KeyModifiers::NONE });
        let _ = w.handle_event(&ev, &mut ctx);
        let opened = w.take_pending_open_detail();
        assert!(opened.is_some());
        assert_eq!(opened.unwrap().name, "gen4");
    }

    #[test]
    fn background_open_sets_background_flag() {
        use sid_core::context::WidgetCtx;
        use sid_core::event::{Event, KeyChord};
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut w = WorkspacesWidget::new(vec![umbrella_ws()], None);
        let mut reg = sid_core::action::ActionRegistry::new();
        let mut ctx = WidgetCtx::new(&mut reg);
        // Shift+O is the universal background-open fallback.
        let ev = Event::Key(KeyChord { code: KeyCode::Char('O'), mods: KeyModifiers::SHIFT });
        let _ = w.handle_event(&ev, &mut ctx);
        assert!(w.take_pending_open_background());
        assert!(w.take_pending_open_detail().is_some());
    }

    #[test]
    fn right_arrow_still_expands_umbrella_without_opening() {
        use sid_core::context::WidgetCtx;
        use sid_core::event::{Event, KeyChord};
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut w = WorkspacesWidget::new(vec![umbrella_ws()], None);
        let mut reg = sid_core::action::ActionRegistry::new();
        let mut ctx = WidgetCtx::new(&mut reg);
        let ev = Event::Key(KeyChord { code: KeyCode::Right, mods: KeyModifiers::NONE });
        let _ = w.handle_event(&ev, &mut ctx);
        assert!(w.state().is_expanded(std::path::Path::new("/stack")));
        assert!(w.take_pending_open_detail().is_none());
    }
```

(Confirm `WidgetCtx::new` / `ActionRegistry::new` signatures from existing tests before running — same caveat as Task 5.)

- [x] **Step 2: Run (expect failure)**

Run: `cargo test -p sid-widgets workspaces::tests::enter_on_umbrella workspaces::tests::background_open workspaces::tests::right_arrow_still`
Expected: `take_pending_open_background` missing; umbrella Enter still only expands.

- [x] **Step 3: Add the background flag + drain, and rewrite the Enter arm**

Add the field next to `pending_open_detail` (struct at line 1298, `pending_open_detail` at 1313):

```rust
    /// Set alongside `pending_open_detail` when the user requested a *background*
    /// open (`Ctrl+Enter` / `O`). The wire layer reads this to choose
    /// `push_background` over `push_detail`.
    pending_open_background: bool,
```

Initialize it in `new` (line 1332 area) and `default` (via `new`): set `pending_open_background: false` in the struct literal.

Add the drain method next to `take_pending_open_detail` (line 1350):

```rust
    /// Drain the background-open flag. Returns `true` once after a background
    /// open was requested; the wire layer then uses `push_background`.
    pub fn take_pending_open_background(&mut self) -> bool {
        std::mem::take(&mut self.pending_open_background)
    }
```

Rewrite the Enter arm (lines 1952–1972) so umbrellas also open. Replace it with:

```rust
                    (KeyCode::Enter, KeyModifiers::NONE) => {
                        // Decision 8: Enter on ANY node opens it as a pushed
                        // subrepo tab — umbrella, satellite, or single repo all
                        // get the same detail layout. Inline tree expansion
                        // stays on →/↓/←.
                        let selected = self.state.selected_workspace().cloned();
                        if let Some(ws) = selected {
                            ctx.emit_action("workspaces.open_detail");
                            self.pending_open_detail = Some(ws);
                        }
                        return EventOutcome::Consumed;
                    }
```

And add a background-open arm just before the pane-gated routing `match self.focused_pane` (after the `'r'` widget-global handler at line 1924). Use the substrate chord helper:

```rust
            // Background-open: Ctrl+Enter (kitty) / Shift+O (universal). Opens
            // the selected node as a background tab without switching focus.
            if chord.is_background_open() {
                if let Some(ws) = self.state.selected_workspace().cloned() {
                    ctx.emit_action("workspaces.open_detail_background");
                    self.pending_open_detail = Some(ws);
                    self.pending_open_background = true;
                }
                return EventOutcome::Consumed;
            }
```

Note for the executor: `is_background_open()` matches `Char('O')` regardless of which modifier flavor the terminal sends, and `Ctrl+Enter` on kitty terminals. Place this check before the `Tab`/`Alt` handlers only if `O` would otherwise be swallowed — it would not (no existing arm binds `O`), so placing it after the `'r'` handler is correct. The umbrella `→/↓/←` expansion arms (lines 1937–1951) are untouched.

- [x] **Step 4: Re-run (expect pass)**

Run: `cargo test -p sid-widgets workspaces`
Expected: the 3 new tests pass; existing workspaces tests still green.

- [x] **Step 5: Commit**

```bash
git add crates/sid-widgets/src/workspaces.rs
git commit -m "feat(sid-widgets): Enter opens any workspace node as a subrepo tab; Ctrl+Enter/O background-open"
```

---

### Task 7: Wire the rewritten detail tab — satellite scan + per-row off-thread git

Update the binary's wire layer to drive the new widget API. Replace the `RepoSummary` scan with a `SatelliteRow` scan that uses `scan_adoptable_repos`, push it via `apply_satellites`, then spawn per-row git loads that land via a new `JobOutcome::RepoGitLoaded`. Honor the background-open flag with `push_background`. All edits are additive new functions + the single existing `maybe_open_pending_workspace_detail` call site.

**Files:**
- Modify: `crates/sid/src/wire.rs` — the `JobOutcome` enum (line ~140), `maybe_open_pending_workspace_detail` (line 1773), `scan_workspace_for_summaries` (line 1849), `drain_job_outcomes` (line 3168), `apply_workspace_detail_scan` (line 3194)

- [x] **Step 1: Add the new job outcome variant + a `SatelliteRow` scan, with tests first**

Add a wire test (in the existing `#[cfg(test)] mod tests` in wire.rs, reusing `build_test_sid_app`):

```rust
    #[test]
    fn scan_umbrella_satellites_finds_repos_and_marks_umbrella() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        std::fs::create_dir_all(tmp.path().join("api").join(".git")).unwrap();
        let rows = scan_umbrella_satellites(
            tmp.path(),
            "gen4",
        );
        // umbrella row first, then satellites
        assert!(rows[0].is_umbrella);
        assert_eq!(rows[0].name, "gen4");
        assert!(rows.iter().any(|r| r.name == "api" && !r.is_umbrella));
    }
```

- [x] **Step 2: Run (expect failure)**

Run: `cargo test -p sid scan_umbrella_satellites_finds`
Expected: `scan_umbrella_satellites` not found.

- [x] **Step 3: Implement**

Add the new `JobOutcome` arms (in the enum at line ~140, after `WorkspaceDetailScanned`):

```rust
    /// A detail tab's satellite list finished scanning. The widget identified
    /// by `tab_id` receives `rows` via `apply_satellites`. No toast — silent.
    WorkspaceSatellitesScanned {
        /// `TabId.as_str()` for the detail tab that requested the scan.
        tab_id: String,
        /// Umbrella row first, then satellites.
        rows: Vec<sid_widgets::SatelliteRow>,
    },
    /// One repo row's git snapshot finished loading off-thread.
    RepoGitLoaded {
        /// Detail tab id the row belongs to.
        tab_id: String,
        /// Absolute repo path (the row key).
        path: std::path::PathBuf,
        /// Loaded snapshot.
        git: sid_widgets::RepoGit,
    },
```

Replace `scan_workspace_for_summaries` (lines 1849–1893) — keep the old fn name removed; add the satellite scanner. The scan is pure (no git2), so it lives as a plain helper:

```rust
/// Build the detail tab's row list: the umbrella row first, then every
/// adoptable satellite under it (one level deep, symlinks resolved). Git
/// snapshots start in the `loading` state; per-row loads fill them in.
fn scan_umbrella_satellites(
    umbrella_path: &std::path::Path,
    umbrella_name: &str,
) -> Vec<sid_widgets::SatelliteRow> {
    use sid_core::workspace_discovery::scan_adoptable_repos;
    use sid_widgets::{RepoGit, SatelliteRow};
    let mut rows = vec![SatelliteRow {
        name: umbrella_name.to_string(),
        path: umbrella_path.to_path_buf(),
        is_umbrella: true,
        git: RepoGit::loading(),
    }];
    for repo in scan_adoptable_repos(umbrella_path) {
        rows.push(SatelliteRow {
            name: repo.name,
            path: repo.path,
            is_umbrella: false,
            git: RepoGit::loading(),
        });
    }
    rows
}
```

Add a per-row git loader. `git_factory` is `Arc<Git2ProviderFactory>` (constructed at wire.rs:668); thread a clone into `maybe_open_pending_workspace_detail`. The loader opens the repo and computes a `RepoGit`:

```rust
/// Open `path` with `factory` and compute its [`sid_widgets::RepoGit`] snapshot.
/// Best-effort: on any git error returns a `?`-branch loaded snapshot so the
/// row stops showing "loading" rather than hanging.
fn load_repo_git(
    factory: &std::sync::Arc<sid_git::Git2ProviderFactory>,
    path: &std::path::Path,
) -> sid_widgets::RepoGit {
    use sid_core::adapters::git::GitProvider;
    use sid_widgets::RepoGit;
    let provider = match factory.open(path) {
        Ok(p) => p,
        Err(_) => return RepoGit::loaded("?".into(), 0, 0, 0),
    };
    let branch = provider
        .current_branch()
        .ok()
        .flatten()
        .map(|b| b.name)
        .unwrap_or_else(|| "?".into());
    let dirty = provider
        .status()
        .map(|s| s.entries.len() as u32)
        .unwrap_or(0);
    // Outgoing = commits on the current branch not on its upstream. Without a
    // revwalk API on the trait, approximate via the branch's ahead count when
    // available; v1 reports 0 when not derivable and a follow-up wires a real
    // ahead/behind once the trait grows it. (Keep the column honest: 0, not a
    // guess.)
    RepoGit::loaded(branch, dirty, 0, 0)
}
```

Note for the executor: the `GitProvider` trait (`crates/sid-core/src/adapters/git.rs`) exposes `current_branch`, `status`, `list_branches`, `commit_log`, `diff` but **no** ahead/behind/outgoing method. Reporting `outgoing = 0` is the honest v1 value. If you want a real outgoing count in this branch, add an `ahead_behind(&self) -> Result<(u32, u32), GitError>` method to the trait and implement it in `sid-git` (git2's `graph_ahead_behind`) — but that is a trait change touching `sid-core` + `sid-git`, so do it as its own task with its own tests, or defer. The plan ships honest zeros and a working header.

Now rewrite `maybe_open_pending_workspace_detail` (line 1773) to: drain both pending flags, build the widget, `push_background` vs `push_detail` per the flag, and spawn the satellite scan + per-row git loads. The key changes — replace the widget construction + scan-spawn tail (lines 1817–1846):

```rust
    // Drain the background-open flag too.
    let background = {
        let tabs = sid_app.app.tabs_mut().tabs_mut();
        tabs.get_mut(parent_idx)
            .and_then(|t| {
                if let Layout::Single(w) = &mut t.layout {
                    w.as_any_mut()
                        .downcast_mut::<sid_widgets::WorkspacesWidget>()
                        .map(|ww| ww.take_pending_open_background())
                } else {
                    None
                }
            })
            .unwrap_or(false)
    };

    let tab_id_str = format!("workspace_detail:{}", workspace.path.display());
    let tab_id = TabId::new(&tab_id_str);

    if sid_app.app.tabs().tabs().iter().any(|t| t.id == tab_id) {
        let _ = sid_app.app.tabs_mut().switch_to(&tab_id);
        return;
    }

    let git_factory = sid_app.git_factory.clone();
    let widget = sid_widgets::WorkspaceDetailWidget::new(workspace.clone(), Some(git_factory.clone()));
    let new_tab = Tab {
        id: tab_id.clone(),
        title: workspace.name.clone(),
        layout: Layout::Single(Box::new(widget)),
        hotkey: None,
        kind: TabKind::Detail { parent_idx },
    };
    let push_result = if background {
        sid_app.app.tabs_mut().push_background(new_tab)
    } else {
        sid_app.app.tabs_mut().push_detail(new_tab)
    };
    if let Err(e) = push_result {
        sid_app
            .toasts
            .push(Toast::error(format!("open workspace detail: {e}")));
        return;
    }
    if !background {
        let _ = sid_app.app.tabs_mut().switch_to(&tab_id);
    }

    // Scan satellites synchronously (cheap fs walk), push them, then spawn one
    // git-load job per row.
    let rows = scan_umbrella_satellites(&workspace.path, &workspace.name);
    let paths: Vec<std::path::PathBuf> = rows.iter().map(|r| r.path.clone()).collect();
    let scan_tab_id = tab_id_str.clone();
    let scan_rows = rows.clone();
    let _ = sid_app.jobs.spawn(async move {
        JobOutcome::WorkspaceSatellitesScanned {
            tab_id: scan_tab_id,
            rows: scan_rows,
        }
    });
    for path in paths {
        let factory = git_factory.clone();
        let job_tab_id = tab_id_str.clone();
        let _ = sid_app.jobs.spawn(async move {
            let git = tokio::task::spawn_blocking(move || load_repo_git(&factory, &path))
                .await
                .unwrap_or_else(|_| sid_widgets::RepoGit::loaded("?".into(), 0, 0, 0));
            // recompute path inside the closure capture isn't possible after move;
            // see note — we capture a second clone below.
            JobOutcome::RepoGitLoaded {
                tab_id: job_tab_id,
                path: std::path::PathBuf::new(), // replaced — see executor note
                git,
            }
        });
    }
```

Note for the executor: the `path` is moved into `spawn_blocking`; capture a second clone before the closure for the `RepoGitLoaded.path` field, e.g. `let path_for_outcome = path.clone();` before `let factory = …`, and use `path: path_for_outcome` in the outcome. Verify `sid_app.git_factory` is a field on `SidApp` — if it is constructed locally in `build_app_full` (line 668) and not stored on `SidApp`, add a `pub git_factory: Arc<Git2ProviderFactory>` field to `SidApp` and populate it in the builders (grep every `SidApp { … }` literal incl. the doc-test in the `JobOutcome` comment and the test fixtures, and add the field). Prefer storing it on `SidApp` — that is the additive, single-source approach. `Git2ProviderFactory` is `Clone`? Confirm with `grep -n "derive" crates/sid-git/src/lib.rs`; if not `Clone`, wrap in `Arc` (it is constructed as `Arc::new(Git2ProviderFactory::new())` at line 668, so `Arc<Git2ProviderFactory>` clones fine).

Add the drain arms in `drain_job_outcomes` (line 3168, after the `WorkspaceDetailScanned` arm):

```rust
            Ok(JobOutcome::WorkspaceSatellitesScanned { tab_id, rows }) => {
                apply_satellites_to_detail(sid_app, &tab_id, rows);
            }
            Ok(JobOutcome::RepoGitLoaded { tab_id, path, git }) => {
                apply_row_git_to_detail(sid_app, &tab_id, &path, git);
            }
```

And the two apply helpers (next to `apply_workspace_detail_scan`, line 3194 — keep that fn for now or remove it once `WorkspaceDetailScanned` is no longer emitted; it is no longer emitted after this task, so delete `WorkspaceDetailScanned` from the enum, its `drain_job_outcomes` arm, and `apply_workspace_detail_scan`):

```rust
/// Push a scanned satellite list to the detail widget identified by `tab_id`.
fn apply_satellites_to_detail(
    sid_app: &mut SidApp,
    tab_id: &str,
    rows: Vec<sid_widgets::SatelliteRow>,
) {
    use sid_core::layout::Layout;
    for tab in sid_app.app.tabs_mut().tabs_mut().iter_mut() {
        if tab.id.as_str() != tab_id {
            continue;
        }
        if let Layout::Single(w) = &mut tab.layout
            && let Some(d) = w
                .as_any_mut()
                .downcast_mut::<sid_widgets::WorkspaceDetailWidget>()
        {
            d.apply_satellites(rows);
        }
        return;
    }
}

/// Push one row's loaded git snapshot to the detail widget identified by `tab_id`.
fn apply_row_git_to_detail(
    sid_app: &mut SidApp,
    tab_id: &str,
    path: &std::path::Path,
    git: sid_widgets::RepoGit,
) {
    use sid_core::layout::Layout;
    for tab in sid_app.app.tabs_mut().tabs_mut().iter_mut() {
        if tab.id.as_str() != tab_id {
            continue;
        }
        if let Layout::Single(w) = &mut tab.layout
            && let Some(d) = w
                .as_any_mut()
                .downcast_mut::<sid_widgets::WorkspaceDetailWidget>()
        {
            d.apply_row_git(path, git);
        }
        return;
    }
}
```

Update the `draw` match for the detail tab if it special-cases `WorkspaceDetailWidget` rendering — grep `render_into_frame` in wire.rs's `draw` and confirm the detail tab calls `widget.render_into_frame(f, area, &theme)`; the new signature is identical (`frame, area, theme`), so no change needed beyond confirming.

- [x] **Step 4: Re-run scoped tests**

Run: `cargo test -p sid scan_umbrella_satellites_finds && cargo test -p sid maybe_open_pending workspace_detail`
Expected: the new test passes; existing detail-related wire tests compile against the new widget API (fix any that referenced the removed `apply_scan_results`/`RepoSummary` table by switching them to `apply_satellites`/`rows()`).

- [x] **Step 5: Commit**

```bash
git add crates/sid/src/wire.rs
git commit -m "feat(sid): wire detail tab — satellite scan + per-row off-thread git loads + background-open"
```

---

### Task 8: Create-new registration wizard (substrate FormSpec) with feature checklist

Add the "create new workspace" wizard as a substrate side-pane form (`FormSpec`/`FormPane`), replacing the modal-based `workspaces.new`. The form: a name field, a path picker, a kind choice (Umbrella/Repo), and a feature checklist (Toggle fields) for the kind-dependent extras. Use the reshape hook so flipping kind to `Repo` hides the umbrella-only feature toggles. Persist via `Store::upsert_workspace`.

**Files:**
- Modify: `crates/sid/src/wire.rs` — add `workspaces_new_form()` builder + a `dispatch_form_submit` arm for `workspaces.create`; open the form from the `N` key handler (replace the `workspaces.new` modal opener at line 2271 with `open_form`)

- [x] **Step 1: Failing test — the form spec shape + reshape**

Add to wire.rs test mod:

```rust
    #[test]
    fn workspaces_new_form_reshapes_on_kind() {
        let mut spec = workspaces_new_form();
        // default kind Umbrella exposes the umbrella feature toggles
        let v = spec.values();
        assert_eq!(v["kind"], "Umbrella");
        assert!(spec.sections.iter().flat_map(|s| &s.fields).any(|f| f.key == "scan_satellites"));
        // flip to Repo, reshape drops umbrella-only toggles
        for s in &mut spec.sections {
            for f in &mut s.fields {
                if f.key == "kind" {
                    if let sid_widgets::Field::Choice { selected, .. } = &mut f.field {
                        *selected = 1; // Repo
                    }
                }
            }
        }
        spec.run_reshape();
        assert_eq!(spec.values()["kind"], "Repo");
        assert!(!spec.sections.iter().flat_map(|s| &s.fields).any(|f| f.key == "scan_satellites"));
    }

    #[test]
    fn dispatch_workspaces_create_persists_workspace() {
        let mut sid_app = build_test_sid_app(Some("workspaces"));
        let tmp = tempfile::tempdir().unwrap();
        let mut values = sid_widgets::FormValues::new();
        values.insert("name".into(), "neo".into());
        values.insert("path".into(), tmp.path().display().to_string());
        values.insert("kind".into(), "Repo".into());
        dispatch_form_submit(&mut sid_app, "workspaces.create", values);
        let ws = sid_app.store.list_workspaces().unwrap();
        assert_eq!(ws.len(), 1);
        assert_eq!(ws[0].name, "neo");
    }
```

- [x] **Step 2: Run (expect failure)**

Run: `cargo test -p sid workspaces_new_form_reshapes dispatch_workspaces_create_persists`
Expected: `workspaces_new_form` missing; `workspaces.create` arm absent.

- [x] **Step 3: Implement the form builder + reshape + dispatch arm**

```rust
/// Build the create-new-workspace side-pane form. Reshape on `kind`: Umbrella
/// shows the satellite-scan + feature toggles; Repo hides them.
pub fn workspaces_new_form() -> sid_widgets::FormSpec {
    sid_widgets::FormSpec::new("workspaces.create", "New Workspace", workspaces_new_sections(&sid_widgets::FormValues::new()))
        .with_reshape(vec!["kind".into()], workspaces_new_sections)
}

fn workspaces_new_sections(values: &sid_widgets::FormValues) -> Vec<sid_widgets::FormSection> {
    use sid_widgets::{Field, FormField, FormSection, SectionKind, Validate};
    let kind = values.get("kind").map(String::as_str).unwrap_or("Umbrella");
    let mut fields = vec![
        FormField::new(
            "name",
            Field::Text { label: "name".into(), value: String::new(), placeholder: Some("e.g. gen4-stack".into()) },
        )
        .with_validate(vec![Validate::NonEmpty]),
        FormField::new(
            "path",
            Field::Picker { label: "path".into(), value: String::new(), hint: "absolute path".into() },
        )
        .with_validate(vec![Validate::NonEmpty]),
        FormField::new(
            "kind",
            Field::Choice {
                label: "kind".into(),
                options: vec!["Umbrella".into(), "Repo".into()],
                selected: if kind == "Repo" { 1 } else { 0 },
            },
        ),
    ];
    let mut sections = vec![FormSection { title: "Workspace".into(), kind: SectionKind::Editable, fields: std::mem::take(&mut fields) }];
    if kind == "Umbrella" {
        sections.push(FormSection {
            title: "Features".into(),
            kind: SectionKind::Editable,
            fields: vec![
                FormField::new("scan_satellites", Field::Toggle { label: "scan satellites now".into(), value: true }),
                FormField::new("register_claude_md", Field::Toggle { label: "read CLAUDE.md actions".into(), value: true }),
            ],
        });
    }
    sections
}
```

Add the `dispatch_form_submit` arm. The substrate ships `dispatch_form_submit` with only a wildcard; add a `workspaces.create` arm *before* the wildcard:

```rust
        "workspaces.create" => {
            let name = values.get("name").cloned().unwrap_or_default();
            let path_str = values.get("path").cloned().unwrap_or_default();
            let kind = match values.get("kind").map(String::as_str) {
                Some("Repo") => sid_core::workspace_metadata::WorkspaceKind::Repo,
                _ => sid_core::workspace_metadata::WorkspaceKind::Umbrella,
            };
            match std::fs::canonicalize(&path_str) {
                Ok(path) => {
                    let ws = sid_store::Workspace {
                        path,
                        name: name.clone(),
                        kind,
                        manifest_hash: 0,
                        last_seen: sid_store::now_epoch(),
                        parent: None,
                    };
                    match sid_app.store.upsert_workspace(&ws) {
                        Ok(()) => {
                            refresh_workspaces_widget(sid_app);
                            sid_app.toasts.push(Toast::success(format!("workspace '{name}' added")));
                        }
                        Err(e) => {
                            sid_app.toasts.push(Toast::error(format!("add workspace: {e}")));
                        }
                    }
                }
                Err(e) => {
                    sid_app.toasts.push(Toast::error(format!("bad path '{path_str}': {e}")));
                }
            }
        }
```

Open the form from `N`. The current opener is the modal `workspaces_modal_for_key` (line 2263). The cleanest additive change: in the global key router where `workspaces_modal_for_key` is consulted, intercept `N`/`n` on the workspaces tab to call `open_form(sid_app, workspaces_new_form())` instead. Read how the router dispatches modal openers (grep `workspaces_modal_for_key` call site) and add the `N` → `open_form` branch ahead of it; leave the `A`/`R` modal arms intact for Task 9/keep. Confirm `now_epoch` is re-exported from `sid_store` (it is — `crates/sid-store/src/lib.rs:256`).

- [x] **Step 4: Re-run (expect pass)**

Run: `cargo test -p sid workspaces_new_form_reshapes dispatch_workspaces_create_persists`
Expected: PASS.

- [x] **Step 5: Commit**

```bash
git add crates/sid/src/wire.rs
git commit -m "feat(sid): create-new workspace wizard as substrate side-pane form with kind-reshaped feature checklist"
```

---

### Task 9: Adopt-existing wizard — directory scan, pre-checked multi-select confirm

Add the "adopt existing umbrella" flow: scan a chosen directory with `scan_adoptable_repos`, present each found repo as a pre-checked Toggle in a side-pane form, and on submit register the umbrella plus each checked satellite (satellites get `parent = umbrella path`). Use a reshape that rebuilds the toggle list when the scanned `dir` changes.

**Files:**
- Modify: `crates/sid/src/wire.rs` — add `workspaces_adopt_form(dir)` builder + a `workspaces.adopt` dispatch arm; open from a new `D`/`d` ("adopt directory") key on the workspaces tab

- [ ] **Step 1: Failing tests**

```rust
    #[test]
    fn adopt_form_lists_scanned_repos_as_prechecked_toggles() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("api").join(".git")).unwrap();
        std::fs::create_dir_all(tmp.path().join("web").join(".git")).unwrap();
        let spec = workspaces_adopt_form(tmp.path());
        let toggles: Vec<&str> = spec
            .sections
            .iter()
            .flat_map(|s| &s.fields)
            .filter(|f| f.key.starts_with("repo:"))
            .map(|f| f.key.as_str())
            .collect();
        assert_eq!(toggles.len(), 2);
        // every found repo is pre-checked (value true)
        for f in spec.sections.iter().flat_map(|s| &s.fields).filter(|f| f.key.starts_with("repo:")) {
            assert_eq!(f.value_string(), "true");
        }
    }

    #[test]
    fn dispatch_workspaces_adopt_registers_umbrella_and_checked_satellites() {
        let mut sid_app = build_test_sid_app(Some("workspaces"));
        let tmp = tempfile::tempdir().unwrap();
        let api = tmp.path().join("api");
        std::fs::create_dir_all(api.join(".git")).unwrap();
        let api_real = std::fs::canonicalize(&api).unwrap();

        let mut values = sid_widgets::FormValues::new();
        values.insert("dir".into(), tmp.path().display().to_string());
        values.insert("name".into(), "gen4".into());
        values.insert(format!("repo:{}", api_real.display()), "true".into());
        dispatch_form_submit(&mut sid_app, "workspaces.adopt", values);

        let ws = sid_app.store.list_workspaces().unwrap();
        // umbrella + 1 satellite
        assert_eq!(ws.len(), 2);
        let umbrella = ws.iter().find(|w| w.name == "gen4").unwrap();
        let sat = ws.iter().find(|w| w.parent.is_some()).unwrap();
        assert_eq!(sat.parent.as_ref().unwrap(), &std::fs::canonicalize(tmp.path()).unwrap());
    }
```

- [ ] **Step 2: Run (expect failure)**

Run: `cargo test -p sid adopt_form_lists adopt_registers`
Expected: `workspaces_adopt_form` missing; `workspaces.adopt` arm absent.

- [ ] **Step 3: Implement**

```rust
/// Build the adopt-existing-umbrella form for `dir`: a name field plus one
/// pre-checked Toggle per repo found one level under `dir`. The repo path is
/// encoded in the toggle key (`repo:<abs-path>`) so the submit handler can
/// register each checked satellite without a second scan.
pub fn workspaces_adopt_form(dir: &std::path::Path) -> sid_widgets::FormSpec {
    use sid_widgets::{Field, FormField, FormSection, SectionKind, Validate};
    use sid_core::workspace_discovery::scan_adoptable_repos;
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("umbrella").to_string();
    let mut header = vec![
        FormField::new("dir", Field::Display { label: "directory".into(), body: dir.display().to_string() }),
        FormField::new("name", Field::Text { label: "umbrella name".into(), value: name, placeholder: None })
            .with_validate(vec![Validate::NonEmpty]),
    ];
    let repos = scan_adoptable_repos(dir);
    let toggles: Vec<FormField> = repos
        .iter()
        .map(|r| {
            FormField::new(
                format!("repo:{}", r.path.display()),
                Field::Toggle { label: r.name.clone(), value: true },
            )
        })
        .collect();
    let mut sections = vec![FormSection {
        title: "Umbrella".into(),
        kind: SectionKind::Editable,
        fields: std::mem::take(&mut header),
    }];
    if toggles.is_empty() {
        sections.push(FormSection {
            title: "Satellites".into(),
            kind: SectionKind::Info,
            fields: vec![FormField::new("none", Field::Display { label: "found".into(), body: "no git repos found under this directory".into() })],
        });
    } else {
        sections.push(FormSection { title: "Satellites".into(), kind: SectionKind::Editable, fields: toggles });
    }
    sid_widgets::FormSpec::new("workspaces.adopt", "Adopt Existing Umbrella", sections)
}
```

Dispatch arm (before the wildcard, alongside `workspaces.create`):

```rust
        "workspaces.adopt" => {
            let dir_str = values.get("dir").cloned().unwrap_or_default();
            let name = values.get("name").cloned().unwrap_or_default();
            let umbrella_path = match std::fs::canonicalize(&dir_str) {
                Ok(p) => p,
                Err(e) => {
                    sid_app.toasts.push(Toast::error(format!("bad directory '{dir_str}': {e}")));
                    sid_app.form = None;
                    sid_app.form_origin_tab = None;
                    return;
                }
            };
            // Register the umbrella.
            let umbrella = sid_store::Workspace {
                path: umbrella_path.clone(),
                name: name.clone(),
                kind: sid_core::workspace_metadata::WorkspaceKind::Umbrella,
                manifest_hash: 0,
                last_seen: sid_store::now_epoch(),
                parent: None,
            };
            let mut errors = 0usize;
            let mut added = 0usize;
            if sid_app.store.upsert_workspace(&umbrella).is_err() {
                errors += 1;
            } else {
                added += 1;
            }
            // Register each checked satellite (key = "repo:<path>", value "true").
            for (key, val) in values.iter() {
                let Some(path_str) = key.strip_prefix("repo:") else { continue };
                if val != "true" {
                    continue;
                }
                let sat_path = std::path::PathBuf::from(path_str);
                let sat_name = sat_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("repo")
                    .to_string();
                let sat = sid_store::Workspace {
                    path: sat_path,
                    name: sat_name,
                    kind: sid_core::workspace_metadata::WorkspaceKind::Repo,
                    manifest_hash: 0,
                    last_seen: sid_store::now_epoch(),
                    parent: Some(umbrella_path.clone()),
                };
                if sid_app.store.upsert_workspace(&sat).is_err() {
                    errors += 1;
                } else {
                    added += 1;
                }
            }
            refresh_workspaces_widget(sid_app);
            if errors == 0 {
                sid_app.toasts.push(Toast::success(format!("adopted '{name}' + {} satellites", added.saturating_sub(1))));
            } else {
                sid_app.toasts.push(Toast::error(format!("adopted with {errors} error(s)")));
            }
        }
```

Open from `D`/`d` on the workspaces tab. In the same router branch you added the `N` → `open_form` case (Task 8), add: `D`/`d` → resolve a directory and `open_form(sid_app, workspaces_adopt_form(&dir))`. For v1 the directory is the currently-selected workspace path if one is selected, else the first default discovery root; reuse `workspaces_selected_path(sid_app)` (line 3024) and fall back to `default_workspace_roots()` (line 1405). Keep the picker simple — the `dir` Display field shows what was scanned; a future task can add an in-form directory picker.

Note for the executor: `dispatch_form_submit`'s wildcard tail (substrate) sets `sid_app.form = None; sid_app.form_origin_tab = None;` after the match. The `workspaces.adopt` early-return-on-bad-dir branch above sets those itself before returning; do not double-clear. Verify the substrate's exact post-match cleanup and match it (the `return` inside the match arm skips the tail, so the explicit clear is required there).

- [ ] **Step 4: Re-run (expect pass)**

Run: `cargo test -p sid adopt_form_lists adopt_registers`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/sid/src/wire.rs
git commit -m "feat(sid): adopt-existing umbrella wizard — directory scan, pre-checked satellite multi-select, parent-linked persistence"
```

---

### Task 10: Overview list "+ add new" row + `show_add_new_row` honoring

Per master-plan decision 6, the overview list gains a synthetic "+ add new" first row (governed by `show_add_new_row`, default on). Selecting it (Enter) opens the create-new form. The widget gets a `ListCursor` with `add_new` hydrated from the binary. `N` keeps working regardless (decision 6).

**Files:**
- Modify: `crates/sid-widgets/src/workspaces.rs` (`WorkspacesState` — add `add_new: bool` + a `ListCursor`-backed selection; or a minimal flag + Enter arm)
- Modify: `crates/sid/src/wire.rs` (hydrate `add_new` from `load_show_add_new_row` when constructing/refreshing the widget; handle the add-new Enter outcome)

- [ ] **Step 1: Failing tests (widget side)**

```rust
    #[test]
    fn add_new_row_selectable_and_signals_open_form() {
        use sid_core::context::WidgetCtx;
        use sid_core::event::{Event, KeyChord};
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut w = WorkspacesWidget::new(vec![umbrella_ws()], None);
        w.set_show_add_new_row(true);
        // cursor starts on the add-new row when enabled
        assert!(w.add_new_selected());
        let mut reg = sid_core::action::ActionRegistry::new();
        let mut ctx = WidgetCtx::new(&mut reg);
        let ev = Event::Key(KeyChord { code: KeyCode::Enter, mods: KeyModifiers::NONE });
        let _ = w.handle_event(&ev, &mut ctx);
        assert!(w.take_pending_add_new());
        // Enter on add-new must NOT also queue a detail open
        assert!(w.take_pending_open_detail().is_none());
    }

    #[test]
    fn add_new_disabled_hides_row() {
        let mut w = WorkspacesWidget::new(vec![umbrella_ws()], None);
        w.set_show_add_new_row(false);
        assert!(!w.add_new_selected());
    }
```

- [ ] **Step 2: Run (expect failure)**

Run: `cargo test -p sid-widgets workspaces::tests::add_new`
Expected: `set_show_add_new_row` / `add_new_selected` / `take_pending_add_new` missing.

- [ ] **Step 3: Implement on the widget**

Add to `WorkspacesWidget` (next to `pending_open_detail`):

```rust
    /// Whether the synthetic "+ add new" first row is shown (hydrated by the
    /// binary from `load_show_add_new_row`).
    show_add_new_row: bool,
    /// True when the cursor sits on the add-new row and Enter was pressed.
    pending_add_new: bool,
```

Initialize both in `new`/`default` (false / false), then add:

```rust
    /// Hydrate the add-new-row toggle from settings; moves the cursor onto the
    /// add-new row when enabled, clears it when disabled.
    pub fn set_show_add_new_row(&mut self, show: bool) {
        self.show_add_new_row = show;
        self.at_add_new = show; // cursor lands on add-new row when the row is first enabled
    }

    /// Whether the cursor currently points at the synthetic add-new row.
    /// True when `show_add_new_row` is enabled and `at_add_new` is set.
    pub fn add_new_selected(&self) -> bool {
        self.show_add_new_row && self.at_add_new
    }

    /// Drain the add-new-pressed flag.
    pub fn take_pending_add_new(&mut self) -> bool {
        std::mem::take(&mut self.pending_add_new)
    }
```

The selection bridge uses a plain `bool at_add_new` stored on `WorkspacesWidget` — no new accessor on `WorkspacesState` is needed. The invariants are:

- `set_show_add_new_row(true)` sets `at_add_new = true` so the cursor starts on the add-new row.
- `set_show_add_new_row(false)` clears `at_add_new`.
- `↓`/`j` when `at_add_new` is true: clear `at_add_new` (cursor lands on item 0, which is already `state.selected_visible_idx = 0`; no call to `state.select_next()`).
- `↑`/`k` when `at_add_new` is true: clear `at_add_new` and call `state.select_prev()` to wrap to the last visible item.
- `↑`/`k` when `at_add_new` is false and `state.selected_visible_idx == 0` and `show_add_new_row` is true: set `at_add_new = true` (land on add-new row instead of wrapping state).
- All other `↑`/`↓`/`j`/`k` events: `at_add_new` remains false; delegate to `state.select_next()` / `state.select_prev()` as before.

Add `at_add_new: bool` to the `WorkspacesWidget` struct (alongside `pending_add_new`) and initialize it to `false` in `new`/`default`. Rewrite the `↑`/`↓` Tree arms in `handle_event` to incorporate the `at_add_new` gate:

```rust
    (KeyCode::Char('j') | KeyCode::Down, KeyModifiers::NONE) => {
        if self.at_add_new {
            self.at_add_new = false;
            // state.selected_visible_idx stays at 0 — first real item
        } else {
            self.state.select_next();
        }
        return EventOutcome::Consumed;
    }
    (KeyCode::Char('k') | KeyCode::Up, KeyModifiers::NONE) => {
        if self.at_add_new {
            self.at_add_new = false;
            self.state.select_prev(); // wraps to last visible item
        } else if self.show_add_new_row && self.state.selected_visible_idx() == 0 {
            self.at_add_new = true; // land on add-new row
        } else {
            self.state.select_prev();
        }
        return EventOutcome::Consumed;
    }
```

`WorkspacesState::selected_visible_idx()` is a trivial pub accessor returning `self.selected_visible_idx` — add it next to `visible_count` if not already present.

In the Enter arm (Task 6's rewrite), gate on add-new first:

```rust
                    (KeyCode::Enter, KeyModifiers::NONE) => {
                        if self.add_new_selected() {
                            ctx.emit_action("workspaces.add_new");
                            self.pending_add_new = true;
                            return EventOutcome::Consumed;
                        }
                        let selected = self.state.selected_workspace().cloned();
                        if let Some(ws) = selected {
                            ctx.emit_action("workspaces.open_detail");
                            self.pending_open_detail = Some(ws);
                        }
                        return EventOutcome::Consumed;
                    }
```

In `render_tree` (line 1447), prepend a `+ add new` line when `show_add_new_row` is true, styled accent and highlighted when `add_new_selected()`.

- [ ] **Step 4: Wire hydration + drain in the binary**

In `build_app_full` (line 707) and `refresh_workspaces_widget` (line 4772), after constructing/replacing the widget state, call `ww.set_show_add_new_row(load_show_add_new_row(&*store))`. Drain the add-new flag in the same place `maybe_open_pending_workspace_detail` runs (line 1701 area): a new `maybe_open_pending_new_form(sid_app)` that, if the workspaces widget's `take_pending_add_new()` is true, calls `open_form(sid_app, workspaces_new_form())`.

Add a wire test:

```rust
    #[test]
    fn add_new_enter_opens_create_form() {
        let mut sid_app = build_test_sid_app(Some("workspaces"));
        // force add-new selection + flag, then drain
        if let Some(ww) = workspaces_widget_mut(&mut sid_app) {
            ww.set_show_add_new_row(true);
        }
        // simulate the widget having flagged add-new (drive via its public setter path)
        // … then:
        maybe_open_pending_new_form(&mut sid_app);
        // when the flag was set, a form is now open with id workspaces.create
    }
```

Note for the executor: `workspaces_widget_mut` may not exist — reuse the downcast pattern from `maybe_open_pending_workspace_detail` (line 1788) inline, or extract a small `fn workspaces_widget_mut(&mut SidApp) -> Option<&mut WorkspacesWidget>` helper and use it in both spots (additive, single definition). Keep the test honest: set the flag through the real key path if a public setter for `pending_add_new` is undesirable, else add a `#[cfg(test)]` seam. Prefer driving the real `handle_event` Enter on the add-new row.

- [ ] **Step 5: Re-run (expect pass)**

Run: `cargo test -p sid-widgets workspaces::tests::add_new && cargo test -p sid add_new_enter_opens`
Expected: PASS. Accept any churned overview snapshot via `cargo insta review` if the `+ add new` row changed a golden file.

- [ ] **Step 6: Commit**

```bash
git add crates/sid-widgets/src/workspaces.rs crates/sid/src/wire.rs crates/sid-widgets/src/snapshots/
git commit -m "feat(sid-widgets,sid): overview + add new row honoring show_add_new_row; Enter opens create form"
```

---

### Task 11: Branch wrap-up — targeted regression sweep + clippy on touched crates

- [ ] **Step 1: Scoped test sweep**

Run: `cargo test -p sid-core -p sid-widgets -p sid`
Expected: green. Most likely red spots: stale wire tests referencing the removed `apply_scan_results`/`WorkspaceDetailScanned`/`RepoSummary` table — convert them to the new `apply_satellites`/`rows()` API; overview snapshot churn from the add-new row.

- [ ] **Step 2: Clippy on touched crates**

Run: `cargo clippy -p sid-core -p sid-widgets -p sid --all-targets -- -D warnings`
Expected: clean. Watch for: unused `git_factory` field warnings (the `#[allow(dead_code)]` on the detail widget's field covers it), and `as u32` lossy-cast lints on the dirty count (use `u32::try_from(..).unwrap_or(u32::MAX)` if clippy flags it).

- [ ] **Step 3: Tick this plan's checkboxes, then finish the branch**

```bash
git add docs/superpowers/plans/2026-06-11-uxv2-1-workspaces.md
git commit -m "docs(plans): tick uxv2-1 workspaces tasks"
# merge to main per the repo's 'Merge branch #N' convention (see git log)
```
