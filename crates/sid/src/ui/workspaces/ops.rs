//! Workspaces v2 git operations: state, the background runner, and every mutation
//! (stage / commit / discard / stash / branches / fetch / push). Every call runs on the
//! shared runtime via [`AppState::run_git`], never in `render`, and reports back through
//! an inline notice.

use std::time::Instant;

use gpui::{Context, Subscription, Window, prelude::*};
use sid_core::git::{
    Diff, DiffLimits, DiffTarget, GitError, GitProvider, LogEntry, LogQuery, StashEntry,
};
use sid_ui::component::{InputEvent, InputState};
use sid_ui::{Confirm, ConfirmArm, ToastTone};

use super::model::{self, Side, branch_name_problem, friendly_error, key_of, short_oid};
use crate::app::AppState;
use crate::git_registry;
use crate::ui::session::ssh_runtime;

use super::{Fetch, GitPanelError};

/// Commits fetched per log page.
pub(crate) const LOG_PAGE: usize = 50;

/// Loaded log pages so far.
#[derive(Clone, Default)]
pub(crate) struct LogState {
    pub entries: Vec<LogEntry>,
    pub has_more: bool,
}

/// Everything the v2 sub-views keep between frames.
pub(crate) struct GitV2 {
    pub selected_file: Option<(String, Side)>,
    pub diff: Option<Fetch<Diff>>,
    pub commit_input: Option<gpui::Entity<InputState>>,
    pub amend: bool,
    /// Clear the commit box on the next frame (needs a `Window`, which a completion lacks).
    pub clear_commit: bool,
    /// Label of the operation in flight ("Fetching…"); at most one at a time.
    pub busy: Option<&'static str>,
    pub notice: Option<(ToastTone, String)>,
    pub discard_arm: ConfirmArm<u64>,

    pub stash: Option<Fetch<Vec<StashEntry>>>,
    pub stash_input: Option<gpui::Entity<InputState>>,
    pub stash_untracked: bool,
    pub stash_input_clear: bool,
    pub drop_arm: ConfirmArm<usize>,

    pub branch_input: Option<gpui::Entity<InputState>>,
    pub rename_input: Option<gpui::Entity<InputState>>,
    pub rename_from: Option<String>,
    pub delete_arm: ConfirmArm<u64>,
    /// A branch git refused to delete as unmerged, awaiting an explicit force.
    pub force_delete: Option<String>,
    pub branch_input_clear: bool,

    pub log: Option<Fetch<LogState>>,
    pub log_more_loading: bool,

    pub _subs: Vec<Subscription>,
}

impl GitV2 {
    pub(crate) fn new() -> Self {
        Self {
            selected_file: None,
            diff: None,
            commit_input: None,
            amend: false,
            clear_commit: false,
            busy: None,
            notice: None,
            discard_arm: ConfirmArm::new(),
            stash: None,
            stash_input: None,
            stash_untracked: false,
            stash_input_clear: false,
            drop_arm: ConfirmArm::new(),
            branch_input: None,
            rename_input: None,
            rename_from: None,
            delete_arm: ConfirmArm::new(),
            force_delete: None,
            branch_input_clear: false,
            log: None,
            log_more_loading: false,
            _subs: Vec::new(),
        }
    }

    /// Forget everything tied to the previously selected workspace.
    pub(crate) fn reset_for_selection(&mut self) {
        self.selected_file = None;
        self.diff = None;
        self.amend = false;
        self.busy = None;
        self.notice = None;
        self.discard_arm.disarm();
        self.stash = None;
        self.drop_arm.disarm();
        self.rename_from = None;
        self.delete_arm.disarm();
        self.force_delete = None;
        self.log = None;
        self.log_more_loading = false;
    }
}

impl AppState {
    /// Build the v2 text inputs (they need a `Window`) and wire the commit box so
    /// typing re-renders the Commit button's enabled state.
    pub(super) fn ensure_git_v2_widgets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspaces.v2.commit_input.is_none() {
            let commit = cx.new(|cx| InputState::new(window, cx).placeholder("Commit message"));
            let sub = cx.subscribe_in(&commit, window, |_this, _i, ev: &InputEvent, _w, cx| {
                if matches!(ev, InputEvent::Change) {
                    cx.notify();
                }
            });
            self.workspaces.v2._subs.push(sub);
            self.workspaces.v2.commit_input = Some(commit);
            self.workspaces.v2.stash_input = Some(
                cx.new(|cx| InputState::new(window, cx).placeholder("Stash message (optional)")),
            );
            let branch = cx.new(|cx| InputState::new(window, cx).placeholder("New branch name"));
            let sub = cx.subscribe_in(&branch, window, |_this, _i, ev: &InputEvent, _w, cx| {
                if matches!(ev, InputEvent::Change) {
                    cx.notify();
                }
            });
            self.workspaces.v2._subs.push(sub);
            self.workspaces.v2.branch_input = Some(branch);
        }
        if std::mem::take(&mut self.workspaces.v2.stash_input_clear)
            && let Some(i) = self.workspaces.v2.stash_input.clone()
        {
            i.update(cx, |s, cx| s.set_value("", window, cx));
        }
        if std::mem::take(&mut self.workspaces.v2.branch_input_clear)
            && let Some(i) = self.workspaces.v2.branch_input.clone()
        {
            i.update(cx, |s, cx| s.set_value("", window, cx));
        }
        if self.workspaces.v2.clear_commit {
            self.workspaces.v2.clear_commit = false;
            if let Some(i) = self.workspaces.v2.commit_input.clone() {
                i.update(cx, |s, cx| s.set_value("", window, cx));
            }
        }
    }

    pub(super) fn set_notice(&mut self, tone: ToastTone, msg: impl Into<String>) {
        self.workspaces.v2.notice = Some((tone, msg.into()));
    }

    /// Run `work` against the selected workspace's repo off the UI thread, then `apply`
    /// the result on it. With `guard`, a result for a workspace that is no longer
    /// selected is dropped (reads); mutations pass `false` so their outcome is never lost.
    pub(super) fn run_git<T, W, A>(
        &mut self,
        guard: bool,
        cx: &mut Context<Self>,
        work: W,
        apply: A,
    ) where
        T: Send + 'static,
        W: FnOnce(Box<dyn GitProvider>) -> Result<T, GitError> + Send + 'static,
        A: FnOnce(&mut AppState, Result<T, GitError>, &mut Context<AppState>) + 'static,
    {
        let Some((id, root)) = self.selected_workspace_root() else {
            return;
        };
        let generation = self.workspaces.detail_generation;
        cx.spawn(async move |this, cx| {
            let handle = ssh_runtime()
                .spawn(async move { git_registry::factory().open(&root).and_then(work) });
            let outcome = match handle.await {
                Ok(r) => r,
                Err(e) => Err(GitError::Other(format!("git task panicked: {e}"))),
            };
            let _ = this.update(cx, |this, cx| {
                if guard
                    && (this.workspaces.detail_generation != generation
                        || this.workspaces.selected.as_ref() != Some(&id))
                {
                    return;
                }
                apply(this, outcome, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// A write: marks the pane busy, reports failure as a danger notice, and on success
    /// runs `on_ok` then refreshes everything the write could have changed.
    fn mutate<T, W, A>(&mut self, label: &'static str, cx: &mut Context<Self>, work: W, on_ok: A)
    where
        T: Send + 'static,
        W: FnOnce(&mut Box<dyn GitProvider>) -> Result<T, GitError> + Send + 'static,
        A: FnOnce(&mut AppState, T) + 'static,
    {
        if self.workspaces.v2.busy.is_some() {
            return;
        }
        self.workspaces.v2.busy = Some(label);
        self.workspaces.v2.notice = None;
        cx.notify();
        self.run_git(
            false,
            cx,
            move |mut repo| work(&mut repo),
            move |this, out, cx| {
                this.workspaces.v2.busy = None;
                match out {
                    Ok(v) => {
                        on_ok(this, v);
                        this.after_mutation(cx);
                    }
                    Err(e) => this.set_notice(ToastTone::Danger, friendly_error(&e)),
                }
            },
        );
    }

    /// Re-read what a write may have changed. Status keeps showing the old rows until the
    /// new ones land (no flicker); the other lazy views reload when next shown.
    pub(super) fn after_mutation(&mut self, cx: &mut Context<Self>) {
        self.workspaces.branches = None;
        self.workspaces.v2.stash = None;
        self.workspaces.v2.log = None;
        self.refresh_status(cx);
        if let Some((id, root)) = self.selected_workspace_root() {
            self.fetch_summary(id, root, cx);
        }
    }

    pub(super) fn refresh_status(&mut self, cx: &mut Context<Self>) {
        self.run_git(
            true,
            cx,
            |r| r.status(),
            |this, out, cx| {
                this.workspaces.status = Some(Fetch::Done(out.map_err(GitPanelError::from)));
                this.sync_selection();
                this.load_diff(cx);
            },
        );
    }

    /// Keep the selected file selected across a status reload — following it to the
    /// other side when staging moved it — or drop the selection if it is gone.
    fn sync_selection(&mut self) {
        let Some((path, side)) = self.workspaces.v2.selected_file.clone() else {
            return;
        };
        let Some(Fetch::Done(Ok(st))) = &self.workspaces.status else {
            return;
        };
        let b = model::bucket(&st.files);
        let has = |list: &[sid_core::git::FileStatus]| list.iter().any(|f| f.path == path);
        let same = match side {
            Side::Staged => has(&b.staged),
            Side::Unstaged => has(&b.unstaged),
            Side::Untracked => has(&b.untracked),
        };
        if same {
            return;
        }
        self.workspaces.v2.selected_file = if has(&b.unstaged) {
            Some((path, Side::Unstaged))
        } else if has(&b.staged) {
            Some((path, Side::Staged))
        } else if has(&b.untracked) {
            Some((path, Side::Untracked))
        } else {
            None
        };
    }

    pub(super) fn select_file(&mut self, path: String, side: Side, cx: &mut Context<Self>) {
        self.workspaces.v2.selected_file = Some((path, side));
        self.load_diff(cx);
        cx.notify();
    }

    pub(super) fn load_diff(&mut self, cx: &mut Context<Self>) {
        let Some((path, side)) = self.workspaces.v2.selected_file.clone() else {
            self.workspaces.v2.diff = None;
            return;
        };
        if !matches!(self.workspaces.v2.diff, Some(Fetch::Done(_))) {
            self.workspaces.v2.diff = Some(Fetch::Loading);
        }
        let target = match side {
            Side::Staged => DiffTarget::IndexVsHead,
            Side::Unstaged | Side::Untracked => DiffTarget::WorkdirVsIndex,
        };
        let want = (path, side);
        self.run_git(
            true,
            cx,
            move |r| r.diff(&target, &DiffLimits::default()),
            move |this, out, _cx| {
                if this.workspaces.v2.selected_file.as_ref() == Some(&want) {
                    this.workspaces.v2.diff = Some(Fetch::Done(out.map_err(GitPanelError::from)));
                }
            },
        );
    }

    // ---- staging ----------------------------------------------------------------------

    pub(super) fn stage(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        self.mutate("Staging…", cx, move |r| r.stage_paths(&paths), |_, _| {});
    }

    pub(super) fn unstage(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        self.mutate(
            "Unstaging…",
            cx,
            move |r| r.unstage_paths(&paths),
            |_, _| {},
        );
    }

    pub(super) fn stage_all(&mut self, cx: &mut Context<Self>) {
        self.mutate("Staging…", cx, |r| r.stage_all(), |_, _| {});
    }

    pub(super) fn unstage_all(&mut self, cx: &mut Context<Self>) {
        self.mutate("Unstaging…", cx, |r| r.unstage_all(), |_, _| {});
    }

    /// Two-step: the first press arms, the second discards. Tracked files are restored
    /// from the index; untracked files are deleted.
    pub(super) fn discard(&mut self, path: String, side: Side, cx: &mut Context<Self>) {
        let key = key_of(&format!("{path}\0{side:?}"));
        if self.workspaces.v2.discard_arm.press(key, Instant::now()) == Confirm::Armed {
            cx.notify();
            return;
        }
        let was_untracked = side == Side::Untracked;
        let name = path.clone();
        self.mutate(
            "Discarding…",
            cx,
            move |r| {
                let p = [path];
                if was_untracked {
                    r.remove_untracked(&p)
                } else {
                    r.discard_paths(&p)
                }
            },
            move |this, _| {
                this.set_notice(
                    ToastTone::Success,
                    if was_untracked {
                        format!("Deleted {name}.")
                    } else {
                        format!("Discarded changes to {name}.")
                    },
                );
            },
        );
    }

    // ---- commit -----------------------------------------------------------------------

    pub(super) fn commit_message(&self, cx: &gpui::App) -> String {
        self.workspaces
            .v2
            .commit_input
            .as_ref()
            .map(|i| i.read(cx).value().to_string())
            .unwrap_or_default()
    }

    pub(super) fn commit(&mut self, cx: &mut Context<Self>) {
        let msg = self.commit_message(cx);
        let amend = self.workspaces.v2.amend;
        if msg.trim().is_empty() {
            self.set_notice(ToastTone::Warning, friendly_error(&GitError::EmptyMessage));
            cx.notify();
            return;
        }
        self.mutate(
            if amend {
                "Amending…"
            } else {
                "Committing…"
            },
            cx,
            move |r| r.commit(&msg, amend),
            move |this, info| {
                this.workspaces.v2.clear_commit = true;
                this.workspaces.v2.amend = false;
                this.set_notice(
                    ToastTone::Success,
                    format!(
                        "{} {} — {}",
                        if amend { "Amended" } else { "Committed" },
                        short_oid(&info.oid),
                        info.summary
                    ),
                );
            },
        );
    }

    /// Pre-fill nothing; just flips the amend flag (the Commit rule reads it).
    pub(super) fn toggle_amend(&mut self, on: bool, cx: &mut Context<Self>) {
        self.workspaces.v2.amend = on;
        cx.notify();
    }

    // ---- stash ------------------------------------------------------------------------

    pub(super) fn ensure_stash_loaded(&mut self, cx: &mut Context<Self>) {
        if self.workspaces.v2.stash.is_some() {
            return;
        }
        self.workspaces.v2.stash = Some(Fetch::Loading);
        self.run_git(
            true,
            cx,
            |r| r.stash_list(),
            |this, out, _| {
                this.workspaces.v2.stash = Some(Fetch::Done(out.map_err(GitPanelError::from)));
            },
        );
    }

    pub(super) fn stash_push(&mut self, cx: &mut Context<Self>) {
        let msg = self
            .workspaces
            .v2
            .stash_input
            .as_ref()
            .map(|i| i.read(cx).value().trim().to_string())
            .filter(|m| !m.is_empty());
        let untracked = self.workspaces.v2.stash_untracked;
        self.mutate(
            "Stashing…",
            cx,
            move |r| r.stash_push(msg.as_deref(), untracked),
            |this, _| {
                this.workspaces.v2.stash_input_clear = true;
                this.set_notice(ToastTone::Success, "Stashed your changes.");
            },
        );
    }

    pub(super) fn stash_apply(&mut self, index: usize, pop: bool, cx: &mut Context<Self>) {
        self.mutate(
            if pop { "Popping…" } else { "Applying…" },
            cx,
            move |r| {
                if pop {
                    r.stash_pop(index)
                } else {
                    r.stash_apply(index)
                }
            },
            move |this, _| {
                this.set_notice(
                    ToastTone::Success,
                    if pop {
                        "Popped the stash onto your working tree."
                    } else {
                        "Applied the stash. It is still in the list."
                    },
                );
            },
        );
    }

    /// Two-step drop.
    pub(super) fn stash_drop(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.workspaces.v2.drop_arm.press(index, Instant::now()) == Confirm::Armed {
            cx.notify();
            return;
        }
        self.mutate(
            "Dropping…",
            cx,
            move |r| r.stash_drop(index),
            |this, _| this.set_notice(ToastTone::Success, "Dropped the stash."),
        );
    }

    // ---- branches ---------------------------------------------------------------------

    pub(super) fn create_branch(&mut self, cx: &mut Context<Self>) {
        let name = self
            .workspaces
            .v2
            .branch_input
            .as_ref()
            .map(|i| i.read(cx).value().trim().to_string())
            .unwrap_or_default();
        if let Some(problem) = branch_name_problem(&name) {
            self.set_notice(ToastTone::Warning, problem);
            cx.notify();
            return;
        }
        let shown = name.clone();
        self.mutate(
            "Creating branch…",
            cx,
            move |r| r.create_branch(&name, None),
            move |this, _| {
                this.workspaces.v2.branch_input_clear = true;
                this.set_notice(
                    ToastTone::Success,
                    format!("Created '{shown}' at the current commit. Click it to switch."),
                );
            },
        );
    }

    pub(super) fn start_rename_branch(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("New name");
            s.set_value(name.clone(), window, cx);
            s
        });
        input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        self.workspaces.v2.rename_input = Some(input);
        self.workspaces.v2.rename_from = Some(name);
        cx.notify();
    }

    pub(super) fn cancel_rename_branch(&mut self, cx: &mut Context<Self>) {
        self.workspaces.v2.rename_from = None;
        self.workspaces.v2.rename_input = None;
        cx.notify();
    }

    pub(super) fn commit_rename_branch(&mut self, cx: &mut Context<Self>) {
        let (Some(old), Some(input)) = (
            self.workspaces.v2.rename_from.clone(),
            self.workspaces.v2.rename_input.clone(),
        ) else {
            return;
        };
        let new = input.read(cx).value().trim().to_string();
        if let Some(problem) = branch_name_problem(&new) {
            self.set_notice(ToastTone::Warning, problem);
            cx.notify();
            return;
        }
        self.workspaces.v2.rename_from = None;
        self.workspaces.v2.rename_input = None;
        let shown = format!("Renamed '{old}' to '{new}'.");
        self.mutate(
            "Renaming…",
            cx,
            move |r| r.rename_branch(&old, &new),
            move |this, _| this.set_notice(ToastTone::Success, shown),
        );
    }

    /// Delete `name`. The current branch is refused up front; an unmerged branch is not
    /// deleted until the user explicitly forces it (`force`).
    pub(super) fn delete_branch(&mut self, name: String, force: bool, cx: &mut Context<Self>) {
        let is_current = matches!(
            &self.workspaces.branches,
            Some(Fetch::Done(Ok(bs))) if bs.iter().any(|b| b.name == name && b.is_current)
        );
        if is_current {
            self.set_notice(
                ToastTone::Warning,
                friendly_error(&GitError::BranchIsCurrent(name)),
            );
            cx.notify();
            return;
        }
        if !force
            && self
                .workspaces
                .v2
                .delete_arm
                .press(key_of(&name), Instant::now())
                == Confirm::Armed
        {
            cx.notify();
            return;
        }
        if self.workspaces.v2.busy.is_some() {
            return;
        }
        self.workspaces.v2.force_delete = None;
        self.workspaces.v2.busy = Some("Deleting branch…");
        self.workspaces.v2.notice = None;
        cx.notify();
        let n = name.clone();
        self.run_git(
            false,
            cx,
            move |mut repo| repo.delete_branch(&n, force),
            move |this, out, cx| {
                this.workspaces.v2.busy = None;
                match out {
                    Ok(()) => {
                        this.set_notice(ToastTone::Success, format!("Deleted '{name}'."));
                        this.after_mutation(cx);
                    }
                    Err(GitError::BranchNotMerged(n)) => this.workspaces.v2.force_delete = Some(n),
                    Err(e) => this.set_notice(ToastTone::Danger, friendly_error(&e)),
                }
            },
        );
    }

    // ---- log --------------------------------------------------------------------------

    pub(super) fn ensure_log_loaded(&mut self, cx: &mut Context<Self>) {
        if self.workspaces.v2.log.is_some() {
            return;
        }
        self.workspaces.v2.log = Some(Fetch::Loading);
        self.run_git(
            true,
            cx,
            |r| {
                r.log_page(&LogQuery {
                    limit: LOG_PAGE,
                    ..LogQuery::default()
                })
            },
            |this, out, _| {
                this.workspaces.v2.log = Some(Fetch::Done(
                    out.map(|p| LogState {
                        entries: p.entries,
                        has_more: p.has_more,
                    })
                    .map_err(GitPanelError::from),
                ));
            },
        );
    }

    pub(super) fn load_more_log(&mut self, cx: &mut Context<Self>) {
        let Some(Fetch::Done(Ok(state))) = &self.workspaces.v2.log else {
            return;
        };
        if self.workspaces.v2.log_more_loading || !state.has_more {
            return;
        }
        let skip = state.entries.len();
        self.workspaces.v2.log_more_loading = true;
        cx.notify();
        self.run_git(
            true,
            cx,
            move |r| {
                r.log_page(&LogQuery {
                    skip,
                    limit: LOG_PAGE,
                    ..LogQuery::default()
                })
            },
            |this, out, _| {
                this.workspaces.v2.log_more_loading = false;
                match out {
                    Ok(page) => {
                        if let Some(Fetch::Done(Ok(s))) = &mut this.workspaces.v2.log {
                            s.entries.extend(page.entries);
                            s.has_more = page.has_more;
                        }
                    }
                    Err(e) => this.set_notice(ToastTone::Danger, friendly_error(&e)),
                }
            },
        );
    }

    // ---- network ----------------------------------------------------------------------

    pub(super) fn fetch_remote(&mut self, cx: &mut Context<Self>) {
        self.mutate(
            "Fetching…",
            cx,
            |r| {
                let remotes = r.remotes()?;
                let name = remotes
                    .iter()
                    .find(|n| n.as_str() == "origin")
                    .or(remotes.first())
                    .cloned()
                    .ok_or_else(|| GitError::RemoteNotFound("origin".into()))?;
                r.fetch(&name)
            },
            |this, rep| {
                let moved = rep.updated_refs.len();
                let tail = match (rep.ahead, rep.behind) {
                    (Some(a), Some(b)) => format!(" You are {a} ahead and {b} behind."),
                    _ => String::new(),
                };
                this.set_notice(
                    ToastTone::Success,
                    if moved == 0 {
                        format!("Fetched {} — already up to date.{tail}", rep.remote)
                    } else {
                        format!("Fetched {} — {moved} ref(s) updated.{tail}", rep.remote)
                    },
                );
            },
        );
    }

    pub(super) fn push_current(&mut self, cx: &mut Context<Self>) {
        self.mutate(
            "Pushing…",
            cx,
            |r| r.push(),
            |this, rep| {
                this.set_notice(
                    ToastTone::Success,
                    if rep.up_to_date {
                        format!("{} is already up to date on {}.", rep.branch, rep.remote)
                    } else {
                        format!("Pushed {} to {}.", rep.branch, rep.remote)
                    },
                );
            },
        );
    }
}
