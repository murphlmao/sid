//! Workspaces v2 sub-views: Changes (with the diff viewer and commit box), Branches,
//! Stash and Log, plus the Fetch/Push controls and the notice banner. Render-only: every
//! click routes into [`super::ops`].

use std::time::Instant;

use gpui::{
    AnyElement, ClickEvent, Context, Div, Hsla, IntoElement, KeyDownEvent, SharedString, div,
    prelude::*, px, rgb,
};
use sid_core::git::{DiffLine, DiffLineKind, FileDiff, FileStatus, RefKind, StashEntry};
use sid_ui::theme::{self, Theme};
use sid_ui::{
    Badge, BadgeTone, Button, ButtonSize, CheckState, Checkbox, ConfirmButton, EmptyState, Icon,
    IconButton, List, Row, StyledExt as _, TextInput, Toast, Typography as _, h_flex, scaled,
};

use super::model::{self, Side, commit_enabled, diff_status_label, gutter, kind_letter, short_oid};
use super::ops::LogState;
use super::{Fetch, commit_age, now_secs, text_cell};
use crate::app::AppState;

/// Diff rows painted per file. The provider already caps bytes; this caps elements so a
/// long generated file cannot stall a frame.
const MAX_DIFF_ROWS: usize = 3000;

/// `color` at `alpha` — the tinted fill behind an added/removed line, derived from the
/// theme token so it follows the theme.
fn tint(color: u32, alpha: f32) -> Hsla {
    let h: Hsla = rgb(color).into();
    h.opacity(alpha)
}

fn section_label(t: &Theme, ink: u32, text: String) -> Div {
    div().text_label(t).text_color(rgb(ink)).child(text)
}

impl AppState {
    /// Fetch / Push and the in-flight label, shown in the detail header.
    pub(super) fn remote_controls(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let busy = self.workspaces.v2.busy;
        let fetching = busy == Some("Fetching…");
        let pushing = busy == Some("Pushing…");
        h_flex()
            .gap_2()
            .child(
                Button::new("ws-fetch", "Fetch")
                    .small()
                    .icon(Icon::ArrowDown)
                    .tooltip("Download new commits from the remote")
                    .loading(fetching)
                    .disabled(busy.is_some())
                    .on_click(cx.listener(|this, _ev: &ClickEvent, _w, cx| this.fetch_remote(cx))),
            )
            .child(
                Button::new("ws-push", "Push")
                    .small()
                    .icon(Icon::ArrowUp)
                    .tooltip("Push the current branch to its upstream")
                    .loading(pushing)
                    .disabled(busy.is_some())
                    .on_click(cx.listener(|this, _ev: &ClickEvent, _w, cx| this.push_current(cx))),
            )
    }

    /// The result/progress banner above the sub-view.
    pub(super) fn v2_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if let Some(label) = self.workspaces.v2.busy {
            return Some(Toast::info(label).into_any_element());
        }
        let (tone, msg) = self.workspaces.v2.notice.clone()?;
        Some(
            Toast::new(tone, msg)
                .on_dismiss(cx.listener(|this, _ev: &ClickEvent, _w, cx| {
                    this.workspaces.v2.notice = None;
                    cx.notify();
                }))
                .into_any_element(),
        )
    }

    // ---- Changes ----------------------------------------------------------------------

    pub(super) fn repo_changes(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme_owned = theme::active(cx).clone();
        let t = &theme_owned;
        let (muted, danger, success, warning) = (t.muted, t.danger, t.success, t.warning);

        let status = match self.workspaces.status.clone() {
            None | Some(Fetch::Loading) => return text_cell(t, muted, "Loading changes…"),
            Some(Fetch::Done(Err(e))) => return text_cell(t, danger, e.to_string()),
            Some(Fetch::Done(Ok(s))) => s,
        };
        let b = model::bucket(&status.files);
        let selected = self.workspaces.v2.selected_file.clone();
        let busy = self.workspaces.v2.busy.is_some();
        let armed_key = self.workspaces.v2.discard_arm.armed_key(Instant::now());
        let staged_n = b.staged.len();

        let file_rows = |files: &[FileStatus], side: Side, cx: &mut Context<Self>| {
            let (row_id, chk_id): (&'static str, &'static str) = match side {
                Side::Staged => ("ws-chg-staged", "ws-chk-staged"),
                Side::Unstaged => ("ws-chg-unstaged", "ws-chk-unstaged"),
                Side::Untracked => ("ws-chg-untracked", "ws-chk-untracked"),
            };
            files
                .iter()
                .enumerate()
                .map(|(ix, f)| {
                    let is_sel = selected.as_ref() == Some(&(f.path.clone(), side));
                    let letter = kind_letter(if side == Side::Staged {
                        f.index
                    } else {
                        f.worktree
                    });
                    let entity = cx.entity();
                    let p = f.path.clone();
                    let check = Checkbox::new(
                        SharedString::from(format!("{chk_id}-{ix}")),
                        CheckState::from_bool(side == Side::Staged),
                    )
                    .disabled(busy)
                    .on_toggle({
                        let p = p.clone();
                        move |_state, _w, cx| {
                            let p = p.clone();
                            entity.update(cx, |this, cx| {
                                if side == Side::Staged {
                                    this.unstage(vec![p], cx);
                                } else {
                                    this.stage(vec![p], cx);
                                }
                            });
                        }
                    });
                    let key = model::key_of(&format!("{}\0{side:?}", f.path));
                    let discard = (side != Side::Staged).then(|| {
                        let p = p.clone();
                        ConfirmButton::new(
                            SharedString::from(format!("{row_id}-discard-{ix}")),
                            if side == Side::Untracked {
                                "Delete"
                            } else {
                                "Discard"
                            },
                        )
                        .armed(armed_key == Some(key))
                        .armed_label("Sure?")
                        .icon(Icon::Trash)
                        .tooltip(if side == Side::Untracked {
                            "Delete this file from disk — it can't be undone"
                        } else {
                            "Throw away your edits to this file — it can't be undone"
                        })
                        .on_press(cx.listener(
                            move |this, _ev: &ClickEvent, _w, cx| {
                                this.discard(p.clone(), side, cx);
                            },
                        ))
                    });
                    let name_ink = if is_sel { t.fg_strong } else { t.fg };
                    Row::new((row_id, ix))
                        .selected(is_sel)
                        .compact()
                        .leading(check)
                        .child(
                            div()
                                .min_w(px(0.))
                                .clamp_one_line()
                                .text_mono(t)
                                .text_color(rgb(name_ink))
                                .child(f.path.clone()),
                        )
                        .meta(Badge::new(letter).tone(match side {
                            Side::Staged => BadgeTone::Success,
                            Side::Unstaged => BadgeTone::Warning,
                            Side::Untracked => BadgeTone::Neutral,
                        }))
                        .when_some(discard, |row, d| row.action(d))
                        .on_click(cx.listener(move |this, _ev: &ClickEvent, _w, cx| {
                            this.select_file(p.clone(), side, cx);
                        }))
                        .into_any_element()
                })
                .collect::<Vec<_>>()
        };

        let header = |label: &str, n: usize, ink: u32, action: Option<AnyElement>| -> AnyElement {
            h_flex()
                .justify_between()
                .px_2()
                .pt_2()
                .child(section_label(t, ink, format!("{label} · {n}")))
                .children(action)
                .into_any_element()
        };

        let mut list = div().flex().flex_col().gap_1();
        if !b.conflicted.is_empty() {
            list = list.child(header("CONFLICTED", b.conflicted.len(), danger, None)).child(
                div().px_2().child(
                    Toast::warning(
                        "These files have merge conflicts. Resolve them in your editor, then stage them.",
                    ),
                ),
            );
            let rows = file_rows(&b.conflicted, Side::Unstaged, cx);
            list = list.child(List::stack().children(rows));
        }
        if staged_n > 0 {
            let unstage_all = Button::new("ws-unstage-all", "Unstage all")
                .small()
                .ghost()
                .disabled(busy)
                .on_click(cx.listener(|this, _ev: &ClickEvent, _w, cx| this.unstage_all(cx)))
                .into_any_element();
            list = list.child(header("STAGED", staged_n, success, Some(unstage_all)));
            list = list.child(List::stack().children(file_rows(&b.staged, Side::Staged, cx)));
        }
        let unstaged_total = b.unstaged.len() + b.untracked.len();
        if !b.unstaged.is_empty() {
            let stage_all = Button::new("ws-stage-all", "Stage all")
                .small()
                .ghost()
                .disabled(busy)
                .on_click(cx.listener(|this, _ev: &ClickEvent, _w, cx| this.stage_all(cx)))
                .into_any_element();
            list = list.child(header(
                "CHANGES",
                b.unstaged.len(),
                warning,
                Some(stage_all),
            ));
            list = list.child(List::stack().children(file_rows(&b.unstaged, Side::Unstaged, cx)));
        }
        if !b.untracked.is_empty() {
            let stage_all = (b.unstaged.is_empty()).then(|| {
                Button::new("ws-stage-all-u", "Stage all")
                    .small()
                    .ghost()
                    .disabled(busy)
                    .on_click(cx.listener(|this, _ev: &ClickEvent, _w, cx| this.stage_all(cx)))
                    .into_any_element()
            });
            list = list.child(header("UNTRACKED", b.untracked.len(), muted, stage_all));
            list = list.child(List::stack().children(file_rows(&b.untracked, Side::Untracked, cx)));
        }
        let nothing = staged_n == 0 && unstaged_total == 0 && b.conflicted.is_empty();
        let files_pane: AnyElement = if nothing {
            EmptyState::new("Working tree clean")
                .guidance("Nothing to commit. Edit a file and it shows up here.")
                .icon(Icon::Check)
                .into_any_element()
        } else {
            div()
                .id("ws-changes-files")
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .child(list)
                .into_any_element()
        };

        // ---- commit box ----
        let message = self.commit_message(cx);
        let amend = self.workspaces.v2.amend;
        let enabled = commit_enabled(staged_n, &message, amend) && !busy;
        let hint = if staged_n == 0 && !amend {
            Some("Tick files above to stage them for the commit.")
        } else if message.trim().is_empty() {
            Some("Write a message to describe the commit.")
        } else {
            None
        };
        let commit_box = self.workspaces.v2.commit_input.clone().map(|input| {
            div()
                .id("ws-commit-box")
                .flex_none()
                .flex()
                .flex_col()
                .gap_2()
                .p_2()
                .border_t_1()
                .border_color(rgb(t.border))
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _w, cx| {
                    if ev.keystroke.key == "enter"
                        && (ev.keystroke.modifiers.control || ev.keystroke.modifiers.platform)
                    {
                        cx.stop_propagation();
                        let msg = this.commit_message(cx);
                        let staged = this.staged_count();
                        if commit_enabled(staged, &msg, this.workspaces.v2.amend) {
                            this.commit(cx);
                        }
                    }
                }))
                .child(TextInput::new(&input))
                .child(
                    h_flex()
                        .justify_between()
                        .child(
                            Checkbox::new("ws-amend", CheckState::from_bool(amend))
                                .label("Amend last commit")
                                .on_toggle({
                                    let entity = cx.entity();
                                    move |state, _w, cx| {
                                        entity.update(cx, |this, cx| {
                                            this.toggle_amend(state.is_lit(), cx)
                                        })
                                    }
                                }),
                        )
                        .child(
                            Button::new("ws-commit", if amend { "Amend commit" } else { "Commit" })
                                .primary()
                                .small()
                                .disabled(!enabled)
                                .tooltip("Ctrl+Enter")
                                .on_click(
                                    cx.listener(|this, _ev: &ClickEvent, _w, cx| this.commit(cx)),
                                ),
                        ),
                )
                .children(hint.map(|h| div().text_meta(t).child(h)))
        });

        let left = div()
            .flex_none()
            .w(scaled(380.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(rgb(t.border))
            .child(files_pane)
            .children(commit_box);

        div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h(px(0.))
            .child(left)
            .child(self.diff_view(cx))
            .into_any_element()
    }

    fn staged_count(&self) -> usize {
        match &self.workspaces.status {
            Some(Fetch::Done(Ok(s))) => s.files.iter().filter(|f| f.is_staged()).count(),
            _ => 0,
        }
    }

    // ---- diff viewer ------------------------------------------------------------------

    fn diff_view(&self, _cx: &mut Context<Self>) -> AnyElement {
        let t = theme::active(_cx);
        let pane = || {
            div()
                .flex_1()
                .min_w(px(0.))
                .min_h(px(0.))
                .flex()
                .flex_col()
                .bg(rgb(t.well))
        };
        let Some((path, side)) = self.workspaces.v2.selected_file.clone() else {
            return pane()
                .child(
                    EmptyState::new("Select a file")
                        .guidance("Pick a changed file on the left to see exactly what changed.")
                        .icon(Icon::File),
                )
                .into_any_element();
        };
        let diff = match &self.workspaces.v2.diff {
            None | Some(Fetch::Loading) => {
                return pane()
                    .p_3()
                    .child(text_cell(t, t.muted, "Loading diff…"))
                    .into_any_element();
            }
            Some(Fetch::Done(Err(e))) => {
                return pane()
                    .p_3()
                    .child(text_cell(t, t.danger, e.to_string()))
                    .into_any_element();
            }
            Some(Fetch::Done(Ok(d))) => d,
        };
        let Some(file) = diff.files.iter().find(|f| f.display_path() == path) else {
            return pane()
                .child(
                    EmptyState::new("No textual changes")
                        .guidance(
                            "This file is empty or only its permissions changed, so there's \
                             nothing to show line by line.",
                        )
                        .icon(Icon::File),
                )
                .into_any_element();
        };

        let head = h_flex()
            .flex_none()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(t.border))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .clamp_one_line()
                    .text_mono(t)
                    .text_color(rgb(t.fg_strong))
                    .child(path.clone()),
            )
            .child(Badge::new(diff_status_label(file.status)))
            .child(
                div()
                    .flex_none()
                    .text_mono_meta(t)
                    .text_color(rgb(t.success))
                    .child(format!("+{}", file.additions)),
            )
            .child(
                div()
                    .flex_none()
                    .text_mono_meta(t)
                    .text_color(rgb(t.danger))
                    .child(format!("-{}", file.deletions)),
            )
            .child(Badge::new(match side {
                Side::Staged => "staged",
                Side::Unstaged => "unstaged",
                Side::Untracked => "untracked",
            }));

        let body: AnyElement = if file.binary {
            EmptyState::new("Binary file")
                .guidance("This file isn't text, so there's no line diff to show.")
                .icon(Icon::File)
                .into_any_element()
        } else {
            let (rows, cut) = diff_rows(t, file);
            let notice = (file.truncated || cut).then(|| {
                div().px_3().py_2().child(Toast::warning(if cut {
                    format!(
                        "This diff is long — showing the first {MAX_DIFF_ROWS} lines. Open the \
                         file in your editor for the rest."
                    )
                } else {
                    "This diff was cut short because the file is large. Open it in your editor \
                     for the full change."
                        .to_string()
                }))
            });
            div()
                .flex_1()
                .min_h(px(0.))
                .flex()
                .flex_col()
                .children(notice)
                .child(
                    div()
                        .id("ws-diff-scroll")
                        .flex_1()
                        .min_h(px(0.))
                        .overflow_scroll()
                        .child(div().flex().flex_col().min_w_full().children(rows)),
                )
                .into_any_element()
        };
        pane().child(head).child(body).into_any_element()
    }

    // ---- Stash ------------------------------------------------------------------------

    pub(super) fn repo_stash(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::active(cx);
        let busy = self.workspaces.v2.busy.is_some();
        let untracked = self.workspaces.v2.stash_untracked;
        let armed = self.workspaces.v2.drop_arm.armed_key(Instant::now());

        let form = self.workspaces.v2.stash_input.clone().map(|input| {
            div()
                .id("ws-stash-form")
                .flex()
                .flex_col()
                .gap_2()
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _w, cx| {
                    if ev.keystroke.key == "enter" {
                        cx.stop_propagation();
                        this.stash_push(cx);
                    }
                }))
                .child(TextInput::new(&input))
                .child(
                    h_flex()
                        .justify_between()
                        .child(
                            Checkbox::new("ws-stash-untracked", CheckState::from_bool(untracked))
                                .label("Include untracked files")
                                .on_toggle({
                                    let entity = cx.entity();
                                    move |state, _w, cx| {
                                        entity.update(cx, |this, cx| {
                                            this.workspaces.v2.stash_untracked = state.is_lit();
                                            cx.notify();
                                        })
                                    }
                                }),
                        )
                        .child(
                            Button::new("ws-stash-push", "Stash changes")
                                .primary()
                                .small()
                                .disabled(busy)
                                .on_click(cx.listener(|this, _ev: &ClickEvent, _w, cx| {
                                    this.stash_push(cx)
                                })),
                        ),
                )
        });

        let list: AnyElement = match self.workspaces.v2.stash.clone() {
            None | Some(Fetch::Loading) => text_cell(t, t.muted, "Loading stashes…"),
            Some(Fetch::Done(Err(e))) => text_cell(t, t.danger, e.to_string()),
            Some(Fetch::Done(Ok(entries))) if entries.is_empty() => EmptyState::new("No stashes")
                .guidance(
                    "Stash parks your uncommitted work so you can switch tasks, then bring it back.",
                )
                .icon(Icon::Folder)
                .into_any_element(),
            Some(Fetch::Done(Ok(entries))) => {
                let rows: Vec<AnyElement> = entries
                    .iter()
                    .map(|e| self.stash_row(e, armed == Some(e.index), busy, cx))
                    .collect();
                List::stack().children(rows).into_any_element()
            }
        };

        div()
            .flex()
            .flex_col()
            .gap_3()
            .children(form)
            .child(list)
            .into_any_element()
    }

    fn stash_row(
        &self,
        e: &StashEntry,
        armed: bool,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme::active(cx);
        let i = e.index;
        Row::new(("ws-stash", i))
            .compact()
            .leading(Badge::new(format!("stash@{{{i}}}")))
            .child(
                div()
                    .min_w(px(0.))
                    .clamp_one_line()
                    .text_body(t)
                    .text_color(rgb(t.fg))
                    .child(e.message.clone()),
            )
            .when_some(e.branch.clone(), |r, b| r.meta(Badge::new(b)))
            .action(
                Button::new(("ws-stash-apply", i), "Apply")
                    .small()
                    .ghost()
                    .tooltip("Re-apply it and keep the stash")
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, _w, cx| {
                        this.stash_apply(i, false, cx)
                    })),
            )
            .action(
                Button::new(("ws-stash-pop", i), "Pop")
                    .small()
                    .ghost()
                    .tooltip("Re-apply it and remove it from the list")
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, _w, cx| {
                        this.stash_apply(i, true, cx)
                    })),
            )
            .action(
                ConfirmButton::new(("ws-stash-drop", i), "Drop")
                    .armed(armed)
                    .armed_label("Sure?")
                    .icon(Icon::Trash)
                    .tooltip("Delete this stash for good")
                    .on_press(
                        cx.listener(move |this, _ev: &ClickEvent, _w, cx| this.stash_drop(i, cx)),
                    ),
            )
            .into_any_element()
    }

    // ---- Branches ---------------------------------------------------------------------

    pub(super) fn repo_branches(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::active(cx);
        let busy = self.workspaces.v2.busy.is_some();

        let error_line =
            self.workspaces.checkout_error.clone().map(|e| {
                Toast::danger(format!("Couldn't switch branches: {e}")).into_any_element()
            });

        let create = self.workspaces.v2.branch_input.clone().map(|input| {
            div()
                .id("ws-branch-create")
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _w, cx| {
                    if ev.keystroke.key == "enter" {
                        cx.stop_propagation();
                        this.create_branch(cx);
                    }
                }))
                .child(div().flex_1().min_w(px(0.)).child(TextInput::new(&input)))
                .child(
                    Button::new("ws-branch-new", "Create branch")
                        .primary()
                        .small()
                        .icon(Icon::Add)
                        .tooltip("Create it at the current commit")
                        .disabled(busy)
                        .on_click(
                            cx.listener(|this, _ev: &ClickEvent, _w, cx| this.create_branch(cx)),
                        ),
                )
        });

        let force = self.workspaces.v2.force_delete.clone().map(|name| {
            let n = name.clone();
            Toast::warning(format!(
                "'{name}' has commits that aren't merged anywhere else. Deleting it loses them."
            ))
            .action(
                Button::new("ws-force-delete", "Delete anyway")
                    .danger()
                    .small()
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, _w, cx| {
                        this.delete_branch(n.clone(), true, cx)
                    })),
            )
            .action(
                Button::new("ws-force-cancel", "Keep it")
                    .small()
                    .on_click(cx.listener(|this, _ev: &ClickEvent, _w, cx| {
                        this.workspaces.v2.force_delete = None;
                        cx.notify();
                    })),
            )
            .into_any_element()
        });

        let body: AnyElement = match self.workspaces.branches.clone() {
            None | Some(Fetch::Loading) => text_cell(t, t.muted, "Loading branches…"),
            Some(Fetch::Done(Err(e))) => text_cell(t, t.danger, e.to_string()),
            Some(Fetch::Done(Ok(branches))) => {
                let pending = self.workspaces.checkout_pending.clone();
                let armed = self.workspaces.v2.delete_arm.armed_key(Instant::now());
                let renaming = self.workspaces.v2.rename_from.clone();
                let rows: Vec<AnyElement> = branches
                    .into_iter()
                    .enumerate()
                    .map(|(ix, b)| {
                        let name = b.name.clone();
                        let is_current = b.is_current;
                        let is_pending = pending.as_deref() == Some(name.as_str());
                        if renaming.as_deref() == Some(name.as_str())
                            && let Some(input) = self.workspaces.v2.rename_input.clone()
                        {
                            return Row::new(("ws-branch", ix))
                                .compact()
                                .child(
                                    div()
                                        .id(("ws-branch-rename", ix))
                                        .flex_1()
                                        .on_key_down(cx.listener(
                                            |this, ev: &KeyDownEvent, _w, cx| {
                                                match ev.keystroke.key.as_str() {
                                                    "enter" => {
                                                        cx.stop_propagation();
                                                        this.commit_rename_branch(cx);
                                                    }
                                                    "escape" => {
                                                        cx.stop_propagation();
                                                        this.cancel_rename_branch(cx);
                                                    }
                                                    _ => {}
                                                }
                                            },
                                        ))
                                        .child(TextInput::new(&input)),
                                )
                                .action(
                                    Button::new(("ws-branch-rename-ok", ix), "Rename")
                                        .primary()
                                        .small()
                                        .on_click(cx.listener(|this, _ev: &ClickEvent, _w, cx| {
                                            this.commit_rename_branch(cx)
                                        })),
                                )
                                .action(
                                    Button::new(("ws-branch-rename-no", ix), "Cancel")
                                        .small()
                                        .on_click(cx.listener(|this, _ev: &ClickEvent, _w, cx| {
                                            this.cancel_rename_branch(cx)
                                        })),
                                )
                                .into_any_element();
                        }
                        let rename_name = name.clone();
                        let delete_name = name.clone();
                        let checkout_name = name.clone();
                        let key = model::key_of(&name);
                        Row::new(("ws-branch", ix))
                            .compact()
                            .selected(is_current)
                            .child(
                                div()
                                    .min_w(px(0.))
                                    .clamp_one_line()
                                    .text_mono(t)
                                    .text_color(rgb(if is_current { t.fg_strong } else { t.fg }))
                                    .child(name),
                            )
                            .when(is_current, |row| row.meta(Badge::new("current").solid()))
                            .when_some(b.upstream.clone(), |row, u| {
                                row.meta(div().text_mono_meta(t).child(u))
                            })
                            .when(is_pending, |row| {
                                row.meta(div().text_meta(t).child("switching…"))
                            })
                            .action(
                                IconButton::new(
                                    ("ws-branch-rename-btn", ix),
                                    Icon::Rename,
                                    "Rename branch",
                                )
                                .small()
                                .disabled(busy)
                                .on_click(cx.listener(
                                    move |this, _ev: &ClickEvent, window, cx| {
                                        this.start_rename_branch(rename_name.clone(), window, cx)
                                    },
                                )),
                            )
                            .when(!is_current, |row| {
                                row.action(
                                    ConfirmButton::new(("ws-branch-del", ix), "Delete")
                                        .armed(armed == Some(key))
                                        .armed_label("Sure?")
                                        .icon(Icon::Trash)
                                        .tooltip("Delete this branch")
                                        .on_press(cx.listener(
                                            move |this, _ev: &ClickEvent, _w, cx| {
                                                this.delete_branch(delete_name.clone(), false, cx)
                                            },
                                        )),
                                )
                                .on_click(cx.listener(
                                    move |this, _ev: &ClickEvent, _w, cx| {
                                        this.checkout_branch(checkout_name.clone(), cx)
                                    },
                                ))
                            })
                            .into_any_element()
                    })
                    .collect();
                List::stack().children(rows).into_any_element()
            }
        };

        div()
            .flex()
            .flex_col()
            .gap_3()
            .children(error_line)
            .children(force)
            .children(create)
            .child(body)
            .into_any_element()
    }

    // ---- Log --------------------------------------------------------------------------

    pub(super) fn repo_log(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::active(cx);
        match self.workspaces.v2.log.clone() {
            None | Some(Fetch::Loading) => text_cell(t, t.muted, "Loading history…"),
            Some(Fetch::Done(Err(e))) => text_cell(t, t.danger, e.to_string()),
            Some(Fetch::Done(Ok(LogState { entries, .. }))) if entries.is_empty() => {
                EmptyState::new("No commits yet")
                    .guidance("Make the first commit from the Changes tab.")
                    .icon(Icon::File)
                    .into_any_element()
            }
            Some(Fetch::Done(Ok(LogState { entries, has_more }))) => {
                let now = now_secs();
                let rows: Vec<AnyElement> = entries
                    .iter()
                    .enumerate()
                    .map(|(ix, e)| {
                        let badges: Vec<AnyElement> = e
                            .refs
                            .iter()
                            .map(|r| match r.kind {
                                RefKind::Head => {
                                    Badge::new(r.name.clone()).tone(BadgeTone::Accent).solid()
                                }
                                RefKind::Branch => {
                                    Badge::new(r.name.clone()).tone(BadgeTone::Success)
                                }
                                RefKind::RemoteBranch => Badge::new(r.name.clone()).outline(),
                                RefKind::Tag => Badge::new(r.name.clone()).tone(BadgeTone::Warning),
                            })
                            .map(IntoElement::into_any_element)
                            .collect();
                        Row::new(("ws-log", ix))
                            .compact()
                            .leading(
                                div()
                                    .text_mono_meta(t)
                                    .text_color(rgb(t.muted))
                                    .child(short_oid(&e.commit.oid).to_string()),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .min_w(px(0.))
                                    .child(
                                        div()
                                            .min_w(px(0.))
                                            .clamp_one_line()
                                            .text_body(t)
                                            .text_color(rgb(t.fg))
                                            .child(e.commit.summary.clone()),
                                    )
                                    .children(
                                        badges.into_iter().map(|b| div().flex_none().child(b)),
                                    ),
                            )
                            .meta(div().text_meta(t).child(e.commit.author_name.clone()))
                            .meta(
                                div()
                                    .text_meta(t)
                                    .child(commit_age(now, e.commit.timestamp_secs)),
                            )
                            .into_any_element()
                    })
                    .collect();
                let more =
                    has_more.then(|| {
                        let loading = self.workspaces.v2.log_more_loading;
                        div().flex().justify_center().py_2().child(
                            Button::new("ws-log-more", "Load more")
                                .small()
                                .size(ButtonSize::Sm)
                                .loading(loading)
                                .disabled(loading)
                                .on_click(cx.listener(|this, _ev: &ClickEvent, _w, cx| {
                                    this.load_more_log(cx)
                                })),
                        )
                    });
                div()
                    .flex()
                    .flex_col()
                    .child(List::stack().children(rows))
                    .children(more)
                    .into_any_element()
            }
        }
    }
}

/// Flatten a file's hunks into painted rows, capped at [`MAX_DIFF_ROWS`]. The bool is
/// whether the cap cut anything.
fn diff_rows(t: &Theme, file: &FileDiff) -> (Vec<AnyElement>, bool) {
    let mut out: Vec<AnyElement> = Vec::new();
    for h in &file.hunks {
        if out.len() >= MAX_DIFF_ROWS {
            return (out, true);
        }
        out.push(
            div()
                .w_full()
                .flex_none()
                .px_3()
                .py_1()
                .bg(rgb(t.surface))
                .text_mono_meta(t)
                .text_color(rgb(t.muted))
                .whitespace_nowrap()
                .child(h.header.trim_end().to_string())
                .into_any_element(),
        );
        for l in &h.lines {
            if out.len() >= MAX_DIFF_ROWS {
                return (out, true);
            }
            out.push(diff_line(t, l));
        }
    }
    (out, false)
}

fn diff_line(t: &Theme, l: &DiffLine) -> AnyElement {
    let (sign, ink, bg) = match l.kind {
        DiffLineKind::Added => ("+", t.success, Some(tint(t.success, 0.14))),
        DiffLineKind::Removed => ("-", t.danger, Some(tint(t.danger, 0.14))),
        DiffLineKind::Context => (" ", t.fg, None),
    };
    let (old, new) = gutter(l);
    let num = |s: String| {
        div()
            .flex_none()
            .w(scaled(48.))
            .pr_2()
            .text_right()
            .text_mono_meta(t)
            .text_color(rgb(t.faint))
            .child(s)
    };
    h_flex()
        .w_full()
        .flex_none()
        .items_start()
        .when_some(bg, |d, c| d.bg(c))
        .child(num(old))
        .child(num(new))
        .child(
            div()
                .flex_none()
                .w(scaled(16.))
                .text_mono(t)
                .text_color(rgb(ink))
                .child(sign),
        )
        .child(
            div()
                .flex_none()
                .pr_3()
                .text_mono(t)
                .text_color(rgb(if matches!(l.kind, DiffLineKind::Context) {
                    t.fg
                } else {
                    ink
                }))
                .whitespace_nowrap()
                .child(l.text.replace('\t', "    ")),
        )
        .into_any_element()
}
