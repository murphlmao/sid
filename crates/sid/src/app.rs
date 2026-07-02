//! The sid application: state + rendering (P3.1).
//!
//! [`AppState`] is the single gpui entity. It owns the open [`Store`], the current
//! [`Scope`], the active tab, and a **cached** composed host list. Events mutate the state
//! and call `cx.notify()`; `render` paints from the cache and never does I/O (the store's
//! reads return `Result` and touch redb + the filesystem, so they run on events only).
//!
//! P3.1 wires the SSH tab's host list to `Store::read_hosts` — the first time the store and
//! the GUI meet on screen. P3.2 adds the write side: the [`HostForm`] modal (add/edit with
//! the `save to:` dialog) and the secret lifecycle against the [`SecretStore`]. Other tabs
//! are placeholders for later slices.

use gpui::{
    ClickEvent, Context, Entity, FontWeight, SharedString, Subscription, Window, anchored,
    deferred, div, point, prelude::*, px, rgb, rgba, uniform_list,
};
use sid_secrets::{SecretId, SecretStore};
use sid_store::{
    Attributed, AuthMethod, Host, Scope, Store, ViewFilters, WorkspaceId, WorkspaceMeta,
};

use crate::ui::db_tab::DbTabState;
use crate::ui::host_form::{
    HostForm, HostFormEvent, Submission, add_guard, plan_secret, stage_secret,
};
use crate::ui::network_tab::NetworkTabState;
use crate::ui::{SessionStatus, SshSession};

// ---- neutral grayscale palette (theming deferred) --------------------------
const BG: u32 = 0x161618;
const TITLEBAR_BG: u32 = 0x1d1d20;
const TABSTRIP_BG: u32 = 0x1a1a1d;
const BORDER: u32 = 0x2c2c30;
const FG: u32 = 0xdcdce0;
const FG_DIM: u32 = 0x8a8a90;
const ACTIVE_BG: u32 = 0x33343a;
const ACTIVE_FG: u32 = 0xffffff;
const BRAND: u32 = 0x5a9ad0;
const WS_FG: u32 = 0xa98bd0;
const ROW_ALT: u32 = 0x1c1c20;
const DANGER: u32 = 0xd08a8a;

/// Monospace family for host subtitles (gpui falls back to a proportional font if the
/// named family is missing, so we name a concrete, near-universal Linux mono family).
const MONO: &str = "DejaVu Sans Mono";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Ssh,
    Database,
    Network,
    Workspaces,
    System,
}

impl Tab {
    const ALL: [Tab; 5] = [
        Tab::Ssh,
        Tab::Database,
        Tab::Network,
        Tab::Workspaces,
        Tab::System,
    ];

    fn label(self) -> &'static str {
        match self {
            Tab::Ssh => "SSH / SFTP",
            Tab::Database => "Database",
            Tab::Network => "Network",
            Tab::Workspaces => "Workspaces",
            Tab::System => "System",
        }
    }
}

/// Map a tab name (case-insensitive) to a [`Tab`] — `ssh|database|network|workspaces|
/// system`, anything else is `None`. Pure/string-in so it's unit-testable without env
/// fiddling; [`tab_from_env`] is the thin env-reading wrapper around it.
fn tab_from_str(name: &str) -> Option<Tab> {
    match name.to_lowercase().as_str() {
        "ssh" => Some(Tab::Ssh),
        "database" => Some(Tab::Database),
        "network" => Some(Tab::Network),
        "workspaces" => Some(Tab::Workspaces),
        "system" => Some(Tab::System),
        _ => None,
    }
}

/// `SID_START_TAB` support for visual-debugging tooling (`scripts/sid-shot.sh`): lets a
/// screenshot script launch straight into a given tab instead of always landing on the
/// SSH default. Unset or unrecognized -> `None`, leaving the normal default in place.
fn tab_from_env() -> Option<Tab> {
    std::env::var("SID_START_TAB")
        .ok()
        .and_then(|v| tab_from_str(&v))
}

/// One entry in the scope switcher.
pub(crate) struct ScopeChoice {
    pub(crate) label: SharedString,
    pub(crate) scope: Scope,
}

/// The single application entity.
pub struct AppState {
    pub(crate) store: Store,
    /// The secret backend (OS keyring or the in-memory fallback). All secret bytes go
    /// through here; the store only ever sees opaque `secret_ref` ids.
    pub(crate) secrets: Box<dyn SecretStore>,
    pub(crate) scope: Scope,
    active_tab: Tab,
    pub(crate) filters: ViewFilters,
    pub(crate) scopes: Vec<ScopeChoice>,
    hosts: Vec<Attributed<Host>>,
    pub(crate) error: Option<String>,
    /// The open host add/edit modal, if any.
    form: Option<Entity<HostForm>>,
    /// Keeps the form's event subscription alive exactly as long as the form is open.
    _form_subscription: Option<Subscription>,
    /// The row whose ✕ has been clicked once, keyed by (alias, origin) — the second
    /// click on the same row executes the delete.
    armed_delete: Option<(String, Scope)>,
    /// The connected (or connecting/failed) SSH session, if a host's ⚡ connect has been
    /// clicked — paired with a header label so the back-strip can show which host it is
    /// without `SshSession` needing to expose that itself. `Some` swaps the SSH tab's
    /// content from the host list to the split terminal + file-browser view (P3.5).
    session: Option<(SharedString, Entity<SshSession>)>,
    /// Database tab state (W3): the connection list, its own add/edit modal, and (W5)
    /// the active query session. Lives in its own module (`ui::db_tab`) — see that
    /// file's second `impl AppState` block for the render/mutation methods that operate
    /// on it via `pub(crate)` field access.
    pub(crate) db: DbTabState,
    /// Network tab state (inc-1): live/ephemeral ports + interfaces view, no store/
    /// scope/secrets. Lives in its own module (`ui::network_tab`), same shape as `db`.
    pub(crate) network: NetworkTabState,
}

impl AppState {
    /// Build the app state over an open store + secret backend and load the initial
    /// (Global) view. A `startup_warning` (the keyring fallback notice) is surfaced
    /// through the same error line store errors use.
    pub fn new(
        store: Store,
        secrets: Box<dyn SecretStore>,
        startup_warning: Option<String>,
    ) -> Self {
        let db = DbTabState::new(&store, &Scope::Global, ViewFilters::default());
        let network = NetworkTabState::new();
        let mut state = Self {
            store,
            secrets,
            scope: Scope::Global,
            active_tab: tab_from_env().unwrap_or(Tab::Ssh),
            filters: ViewFilters::default(),
            scopes: Vec::new(),
            hosts: Vec::new(),
            error: None,
            form: None,
            _form_subscription: None,
            armed_delete: None,
            session: None,
            db,
            network,
        };
        state.reload_scopes();
        state.refresh();
        // Set after the initial refresh so it isn't wiped by the successful read; it
        // stays visible until the next store event replaces it.
        if startup_warning.is_some() {
            state.error = startup_warning;
        }
        state
    }

    /// Rebuild the scope switcher entries: Global + each registered workspace.
    fn reload_scopes(&mut self) {
        let mut scopes = vec![ScopeChoice {
            label: "Global".into(),
            scope: Scope::Global,
        }];
        match self.store.global().list_workspaces() {
            Ok(list) => {
                for w in list {
                    scopes.push(ScopeChoice {
                        label: w.name.clone().into(),
                        scope: Scope::Workspace(w.id),
                    });
                }
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        self.scopes = scopes;
    }

    /// Re-query the composed host list for the current scope + filters. Any refresh
    /// changes the row set, so a pending delete confirmation is disarmed.
    fn refresh(&mut self) {
        self.armed_delete = None;
        match self.store.read_hosts(&self.scope, self.filters) {
            Ok(hosts) => {
                self.hosts = hosts;
                self.error = None;
            }
            Err(e) => {
                self.hosts = Vec::new();
                self.error = Some(e.to_string());
            }
        }
    }

    fn set_scope(&mut self, scope: Scope) {
        self.scope = scope;
        self.refresh();
        self.refresh_db();
    }

    // ---- host form (A6) ------------------------------------------------------

    /// The active workspace scope + its switcher label, if a workspace is focused.
    /// Feeds the form's `save to: workspace` option.
    pub(crate) fn active_workspace(&self) -> Option<(Scope, SharedString)> {
        match &self.scope {
            Scope::Global => None,
            Scope::Workspace(_) => {
                let label = self
                    .scopes
                    .iter()
                    .find(|c| c.scope == self.scope)
                    .map(|c| c.label.clone())
                    .unwrap_or_else(|| "workspace".into());
                Some((self.scope.clone(), label))
            }
        }
    }

    /// Open the empty add form, preselecting `save to:` from the persisted
    /// [`sid_store::Settings::default_scope`].
    fn open_add_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let default_scope = self
            .store
            .settings()
            .map(|s| s.default_scope)
            .unwrap_or_default();
        let workspace = self.active_workspace();
        let form = cx.new(|cx| HostForm::new_add(cx, workspace, default_scope));
        self.open_form(form, window, cx);
    }

    // ---- row actions (A7) ----------------------------------------------------

    /// ✎ Open the edit form prefilled with `host`, writing back into `origin` on save.
    fn open_edit_form(
        &mut self,
        host: Host,
        origin: Scope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.armed_delete = None;
        let workspace = self.active_workspace();
        let form = cx.new(|cx| HostForm::new_edit(cx, host, origin, workspace));
        self.open_form(form, window, cx);
    }

    /// ✕ (second click) Remove the record from **its origin layer**, then its secret
    /// from the keyring. Deleting a workspace copy un-shadows a global duplicate — that
    /// is attributive behavior, not loss.
    fn delete_row(
        &mut self,
        alias: &str,
        origin: &Scope,
        secret_ref: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        self.armed_delete = None;
        match self.store.delete_host(alias, origin) {
            Ok(_removed) => {
                let mut post_warning = None;
                if let Some(id) = secret_ref
                    && let Err(e) = self.secrets.delete(&SecretId::new(id))
                {
                    post_warning =
                        Some(format!("host deleted, but deleting its secret failed: {e}"));
                }
                self.refresh();
                if post_warning.is_some() {
                    self.error = post_warning;
                }
            }
            Err(e) => self.error = Some(e.to_string()),
        }
        cx.notify();
    }

    /// ⤒ Move a workspace-origin record up to global. A store-side conflict (the global
    /// layer already holds the alias — e.g. the demo seed's duplicate `vps-1`) surfaces
    /// verbatim in the header error line; nothing is overwritten.
    fn promote_row(&mut self, alias: &str, origin: &Scope, cx: &mut Context<Self>) {
        self.armed_delete = None;
        let Scope::Workspace(id) = origin else {
            return;
        };
        match self.store.promote_host(alias, id) {
            Ok(()) => self.refresh(),
            Err(e) => self.error = Some(e.to_string()),
        }
        cx.notify();
    }

    /// ⤓ Move a global-origin record down into the active workspace. Conflicts surface
    /// verbatim, exactly like promote.
    fn demote_row(&mut self, alias: &str, cx: &mut Context<Self>) {
        self.armed_delete = None;
        let Scope::Workspace(id) = self.scope.clone() else {
            return;
        };
        match self.store.demote_host(alias, &id) {
            Ok(()) => self.refresh(),
            Err(e) => self.error = Some(e.to_string()),
        }
        cx.notify();
    }

    // ---- SSH session connect (P3.5 split session) ------------------------------

    /// ⚡ connect: open a combined [`SshSession`] for `host` — one connection backing both
    /// the terminal and the file browser — and swap the SSH tab over to it. Only one
    /// session is live at a time — connecting a second host disconnects the first rather
    /// than leaking its background read-loop.
    fn connect_host(&mut self, host: Host, cx: &mut Context<Self>) {
        if let Some((_, old)) = self.session.take() {
            old.update(cx, |session, _cx| session.disconnect());
        }
        let label: SharedString =
            format!("{} — {}@{}:{}", host.alias, host.user, host.host, host.port).into();
        let known_hosts_path = data_dir().join("known_hosts");
        let session = SshSession::open(host, self.secrets.as_ref(), known_hosts_path, cx);
        self.session = Some((label, session));
        self.error = None;
        cx.notify();
    }

    /// ← disconnect: close the shell + sftp + client (if still live) and return the SSH
    /// tab to the host list, unchanged since the connect.
    fn close_session(&mut self, cx: &mut Context<Self>) {
        if let Some((_, session)) = self.session.take() {
            session.update(cx, |session, _cx| session.disconnect());
        }
        cx.notify();
    }

    fn open_form(&mut self, form: Entity<HostForm>, window: &mut Window, cx: &mut Context<Self>) {
        form.read(cx).focus_first(window, cx);
        self._form_subscription = Some(cx.subscribe(&form, Self::on_form_event));
        self.form = Some(form);
        cx.notify();
    }

    fn close_form(&mut self, cx: &mut Context<Self>) {
        self.form = None;
        self._form_subscription = None;
        cx.notify();
    }

    fn on_form_event(
        &mut self,
        form: Entity<HostForm>,
        event: &HostFormEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            HostFormEvent::Cancel => self.close_form(cx),
            HostFormEvent::Submit(submission) => match self.perform_submit(submission) {
                Ok(post_warning) => {
                    self.close_form(cx);
                    self.refresh();
                    if post_warning.is_some() {
                        self.error = post_warning;
                    }
                    cx.notify();
                }
                // Guard/secret/store failures land in the form's error line; the form
                // stays open so nothing typed is lost.
                Err(msg) => form.update(cx, |f, cx| f.set_error(msg, cx)),
            },
        }
    }

    /// Run a submission end-to-end: add-mode guard → stage the secret plan → write the
    /// host → delete any superseded secret. Returns a non-fatal warning to surface
    /// after success (e.g. the old secret could not be deleted).
    fn perform_submit(&self, submission: &Submission) -> Result<Option<String>, String> {
        let is_edit = submission.old.is_some();
        let target_holds = self
            .layer_holds_alias(&submission.target, &submission.host.alias)
            .map_err(|e| e.to_string())?;
        add_guard(is_edit, target_holds, &self.layer_label(&submission.target))?;

        let plan = plan_secret(
            submission.old.as_ref(),
            &submission.host.auth,
            submission.secret.is_some(),
        );
        let staged = stage_secret(
            self.secrets.as_ref(),
            &plan,
            &submission.host.alias,
            submission.secret.as_deref(),
        )?;

        let mut host = submission.host.clone();
        host.secret_ref = staged.secret_ref.clone();
        if let Err(e) = self.store.write_host(&host, &submission.target) {
            // Roll back a freshly minted secret so a failed write never orphans one.
            if staged.minted
                && let Some(id) = &staged.secret_ref
            {
                let _ = self.secrets.delete(&SecretId::new(id.clone()));
            }
            return Err(e.to_string());
        }

        // Only after the write is durable is the superseded secret deleted.
        let mut post_warning = None;
        if let Some(old_id) = &staged.delete_after_write
            && let Err(e) = self.secrets.delete(&SecretId::new(old_id.clone()))
        {
            post_warning = Some(format!("saved, but deleting the old secret failed: {e}"));
        }
        Ok(post_warning)
    }

    /// Whether `target`'s **own layer** already holds `alias` (the add-mode guard's
    /// question). Reads the layer directly — the composed default view collapses
    /// duplicates, which would hide exactly the record the guard must see.
    fn layer_holds_alias(&self, target: &Scope, alias: &str) -> sid_store::Result<bool> {
        match target {
            Scope::Global => Ok(self.store.global().get_host(alias)?.is_some()),
            Scope::Workspace(_) => {
                let filters = ViewFilters {
                    collapse_duplicates: false,
                    hide_global: true,
                };
                let hosts = self.store.read_hosts(target, filters)?;
                Ok(hosts.iter().any(|a| a.item.alias == alias))
            }
        }
    }

    /// Human name for a layer, matching the origin badges (`⌂ global` / workspace name).
    pub(crate) fn layer_label(&self, target: &Scope) -> String {
        match target {
            Scope::Global => "⌂ global".into(),
            Scope::Workspace(_) => self
                .scopes
                .iter()
                .find(|c| c.scope == *target)
                .map(|c| c.label.to_string())
                .unwrap_or_else(|| "workspace".into()),
        }
    }

    /// Badge label + color for an item's origin layer.
    fn origin_badge(&self, a: &Attributed<Host>) -> (SharedString, u32) {
        let (mut label, color): (SharedString, u32) = match &a.origin {
            Scope::Global => ("⌂ global".into(), BRAND),
            Scope::Workspace(id) => {
                let name = self
                    .scopes
                    .iter()
                    .find(|c| matches!(&c.scope, Scope::Workspace(w) if w == id))
                    .map(|c| c.label.clone())
                    .unwrap_or_else(|| "workspace".into());
                (name, WS_FG)
            }
        };
        if a.duplicate {
            label = format!("{label} · dup").into();
        }
        (label, color)
    }

    // ---- rendering helpers --------------------------------------------------

    fn titlebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.scope.clone();
        let buttons: Vec<_> = self
            .scopes
            .iter()
            .enumerate()
            .map(|(ix, choice)| {
                let active = choice.scope == current;
                let target = choice.scope.clone();
                div()
                    .id(("scope", ix))
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .bg(rgb(if active { ACTIVE_BG } else { TITLEBAR_BG }))
                    .text_color(rgb(if active { ACTIVE_FG } else { FG_DIM }))
                    .child(choice.label.clone())
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, _win, cx| {
                        this.set_scope(target.clone());
                        cx.notify();
                    }))
            })
            .collect();

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .w_full()
            .h(px(44.))
            .px_4()
            .bg(rgb(TITLEBAR_BG))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .text_color(rgb(BRAND))
                    .font_weight(FontWeight::BOLD)
                    .child("✦ sid"),
            )
            .child(div().text_xs().text_color(rgb(FG_DIM)).child("scope"))
            .children(buttons)
    }

    fn tab_strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_tab;
        let tabs: Vec<_> = Tab::ALL
            .iter()
            .enumerate()
            .map(|(ix, &tab)| {
                let is_active = tab == active;
                div()
                    .id(("tab", ix))
                    .px_4()
                    .py_2()
                    .text_sm()
                    .cursor_pointer()
                    .text_color(rgb(if is_active { ACTIVE_FG } else { FG_DIM }))
                    .border_b_2()
                    .border_color(rgb(if is_active { BRAND } else { TABSTRIP_BG }))
                    .child(tab.label())
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, _win, cx| {
                        this.active_tab = tab;
                        cx.notify();
                    }))
            })
            .collect();

        div()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h(px(40.))
            .px_2()
            .bg(rgb(TABSTRIP_BG))
            .border_b_1()
            .border_color(rgb(BORDER))
            .children(tabs)
    }

    fn ssh_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some((label, session)) = self.session.clone() {
            return self.session_pane(label, session, cx).into_any_element();
        }

        let count = self.hosts.len();
        let sub: SharedString = match &self.error {
            Some(e) => format!("error: {e}").into(),
            None => format!("{count} hosts · union of this scope, deduped").into(),
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(div().flex_1().text_sm().text_color(rgb(FG_DIM)).child(sub))
                    .child(
                        div()
                            .id("add-host")
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .text_sm()
                            .cursor_pointer()
                            .bg(rgb(ACTIVE_BG))
                            .text_color(rgb(ACTIVE_FG))
                            .child("＋ Add host")
                            .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                                this.open_add_form(window, cx);
                            })),
                    ),
            )
            .child(
                uniform_list(
                    "hosts",
                    count,
                    cx.processor(|this, range: std::ops::Range<usize>, _win, cx| {
                        range.map(|ix| this.host_row(ix, cx)).collect::<Vec<_>>()
                    }),
                )
                .flex_1(),
            )
            .into_any_element()
    }

    /// The connected view (P3.5): a disconnect strip showing `user@host` above the
    /// [`SshSession`] entity, which paints its own connecting/failed/closed/split
    /// (terminal + file panel) states.
    fn session_pane(
        &self,
        label: SharedString,
        session: Entity<SshSession>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let status = match session.read(cx).status() {
            SessionStatus::Connecting => "connecting…",
            SessionStatus::Connected => "connected",
            SessionStatus::Failed(_) => "failed",
            SessionStatus::Closed => "closed",
        };
        let header: SharedString = format!("{label} · {status}").into();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .id("session-disconnect")
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .text_sm()
                            .cursor_pointer()
                            .bg(rgb(ACTIVE_BG))
                            .text_color(rgb(ACTIVE_FG))
                            .child("← disconnect")
                            .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| {
                                this.close_session(cx);
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(rgb(FG_DIM))
                            .child(header),
                    ),
            )
            .child(div().flex().flex_col().flex_1().child(session))
    }

    fn host_row(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let a = &self.hosts[ix];
        let host = a.item.clone();
        let origin = a.origin.clone();
        let alias: SharedString = host.alias.clone().into();
        let subtitle: SharedString = format!("{}@{}:{}", host.user, host.host, host.port).into();
        let (badge, badge_color) = self.origin_badge(a);
        let alt = ix % 2 == 1;
        let armed = delete_click_executes(
            self.armed_delete.as_ref(),
            &(host.alias.clone(), origin.clone()),
        );

        // Small text-button factory for the row's action strip.
        let action = |id: (&'static str, usize), label: SharedString, color: u32| {
            div()
                .id(id)
                .px_2()
                .py_1()
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .text_color(rgb(color))
                .hover(|s| s.bg(rgb(ACTIVE_BG)))
                .child(label)
        };

        // ⤒ promote: workspace-origin rows only.
        let promote = can_promote(&origin).then(|| {
            let alias = host.alias.clone();
            let origin = origin.clone();
            action(("promote", ix), "⤒".into(), FG_DIM).on_click(cx.listener(
                move |this, _ev: &ClickEvent, _window, cx| {
                    this.promote_row(&alias, &origin, cx);
                },
            ))
        });

        // ⤓ demote: global-origin rows while a workspace scope is active.
        let demote = can_demote(&origin, &self.scope).then(|| {
            let alias = host.alias.clone();
            action(("demote", ix), "⤓".into(), FG_DIM).on_click(cx.listener(
                move |this, _ev: &ClickEvent, _window, cx| {
                    this.demote_row(&alias, cx);
                },
            ))
        });

        // ⚡ connect: opens a combined SshSession (terminal + file panel, P3.5) over this
        // row's host.
        let connect = {
            let host = host.clone();
            action(("connect", ix), "⚡ connect".into(), BRAND).on_click(cx.listener(
                move |this, _ev: &ClickEvent, _window, cx| {
                    this.connect_host(host.clone(), cx);
                },
            ))
        };

        // ✎ edit: opens the form prefilled with this row's record.
        let edit = {
            let host = host.clone();
            let origin = origin.clone();
            action(("edit", ix), "✎".into(), FG_DIM).on_click(cx.listener(
                move |this, _ev: &ClickEvent, window, cx| {
                    this.open_edit_form(host.clone(), origin.clone(), window, cx);
                },
            ))
        };

        // ✕ delete: two-click confirm — the first click arms this row, the second
        // deletes from the row's origin layer (and its secret from the keyring).
        let delete = {
            let alias = host.alias.clone();
            let origin = origin.clone();
            let secret_ref = host.secret_ref.clone();
            let (label, color) = if armed {
                ("✕ confirm?", DANGER)
            } else {
                ("✕", FG_DIM)
            };
            action(("delete", ix), label.into(), color).on_click(cx.listener(
                move |this, _ev: &ClickEvent, _window, cx| {
                    let key = (alias.clone(), origin.clone());
                    if delete_click_executes(this.armed_delete.as_ref(), &key) {
                        this.delete_row(&alias, &origin, secret_ref.as_deref(), cx);
                    } else {
                        this.armed_delete = Some(key);
                        cx.notify();
                    }
                },
            ))
        };

        div()
            .flex()
            .flex_col()
            .gap_1()
            .w_full()
            .px_4()
            .py_2()
            .bg(rgb(if alt { ROW_ALT } else { BG }))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(FG))
                            .child(alias),
                    )
                    .child(div().text_xs().text_color(rgb(badge_color)).child(badge))
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .children(promote)
                            .children(demote)
                            .child(connect)
                            .child(edit)
                            .child(delete),
                    ),
            )
            .child(
                div()
                    .font_family(MONO)
                    .text_xs()
                    .text_color(rgb(FG_DIM))
                    .child(subtitle),
            )
    }

    fn placeholder(&self, tab: Tab) -> impl IntoElement {
        div()
            .flex()
            .flex_1()
            .items_center()
            .justify_center()
            .text_color(rgb(FG_DIM))
            .child(format!("{} — coming next", tab.label()))
    }
}

impl Render for AppState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.active_tab {
            Tab::Ssh => self.ssh_tab(cx).into_any_element(),
            // `db_tab` needs `window` (W5) to lazily build the SQL editor/results table
            // on first paint — `InputState::new`/`TableState::new` both require it.
            Tab::Database => self.db_tab(window, cx),
            // `network_tab` needs `window` for the same reason (`TableState::new`).
            Tab::Network => self.network_tab(window, cx),
            other => self.placeholder(other).into_any_element(),
        };

        // Modal overlay: `anchored` pins a viewport-sized, occluding backdrop at the
        // window origin; `deferred` paints it above everything else.
        let overlay = self.form.clone().map(|form| {
            let viewport = window.viewport_size();
            deferred(
                anchored().position(point(px(0.), px(0.))).child(
                    div()
                        .occlude()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(viewport.width)
                        .h(viewport.height)
                        .bg(rgba(0x000000a8))
                        .child(form),
                ),
            )
            .with_priority(1)
        });
        // The DB connection add/edit modal (W4) — the exact mirror of `overlay` above,
        // over `self.db.form` instead of `self.form`.
        let db_overlay = self.db.form.clone().map(|form| {
            let viewport = window.viewport_size();
            deferred(
                anchored().position(point(px(0.), px(0.))).child(
                    div()
                        .occlude()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(viewport.width)
                        .h(viewport.height)
                        .bg(rgba(0x000000a8))
                        .child(form),
                ),
            )
            .with_priority(1)
        });

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(BG))
            .text_color(rgb(FG))
            .child(self.titlebar(cx))
            .child(self.tab_strip(cx))
            .child(div().flex().flex_col().flex_1().child(content))
            .children(overlay)
            .children(db_overlay)
    }
}

// ---- store bootstrap -------------------------------------------------------

/// The global data directory: `$XDG_DATA_HOME/sid` (or `~/.local/share/sid`).
pub fn data_dir() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| ".".into());
            home.join(".local").join("share")
        });
    base.join("sid")
}

/// Open the global store, seeding a small demo set on first run so the attributive
/// composition is visible immediately.
pub fn open_store() -> Store {
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    // Distinct filename from the archived TUI POC's `sid.redb` (incompatible schema at the
    // same machine-global path) so the rebuild starts from a clean store.
    let store = Store::open(&dir.join("store.redb")).expect("open sid store");
    seed_if_empty(&store, &dir);
    store
}

/// Open the default secret backend: the OS keyring if a startup durability probe
/// passes, otherwise an in-memory fallback plus a human-readable warning.
///
/// The warning (`Some(..)`) means the fallback is in use — secrets entered this session
/// will not survive a restart. The app does not yet surface this in the UI; for now the
/// caller is expected to log it or wire it into the header error line once that exists.
pub fn open_secrets() -> (Box<dyn sid_secrets::SecretStore>, Option<String>) {
    let (secrets, warning) = sid_secrets::open_default_secrets();
    let warning = warning.map(|w| {
        format!(
            "{w} — install a Secret Service provider (e.g. 'sudo pacman -S gnome-keyring') \
             to fix this permanently"
        )
    });
    (secrets, warning)
}

fn seed_if_empty(store: &Store, dir: &std::path::Path) {
    let no_hosts = store
        .global()
        .list_hosts()
        .map(|h| h.is_empty())
        .unwrap_or(false);
    let no_ws = store
        .global()
        .list_workspaces()
        .map(|w| w.is_empty())
        .unwrap_or(false);

    // The DB connection seed is gated independently of hosts/workspaces below: on a dev
    // machine whose store already has hosts (the common case once the SSH slice is in
    // daily use), the host/workspace gate is permanently tripped, and a connections seed
    // added later than that first run would otherwise never fire. Each demo dataset gets
    // its own empty-state check so W3's DB seed still lands on existing stores.
    let no_connections = store
        .global()
        .list_connections()
        .map(|c| c.is_empty())
        .unwrap_or(false);
    if no_connections {
        // The demo connection's file must exist before `run_query` (W5) opens it: saved
        // connections always open with `sqlite_mode: None` -> `SqliteMode::OpenExisting`
        // (see `db_tab::run_first_page`), which requires the file to already be there.
        // An empty file is a valid, openable SQLite database (rusqlite/sqlite3 initialize
        // it lazily on first write), so a bare `File::create` is enough.
        let demo_db = dir.join("demo.db");
        let _ = std::fs::File::create(&demo_db);
        let _ = store.write_connection(
            &sid_store::DbConnection {
                id: "demo-sqlite".into(),
                dsn: demo_db.to_string_lossy().into_owned(),
                secret_ref: None,
                kind: sid_core::db::DbKind::Sqlite,
                name: "demo sqlite (local file)".into(),
            },
            &Scope::Global,
        );
    }

    if !(no_hosts && no_ws) {
        return;
    }

    let global = |alias: &str, user: &str, host: &str| Host {
        alias: alias.into(),
        user: user.into(),
        host: host.into(),
        port: 22,
        secret_ref: None,
        auth: AuthMethod::default(),
    };
    let _ = store.write_host(
        &global("home-server", "you", "192.168.1.10"),
        &Scope::Global,
    );
    let _ = store.write_host(&global("vps-1", "root", "5.5.5.5"), &Scope::Global);

    // A demo workspace under the data dir, with a duplicate (`vps-1`) to show composition.
    let root = dir.join("demo-workspace");
    let _ = std::fs::create_dir_all(&root);
    let id = WorkspaceId::from_root(&root);
    let _ = store.register_workspace(&WorkspaceMeta {
        id: id.clone(),
        root,
        name: "acme-api (demo)".into(),
    });
    let ws = Scope::Workspace(id);
    let _ = store.write_host(
        &global("staging", "deploy", "staging.acme-api.internal"),
        &ws,
    );
    let _ = store.write_host(&global("prod", "deploy", "prod.acme-api.internal"), &ws);
    let _ = store.write_host(&global("vps-1", "admin", "5.5.5.5"), &ws); // duplicates global vps-1
}

// ---- row-action routing (pure, unit-tested) ---------------------------------

/// Whether a row offers ⤒ promote: only records that live in a workspace layer.
pub(crate) fn can_promote(origin: &Scope) -> bool {
    matches!(origin, Scope::Workspace(_))
}

/// Whether a row offers ⤓ demote: only global-layer records, and only while a workspace
/// scope is active to receive them.
pub(crate) fn can_demote(origin: &Scope, current_scope: &Scope) -> bool {
    matches!(origin, Scope::Global) && matches!(current_scope, Scope::Workspace(_))
}

/// Two-click delete: `true` when the clicked row is the one already armed. Keyed on
/// (alias, origin) so the same alias in the *other* layer never inherits the confirm.
pub(crate) fn delete_click_executes(
    armed: Option<&(String, Scope)>,
    clicked: &(String, Scope),
) -> bool {
    armed == Some(clicked)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(id: &str) -> Scope {
        Scope::Workspace(WorkspaceId(id.to_string()))
    }

    #[test]
    fn promote_offered_only_on_workspace_origin_rows() {
        assert!(can_promote(&ws("/w")));
        assert!(!can_promote(&Scope::Global));
    }

    #[test]
    fn demote_offered_only_on_global_rows_in_a_workspace_scope() {
        assert!(can_demote(&Scope::Global, &ws("/w")));
        assert!(!can_demote(&Scope::Global, &Scope::Global));
        assert!(!can_demote(&ws("/w"), &ws("/w")));
        assert!(!can_demote(&ws("/w"), &Scope::Global));
    }

    #[test]
    fn delete_needs_two_clicks_on_the_same_row() {
        let row = ("vps-1".to_string(), Scope::Global);
        // First click arms (nothing armed yet)…
        assert!(!delete_click_executes(None, &row));
        // …second click on the same row executes.
        assert!(delete_click_executes(Some(&row), &row));
    }

    #[test]
    fn tab_from_str_maps_known_names_case_insensitively() {
        assert!(matches!(tab_from_str("ssh"), Some(Tab::Ssh)));
        assert!(matches!(tab_from_str("Database"), Some(Tab::Database)));
        assert!(matches!(tab_from_str("NETWORK"), Some(Tab::Network)));
        assert!(matches!(tab_from_str("Workspaces"), Some(Tab::Workspaces)));
        assert!(matches!(tab_from_str("SYSTEM"), Some(Tab::System)));
        assert!(tab_from_str("bogus").is_none());
        assert!(tab_from_str("").is_none());
    }

    #[test]
    fn delete_confirm_never_leaks_across_layers_of_a_duplicate_alias() {
        // The demo seed holds `vps-1` in BOTH layers; arming one copy must not confirm
        // the other (they are distinct records under the attributive invariant).
        let global_row = ("vps-1".to_string(), Scope::Global);
        let ws_row = ("vps-1".to_string(), ws("/w"));
        assert!(!delete_click_executes(Some(&global_row), &ws_row));
        assert!(!delete_click_executes(Some(&ws_row), &global_row));
        // A different alias re-arms rather than confirming.
        let other = ("prod".to_string(), Scope::Global);
        assert!(!delete_click_executes(Some(&global_row), &other));
    }
}
