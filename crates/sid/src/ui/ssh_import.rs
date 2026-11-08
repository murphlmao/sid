//! Import SSH connections from `~/.ssh/config` — a dialog reachable from the SSH home
//! header and its empty state (and callable by the chrome for a first-run banner via
//! [`AppState::open_ssh_import`]).
//!
//! The parser lives in `sid_ssh::ssh_config`; this module is the review step in front of
//! it: every candidate as a checkbox row (pre-checked unless it duplicates something
//! saved), duplicates greyed with *why*, the parser's notes (skipped `Match` blocks...)
//! as meta text, a `save to` chooser and one primary action. Writes go through the same
//! attributive add-guard the host form uses: an import never clobbers a record.
//!
//! Pure/unit-tested: [`ImportSelection`] (the checkbox/dedupe state), [`import_label`]
//! and [`apply_import`] (the guarded write loop, store-agnostic). Rendering is
//! observation-gated.

use gpui::{
    App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Render,
    SharedString, Subscription, Window, actions, div, prelude::*, px, rgb,
};
use sid_ssh::ssh_config::{
    Duplicate, ImportCandidate, ImportEnv, ParsedConfig, find_duplicate, parse_file,
};
use sid_store::{DefaultScope, Host, Scope};

use crate::app::AppState;
use crate::ui::host_form::{SaveTarget, add_guard, preselect};
use sid_ui::theme;
use sid_ui::{
    Button, CheckState, Checkbox, EmptyState, Icon, List, Modal, Radio, Row, StyledExt as _, Toast,
    Typography as _, h_flex, scaled, v_flex,
};

actions!(
    ssh_import,
    [
        /// Close the dialog (bound to `escape`).
        ImportCancel,
        /// Import the checked connections (bound to `ctrl-enter`; plain Enter toggles the
        /// focused checkbox).
        ImportSubmit,
    ]
);

/// The dialog's key context.
pub(crate) const IMPORT_CONTEXT: &str = "SshImport";

// ---- pure state (unit-tested) --------------------------------------------------------

/// One candidate row.
#[derive(Clone, Debug)]
pub(crate) struct ImportRow {
    pub candidate: ImportCandidate,
    /// Why this row can't be imported, if it can't.
    pub duplicate: Option<Duplicate>,
    pub checked: bool,
}

impl ImportRow {
    /// "already saved as {alias}" for a duplicate, `None` otherwise.
    pub(crate) fn duplicate_note(&self) -> Option<String> {
        self.duplicate.as_ref().map(|d| match d {
            Duplicate::SameAlias(a) | Duplicate::SameEndpoint(a) => {
                format!("already saved as {a}")
            }
        })
    }
}

/// The checkbox state of the whole dialog.
#[derive(Clone, Debug, Default)]
pub(crate) struct ImportSelection {
    pub rows: Vec<ImportRow>,
    pub notes: Vec<String>,
    local_user: String,
}

impl ImportSelection {
    /// Build from a parse. A candidate that duplicates something saved is unchecked and
    /// locked; so is a second candidate that duplicates an *earlier one in this same
    /// import* (alias or endpoint), so one click can never write two colliding records.
    pub(crate) fn from_parsed(parsed: ParsedConfig, local_user: &str, existing: &[Host]) -> Self {
        let mut pool: Vec<Host> = existing.to_vec();
        let mut rows = Vec::new();
        for candidate in parsed.candidates {
            let duplicate = find_duplicate(&candidate, local_user, &pool);
            if duplicate.is_none() {
                pool.push(candidate.to_host(local_user));
            }
            rows.push(ImportRow {
                checked: duplicate.is_none(),
                duplicate,
                candidate,
            });
        }
        Self {
            rows,
            notes: parsed.notes,
            local_user: local_user.to_string(),
        }
    }

    /// Flip row `ix`. Duplicates cannot be checked. Returns whether anything changed.
    pub(crate) fn toggle(&mut self, ix: usize) -> bool {
        match self.rows.get_mut(ix) {
            Some(row) if row.duplicate.is_none() => {
                row.checked = !row.checked;
                true
            }
            _ => false,
        }
    }

    /// Check (or uncheck) every row that can be.
    pub(crate) fn set_all(&mut self, checked: bool) {
        for row in self.rows.iter_mut().filter(|r| r.duplicate.is_none()) {
            row.checked = checked;
        }
    }

    /// How many rows are checked.
    pub(crate) fn checked_count(&self) -> usize {
        self.rows.iter().filter(|r| r.checked).count()
    }

    /// How many rows could be checked at all.
    pub(crate) fn selectable_count(&self) -> usize {
        self.rows.iter().filter(|r| r.duplicate.is_none()).count()
    }

    /// The store records for the checked rows.
    pub(crate) fn selected_hosts(&self) -> Vec<Host> {
        self.rows
            .iter()
            .filter(|r| r.checked)
            .map(|r| r.candidate.to_host(&self.local_user))
            .collect()
    }
}

/// The primary action's label.
pub(crate) fn import_label(n: usize) -> String {
    match n {
        0 => "Import".into(),
        1 => "Import 1 connection".into(),
        n => format!("Import {n} connections"),
    }
}

/// What a guarded import did.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ImportOutcome {
    pub imported: usize,
    /// Aliases the target layer already held (refused by the add guard, never clobbered).
    pub skipped: Vec<String>,
}

/// Write `hosts` one by one through the attributive add-guard. `holds` asks whether the
/// target layer already has an alias; `write` persists one record. A store error stops
/// the loop and is returned with what was done so far irrelevant — nothing is rolled
/// back because each record is independent and already durable.
pub(crate) fn apply_import(
    hosts: &[Host],
    target_label: &str,
    holds: impl Fn(&str) -> Result<bool, String>,
    mut write: impl FnMut(&Host) -> Result<(), String>,
) -> Result<ImportOutcome, String> {
    let mut out = ImportOutcome::default();
    for host in hosts {
        let held = holds(&host.alias)?;
        if add_guard(false, held, target_label).is_err() {
            out.skipped.push(host.alias.clone());
            continue;
        }
        write(host)?;
        out.imported += 1;
    }
    Ok(out)
}

// ---- the dialog ----------------------------------------------------------------------

/// What the dialog tells its owner.
pub(crate) enum SshImportEvent {
    Cancel,
    Submit { hosts: Vec<Host>, target: Scope },
}

/// Where the dialog is in its life.
enum Phase {
    /// The config could not be read (missing file...). The message says what to try.
    Unreadable(SharedString),
    Review,
    /// The config is being read and parsed on a background task.
    Loading,
}

pub(crate) struct SshImportModal {
    phase: Phase,
    selection: ImportSelection,
    path: SharedString,
    save_to: Option<SaveTarget>,
    workspace: Option<(Scope, SharedString)>,
    error: Option<SharedString>,
    focus_handle: FocusHandle,
}

impl SshImportModal {
    pub(crate) fn new(
        cx: &mut Context<Self>,
        existing: &[Host],
        workspace: Option<(Scope, SharedString)>,
        default_scope: DefaultScope,
    ) -> Self {
        let env = ImportEnv::from_process();
        let path = env.default_config_path();
        let home = env.home.display().to_string();
        let tilde = move |text: &str| match home.is_empty() {
            true => text.to_string(),
            false => text.replace(&home, "~"),
        };
        let shown_path: SharedString = tilde(&path.display().to_string()).into();
        // Reading and parsing (Includes, globs) touches the disk: do it off the UI thread.
        let existing = existing.to_vec();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    match parse_file(&path, &env) {
                        Ok(parsed) => {
                            let mut sel =
                                ImportSelection::from_parsed(parsed, &env.local_user, &existing);
                            sel.notes = sel.notes.iter().map(|n| tilde(n)).collect();
                            (Phase::Review, sel)
                        }
                        Err(e) => (
                            Phase::Unreadable(
                                format!(
                                    "Couldn't read {}: {e}. Check that the file exists and that sid can read it.",
                                    path.display()
                                )
                                .into(),
                            ),
                            ImportSelection::default(),
                        ),
                    }
                })
                .await;
            let _ = this.update(cx, |m, cx| {
                (m.phase, m.selection) = result;
                cx.notify();
            });
        })
        .detach();
        Self {
            phase: Phase::Loading,
            selection: ImportSelection::default(),
            path: shown_path,
            save_to: preselect(default_scope, workspace.is_some()),
            workspace,
            error: None,
            focus_handle: cx.focus_handle(),
        }
    }

    pub(crate) fn focus_first(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus_handle, cx);
    }

    pub(crate) fn set_error(&mut self, msg: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.error = Some(msg.into());
        cx.notify();
    }

    fn target(&self) -> Option<Scope> {
        match self.save_to? {
            SaveTarget::Global => Some(Scope::Global),
            SaveTarget::Workspace => self.workspace.as_ref().map(|(s, _)| s.clone()),
        }
    }

    fn can_submit(&self) -> bool {
        matches!(self.phase, Phase::Review)
            && self.selection.checked_count() > 0
            && self.target().is_some()
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.target() else {
            self.set_error("Choose where to save these connections first.", cx);
            return;
        };
        if self.selection.checked_count() == 0 {
            self.set_error("Check at least one connection to import.", cx);
            return;
        }
        cx.emit(SshImportEvent::Submit {
            hosts: self.selection.selected_hosts(),
            target,
        });
    }

    fn scope_option(
        &self,
        id: &'static str,
        title: &'static str,
        note: String,
        target: SaveTarget,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme::active(cx).clone();
        let selected = self.save_to == Some(target);
        Row::new(id)
            .compact()
            .selected(selected)
            .leading(Radio::new(selected).enabled(enabled))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_body(&t)
                            .when(!enabled, |el| el.text_color(rgb(t.muted)))
                            .child(title),
                    )
                    .child(div().text_meta(&t).child(note)),
            )
            .when(enabled, |row| {
                row.on_click(cx.listener(move |this, _ev: &ClickEvent, _w, cx| {
                    this.save_to = Some(target);
                    cx.notify();
                }))
            })
    }

    fn candidate_row(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme::active(cx).clone();
        let row = &self.selection.rows[ix];
        let c = &row.candidate;
        let dup = row.duplicate.is_some();
        let user = c.user.clone().unwrap_or_else(|| "(local user)".into());
        let endpoint: SharedString = format!("{user}@{}:{}", c.hostname, c.port).into();
        let state = CheckState::from_bool(row.checked);
        h_flex()
            .w_full()
            .gap_3()
            .py_1p5()
            .px_2()
            .child(
                Checkbox::new(format!("ssh-import-row-{ix}"), state)
                    .disabled(dup)
                    .on_toggle({
                        let this = cx.entity();
                        move |_state, _window, cx| {
                            this.update(cx, |m, cx| {
                                if m.selection.toggle(ix) {
                                    cx.notify();
                                }
                            });
                        }
                    }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .child(
                        div()
                            .text_body(&t)
                            .clamp_one_line()
                            .when(dup, |el| el.text_color(rgb(t.muted)))
                            .child(SharedString::from(c.alias.clone())),
                    )
                    .child(div().text_mono_meta(&t).clamp_one_line().child(endpoint)),
            )
            .child(
                div()
                    .flex_none()
                    .max_w(scaled(220.))
                    .text_meta(&t)
                    .clamp_one_line()
                    .child(SharedString::from(
                        row.duplicate_note()
                            .or_else(|| {
                                c.identity_file.as_ref().map(|k| {
                                    format!("Key {}", k.rsplit(['/', '\\']).next().unwrap_or(k))
                                })
                            })
                            .unwrap_or_else(|| "Agent".into()),
                    )),
            )
    }
}

impl EventEmitter<SshImportEvent> for SshImportModal {}

impl Focusable for SshImportModal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SshImportModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::active(cx).clone();
        let n = self.selection.checked_count();
        let ws_active = self.workspace.is_some();

        let body = match &self.phase {
            Phase::Loading => div()
                .child(
                    EmptyState::new("Reading SSH config")
                        .icon(Icon::File)
                        .compact(),
                )
                .into_any_element(),
            Phase::Unreadable(msg) => div()
                .child(
                    EmptyState::new("No SSH config to import")
                        .icon(Icon::File)
                        .guidance(msg.clone())
                        .compact(),
                )
                .into_any_element(),
            Phase::Review if self.selection.rows.is_empty() => div()
                .child(
                    EmptyState::new("Nothing to import")
                        .icon(Icon::File)
                        .guidance(format!(
                            "{} has no concrete Host entries (wildcard patterns are skipped).",
                            self.path
                        ))
                        .compact(),
                )
                .into_any_element(),
            Phase::Review => {
                let all = self.selection.checked_count() == self.selection.selectable_count();
                let rows: Vec<_> = (0..self.selection.rows.len())
                    .map(|ix| self.candidate_row(ix, cx).into_any_element())
                    .collect();
                v_flex()
                    .gap_3()
                    .child(
                        h_flex()
                            .justify_between()
                            .child(
                                div()
                                    .text_meta(&t)
                                    .clamp_one_line()
                                    .child(self.path.clone()),
                            )
                            .child(
                                Button::new(
                                    "ssh-import-all",
                                    if all { "Select none" } else { "Select all" },
                                )
                                .small()
                                .ghost()
                                .on_click(cx.listener(
                                    move |this, _ev: &ClickEvent, _w, cx| {
                                        this.selection.set_all(!all);
                                        cx.notify();
                                    },
                                )),
                            ),
                    )
                    .child(
                        List::scrolling("ssh-import-list")
                            .max_h(scaled(300.))
                            .children(rows),
                    )
                    .into_any_element()
            }
        };

        let notes = self.selection.notes.iter().map(|note| {
            div()
                .text_meta(&t)
                .child(SharedString::from(note.clone()))
                .into_any_element()
        });

        let can = self.can_submit();
        let submit_label = import_label(n);
        let mut submit = Button::new("ssh-import-go", submit_label)
            .primary()
            .icon(Icon::Add)
            .disabled(!can);
        if can {
            submit = submit.on_click(cx.listener(|this, _ev: &ClickEvent, _w, cx| this.submit(cx)));
        }

        div()
            .key_context(IMPORT_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_this, _: &ImportCancel, _w, cx| {
                cx.emit(SshImportEvent::Cancel);
            }))
            .on_action(cx.listener(|this, _: &ImportSubmit, _w, cx| {
                if this.can_submit() {
                    this.submit(cx);
                }
            }))
            .child(
                Modal::new("ssh-import", "Import from SSH config")
                    .on_dismiss(cx.listener(|_this, _ev: &ClickEvent, _w, cx| {
                        cx.emit(SshImportEvent::Cancel);
                    }))
                    .child(body)
                    .children(notes)
                    .when(matches!(self.phase, Phase::Review), |m| {
                        m.child(
                            v_flex()
                                .gap_1()
                                .child(div().text_label(&t).child("Save to"))
                                .child(self.scope_option(
                                    "ssh-import-ws",
                                    "Workspace",
                                    match &self.workspace {
                                        Some((_, name)) => format!("{name} - travels with git"),
                                        None => "no workspace focused".into(),
                                    },
                                    SaveTarget::Workspace,
                                    ws_active,
                                    cx,
                                ))
                                .child(self.scope_option(
                                    "ssh-import-global",
                                    "Global",
                                    "everywhere, never lost".into(),
                                    SaveTarget::Global,
                                    true,
                                    cx,
                                )),
                        )
                    })
                    .when(
                        matches!(self.phase, Phase::Review)
                            && self.save_to.is_none()
                            && n > 0
                            && self.error.is_none(),
                        |m| {
                            m.child(
                                div()
                                    .text_meta(&t)
                                    .child("Choose where to save these connections to continue."),
                            )
                        },
                    )
                    .when_some(self.error.clone(), |m, e| m.child(Toast::danger(e)))
                    .footer(Button::new("ssh-import-cancel", "Cancel").ghost().on_click(
                        cx.listener(|_this, _ev: &ClickEvent, _w, cx| {
                            cx.emit(SshImportEvent::Cancel);
                        }),
                    ))
                    .footer(submit),
            )
    }
}

// ---- AppState glue --------------------------------------------------------------------

/// The import dialog's slot on `HomeTabState`.
#[derive(Default)]
pub(crate) struct ImportSlot {
    pub modal: Option<Entity<SshImportModal>>,
    pub subscription: Option<Subscription>,
}

impl AppState {
    /// Open the "Import from SSH config" dialog. Callable from anywhere that has the
    /// `AppState` (the SSH home header, its empty state, the chrome's first-run banner).
    pub(crate) fn open_ssh_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let existing: Vec<Host> = self.hosts.iter().map(|a| a.item.clone()).collect();
        let workspace = self.active_workspace();
        let default_scope = self
            .store
            .settings()
            .map(|s| s.default_scope)
            .unwrap_or_default();
        let modal = cx.new(|cx| SshImportModal::new(cx, &existing, workspace, default_scope));
        modal.update(cx, |m, cx| m.focus_first(window, cx));
        self.ssh_home.import.subscription =
            Some(cx.subscribe_in(&modal, window, Self::on_ssh_import_event));
        self.ssh_home.import.modal = Some(modal);
        cx.notify();
    }

    fn close_ssh_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ssh_home.import = ImportSlot::default();
        window.focus(&self.root_focus, cx);
        cx.notify();
    }

    fn on_ssh_import_event(
        &mut self,
        modal: &Entity<SshImportModal>,
        event: &SshImportEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SshImportEvent::Submit { hosts, target } = event else {
            self.close_ssh_import(window, cx);
            return;
        };
        let label = self.layer_label(target);
        let result = apply_import(
            hosts,
            &label,
            |alias| {
                self.layer_holds_alias(target, alias)
                    .map_err(|e| e.to_string())
            },
            |host| {
                self.store
                    .write_host(host, target)
                    .map_err(|e| e.to_string())
            },
        );
        match result {
            Ok(outcome) => {
                self.close_ssh_import(window, cx);
                self.refresh();
                if !outcome.skipped.is_empty() {
                    self.error = Some(format!(
                        "Imported {}; skipped {} already in {label}: {}",
                        outcome.imported,
                        outcome.skipped.len(),
                        outcome.skipped.join(", ")
                    ));
                }
                cx.notify();
            }
            Err(msg) => {
                self.refresh();
                modal.update(cx, |m, cx| {
                    m.set_error(format!("Couldn't save the connections: {msg}"), cx)
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sid_ssh::ssh_config::parse_str;
    use sid_store::AuthMethod;
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    fn env() -> ImportEnv {
        ImportEnv {
            home: PathBuf::from("/home/me"),
            local_user: "me".into(),
        }
    }

    fn parsed(text: &str) -> ParsedConfig {
        parse_str(text, "config", Path::new("/nonexistent"), &env())
    }

    fn saved(alias: &str, user: &str, host: &str, port: u16) -> Host {
        Host {
            alias: alias.into(),
            user: user.into(),
            host: host.into(),
            port,
            secret_ref: None,
            auth: AuthMethod::default(),
            folder: None,
        }
    }

    const CFG: &str = "Host web\n  HostName web.example.com\n  User deploy\nHost db\n  HostName db.example.com\n  Port 2222\n";

    #[test]
    fn fresh_candidates_are_prechecked() {
        let s = ImportSelection::from_parsed(parsed(CFG), "me", &[]);
        assert_eq!(s.rows.len(), 2);
        assert_eq!(s.checked_count(), 2);
        assert_eq!(s.selectable_count(), 2);
    }

    #[test]
    fn same_alias_and_same_endpoint_are_locked_and_unchecked() {
        let existing = vec![
            saved("web", "x", "elsewhere", 22),
            saved("other", "me", "db.example.com", 2222),
        ];
        let s = ImportSelection::from_parsed(parsed(CFG), "me", &existing);
        assert_eq!(s.checked_count(), 0);
        assert_eq!(s.rows[0].duplicate_note().unwrap(), "already saved as web");
        assert_eq!(
            s.rows[1].duplicate_note().unwrap(),
            "already saved as other"
        );
    }

    #[test]
    fn toggle_flips_a_row_but_never_a_duplicate() {
        let existing = vec![saved("web", "deploy", "web.example.com", 22)];
        let mut s = ImportSelection::from_parsed(parsed(CFG), "me", &existing);
        assert!(!s.toggle(0), "a duplicate cannot be checked");
        assert!(s.toggle(1));
        assert_eq!(s.checked_count(), 0);
        assert!(s.toggle(1));
        assert_eq!(s.checked_count(), 1);
        assert!(!s.toggle(99));
    }

    #[test]
    fn select_all_and_none_skip_duplicates() {
        let existing = vec![saved("web", "deploy", "web.example.com", 22)];
        let mut s = ImportSelection::from_parsed(parsed(CFG), "me", &existing);
        s.set_all(false);
        assert_eq!(s.checked_count(), 0);
        s.set_all(true);
        assert_eq!(s.checked_count(), 1);
        assert!(!s.rows[0].checked);
    }

    #[test]
    fn selected_hosts_fill_the_local_user() {
        let s = ImportSelection::from_parsed(parsed(CFG), "me", &[]);
        let hosts = s.selected_hosts();
        assert_eq!(hosts[1].user, "me");
        assert_eq!(hosts[1].port, 2222);
    }

    #[test]
    fn label_pluralises() {
        assert_eq!(import_label(0), "Import");
        assert_eq!(import_label(1), "Import 1 connection");
        assert_eq!(import_label(3), "Import 3 connections");
    }

    #[test]
    fn apply_import_skips_held_aliases_and_writes_the_rest() {
        let written = RefCell::new(Vec::new());
        let hosts = vec![saved("a", "u", "h", 22), saved("b", "u", "h2", 22)];
        let out = apply_import(
            &hosts,
            "global",
            |alias| Ok(alias == "a"),
            |h| {
                written.borrow_mut().push(h.alias.clone());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(out.imported, 1);
        assert_eq!(out.skipped, vec!["a".to_string()]);
        assert_eq!(*written.borrow(), vec!["b".to_string()]);
    }

    #[test]
    fn apply_import_stops_on_a_store_error() {
        let hosts = vec![saved("a", "u", "h", 22)];
        let err = apply_import(&hosts, "global", |_| Ok(false), |_| Err("disk full".into()));
        assert_eq!(err, Err("disk full".into()));
    }
}
