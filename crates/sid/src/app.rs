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

use crate::ui::host_form::{
    HostForm, HostFormEvent, Submission, add_guard, plan_secret, stage_secret,
};

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

/// One entry in the scope switcher.
struct ScopeChoice {
    label: SharedString,
    scope: Scope,
}

/// The single application entity.
pub struct AppState {
    store: Store,
    /// The secret backend (OS keyring or the in-memory fallback). All secret bytes go
    /// through here; the store only ever sees opaque `secret_ref` ids.
    secrets: Box<dyn SecretStore>,
    scope: Scope,
    active_tab: Tab,
    filters: ViewFilters,
    scopes: Vec<ScopeChoice>,
    hosts: Vec<Attributed<Host>>,
    error: Option<String>,
    /// The open host add/edit modal, if any.
    form: Option<Entity<HostForm>>,
    /// Keeps the form's event subscription alive exactly as long as the form is open.
    _form_subscription: Option<Subscription>,
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
        let mut state = Self {
            store,
            secrets,
            scope: Scope::Global,
            active_tab: Tab::Ssh,
            filters: ViewFilters::default(),
            scopes: Vec::new(),
            hosts: Vec::new(),
            error: None,
            form: None,
            _form_subscription: None,
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

    /// Re-query the composed host list for the current scope + filters.
    fn refresh(&mut self) {
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
    }

    // ---- host form (A6) ------------------------------------------------------

    /// The active workspace scope + its switcher label, if a workspace is focused.
    /// Feeds the form's `save to: workspace` option.
    fn active_workspace(&self) -> Option<(Scope, SharedString)> {
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
    fn layer_label(&self, target: &Scope) -> String {
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
                    cx.processor(|this, range: std::ops::Range<usize>, _win, _cx| {
                        range.map(|ix| this.host_row(ix)).collect::<Vec<_>>()
                    }),
                )
                .flex_1(),
            )
    }

    fn host_row(&self, ix: usize) -> impl IntoElement + use<> {
        let a = &self.hosts[ix];
        let alias: SharedString = a.item.alias.clone().into();
        let subtitle: SharedString =
            format!("{}@{}:{}", a.item.user, a.item.host, a.item.port).into();
        let (badge, badge_color) = self.origin_badge(a);
        let alt = ix % 2 == 1;

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
                    .child(div().text_xs().text_color(rgb(badge_color)).child(badge)),
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
    sid_secrets::open_default_secrets()
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
