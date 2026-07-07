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
    ClickEvent, Context, Corner, Entity, FocusHandle, FontWeight, KeyDownEvent, SharedString,
    Subscription, Window, anchored, deferred, div, point, prelude::*, px, rgb, rgba,
};
use sid_secrets::{SecretId, SecretStore};
use sid_store::{
    Attributed, AuthMethod, Host, PanelSide, Scope, Store, ViewFilters, WorkspaceId, WorkspaceMeta,
};

use crate::keymap::{self, Action, FocusContext};
use crate::ssh_connect;
use crate::ui::command_palette::PaletteState;
use crate::ui::db_tab::DbTabState;
use crate::ui::host_form::{
    HostForm, HostFormEvent, Submission, add_guard, plan_secret, stage_secret,
};
use crate::ui::network_tab::NetworkTabState;
use crate::ui::password_prompt::{PasswordPromptEvent, PasswordPromptModal};
use crate::ui::settings_tab::SettingsTabState;
use crate::ui::ssh_home::HomeTabState;
use crate::ui::systems_tab::SystemsTabState;
use crate::ui::theme;
use crate::ui::{SessionStatus, SshSession, SshSessionEvent};

/// Monospace family for host subtitles (gpui falls back to a proportional font if the
/// named family is missing, so we name a concrete, near-universal Linux mono family).
const MONO: &str = "DejaVu Sans Mono";

// `pub(crate)` (not private): `ui::systems_tab`'s periodic refresh loop needs to read
// `AppState::active_tab` (via the `active_tab()` accessor below) to stop refreshing the
// instant the user switches away from `Tab::System` — see that module's doc comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    Ssh,
    Database,
    Network,
    Workspaces,
    System,
    Settings,
}

impl Tab {
    const ALL: [Tab; 6] = [
        Tab::Ssh,
        Tab::Database,
        Tab::Network,
        Tab::Workspaces,
        Tab::System,
        Tab::Settings,
    ];

    fn label(self) -> &'static str {
        match self {
            Tab::Ssh => "SSH / SFTP",
            Tab::Database => "Database",
            Tab::Network => "Network",
            Tab::Workspaces => "Workspaces",
            Tab::System => "System",
            Tab::Settings => "Settings",
        }
    }
}

/// Map a tab name (case-insensitive) to a [`Tab`] — `ssh|database|network|workspaces|
/// system|settings`, anything else is `None`. Pure/string-in so it's unit-testable
/// without env fiddling; [`tab_from_env`] is the thin env-reading wrapper around it.
fn tab_from_str(name: &str) -> Option<Tab> {
    match name.to_lowercase().as_str() {
        "ssh" => Some(Tab::Ssh),
        "database" => Some(Tab::Database),
        "network" => Some(Tab::Network),
        "workspaces" => Some(Tab::Workspaces),
        "system" => Some(Tab::System),
        "settings" => Some(Tab::Settings),
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
    /// The composed host list for the active scope — `pub(crate)` so `ui::ssh_home`'s
    /// tree sidebar can read it directly (same convention as `ui::db_tab`; see that
    /// module's doc comment).
    pub(crate) hosts: Vec<Attributed<Host>>,
    pub(crate) error: Option<String>,
    /// Whether the effective secret backend is degraded (memory fallback — the keyring
    /// is disabled, or it failed the startup probe). Drives the warning badge at the
    /// tab strip's right end (see `secret_status_badge`) and the memory-aware password
    /// helper copy in the host/DB forms (round-D §A.5); a healthy keyring renders
    /// nothing and shows the normal copy. `pub(crate)` so `ui::host_form`/
    /// `ui::db_conn_form` construction sites (here and in `ui::db_tab`) can read it.
    pub(crate) secrets_degraded: bool,
    /// The full secret-backend status line (`secret_status_message`'s output: backend,
    /// warning, recommendation) — shown in the warning badge's popover on click, and
    /// (round-E §C) under the Settings screen's keyring toggle. Set once at startup;
    /// nothing currently changes the backend mid-session, so this never needs to be
    /// refreshed after `AppState::new`. `pub(crate)` so `ui::settings_tab`'s Behavior
    /// section can read it directly, same convention as `secrets_degraded` above.
    pub(crate) secrets_status_detail: String,
    /// Whether the warning badge's popover is open.
    secret_badge_open: bool,
    /// The open host add/edit modal, if any.
    form: Option<Entity<HostForm>>,
    /// Keeps the form's event subscription alive exactly as long as the form is open.
    _form_subscription: Option<Subscription>,
    /// The row whose ✕ has been clicked once, keyed by (alias, origin) — the second
    /// click on the same row executes the delete. `pub(crate)` so `ui::ssh_home`'s tree
    /// rows share the exact same two-click state as the host-list rows below.
    pub(crate) armed_delete: Option<(String, Scope)>,
    /// Every live SSH session (ssh-v3): each fully independent (own client/reader/
    /// writer/shell/sftp — the P3.5 split carries over unchanged per-session). Replaces
    /// the old single-`Option` field now that MobaXterm-style multi-session tabs are the
    /// SSH tab's whole shape.
    pub(crate) ssh_sessions: Vec<SshTab>,
    /// Which SSH session tab is active. `None` is the Home tab (the connection
    /// manager + saved-connections tree); `Some(ix)` indexes `ssh_sessions`.
    pub(crate) active_session: Option<usize>,
    /// Cached from `Settings.file_browser_side` at startup. New sessions open docked to
    /// this side; the file panel's `⇄ dock` control (any open session) flips + persists
    /// it and fans the update out to every live session — see `on_session_event`.
    pub(crate) file_browser_side: PanelSide,
    /// The SSH tab's Home-state view-local UI state (tree collapse/search/inline
    /// rename+folder-edit) — lives in its own module (`ui::ssh_home`), same shape as
    /// `db`/`network` below.
    pub(crate) ssh_home: HomeTabState,
    /// Database tab state (W3): the connection list, its own add/edit modal, and (W5)
    /// the active query session. Lives in its own module (`ui::db_tab`) — see that
    /// file's second `impl AppState` block for the render/mutation methods that operate
    /// on it via `pub(crate)` field access.
    pub(crate) db: DbTabState,
    /// Network tab state (inc-1): live/ephemeral ports + interfaces view, no store/
    /// scope/secrets. Lives in its own module (`ui::network_tab`), same shape as `db`.
    pub(crate) network: NetworkTabState,
    /// Systems tab state (Round D §C): live/ephemeral host overview + processes view,
    /// same "no store/scope/secrets" shape as `network`. Lives in its own module
    /// (`ui::systems_tab`).
    pub(crate) systems: SystemsTabState,
    /// Settings tab state (round-E §C): a cached snapshot of the persisted
    /// `Settings` (unlike `network`/`systems`, this tab does read/write the store
    /// directly — the cache exists purely so `render` never re-reads it, per this
    /// module's own "render never does I/O" rule) plus a surfaced write-failure
    /// line. Lives in its own module (`ui::settings_tab`), same "second `impl
    /// AppState` block" convention as `db`/`network`/`systems`.
    pub(crate) settings: SettingsTabState,
    /// The open connect-time password prompt (SSH connect, DB run/schema-refresh), if
    /// any — see `open_password_prompt`.
    password_prompt: Option<Entity<PasswordPromptModal>>,
    /// Keeps the modal's event subscription alive exactly as long as it's open.
    _password_prompt_subscription: Option<Subscription>,
    /// What submitting `password_prompt` resumes — which connect/query attempt to
    /// retry, and (for a pre-existing `secret_ref`) where to `secrets.put` the entered
    /// password so a normal retry finds it. `None` whenever `password_prompt` is
    /// `None`.
    pending_secret_prompt: Option<PendingSecretPrompt>,
    /// The command palette's open/query/selection state (`Ctrl+K`) — `None` when
    /// closed. `pub(crate)` so `ui::command_palette`'s `impl AppState` block (same
    /// convention as `ui::db_tab`/`ui::ssh_home`) can read/mutate it directly.
    pub(crate) palette: Option<PaletteState>,
    /// The `?` keyboard cheat-sheet overlay's open state.
    cheat_sheet_open: bool,
    /// A stable focus target tracked on the outermost element (see `Render::render`'s
    /// `.track_focus`), unconditionally re-rendered on every frame. Load-bearing for
    /// the keyboard system: gpui falls back to a *degenerate, single-node* dispatch
    /// path — bypassing `handle_root_key_down`'s `.capture_key_down` entirely — the
    /// instant `window.focus`'s target isn't part of the current render frame (e.g.
    /// the SSH terminal's handle, right after switching to another primary tab makes
    /// it stop rendering). Every place that changes `active_tab`/`active_session`
    /// re-focuses either the newly active session's terminal or, failing that, this
    /// handle — see `refocus_stable_target` — so a keyboard-only user is never left
    /// with a dangling focus that silently kills every further shortcut.
    ///
    /// `pub(crate)` so the `impl AppState` methods that live in `ui::db_tab` (e.g.
    /// `close_db_form`) can refocus it on form close, same as the host-form path here.
    pub(crate) root_focus: FocusHandle,
}

/// One live SSH session tab (ssh-v3): the entity, its `user@host` display label, which
/// saved (alias, origin) row it was opened from (if any — `None` for an ephemeral
/// quick-connect that was never saved, so the home tree's live-dot only tracks saved
/// hosts), and the subscription that lets its `⇄ dock` toggle — fired as an event,
/// since `SshSession` never touches `Store` itself — reach [`AppState::on_session_event`].
pub(crate) struct SshTab {
    pub(crate) label: SharedString,
    pub(crate) session: Entity<SshSession>,
    pub(crate) source: Option<(String, Scope)>,
    _dock_toggle: Subscription,
}

/// What submitting the connect-time [`PasswordPromptModal`] resumes (round-D §A.4) —
/// captured when the prompt opens, consumed exactly once in
/// [`AppState::on_password_prompt_event`]. `pub(crate)` — `ui::db_tab`'s
/// `AppState::run_query`/`refresh_schema` construct the `Db` variant directly.
pub(crate) enum PendingSecretPrompt {
    /// An SSH connect (`connect_host`) whose host uses `Password` auth but had no
    /// concretely resolvable secret (missing entirely, or a dangling `secret_ref`).
    /// The password is spliced straight into the retried connect attempt; if `host`
    /// already carries a `secret_ref`, it's also `secrets.put` under that id first so
    /// the rest of the session remembers it.
    Ssh {
        host: Host,
        source: Option<(String, Scope)>,
    },
    /// A DB action (`run_query`/`refresh_schema`) whose active connection's
    /// `secret_ref` was dangling. The password is `secrets.put` under `secret_ref`
    /// (always `Some` in this variant — see `db_tab::needs_password_prompt`'s doc
    /// comment), then `retry` is re-run so the normal resolve path picks it up.
    Db {
        secret_ref: String,
        retry: crate::ui::db_tab::DbRetry,
    },
}

impl AppState {
    /// Build the app state over an open store + resolved secret backend and load the
    /// initial (Global) view.
    ///
    /// `secrets_degraded`/`secrets_status` come from `open_secrets`: whether the
    /// effective backend is memory (vs. a healthy keyring), and the full status text
    /// (backend, warning, recommendation) — the former gates the warning badge (see
    /// `secret_status_badge`), the latter feeds its popover. Round-D §A dropped the
    /// startup unlock-or-create modal entirely (the encrypted-file backend is no longer
    /// wired into `sid_secrets::resolve_secret_store`'s chain) and the persistent
    /// "secrets: …" banner along with it — a degraded backend now shows as this small
    /// badge instead of taking over the SSH tab's status line.
    ///
    /// `seed_lists` is `open_store`'s `seed_if_empty` call, already read (and, on a
    /// first launch, re-read post-seed) — see [`SeedLists`]'s doc comment. Consuming it
    /// via [`Self::apply_seed_lists`] here means this constructor doesn't immediately
    /// re-issue the same hosts/workspaces reads `seed_if_empty` just did (perf audit
    /// finding #7).
    pub fn new(
        store: Store,
        seed_lists: SeedLists,
        secrets: Box<dyn SecretStore>,
        secrets_degraded: bool,
        secrets_status: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let db = DbTabState::new(&store, &Scope::Global, ViewFilters::default());
        let network = NetworkTabState::new();
        let systems = SystemsTabState::new();
        // Read before `store` moves into the struct literal below — `.settings()` only
        // borrows. Falls back to `PanelSide::default()` (Left) on a read error, same as
        // every other `Settings` read in this constructor's neighborhood.
        let file_browser_side = store
            .settings()
            .map(|s| s.file_browser_side)
            .unwrap_or_default();
        // Also reads (and caches) `Settings` — see `SettingsTabState`'s doc comment for
        // why the Settings screen keeps its own snapshot rather than re-reading
        // `store.settings()` from `render`.
        let settings = SettingsTabState::new(&store);
        let mut state = Self {
            store,
            secrets,
            scope: Scope::Global,
            active_tab: tab_from_env().unwrap_or(Tab::Ssh),
            filters: ViewFilters::default(),
            scopes: Vec::new(),
            hosts: Vec::new(),
            error: None,
            secrets_degraded,
            secrets_status_detail: secrets_status,
            secret_badge_open: false,
            form: None,
            _form_subscription: None,
            armed_delete: None,
            ssh_sessions: Vec::new(),
            active_session: None,
            file_browser_side,
            ssh_home: HomeTabState::new(cx),
            db,
            network,
            systems,
            settings,
            password_prompt: None,
            _password_prompt_subscription: None,
            pending_secret_prompt: None,
            palette: None,
            cheat_sheet_open: false,
            root_focus: cx.focus_handle(),
        };
        state.apply_seed_lists(seed_lists);
        state
    }

    /// Rebuild the scope switcher from the store at RUNTIME — the seam the Workspaces
    /// tab uses after registering/unregistering a workspace (this closes the
    /// long-standing "reload_scopes was inlined into apply_seed_lists, startup-only"
    /// caveat from HANDOFF). If the focused scope's workspace disappeared, falls back
    /// to Global and refreshes every scoped view.
    // dead_code: consumed by the Workspaces tab (in-flight track U) — remove with it.
    #[allow(dead_code)]
    pub(crate) fn reload_scopes_runtime(&mut self, cx: &mut Context<Self>) {
        match self.store.list_workspaces() {
            Ok(list) => self.scopes = build_scope_choices(list),
            Err(e) => {
                self.error = Some(e.to_string());
                cx.notify();
                return;
            }
        }
        let focused_still_exists = self.scopes.iter().any(|c| c.scope == self.scope);
        if !focused_still_exists {
            self.set_scope(Scope::Global);
        }
        cx.notify();
    }

    /// Populate the initial scope switcher + host list from `seed_lists` — the reads
    /// `open_store`'s `seed_if_empty` already performed — instead of re-issuing
    /// `list_workspaces`/`read_hosts` here (perf audit finding #7). Builds the scope
    /// switcher the same way the (pre-this-change) `reload_scopes` did, and the host
    /// list the same way [`Self::refresh`] does, in the same order (workspaces first,
    /// hosts second) so the error-handling priority matches exactly: a hosts-read
    /// success clears `self.error` even over a stale workspaces-read error, matching
    /// `refresh`'s existing "freshest word on `self.error`" contract — for the one case
    /// both ever ran against, `Scope::Global` with `ViewFilters::default()`.
    ///
    /// At `Scope::Global`, `Store::read_hosts` is exactly `list_hosts()` mapped into
    /// `Attributed { origin: Scope::Global, duplicate: false, .. }` — no workspace ever
    /// enters the Global-scope composition (`sid_store::composer::compose` with
    /// `workspace: None`) — so reusing `seed_lists.hosts` here is not an approximation,
    /// it's the identical result `refresh()` would have read.
    fn apply_seed_lists(&mut self, seed_lists: SeedLists) {
        self.armed_delete = None;

        match seed_lists.workspaces {
            Ok(list) => self.scopes = build_scope_choices(list),
            Err(e) => {
                self.scopes = build_scope_choices(Vec::new());
                self.error = Some(e);
            }
        }

        match seed_lists.hosts {
            Ok(hosts) => {
                self.hosts = hosts
                    .into_iter()
                    .map(|item| Attributed {
                        item,
                        origin: Scope::Global,
                        duplicate: false,
                    })
                    .collect();
                self.error = None;
            }
            Err(e) => {
                self.hosts = Vec::new();
                self.error = Some(e);
            }
        }
    }

    /// Re-query the composed host list for the current scope + filters. Any refresh
    /// changes the row set, so a pending delete confirmation is disarmed. `pub(crate)`
    /// so `ui::ssh_home`'s rename/folder-edit commits can reload the tree the same way
    /// every other host-list mutation here does (same convention as `db_tab`'s
    /// `refresh_db`).
    pub(crate) fn refresh(&mut self) {
        self.armed_delete = None;
        match self.store.read_hosts(&self.scope, self.filters) {
            Ok(hosts) => {
                self.hosts = hosts;
                // Surface duplicate-identity records in the focused workspace's
                // committed config (a git-merge artifact) on the status line — the
                // store keeps them losslessly, but an explicit edit collapses the
                // copies to one, and the user should know that before it happens.
                self.error = match self.store.workspace_duplicates(&self.scope) {
                    Ok(dups) if !dups.is_empty() => Some(format!(
                        "workspace config has duplicate entries: {} — editing one \
                         collapses its copies to the edited value",
                        dups.join(", ")
                    )),
                    _ => None,
                };
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

    /// Which primary tab is active. `pub(crate)` (rather than exposing `active_tab`
    /// itself) so `ui::systems_tab`'s periodic refresh loop can check "is the Systems
    /// tab still the one on screen" without the field itself needing wider visibility.
    pub(crate) fn active_tab(&self) -> Tab {
        self.active_tab
    }

    /// Open the empty add form, preselecting `save to:` from the persisted
    /// [`sid_store::Settings::default_scope`]. `pub(crate)` so every add-connection
    /// entry point the ssh-v3 discoverability pass added — `ui::ssh_home`'s sidebar
    /// header button, its tree's empty-space context menu, and this tab strip's `+`
    /// when already on Home (see `session_tab_strip`) — all go through this one path,
    /// same as the pre-existing `main` pane's `+ Add host` button below.
    pub(crate) fn open_add_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let default_scope = self
            .store
            .settings()
            .map(|s| s.default_scope)
            .unwrap_or_default();
        let workspace = self.active_workspace();
        let degraded = self.secrets_degraded;
        let form = cx.new(|cx| HostForm::new_add(cx, workspace, default_scope, degraded));
        self.open_form(form, window, cx);
    }

    // ---- row actions (A7) ----------------------------------------------------

    /// ✎ Open the edit form prefilled with `host`, writing back into `origin` on save.
    /// `pub(crate)` so `ui::ssh_home`'s tree right-click menu's "Edit…" item shares this
    /// exact path with the `main` pane's own `✎ edit` row action.
    pub(crate) fn open_edit_form(
        &mut self,
        host: Host,
        origin: Scope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.armed_delete = None;
        let workspace = self.active_workspace();
        let degraded = self.secrets_degraded;
        let form = cx.new(|cx| HostForm::new_edit(cx, host, origin, workspace, degraded));
        self.open_form(form, window, cx);
    }

    /// ✕ (second click) Remove the record from **its origin layer**, then its secret
    /// from the keyring. Deleting a workspace copy un-shadows a global duplicate — that
    /// is attributive behavior, not loss. `pub(crate)` so `ui::ssh_home`'s tree rows
    /// share this exact delete path (and `armed_delete` state) with the host-list rows.
    pub(crate) fn delete_row(
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
    pub(crate) fn promote_row(&mut self, alias: &str, origin: &Scope, cx: &mut Context<Self>) {
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
    pub(crate) fn demote_row(&mut self, alias: &str, cx: &mut Context<Self>) {
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

    // ---- SSH multi-session tabs (ssh-v3) ----------------------------------------

    /// connect (or quick-connect): resolve `host`'s secret and, if it's concretely
    /// available (or none is needed), open a new, independent [`SshSession`] and switch
    /// to it — ssh-v3 makes every session fully independent, so connecting a second (or
    /// third, …) host no longer disconnects any other open tab. `source` identifies
    /// which saved (alias, origin) row this came from, for the home tree's live-dot —
    /// `None` for an ephemeral quick-connect host that was never saved.
    ///
    /// Round-D §A.4: a `Password`-auth host with nothing concretely resolvable (missing
    /// entirely, or a dangling `secret_ref`) opens the connect-time password prompt
    /// instead of failing outright — see [`ssh_connect::needs_password_prompt`] and
    /// [`Self::on_password_prompt_event`].
    pub(crate) fn connect_host(
        &mut self,
        host: Host,
        source: Option<(String, Scope)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let secret = ssh_connect::resolve_secret(self.secrets.as_ref(), &host);
        if ssh_connect::needs_password_prompt(&host.auth, &secret) {
            let label: SharedString = format!("{}@{}", host.user, host.host).into();
            self.open_password_prompt(label, PendingSecretPrompt::Ssh { host, source }, window, cx);
            return;
        }
        self.finish_connect(host, source, secret, cx);
    }

    /// The connect-or-open half of [`Self::connect_host`], split out so the password
    /// prompt's submit handler ([`Self::on_password_prompt_event`]) can resume here
    /// directly with a one-shot password, bypassing a second `resolve_secret` call that
    /// would just fail the same way again.
    fn finish_connect(
        &mut self,
        host: Host,
        source: Option<(String, Scope)>,
        secret: Result<Option<Vec<u8>>, String>,
        cx: &mut Context<Self>,
    ) {
        let label: SharedString = format!("{}@{}", host.user, host.host).into();
        let known_hosts_path = data_dir().join("known_hosts");
        let session = SshSession::open(host, secret, known_hosts_path, self.file_browser_side, cx);
        let dock_toggle = cx.subscribe(&session, Self::on_session_event);
        self.ssh_sessions.push(SshTab {
            label,
            session,
            source,
            _dock_toggle: dock_toggle,
        });
        self.active_session = Some(self.ssh_sessions.len() - 1);
        self.error = None;
        cx.notify();
    }

    /// `+`/`✕` on a live tab go through the SAME "back to home" verb the mockup uses —
    /// `new_session` currently just means "show Home", same as [`Self::go_home`]. Kept
    /// as its own method (rather than an alias) since the keyboard track (`Ctrl+T`?)
    /// binds to this name specifically, and it may grow its own behavior later (e.g. a
    /// picker) without every `+` caller needing to change.
    pub(crate) fn new_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.go_home(window, cx);
    }

    /// `home`: show the Home tab (the connection manager + saved-connections tree).
    /// Doesn't touch any live session — switching to Home and back leaves every open
    /// tab exactly as it was. Refocuses `root_focus` (see that field's doc comment) —
    /// the session being left has nothing to hand focus off to.
    pub(crate) fn go_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.active_session = None;
        window.focus(&self.root_focus);
        cx.notify();
    }

    /// Click a session tab (or `Ctrl+Tab` cycling — see `cycle_tabs`): make it active.
    /// A stale/out-of-range `ix` (shouldn't happen — every caller derives `ix` from
    /// `ssh_sessions` itself) is a silent no-op rather than a panic. Restores keyboard
    /// focus onto the newly active session's terminal — without this, switching tabs
    /// leaves the *previous* session's (now-unmounted) terminal as the window's
    /// recorded focus target, which silently breaks all further keyboard dispatch (see
    /// `root_focus`'s doc comment).
    pub(crate) fn activate_session(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(tab) = self.ssh_sessions.get(ix) {
            self.active_session = Some(ix);
            window.focus(&tab.session.read(cx).terminal_focus_handle());
            cx.notify();
        }
    }

    /// `✕` on a session tab: disconnect it (shell + sftp + client), remove its tab, and
    /// fix up `active_session` — see [`next_active_after_close`] for the exact
    /// close-left-of-active / close-active / close-last-tab-goes-home bookkeeping this
    /// delegates to (pure, unit-tested). Refocuses whatever tab is now active (another
    /// session's terminal, or `root_focus` if that lands on Home) — same reasoning as
    /// [`Self::activate_session`].
    pub(crate) fn close_session(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.ssh_sessions.len() {
            return;
        }
        let tab = self.ssh_sessions.remove(ix);
        tab.session.update(cx, |session, _cx| session.disconnect());
        self.active_session =
            next_active_after_close(self.active_session, ix, self.ssh_sessions.len());
        self.refocus_stable_target(window, cx);
        cx.notify();
    }

    /// Ensure keyboard focus never dangles after an `active_tab`/`active_session`
    /// change: focuses the active session's terminal when the SSH tab is showing a
    /// live session, else `root_focus` — see that field's doc comment for why this
    /// matters (a stale focus target silently kills all further keyboard dispatch).
    /// Called by every path that mutates either field.
    fn refocus_stable_target(&self, window: &mut Window, cx: &Context<Self>) {
        if self.active_tab == Tab::Ssh
            && let Some(ix) = self.active_session
            && let Some(tab) = self.ssh_sessions.get(ix)
        {
            window.focus(&tab.session.read(cx).terminal_focus_handle());
        } else {
            window.focus(&self.root_focus);
        }
    }

    /// Routes every [`SshSessionEvent`] a live session fires. Currently just the `⇄
    /// dock` toggle; `SshSession` never touches `Store` itself (see that event's doc
    /// comment), so persisting the flip and fanning it out to every other open tab is
    /// this method's job.
    fn on_session_event(
        &mut self,
        _session: Entity<SshSession>,
        event: &SshSessionEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            SshSessionEvent::ToggleDockSide => self.toggle_dock_side(cx),
        }
    }

    /// Flip `file_browser_side`, persist it to `Settings`, and push it to every live
    /// session so all open tabs stay in sync with the one (global) setting — not just
    /// the tab whose header was clicked.
    fn toggle_dock_side(&mut self, cx: &mut Context<Self>) {
        self.file_browser_side = match self.file_browser_side {
            PanelSide::Left => PanelSide::Right,
            PanelSide::Right => PanelSide::Left,
        };
        if let Ok(mut settings) = self.store.settings() {
            settings.file_browser_side = self.file_browser_side;
            let _ = self.store.set_settings(&settings);
        }
        let side = self.file_browser_side;
        for tab in &self.ssh_sessions {
            tab.session
                .update(cx, |session, cx| session.set_dock_side(side, cx));
        }
        cx.notify();
    }

    fn open_form(&mut self, form: Entity<HostForm>, window: &mut Window, cx: &mut Context<Self>) {
        form.read(cx).focus_first(window, cx);
        // `subscribe_in` (not `subscribe`) so `on_form_event` gets a `&mut Window` —
        // needed to refocus `root_focus` on close (see that field's doc comment: a
        // form dismissed via Escape leaves its now-dropped field's `FocusHandle` as
        // the window's stale focus target, which silently breaks all further keyboard
        // dispatch otherwise).
        self._form_subscription = Some(cx.subscribe_in(&form, window, Self::on_form_event));
        self.form = Some(form);
        cx.notify();
    }

    fn close_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.form = None;
        self._form_subscription = None;
        window.focus(&self.root_focus);
        cx.notify();
    }

    fn on_form_event(
        &mut self,
        form: &Entity<HostForm>,
        event: &HostFormEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            HostFormEvent::Cancel => self.close_form(window, cx),
            HostFormEvent::Submit(submission) => match self.perform_submit(submission) {
                Ok(post_warning) => {
                    self.close_form(window, cx);
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

    // ---- connect-time password prompt (round-D §A.4) --------------------------

    /// Open the connect-time password prompt: `label` names what it's for ("password
    /// for {label}"), `pending` says what submitting it resumes. Mirrors
    /// `open_form`/`open_db_form`'s subscribe-then-store shape exactly, including the
    /// `subscribe_in` (not `subscribe`) choice — `on_password_prompt_event` needs a
    /// `&mut Window` to refocus `root_focus` on close (see that field's doc comment).
    /// `pub(crate)` so `ui::db_tab`'s `AppState::run_query`/`refresh_schema` can open it
    /// the same way `connect_host` does.
    pub(crate) fn open_password_prompt(
        &mut self,
        label: SharedString,
        pending: PendingSecretPrompt,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let modal = cx.new(|cx| PasswordPromptModal::new(cx, label));
        modal.read(cx).focus_first(window, cx);
        self._password_prompt_subscription =
            Some(cx.subscribe_in(&modal, window, Self::on_password_prompt_event));
        self.password_prompt = Some(modal);
        self.pending_secret_prompt = Some(pending);
        cx.notify();
    }

    fn close_password_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.password_prompt = None;
        self._password_prompt_subscription = None;
        self.pending_secret_prompt = None;
        window.focus(&self.root_focus);
        cx.notify();
    }

    /// `Cancel` leaves the triggering connect/query attempt failed — nothing retries on
    /// its own. `Submit` puts the password under the pending action's `secret_ref`
    /// (SSH: only if the host already had one; DB: always, per
    /// `PendingSecretPrompt::Db`'s doc comment) and resumes whatever was waiting.
    /// Plaintext only ever goes two places from here: `secrets.put` and the immediate
    /// retried connect/query attempt — never logged, never written to config.
    fn on_password_prompt_event(
        &mut self,
        _modal: &Entity<PasswordPromptModal>,
        event: &PasswordPromptEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let PasswordPromptEvent::Submit(password) = event else {
            self.close_password_prompt(window, cx);
            return;
        };
        let password = password.clone();
        let Some(pending) = self.pending_secret_prompt.take() else {
            self.close_password_prompt(window, cx);
            return;
        };
        self.close_password_prompt(window, cx);
        match pending {
            PendingSecretPrompt::Ssh { host, source } => {
                if let Some(secret_ref) = host.secret_ref.clone() {
                    let _ = self
                        .secrets
                        .put(&SecretId::new(secret_ref), password.as_bytes());
                }
                self.finish_connect(host, source, Ok(Some(password.into_bytes())), cx);
            }
            PendingSecretPrompt::Db { secret_ref, retry } => {
                let _ = self
                    .secrets
                    .put(&SecretId::new(secret_ref), password.as_bytes());
                match retry {
                    crate::ui::db_tab::DbRetry::RunQuery => self.run_query(window, cx),
                    crate::ui::db_tab::DbRetry::RefreshSchema => self.refresh_schema(window, cx),
                }
            }
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
            Scope::Global => "global".into(),
            Scope::Workspace(_) => self
                .scopes
                .iter()
                .find(|c| c.scope == *target)
                .map(|c| c.label.to_string())
                .unwrap_or_else(|| "workspace".into()),
        }
    }

    /// Badge label + color for an item's origin layer.
    ///
    /// Global uses the theme's `accent`; workspace uses `success` — the theme has no
    /// second accent tone, and `success` reads as a clearly distinct hue from `accent`
    /// in every built-in (cosmos pairs a pale cyan-blue against the red accent, an echo
    /// of the pre-sweep brand blue).
    pub(crate) fn origin_badge(
        &self,
        a: &Attributed<Host>,
        cx: &Context<Self>,
    ) -> (SharedString, u32) {
        let t = theme::active(cx);
        // `global` is the common, unremarkable origin — muted, not accent: the badge
        // is orientation, not a call to action. Workspace origins (and `· dup`
        // shadowing) are the notable cases and keep a color.
        let (mut label, color): (SharedString, u32) = match &a.origin {
            Scope::Global => ("global".into(), t.faint),
            Scope::Workspace(id) => {
                let name = self
                    .scopes
                    .iter()
                    .find(|c| matches!(&c.scope, Scope::Workspace(w) if w == id))
                    .map(|c| c.label.clone())
                    .unwrap_or_else(|| "workspace".into());
                (name, t.success)
            }
        };
        if a.duplicate {
            label = format!("{label} · dup").into();
        }
        (label, color)
    }

    // ---- keyboard-driven system (2026-07-02 plan) -----------------------------

    /// Whether a modal that should own the keyboard exclusively is open (the host or DB
    /// connection form, the connect-time password prompt). The root key dispatcher
    /// stays out of the way entirely while one of these is up — `ui::command_palette`'s
    /// `toggle_palette` already declines to open *over* one for the same reason.
    pub(crate) fn blocking_modal_open(&self) -> bool {
        self.form.is_some() || self.db.form.is_some() || self.password_prompt.is_some()
    }

    /// Whether the active SSH session's terminal currently holds keyboard focus — the
    /// one axis [`FocusContext`] is gated on (see that type's doc comment in `keymap.rs`
    /// for why: a focused terminal needs first dibs on `Ctrl+<letter>`).
    fn focus_context(&self, window: &mut Window, cx: &mut Context<Self>) -> FocusContext {
        let terminal_focused = self
            .active_session
            .and_then(|ix| self.ssh_sessions.get(ix))
            .is_some_and(|tab| {
                tab.session
                    .read(cx)
                    .terminal_focus_handle()
                    .is_focused(window)
            });
        if terminal_focused {
            FocusContext::Terminal
        } else {
            FocusContext::Normal
        }
    }

    /// The root-level key handler, registered with `.capture_key_down` on the outermost
    /// element (see `Render::render` below) so it sees every keystroke **before** any
    /// descendant — the terminal included — gets a chance at it. It only
    /// `cx.stop_propagation()`s the keystrokes it actually claims; everything else
    /// (including, deliberately, plain `Ctrl+<letter>` while a terminal is focused)
    /// falls through untouched to whatever's actually focused.
    fn handle_root_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.blocking_modal_open() {
            return;
        }

        let key = event.keystroke.key.as_str();
        let m = &event.keystroke.modifiers;
        let plain_ctrl = m.control && !m.alt && !m.shift && !m.platform;

        // While the palette is open, it claims its own navigation keys outright. These
        // aren't `keymap` registry entries (they're palette-internal, not global
        // actions reachable any other way), so they're special-cased here rather than
        // resolved below.
        if self.palette.is_some() {
            match key {
                "escape" => {
                    cx.stop_propagation();
                    self.close_palette(cx);
                    return;
                }
                "enter" => {
                    cx.stop_propagation();
                    self.palette_confirm(window, cx);
                    return;
                }
                "up" => {
                    cx.stop_propagation();
                    self.palette_move_selection(-1, cx);
                    return;
                }
                "down" => {
                    cx.stop_propagation();
                    self.palette_move_selection(1, cx);
                    return;
                }
                "n" if plain_ctrl => {
                    cx.stop_propagation();
                    self.palette_move_selection(1, cx);
                    return;
                }
                "p" if plain_ctrl => {
                    cx.stop_propagation();
                    self.palette_move_selection(-1, cx);
                    return;
                }
                _ => {}
            }
        }

        if self.cheat_sheet_open && key == "escape" {
            cx.stop_propagation();
            self.cheat_sheet_open = false;
            cx.notify();
            return;
        }

        let focus = self.focus_context(window, cx);
        let Some(action) = keymap::resolve(&event.keystroke, focus, &keymap::default_bindings())
        else {
            return;
        };

        // The one rule `keymap::resolve`'s pure `(Keystroke, FocusContext)` lookup can't
        // express on its own: the bare `?` cheat-sheet binding must never steal a
        // literal `?` from whatever text field currently has focus. Every text-entry
        // widget in this app calls `track_focus`, so "nothing at all is focused" is a
        // safe, generic proxy for "you're not mid-typing somewhere" — it never swallows
        // a real keystroke; the only cost is the cheat sheet occasionally staying closed
        // when some non-text focus holder (e.g. a keyboard-navigable list) has focus.
        if action == Action::CheatSheet && window.focused(cx).is_some() {
            return;
        }

        cx.stop_propagation();
        self.dispatch_action(action, window, cx);
    }

    /// Route a resolved [`Action`] to whatever it does. `handle_root_key_down` above and
    /// the palette's `Enter` confirm (`ui::command_palette::palette_confirm`) are the
    /// only two callers.
    pub(crate) fn dispatch_action(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            Action::CommandPalette => self.toggle_palette(window, cx),
            Action::PrimaryTab(n) => {
                if let Some(ix) = (n as usize).checked_sub(1)
                    && let Some(&tab) = Tab::ALL.get(ix)
                {
                    self.active_tab = tab;
                    self.close_palette(cx);
                    self.refocus_stable_target(window, cx);
                    cx.notify();
                }
            }
            Action::CycleTabForward => self.cycle_tabs(false, window, cx),
            Action::CycleTabBack => self.cycle_tabs(true, window, cx),
            Action::CycleSessionForward => self.cycle_sessions(false, window, cx),
            Action::CycleSessionBack => self.cycle_sessions(true, window, cx),
            Action::NewSession => {
                if self.active_tab == Tab::Ssh {
                    self.new_session(window, cx);
                }
            }
            Action::CloseSession => {
                if self.active_tab == Tab::Ssh
                    && let Some(ix) = self.active_session
                {
                    self.close_session(ix, window, cx);
                }
            }
            Action::Settings => {
                // Round-E §C: a real Settings screen now lives at `Tab::Settings`
                // (Theme/Behavior/Keyboard/Storage) — Settings -> Keymap rebinding
                // itself stays deferred, per the plan; everything else here is live.
                self.active_tab = Tab::Settings;
                self.close_palette(cx);
                self.refocus_stable_target(window, cx);
                cx.notify();
            }
            Action::CheatSheet => {
                self.cheat_sheet_open = !self.cheat_sheet_open;
                self.close_palette(cx);
                cx.notify();
            }
            Action::FocusFilter => {
                // Only the Network tab has a filter input wired up so far — other tabs
                // simply don't have one yet, so this is a no-op there (per the plan's
                // "other tabs: no-op for now").
                if self.active_tab == Tab::Network {
                    self.network.focus_filter(window, cx);
                    cx.notify();
                }
            }
        }
    }

    /// `Ctrl+Tab`/`Ctrl+Shift+Tab`: cycle the primary tabs — unconditionally. This used
    /// to switch to cycling *session* tabs whenever the SSH tab was active, which
    /// trapped a primary-tab cycle the moment it landed on SSH (Murphy: "it lags and
    /// then wont continue"). Session tabs now cycle on their own chords — see
    /// [`Self::cycle_sessions`].
    fn cycle_tabs(&mut self, backwards: bool, window: &mut Window, cx: &mut Context<Self>) {
        let len = Tab::ALL.len();
        let current = Tab::ALL
            .iter()
            .position(|&t| t == self.active_tab)
            .unwrap_or(0);
        self.active_tab = Tab::ALL[cycle_index(current, len, backwards)];
        self.refocus_stable_target(window, cx);
        cx.notify();
    }

    /// `Ctrl+PgDn`/`Ctrl+PgUp`: cycle SSH session tabs (Home is its own stop). A no-op
    /// on every other primary tab — the chord is about sessions, not a second way to
    /// leave the tab you're on. Ends by restoring keyboard focus onto whatever's now
    /// active (see `activate_session`/`go_home`) so keyboard-only cycling never
    /// dead-ends on a dangling focus.
    fn cycle_sessions(&mut self, backwards: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_tab != Tab::Ssh {
            return;
        }
        match cycle_session_index(self.active_session, self.ssh_sessions.len(), backwards) {
            Some(ix) => self.activate_session(ix, window, cx),
            None => self.go_home(window, cx),
        }
    }

    /// The `?` cheat-sheet overlay: one row per [`keymap::Action`] naming its default
    /// shortcut. Same `deferred`/`anchored` backdrop pattern as every other overlay
    /// here.
    fn cheat_sheet_overlay(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        if !self.cheat_sheet_open {
            return None;
        }
        let t = theme::active(cx);
        let (fg, accent, surface, border, muted, selection) =
            (t.fg, t.accent, t.surface, t.border, t.muted, t.selection);
        let bindings = keymap::default_bindings();
        let viewport = window.viewport_size();
        let rows: Vec<_> = keymap::ALL_ACTIONS
            .iter()
            .map(|&action| {
                let shortcut = keymap::primary_shortcut(action, &bindings).unwrap_or_default();
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .px_3()
                    .py_2()
                    .child(div().text_sm().text_color(rgb(fg)).child(action.label()))
                    .child(div().text_sm().text_color(rgb(accent)).child(shortcut))
            })
            .collect();

        Some(
            deferred(
                anchored().position(point(px(0.), px(0.))).child(
                    div()
                        .id("cheat-sheet-backdrop")
                        .occlude()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(viewport.width)
                        .h(viewport.height)
                        .bg(rgba(0x000000a8))
                        .child(
                            div()
                                .w(px(420.))
                                .flex()
                                .flex_col()
                                .bg(rgb(surface))
                                .border_1()
                                .border_color(rgb(border))
                                .rounded_md()
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .justify_between()
                                        .px_3()
                                        .py_2()
                                        .border_b_1()
                                        .border_color(rgb(border))
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(rgb(fg))
                                                .child("Keyboard Shortcuts"),
                                        )
                                        .child(
                                            div()
                                                .id("cheat-sheet-close")
                                                .px_2()
                                                .py_1()
                                                .rounded_md()
                                                .cursor_pointer()
                                                .text_color(rgb(muted))
                                                .hover(|s| s.bg(rgb(selection)))
                                                .child("✕ close")
                                                .on_click(cx.listener(
                                                    |this, _ev: &ClickEvent, _window, cx| {
                                                        this.cheat_sheet_open = false;
                                                        cx.notify();
                                                    },
                                                )),
                                        ),
                                )
                                .child(div().flex().flex_col().py_1().children(rows)),
                        ),
                ),
            )
            .with_priority(2),
        )
    }

    // ---- rendering helpers --------------------------------------------------

    /// The single top chrome bar: `✦ sid` wordmark, the primary tabs, then (right-
    /// aligned) the scope switcher chips and the secrets warning badge. One bar, not
    /// the previous two stacked ones — a whole row of chrome bought nothing but
    /// vertical clutter, and scope-switching is an occasional act that belongs at the
    /// edge, not on its own strip above everything.
    fn tab_strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::active(cx);
        let (surface, border, accent, muted, fg_strong, selection) = (
            t.surface,
            t.border,
            t.accent,
            t.muted,
            t.fg_strong,
            t.selection,
        );

        let active = self.active_tab;
        let tabs: Vec<_> = Tab::ALL
            .iter()
            .enumerate()
            .map(|(ix, &tab)| {
                let is_active = tab == active;
                div()
                    .id(("tab", ix))
                    .px_3()
                    .h_full()
                    .flex()
                    .items_center()
                    .text_sm()
                    .cursor_pointer()
                    .text_color(rgb(if is_active { fg_strong } else { muted }))
                    .border_b_2()
                    .border_color(rgb(if is_active { accent } else { surface }))
                    .child(tab.label())
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                        this.active_tab = tab;
                        // Mouse-driven tab switches need the same refocus as the
                        // keyboard path (`dispatch_action`'s `PrimaryTab` arm) — see
                        // `root_focus`'s doc comment: leaving a tab that had something
                        // focused (the SSH terminal, a DB-tab input, ...) without
                        // claiming a new, currently-rendered focus target silently
                        // breaks every keyboard shortcut until the next mouse click.
                        this.refocus_stable_target(window, cx);
                        cx.notify();
                    }))
            })
            .collect();

        let current = self.scope.clone();
        let scope_chips: Vec<_> = self
            .scopes
            .iter()
            .enumerate()
            .map(|(ix, choice)| {
                let is_active = choice.scope == current;
                let target = choice.scope.clone();
                div()
                    .id(("scope", ix))
                    .px_2()
                    .py(px(3.))
                    .rounded_md()
                    .text_xs()
                    .cursor_pointer()
                    .bg(rgb(if is_active { selection } else { surface }))
                    .text_color(rgb(if is_active { fg_strong } else { muted }))
                    .hover(|s| s.bg(rgb(selection)))
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
            .w_full()
            .h(px(42.))
            .px_3()
            .gap_1()
            .bg(rgb(surface))
            .border_b_1()
            .border_color(rgb(border))
            .child(
                div()
                    .pr_2()
                    .text_color(rgb(accent))
                    .font_weight(FontWeight::BOLD)
                    .child("✦ sid"),
            )
            .children(tabs)
            .child(div().flex_1()) // spacer — scope chips + badge live at the right edge
            .children(scope_chips)
            .children(self.secret_status_badge(cx))
    }

    /// Round-D §A's warning badge: a small yellow `!` pill at the primary tab strip's
    /// right end, rendered only while `secrets_degraded` is true (memory fallback — a
    /// healthy keyring renders nothing at all, here or anywhere else). Replaces the old
    /// persistent "secrets: …" status line: nothing takes up permanent screen space
    /// unless something is actually degraded. Click toggles a small popover — anchored
    /// at the badge's own flow position (`Corner::TopRight`, same trigger-attached
    /// pattern as `db_tab`'s export menu), not a full-viewport modal — showing the full
    /// `secret_status_message` text (backend, warning, recommendation).
    fn secret_status_badge(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        if !self.secrets_degraded {
            return None;
        }
        let t = theme::active(cx);
        let (warning, border, surface, fg) = (t.warning, t.border, t.surface, t.fg);
        let badge = div()
            .id("secret-status-badge")
            .px_2()
            .py(px(2.))
            .rounded_full()
            .text_xs()
            .font_weight(FontWeight::BOLD)
            .cursor_pointer()
            .bg(rgb(warning))
            // Deliberately not a theme token: a near-black label reads clearly against
            // every theme's `warning` tone (they're all mid-brightness ambers), which a
            // theme-following text color could not guarantee (e.g. cosmos-light's `bg`
            // is a light off-white — unreadable on the same amber pill).
            .text_color(rgb(0x1a1a1a))
            .child("!")
            .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| {
                this.secret_badge_open = !this.secret_badge_open;
                cx.notify();
            }));

        let popover = self.secret_badge_open.then(|| {
            deferred(
                anchored()
                    .anchor(Corner::TopRight)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        div()
                            .id("secret-status-popover")
                            .occlude()
                            .mt_1()
                            .max_w(px(360.))
                            .p_3()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(border))
                            .bg(rgb(surface))
                            .text_xs()
                            .text_color(rgb(fg))
                            .child(self.secrets_status_detail.clone()),
                    ),
            )
            .with_priority(2)
        });

        Some(
            div()
                .flex()
                .flex_row()
                .items_center()
                .child(badge)
                .children(popover),
        )
    }

    /// The SSH tab, top to bottom: the session tab strip (home · one tab per live
    /// session · +), then — on Home — the full-width status/error bar (see
    /// `ssh_status_bar`) above the single connections surface
    /// (`ui::ssh_home::AppState::ssh_home_main`), or the active session's view (status
    /// strip + that `SshSession` entity, which paints its own terminal/file-browser
    /// split).
    ///
    /// Home used to be a [tree sidebar | host-card list] split showing the SAME hosts
    /// twice, with two vocabularies ("+ Add connection" vs "+ Add host") and two
    /// different action sets. One list, one vocabulary, one add button — the
    /// design-review fix for "the ssh tab feels very redundant".
    fn ssh_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .child(self.session_tab_strip(cx))
            .child(match self.active_session {
                None => div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.))
                    .children(self.ssh_status_bar(cx))
                    .child(self.ssh_home_main(cx).into_any_element())
                    .into_any_element(),
                Some(ix) => self.ssh_session_view(ix, cx).into_any_element(),
            })
    }

    /// The SSH Home tab's error notice: a **full-width bar** between the session tab
    /// strip and the connections surface. `.truncate()` (clip + ellipsis, not wrap)
    /// keeps it to one line no matter how long the message is. `None` — the common
    /// case with no error — renders nothing.
    ///
    /// Round-D §A dropped the startup secrets-backend notice this bar used to double as
    /// (a persistent "secrets: …" line) — a degraded backend now shows as the small
    /// warning badge at the tab strip's right end instead (see `secret_status_badge`).
    fn ssh_status_bar(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        let e = self.error.as_ref()?;
        let t = theme::active(cx);
        let (border, surface, danger) = (t.border, t.surface, t.danger);
        let text: SharedString = format!("error: {e}").into();
        Some(
            div()
                .w_full()
                .px_4()
                .py_1()
                .border_b_1()
                .border_color(rgb(border))
                .bg(rgb(surface))
                .text_xs()
                .text_color(rgb(danger))
                .truncate()
                .child(text),
        )
    }

    /// The session tab strip (ssh-v3): `home` (leftmost, always goes Home) ·
    /// one `● user@host ×` tab per live session (click activates, `×` disconnects +
    /// closes) · `+` (also goes Home, ready for a new connection).
    fn session_tab_strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::active(cx);
        let (bg, surface, border, muted, fg_strong, selection, success, warning, danger) = (
            t.bg,
            t.surface,
            t.border,
            t.muted,
            t.fg_strong,
            t.selection,
            t.success,
            t.warning,
            t.danger,
        );
        let home_selected = self.active_session.is_none();
        let home = div()
            .id("ssh-tab-home")
            .px_2()
            .h(px(30.))
            .text_xs()
            .font_family(MONO)
            .flex()
            .items_center()
            .justify_center()
            .rounded_t_md()
            .cursor_pointer()
            .text_color(rgb(if home_selected { fg_strong } else { muted }))
            .bg(rgb(if home_selected { bg } else { surface }))
            .border_1()
            .border_color(rgb(border))
            .child("home")
            .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| this.go_home(window, cx)));

        let tabs: Vec<_> = self
            .ssh_sessions
            .iter()
            .enumerate()
            .map(|(ix, tab)| {
                let selected = self.active_session == Some(ix);
                let dot_color = match tab.session.read(cx).status() {
                    SessionStatus::Connected => success,
                    SessionStatus::Connecting => warning,
                    SessionStatus::Failed(_) | SessionStatus::Closed => danger,
                };
                let dot = div().text_color(rgb(dot_color)).child("●");
                div()
                    .id(("ssh-session-tab", ix))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .h(px(30.))
                    .rounded_t_md()
                    .bg(rgb(if selected { bg } else { surface }))
                    .text_color(rgb(if selected { fg_strong } else { muted }))
                    .border_1()
                    .border_color(rgb(border))
                    .child(
                        div()
                            .id(("ssh-session-tab-label", ix))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .text_xs()
                            .font_family(MONO)
                            .cursor_pointer()
                            .child(dot)
                            .child(tab.label.clone())
                            .on_click(cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                                this.activate_session(ix, window, cx);
                            })),
                    )
                    .child(
                        div()
                            .id(("ssh-session-tab-close", ix))
                            .px_1()
                            .rounded_md()
                            .text_xs()
                            .cursor_pointer()
                            .text_color(rgb(muted))
                            .hover(|s| s.bg(rgb(selection)).text_color(rgb(danger)))
                            .child("×")
                            .on_click(cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                                this.close_session(ix, window, cx);
                            })),
                    )
            })
            .collect();

        let add = div()
            .id("ssh-tab-add")
            .w(px(30.))
            .h(px(30.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_t_md()
            .cursor_pointer()
            .text_color(rgb(muted))
            .hover(|s| s.bg(rgb(selection)))
            .child("+")
            .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                // Already on Home: `new_session`/`go_home` would be a no-op with no
                // visible effect (this *was* the "tab-strip + does nothing" bug) — the
                // only meaningful next step from there is opening the add-connection
                // form. Coming from a live session tab, + still just goes Home first
                // (mirrors the mockup's "New tab (opens Home)"), ready to add or pick a
                // connection from there.
                if this.active_session.is_none() {
                    this.open_add_form(window, cx);
                } else {
                    this.new_session(window, cx);
                }
            }));

        div()
            .flex()
            .flex_row()
            .items_end()
            .gap_1()
            .px_2()
            .pt_1()
            .bg(rgb(surface))
            .border_b_1()
            .border_color(rgb(border))
            .child(home)
            .children(tabs)
            .child(add)
    }

    /// A session tab's view: a `← close tab` strip showing `user@host · status` above
    /// the [`SshSession`] entity, which paints its own connecting/failed/closed/split
    /// (terminal + file panel, docked per `file_browser_side`) states. `ix` must be a
    /// valid `ssh_sessions` index — the only caller (`ssh_tab`) only reaches this arm
    /// when `active_session == Some(ix)`, an invariant `activate_session`/
    /// `close_session` both maintain.
    fn ssh_session_view(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::active(cx);
        let (border, muted, selection, fg_strong) = (t.border, t.muted, t.selection, t.fg_strong);
        let tab = &self.ssh_sessions[ix];
        let session = tab.session.clone();
        let label = tab.label.clone();
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
                    .border_color(rgb(border))
                    .child(
                        div()
                            .id("session-disconnect")
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .text_sm()
                            .cursor_pointer()
                            .bg(rgb(selection))
                            .text_color(rgb(fg_strong))
                            .child("← close tab")
                            .on_click(cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                                this.close_session(ix, window, cx);
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(rgb(muted))
                            .child(header),
                    ),
            )
            .child(div().flex().flex_col().flex_1().child(session))
    }

    fn placeholder(&self, tab: Tab, cx: &Context<Self>) -> impl IntoElement {
        let muted = theme::active(cx).muted;
        div()
            .flex()
            .flex_1()
            .items_center()
            .justify_center()
            .text_color(rgb(muted))
            .child(format!("{} — coming next", tab.label()))
    }
}

impl Render for AppState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // SID_PERF=1: log any root render over 4ms (a 240Hz frame) to stderr, tagged
        // with the active tab — the cheap, always-available answer to "which screen
        // stutters". Element *build* time only; gpui's layout/paint happens after.
        let perf_start = std::env::var_os("SID_PERF").map(|_| std::time::Instant::now());

        let content = match self.active_tab {
            Tab::Ssh => self.ssh_tab(cx).into_any_element(),
            // `db_tab` needs `window` (W5) to lazily build the SQL editor/results table
            // on first paint — `InputState::new`/`TableState::new` both require it.
            Tab::Database => self.db_tab(window, cx),
            // `network_tab` needs `window` for the same reason (`TableState::new`).
            Tab::Network => self.network_tab(window, cx),
            // `systems_tab` needs `window` for the same reason (`TableState::new`).
            Tab::System => self.systems_tab(window, cx),
            // `settings_tab` needs no lazy widget construction, so it takes `&self`
            // rather than `&mut self` like the tabs above.
            Tab::Settings => self.settings_tab(cx),
            other => self.placeholder(other, cx).into_any_element(),
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
        // The connect-time password prompt — the exact mirror of `overlay` above, over
        // `self.password_prompt` instead of `self.form`.
        let password_prompt_overlay = self.password_prompt.clone().map(|modal| {
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
                        .child(modal),
                ),
            )
            .with_priority(1)
        });

        // Keyboard-driven system (2026-07-02 plan): the palette + cheat-sheet overlays,
        // and the root-level key handler that opens/dispatches them. `capture_key_down`
        // runs *before* any descendant (the terminal included) sees the keystroke — see
        // `handle_root_key_down`'s doc comment for why that ordering is load-bearing.
        let palette_overlay = self.palette_overlay(window, cx);
        let cheat_sheet_overlay = self.cheat_sheet_overlay(window, cx);

        let t = theme::active(cx);
        let (bg, fg) = (t.bg, t.fg);
        let root = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(bg))
            .text_color(rgb(fg))
            .track_focus(&self.root_focus)
            .capture_key_down(cx.listener(Self::handle_root_key_down))
            .child(self.tab_strip(cx))
            .child(div().flex().flex_col().flex_1().child(content))
            .children(overlay)
            .children(db_overlay)
            .children(password_prompt_overlay)
            .children(palette_overlay)
            .children(cheat_sheet_overlay);

        if let Some(start) = perf_start {
            let elapsed = start.elapsed();
            if elapsed.as_millis() >= 4 {
                eprintln!(
                    "sid-perf: render(build) {:?} took {}ms",
                    self.active_tab,
                    elapsed.as_millis()
                );
            }
        }
        root
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
/// composition is visible immediately. Also returns the post-seed hosts/workspaces
/// lists `seed_if_empty` read while doing so — see [`SeedLists`] and
/// [`AppState::apply_seed_lists`] (perf audit finding #7).
pub fn open_store() -> (Store, SeedLists) {
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    // Distinct filename from the archived TUI POC's `sid.redb` (incompatible schema at the
    // same machine-global path) so the rebuild starts from a clean store.
    let store = Store::open(&dir.join("store.redb")).expect("open sid store");
    let seed_lists = seed_if_empty(&store, &dir);
    (store, seed_lists)
}

/// The hosts + workspaces lists `seed_if_empty` reads while checking whether the global
/// store needs first-launch seeding — threaded back through `open_store` so
/// `AppState::new` doesn't immediately re-read the same two tables (perf audit finding
/// #7). Errors are converted to `String` (the same `e.to_string()`
/// `reload_scopes`/`refresh` already do) since nothing downstream needs the original
/// `StoreError`.
///
/// **Regression trap** (see `docs/design/2026-07-02-perf-audit.md` finding #7): these
/// must be the lists AFTER any seeding `seed_if_empty` performs — never its pre-seed
/// emptiness-check reads. On a first launch `list_hosts()`/`list_workspaces()` start
/// empty and `seed_if_empty` then WRITES the demo rows; returning that pre-write
/// snapshot would show a first-launch user an empty host list despite demo data
/// landing on disk (`seed_if_empty`'s own tests cover both cases).
///
/// Deliberately does *not* carry a `connections` list — `DbTabState::new` (in
/// `ui::db_tab`, out of this change's scope) still re-reads that table itself, so
/// there'd be nothing to consume a third field.
pub struct SeedLists {
    pub(crate) hosts: Result<Vec<Host>, String>,
    pub(crate) workspaces: Result<Vec<WorkspaceMeta>, String>,
}

/// Resolve and open the effective secret backend from the persisted
/// [`sid_store::Settings::secret_keyring_enabled`] toggle via
/// [`sid_secrets::resolve_secret_store`]: keyring (if enabled & the startup probe
/// passes), else memory (round-D §A — the encrypted-file backend is no longer a
/// candidate; `Settings::secret_file_enabled` is dormant, see its doc comment).
///
/// Returns the store every secret call site uses, whether the effective backend is
/// degraded (memory — feeds `AppState::secrets_degraded`, which gates the tab strip's
/// warning badge), and the full status text for that badge's popover: which backend is
/// live, plus any warning/recommendation.
pub fn open_secrets(store: &Store) -> (Box<dyn sid_secrets::SecretStore>, bool, String) {
    let settings = store.settings().unwrap_or_default();
    let toggles = sid_secrets::SecretBackendToggles {
        keyring_enabled: settings.secret_keyring_enabled,
    };
    let resolved = sid_secrets::resolve_secret_store(toggles, sid_secrets::probe_keyring);

    let (label, degraded) = match resolved.effective {
        sid_secrets::BackendKind::Keyring => ("OS keyring".to_string(), false),
        sid_secrets::BackendKind::Memory => ("in-memory (no persistence)".to_string(), true),
    };
    let message = secret_status_message(
        &label,
        resolved.warning.as_deref(),
        resolved.recommendation.as_deref(),
    );
    (resolved.store, degraded, message)
}

/// Compose the startup status line for the resolved secret backend: which backend is
/// live, plus any warning/recommendation from `resolve_secret_store`. Pure so the
/// wording is unit-tested without touching a real keyring or vault file.
pub(crate) fn secret_status_message(
    effective: &str,
    warning: Option<&str>,
    recommendation: Option<&str>,
) -> String {
    let mut msg = format!("secrets: {effective}");
    if let Some(w) = warning {
        msg.push_str(&format!(" — {w}"));
    }
    if let Some(r) = recommendation {
        msg.push_str(&format!(" ({r})"));
    }
    msg
}

/// Seed a small demo dataset into `store` on first run (see the module-level doc on
/// `open_store`), and return the post-seed hosts/workspaces lists — see [`SeedLists`]'s
/// doc comment for the regression trap this guards against.
///
/// The two initial reads below (`hosts_before`/`workspaces_before`) double as both the
/// emptiness gate (unchanged from before this function returned anything) AND, in the
/// common already-populated-store case, the returned lists themselves — nothing
/// changed, so there is nothing to re-read. Only the (rare, first-launch-only) branch
/// that actually writes seed rows re-reads those two tables, to fulfil the "post-seed"
/// contract; the already-populated case pays zero extra reads.
fn seed_if_empty(store: &Store, dir: &std::path::Path) -> SeedLists {
    let hosts_before = store.global().list_hosts();
    let no_hosts = hosts_before.as_ref().map(|h| h.is_empty()).unwrap_or(false);
    let workspaces_before = store.global().list_workspaces();
    let no_ws = workspaces_before
        .as_ref()
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
        // The demo connection's file must exist before `run_query` (W5) opens it (saved
        // connections open `SqliteMode::OpenExisting`). Seed it with a small FK-rich
        // sample schema (via `sid_db`, keeping rusqlite behind the adapter) so the DB tab
        // is immediately explorable — schema tree, relationships diagram, and a first
        // `SELECT` all have content — instead of an empty, blank-looking file. Best-effort:
        // fall back to a bare (valid, empty) SQLite file if seeding fails.
        let demo_db = dir.join("demo.db");
        if sid_db::demo::seed_demo_sqlite(&demo_db).is_err() {
            let _ = std::fs::File::create(&demo_db);
        }
        let _ = store.write_connection(
            &sid_store::DbConnection {
                id: "demo-sqlite".into(),
                dsn: demo_db.to_string_lossy().into_owned(),
                secret_ref: None,
                kind: sid_core::db::DbKind::Sqlite,
                name: "demo sqlite (local file)".into(),
                folder: None,
            },
            &Scope::Global,
        );
    }

    if !(no_hosts && no_ws) {
        return SeedLists {
            hosts: hosts_before.map_err(|e| e.to_string()),
            workspaces: workspaces_before.map_err(|e| e.to_string()),
        };
    }

    let global = |alias: &str, user: &str, host: &str| Host {
        alias: alias.into(),
        user: user.into(),
        host: host.into(),
        port: 22,
        secret_ref: None,
        auth: AuthMethod::default(),
        folder: None,
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

    // Regression trap (see `SeedLists`'s doc comment): `hosts_before`/`workspaces_before`
    // are now stale — they were read before the writes above landed. Re-read so the
    // caller gets the lists INCLUDING the rows just seeded, not the pre-seed snapshot.
    SeedLists {
        hosts: store.global().list_hosts().map_err(|e| e.to_string()),
        workspaces: store.global().list_workspaces().map_err(|e| e.to_string()),
    }
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

/// The new `active_session` after closing the tab at `closed_ix` (ssh-v3's session tab
/// strip). Unlike the mockup's JS (which tracks the active tab by a stable key and so
/// never needs to renumber it), `active_session` is a plain `Vec` index, so closing a
/// tab **before** the active one must shift the active index down by one to keep
/// pointing at the same still-open session — closing the active tab itself lands on the
/// tab now at `max(0, closed_ix - 1)` (mirrors the mockup's `order[Math.max(0, ix-1)]`),
/// or `None` (home) if that was the last tab; closing a tab **after** the active one, or
/// while on home (`active == None`), leaves it untouched. `len_after` is
/// `ssh_sessions.len()` **after** the removal (what the caller naturally has on hand).
pub(crate) fn next_active_after_close(
    active: Option<usize>,
    closed_ix: usize,
    len_after: usize,
) -> Option<usize> {
    let a = active?;
    if a == closed_ix {
        if len_after == 0 {
            None
        } else {
            Some(closed_ix.saturating_sub(1).min(len_after - 1))
        }
    } else if a > closed_ix {
        Some(a - 1)
    } else {
        Some(a)
    }
}

/// Wrap-around index cycling over `len` items (`Ctrl+Tab`/`Ctrl+Shift+Tab` on primary
/// tabs). Same algorithm as `ui::text_input::next_focus_index` (the Tab/Shift+Tab form
/// field cycler) — kept as its own tiny pure function here rather than reaching across
/// the `ui` module's privacy boundary for a two-line formula.
/// The scope switcher's choices for a workspace list: Global first, then one chip per
/// registered workspace. Pure — shared by startup (`apply_seed_lists`) and the
/// Workspaces tab's runtime rebuild (`reload_scopes_runtime`).
pub(crate) fn build_scope_choices(workspaces: Vec<WorkspaceMeta>) -> Vec<ScopeChoice> {
    let mut scopes = vec![ScopeChoice {
        label: "Global".into(),
        scope: Scope::Global,
    }];
    for w in workspaces {
        scopes.push(ScopeChoice {
            label: w.name.clone().into(),
            scope: Scope::Workspace(w.id),
        });
    }
    scopes
}

pub(crate) fn cycle_index(current: usize, len: usize, backwards: bool) -> usize {
    if len == 0 {
        return 0;
    }
    if backwards {
        if current == 0 { len - 1 } else { current - 1 }
    } else {
        (current + 1) % len
    }
}

/// `Ctrl+Tab`/`Ctrl+Shift+Tab` on the SSH tab: cycle the virtual sequence [Home,
/// session 0, session 1, ..., session `len - 1`] and back to Home — Home is its own stop,
/// not skipped over between sessions. `len` is `ssh_sessions.len()`.
pub(crate) fn cycle_session_index(
    active: Option<usize>,
    len: usize,
    backwards: bool,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let total = len + 1; // + the Home slot
    let current = active.map(|ix| ix + 1).unwrap_or(0);
    let next = cycle_index(current, total, backwards);
    if next == 0 { None } else { Some(next - 1) }
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

    // ---- ssh-v3 session-tab close bookkeeping (pure) -----------------------------

    #[test]
    fn close_on_home_leaves_active_untouched() {
        // No session active (on home): closing some tab never changes the active pointer.
        assert_eq!(next_active_after_close(None, 0, 2), None);
    }

    #[test]
    fn close_tab_after_active_keeps_active_index() {
        // Active is tab 0; close tab 2 (after it) — index 0 still points at the same tab.
        assert_eq!(next_active_after_close(Some(0), 2, 2), Some(0));
    }

    #[test]
    fn close_tab_before_active_shifts_active_down_one() {
        // Active is tab 2; close tab 0 (before it) — everything shifts, active is now 1.
        assert_eq!(next_active_after_close(Some(2), 0, 2), Some(1));
    }

    #[test]
    fn close_active_tab_lands_on_the_previous_tab() {
        // Active is tab 2 of [0,1,2]; closing it (len_after 2) lands on tab 1.
        assert_eq!(next_active_after_close(Some(2), 2, 2), Some(1));
        // Closing active tab 0 (the leftmost) lands on the new tab 0 (max(0, -1) = 0).
        assert_eq!(next_active_after_close(Some(0), 0, 2), Some(0));
    }

    #[test]
    fn close_the_last_remaining_tab_goes_home() {
        // Closing the only tab (len_after 0) returns to home.
        assert_eq!(next_active_after_close(Some(0), 0, 0), None);
    }

    // ---- keyboard-driven system: tab/session cycling (pure) ----------------------

    #[test]
    fn cycle_index_wraps_forward_and_backward() {
        assert_eq!(cycle_index(0, 3, false), 1);
        assert_eq!(cycle_index(2, 3, false), 0);
        assert_eq!(cycle_index(0, 3, true), 2);
        assert_eq!(cycle_index(2, 3, true), 1);
        // Degenerate: nothing to cycle among.
        assert_eq!(cycle_index(0, 0, false), 0);
        assert_eq!(cycle_index(0, 0, true), 0);
    }

    #[test]
    fn cycle_session_index_has_no_sessions_to_offer() {
        // No live sessions: stays on Home regardless of direction.
        assert_eq!(cycle_session_index(None, 0, false), None);
        assert_eq!(cycle_session_index(None, 0, true), None);
    }

    #[test]
    fn cycle_session_index_visits_home_as_its_own_stop() {
        // [Home, 0, 1] forward from Home lands on session 0.
        assert_eq!(cycle_session_index(None, 2, false), Some(0));
        // Forward from the last session wraps back to Home, not straight to session 0.
        assert_eq!(cycle_session_index(Some(1), 2, false), None);
        // Backward from Home wraps to the last session.
        assert_eq!(cycle_session_index(None, 2, true), Some(1));
        // Backward from session 0 lands on Home.
        assert_eq!(cycle_session_index(Some(0), 2, true), None);
    }

    #[test]
    fn cycle_session_index_full_forward_loop_returns_to_start() {
        let len = 3;
        let mut active = None;
        let mut seen = vec![active];
        for _ in 0..(len + 1) {
            active = cycle_session_index(active, len, false);
            seen.push(active);
        }
        // Home -> 0 -> 1 -> 2 -> Home: a full loop of `len + 1` stops returns to start.
        assert_eq!(seen.first(), seen.last());
        assert_eq!(seen[0], None);
    }

    #[test]
    fn tab_from_str_maps_known_names_case_insensitively() {
        assert!(matches!(tab_from_str("ssh"), Some(Tab::Ssh)));
        assert!(matches!(tab_from_str("Database"), Some(Tab::Database)));
        assert!(matches!(tab_from_str("NETWORK"), Some(Tab::Network)));
        assert!(matches!(tab_from_str("Workspaces"), Some(Tab::Workspaces)));
        assert!(matches!(tab_from_str("SYSTEM"), Some(Tab::System)));
        assert!(matches!(tab_from_str("settings"), Some(Tab::Settings)));
        assert!(matches!(tab_from_str("SETTINGS"), Some(Tab::Settings)));
        assert!(tab_from_str("bogus").is_none());
        assert!(tab_from_str("").is_none());
    }

    #[test]
    fn tab_all_appends_settings_as_the_sixth_tab() {
        assert_eq!(Tab::ALL.len(), 6);
        assert_eq!(Tab::ALL[5], Tab::Settings);
        assert_eq!(Tab::Settings.label(), "Settings");
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

    // ---- secret backend status line (pure) -------------------------------------

    #[test]
    fn secret_status_message_with_no_warning_is_just_the_backend() {
        assert_eq!(
            secret_status_message("OS keyring", None, None),
            "secrets: OS keyring"
        );
    }

    #[test]
    fn secret_status_message_appends_warning_and_recommendation() {
        let msg = secret_status_message(
            "in-memory (no persistence)",
            Some("OS keyring unavailable (no Secret Service)"),
            Some("install a Secret Service provider"),
        );
        assert_eq!(
            msg,
            "secrets: in-memory (no persistence) — OS keyring unavailable (no Secret \
             Service) (install a Secret Service provider)"
        );
    }

    #[test]
    fn secret_status_message_warning_without_recommendation() {
        // The current keyring-or-memory chain always pairs a warning with a
        // recommendation, but the pure formatter must not rely on that.
        let msg = secret_status_message(
            "in-memory (no persistence)",
            Some("the OS keyring is disabled"),
            None,
        );
        assert_eq!(
            msg,
            "secrets: in-memory (no persistence) — the OS keyring is disabled"
        );
    }
}

/// Perf audit finding #7's regression trap, guarded: `seed_if_empty` must return the
/// POST-seed lists, never the pre-seed emptiness-check snapshot — a naive shortcut
/// would show a first-launch user an empty host/workspace list despite demo data
/// having just landed on disk.
#[cfg(test)]
mod seed_tests {
    use super::*;

    fn open_test_store(dir: &std::path::Path) -> Store {
        Store::open(&dir.join("store.redb")).expect("open test store")
    }

    /// An already-populated store (the common case, and the far more frequent one
    /// once the SSH slice is in daily use) trips `seed_if_empty`'s emptiness gate —
    /// no demo rows get written, and the returned lists must be exactly what's
    /// already on disk.
    #[test]
    fn seed_if_empty_returns_existing_lists_when_store_already_populated() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_test_store(dir.path());

        let existing = Host {
            alias: "existing".into(),
            user: "u".into(),
            host: "h".into(),
            port: 22,
            secret_ref: None,
            auth: AuthMethod::default(),
            folder: None,
        };
        store
            .write_host(&existing, &Scope::Global)
            .expect("seed a pre-existing host");
        let ws_root = dir.path().join("ws");
        std::fs::create_dir_all(&ws_root).unwrap();
        let ws_id = WorkspaceId::from_root(&ws_root);
        store
            .register_workspace(&WorkspaceMeta {
                id: ws_id,
                root: ws_root,
                name: "pre-existing-ws".into(),
            })
            .expect("seed a pre-existing workspace");

        let seeded = seed_if_empty(&store, dir.path());

        let hosts = seeded.hosts.expect("hosts read ok");
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "existing");
        let workspaces = seeded.workspaces.expect("workspaces read ok");
        assert_eq!(workspaces.len(), 1);
        assert_eq!(workspaces[0].name, "pre-existing-ws");

        // No demo seeding should have piled on top of the already-populated store.
        assert_eq!(store.global().list_hosts().unwrap().len(), 1);
        assert_eq!(store.global().list_workspaces().unwrap().len(), 1);
    }

    /// The regression trap itself: on a brand-new store, `seed_if_empty` WRITES the
    /// demo hosts/workspace *after* its own emptiness check — the returned lists must
    /// reflect that write, not the empty pre-seed snapshot.
    #[test]
    fn seed_if_empty_returns_the_just_seeded_rows_on_a_fresh_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_test_store(dir.path());

        let seeded = seed_if_empty(&store, dir.path());

        let hosts = seeded.hosts.expect("hosts read ok");
        assert!(
            !hosts.is_empty(),
            "a fresh store's seeded host list must not be empty \
             (the naive pre-seed-snapshot bug this test guards against)"
        );
        let workspaces = seeded.workspaces.expect("workspaces read ok");
        assert!(
            !workspaces.is_empty(),
            "a fresh store's seeded workspace list must not be empty"
        );

        // The returned lists must match what's now actually on disk, not just be
        // non-empty by coincidence.
        assert_eq!(hosts.len(), store.global().list_hosts().unwrap().len());
        assert_eq!(
            workspaces.len(),
            store.global().list_workspaces().unwrap().len()
        );
    }
}
