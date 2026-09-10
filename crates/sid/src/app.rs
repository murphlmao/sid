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
    Anchor, ClickEvent, Context, Div, ElementId, Entity, FocusHandle, KeyDownEvent, Pixels,
    SharedString, Stateful, Subscription, Window, anchored, canvas, deferred, div, point,
    prelude::*, px, rgb, rgba,
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
use crate::ui::workspaces_tab::WorkspacesTabState;
use crate::ui::{SessionStatus, SshSession, SshSessionEvent};
use sid_ui::{
    BadgeTone, Icon, IconButton, ScopeChip, ScopeOrigin, Segment, SegmentSelect, SegmentedControl,
    StatusBar, StatusDot, StatusItem, StyledExt as _, Theme, Tipped as _, Typography as _, UiScale,
    bridge::pressed_of, modal, scaled, theme, toolbar::count_label,
};

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

    /// The tab's mark. Always drawn — beside the word on a wide window, *instead* of it
    /// on a narrow one (see [`TabChrome`]), which only works if the glyph is there all
    /// the time and the eye has already learned it.
    fn icon(self) -> Icon {
        match self {
            Tab::Ssh => Icon::Terminal,
            Tab::Database => Icon::Database,
            Tab::Network => Icon::Interfaces,
            Tab::Workspaces => Icon::Folder,
            Tab::System => Icon::Dashboard,
            Tab::Settings => Icon::Settings,
        }
    }
}

/// The height of both tab strips — the top chrome and the SSH session bar under it.
/// One number, so the two read as the same control at two levels rather than as a bar
/// and a row of chips.
const TAB_STRIP_H: f32 = 42.;

/// The narrowest window the six tab *words* fit in, beside the wordmark and the scope
/// switcher.
///
/// Measured off the 1920px capture: the labels are ~540px, the wordmark ~62, the
/// switcher ~220, the bar's own padding and gaps ~40 — ~860, so 900 is the first round
/// number with headroom. Below it the words go and the icons stay, which is ~280px of
/// tabs instead of ~540.
const LABELLED_TABS_MIN_VIEWPORT: f32 = 900.;

/// Which shape the top bar's tabs take.
///
/// The defect: at 700px the six labels ate the bar and the scope chips painted straight
/// over "System" — the bar has no clip, so an overflowing right-hand group does not get
/// cut off, it gets drawn on top. Something has to give, and it is the words: an
/// icon-only tab still says which tab it is (`.tip()` names it on hover), where a
/// missing tab says nothing at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TabChrome {
    /// Icon + word.
    Labelled,
    /// Icon only, named by a tooltip.
    IconOnly,
}

impl TabChrome {
    /// The breakpoint, as a pure function of the window and the zoom.
    ///
    /// Both sides are *design* pixels — the same currency the layout is authored in —
    /// so a 1280px window at 150% zoom (853 design px of room) collapses just like a
    /// 853px window at 100%. Same rule, and the same reasoning, as Settings'
    /// `rail_fits`.
    pub(crate) fn for_window(viewport_width: Pixels, rem_size: Pixels) -> Self {
        match viewport_width >= scaled(LABELLED_TABS_MIN_VIEWPORT).to_pixels(rem_size) {
            true => TabChrome::Labelled,
            false => TabChrome::IconOnly,
        }
    }

    /// Whether a tab draws its word.
    fn shows_label(self) -> bool {
        matches!(self, TabChrome::Labelled)
    }
}

/// One tab of either strip: the box, the height, and the active treatment.
///
/// Shared so the SSH session bar cannot drift from the chrome above it — that drift is
/// exactly what made the session strip read as a row of unfinished chips beside a real
/// tab bar. An active tab is a 2px `accent` underline and the palette's strongest ink;
/// an inactive one is `muted` on a transparent rule of the same width, so nothing moves
/// by 2px as the selection travels.
///
/// A tab stop at index 0, same as [`SegmentedControl`]'s segments and the scope
/// switcher beside it: this is a view switcher, not a form, so it sorts in paint order
/// alongside the other index-0 controls rather than ahead of them. Enter/Space
/// activation on a focused tab is `gpui::Div`'s own synthesized-click behaviour — any
/// focusable, click-listening element gets it for free, so callers' existing
/// `on_click` needs nothing extra.
///
/// `focus_ring` is **not** spelled as `border_b_2` + a shared `border_color` here, the
/// way [`SegmentedControl`]'s chips do it: `border_color` is one field for all four
/// edges, and the active-tab underline already owns it, unconditionally, for the sake
/// of a control that isn't focused at all. Reusing that field for the ring painted the
/// underline's accent across every edge the instant a merely-*selected*, unfocused tab
/// rendered — a full box around whichever tab you last clicked, not a ring around
/// whichever one Tab is on. The two are independent state (selected vs. focused) and
/// need independent paint: the outer element's own border is the ring alone
/// (`focus_ring`, invisible until focused, same as every other control); the
/// underline is a plain absolutely-positioned bottom bar that answers only to
/// `selected`.
fn chrome_tab(id: impl Into<ElementId>, selected: bool, t: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .tab_index(0)
        .relative()
        .flex()
        .flex_row()
        .items_center()
        // A tab keeps its whole label or it is not a tab: the strip that holds them
        // scrolls rather than squeezing `Workspaces` down to `Wor…`.
        .flex_none()
        .gap_2()
        .px_3()
        .h_full()
        .cursor_pointer()
        .text_body(t)
        .text_color(rgb(if selected { t.fg_strong } else { t.muted }))
        .focus_ring(t)
        .hover(|s| s.bg(rgb(t.selection)))
        // Every tab acknowledges a press, including the already-active one — same
        // reasoning as `SegmentedControl`'s re-click: the handler still fires for it,
        // and a control that doesn't move when pushed reads as dead.
        .active(|s| s.bg(rgb(pressed_of(t, t.selection))))
        .child(
            div()
                .absolute()
                .bottom_0()
                .left_0()
                .w_full()
                .h(px(2.))
                .when(selected, |el| el.bg(rgb(t.accent))),
        )
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

/// The zoom the app opens at (GitHub #4): the persisted `Settings.ui_scale_percent`,
/// unless `SID_UI_SCALE` overrides it *for this run only* — the capture harness's hook
/// for shooting the UI at 150% against a hermetic store, same convention as
/// `SID_START_TAB` and `SID_THEME` (read at startup, never written back).
///
/// Total on both inputs: an unparseable override is ignored rather than fatal, and
/// `UiScale::from_percent` snaps and clamps whatever survives, so neither a hand-edited
/// store row nor `SID_UI_SCALE=nonsense` can open the window at 0%.
fn startup_scale(persisted: u16, env: Option<&str>) -> UiScale {
    let percent = env
        .and_then(|v| v.trim().parse::<u16>().ok())
        .unwrap_or(persisted);
    UiScale::from_percent(percent)
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
    /// is disabled, or it failed the startup probe). Chooses the status bar's secrets
    /// word and ink (see `secrets_fact`) and the memory-aware password helper copy in
    /// the host/DB forms (round-D §A.5). `pub(crate)` so `ui::host_form`/
    /// `ui::db_conn_form` construction sites (here and in `ui::db_tab`) can read it.
    pub(crate) secrets_degraded: bool,
    /// The full secret-backend status line (`secret_status_message`'s output: backend,
    /// warning, recommendation) — shown in the status bar's secrets popover on click,
    /// and (round-E §C) under the Settings screen's keyring toggle. Set once at startup;
    /// nothing currently changes the backend mid-session, so this never needs to be
    /// refreshed after `AppState::new`. `pub(crate)` so `ui::settings_tab`'s Behavior
    /// section can read it directly, same convention as `secrets_degraded` above.
    pub(crate) secrets_status_detail: String,
    /// Whether the status bar's secrets popover is open.
    secrets_detail_open: bool,
    /// Why rendering is on the software path, when it is (`None` = hardware —
    /// the overwhelmingly common case, which renders nothing at all). Set once
    /// at startup from the GPU pre-flight's verdict (see `main`); nothing
    /// changes it mid-session. Drives the second warning badge at the tab
    /// strip's right end (`gpu_status_badge`) — same never-silent rule as
    /// `secrets_degraded`: a degraded rendering path must stay visible, not
    /// vanish into a log.
    render_soft_reason: Option<String>,
    /// Whether the software-rendering badge's popover is open.
    gpu_badge_open: bool,
    /// The open host add/edit modal, if any.
    form: Option<Entity<HostForm>>,
    /// Keeps the form's event subscription alive exactly as long as the form is open.
    _form_subscription: Option<Subscription>,
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
    /// App zoom, cached from `Settings.ui_scale_percent` at startup (GitHub #4).
    ///
    /// One factor for the whole UI. `render` pushes it to `Window::set_rem_size`, which
    /// is what every rem-authored length in gpui and `sid-ui` resolves against; the two
    /// subsystems that do their own pixel arithmetic (table columns, the terminal's cell
    /// grid) read the same number back off the window rather than caching a copy.
    pub(crate) ui_scale: UiScale,
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
    /// Workspaces tab state (track U): the registered-workspace list, per-workspace git
    /// summaries/branches/status/log fetched through `sid_core::git::GitProvider`, and
    /// the Umbrella fleet table. Same "sibling cache, second `impl AppState` block in
    /// its own module" shape as `db`/`network`/`systems`. Lives in `ui::workspaces_tab`.
    pub(crate) workspaces: WorkspacesTabState,
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
    /// (backend, warning, recommendation) — the former picks the status bar's secrets
    /// wording (see `AppState::status_bar`), the latter feeds its popover. Round-D §A
    /// dropped the startup unlock-or-create modal entirely (the encrypted-file backend
    /// is no longer wired into `sid_secrets::resolve_secret_store`'s chain) and the
    /// persistent "secrets: …" banner along with it — the backend now shows as one word
    /// at the foot of the window instead of taking over the SSH tab's status line.
    ///
    /// `seed_lists` is `open_store`'s `seed_if_empty` call, already read (and, on a
    /// first launch, re-read post-seed) — see [`SeedLists`]'s doc comment. Consuming it
    /// via [`Self::apply_seed_lists`] here means this constructor doesn't immediately
    /// re-issue the same hosts/workspaces reads `seed_if_empty` just did (perf audit
    /// finding #7).
    // ponytail: 8 args; `window` joined the list only because `InputState::new` needs
    // one where the old hand-rolled field didn't — a config struct is not worth it for
    // one composition-root constructor called from exactly one place.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: Store,
        seed_lists: SeedLists,
        secrets: Box<dyn SecretStore>,
        secrets_degraded: bool,
        secrets_status: String,
        render_soft_reason: Option<String>,
        window: &mut Window,
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
        // Same read, same fallback — plus the `SID_UI_SCALE` per-run override. See
        // `startup_scale`; `render` is what actually puts the number on the window.
        let ui_scale = startup_scale(
            store
                .settings()
                .map_or(UiScale::default().percent(), |s| s.ui_scale_percent),
            std::env::var("SID_UI_SCALE").ok().as_deref(),
        );
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
            secrets_detail_open: false,
            render_soft_reason,
            gpu_badge_open: false,
            form: None,
            _form_subscription: None,
            ssh_sessions: Vec::new(),
            active_session: None,
            file_browser_side,
            ui_scale,
            ssh_home: HomeTabState::new(window, cx),
            db,
            network,
            systems,
            workspaces: WorkspacesTabState::new(window, cx),
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
    /// tab uses after registering/renaming/unregistering a workspace (this closes the
    /// long-standing "reload_scopes was inlined into apply_seed_lists, startup-only"
    /// caveat from HANDOFF). If the focused scope's workspace disappeared, falls back
    /// to Global and refreshes every scoped view.
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

    /// `pub(crate)` — the Workspaces tab's "Focus scope" row action and its Overview
    /// sub-tab's jump-to-scope-tab affordance (`ui::workspaces_tab`) switch scope from
    /// outside `app.rs`, the same way the tab strip's own scope chips do here.
    pub(crate) fn set_scope(&mut self, scope: Scope) {
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
        let form = cx.new(|cx| HostForm::new_add(window, cx, workspace, default_scope, degraded));
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
        let workspace = self.active_workspace();
        let degraded = self.secrets_degraded;
        let form = cx.new(|cx| HostForm::new_edit(window, cx, host, origin, workspace, degraded));
        self.open_form(form, window, cx);
    }

    /// Remove the record from **its origin layer**, then its secret from the keyring.
    /// Deleting a workspace copy un-shadows a global duplicate — that is attributive
    /// behavior, not loss. `pub(crate)` so `ui::ssh_home`'s right-click menu shares this
    /// exact delete path with the host-list rows.
    ///
    /// There is no arming step any more. It existed because delete used to be an 11px
    /// `✕` on every row, one stray click from destroying a record; the card grid took
    /// delete off the card entirely and left it in the right-click menu, where choosing
    /// it is already a deliberate two-step act.
    pub(crate) fn delete_row(
        &mut self,
        alias: &str,
        origin: &Scope,
        secret_ref: Option<&str>,
        cx: &mut Context<Self>,
    ) {
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
        self.finish_connect(host, source, secret, window, cx);
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let label: SharedString = format!("{}@{}", host.user, host.host).into();
        let known_hosts_path = data_dir().join("known_hosts");
        let session = SshSession::open(
            host,
            secret,
            known_hosts_path,
            self.file_browser_side,
            window,
            cx,
        );
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
        window.focus(&self.root_focus, cx);
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
            let handle = tab.session.read(cx).terminal_focus_handle();
            window.focus(&handle, cx);
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
    fn refocus_stable_target(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_tab == Tab::Ssh
            && let Some(ix) = self.active_session
            && let Some(tab) = self.ssh_sessions.get(ix)
        {
            let handle = tab.session.read(cx).terminal_focus_handle();
            window.focus(&handle, cx);
        } else {
            window.focus(&self.root_focus, cx);
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

    /// Set the app zoom, persist it, and make every surface agree about it.
    ///
    /// Modelled on `toggle_dock_side` above: one global preference, written through to
    /// `Settings`, then repainted.
    ///
    /// There is no fan-out to the live sessions, and that is deliberate. `render` below
    /// pushes the new rem size onto the window; gpui rebuilds the whole element tree
    /// every frame, so each session's `render_grid` reads the new zoom straight off
    /// `window.rem_size()` on the very next one, reshapes its grid at the scaled cell
    /// size, and its existing viewport reconciliation resizes the remote PTY — the same
    /// path a window resize takes. A pushed copy would be a second channel saying the
    /// same thing, with the failure mode of disagreeing with the window.
    ///
    /// A no-op at the ends of the ladder: `zoom_in` at 200% returns 200%, and a store
    /// write for a keystroke that changed nothing would be a redb commit per key repeat.
    pub(crate) fn set_ui_scale(&mut self, scale: UiScale, cx: &mut Context<Self>) {
        if scale == self.ui_scale {
            return;
        }
        self.ui_scale = scale;
        if let Ok(mut settings) = self.store.settings() {
            settings.ui_scale_percent = scale.percent();
            let _ = self.store.set_settings(&settings);
        }
        // `refresh_windows`, not just `notify`: the rem size is a *window* property, so
        // every view in the tree has to lay out again — the same hammer the theme switch
        // uses for the same reason.
        cx.refresh_windows();
        cx.notify();
    }

    fn open_form(&mut self, form: Entity<HostForm>, window: &mut Window, cx: &mut Context<Self>) {
        form.update(cx, |it, cx| it.focus_first(window, cx));
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
        window.focus(&self.root_focus, cx);
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
        let modal = cx.new(|cx| PasswordPromptModal::new(window, cx, label));
        modal.update(cx, |it, cx| it.focus_first(window, cx));
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
        window.focus(&self.root_focus, cx);
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
                self.finish_connect(host, source, Ok(Some(password.into_bytes())), window, cx);
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

    /// The origin chip for an item's layer: `global`, or the workspace's display name,
    /// with `· dup` when the same alias also lives in the other layer.
    ///
    /// This used to hand back a raw `(label, color)` pair and paint `global` in `faint`
    /// and a workspace origin in **`success`** — a status hue spent on metadata, which
    /// left a reader deciding whether a green word meant the row was healthy.
    /// [`ScopeChip`] separates the two origins by weight inside one neutral tone
    /// instead, per `.interface-design/system.md`'s "orientation badges are
    /// `faint`/`muted`; one accent, used sparingly".
    pub(crate) fn scope_chip(&self, a: &Attributed<Host>) -> ScopeChip {
        let origin = match &a.origin {
            Scope::Global => ScopeOrigin::Global,
            Scope::Workspace(id) => ScopeOrigin::Workspace(
                self.scopes
                    .iter()
                    .find(|c| matches!(&c.scope, Scope::Workspace(w) if w == id))
                    .map(|c| c.label.clone())
                    .unwrap_or_else(|| "workspace".into()),
            ),
        };
        ScopeChip::new(origin).duplicate(a.duplicate)
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

        // Settings -> Keymap capture mode owns the next chord outright: a rebind has to
        // be able to capture chords this dispatcher would otherwise claim (`Ctrl+2`
        // switches tabs). Self-terminating — see `ui::settings_tab::capture_rebind`,
        // where the capture itself lives.
        if self.settings.is_capturing() {
            cx.stop_propagation();
            self.capture_rebind(&event.keystroke, cx);
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
        let Some(action) = keymap::resolve(&event.keystroke, focus, &self.effective_bindings())
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
            Action::ZoomIn => self.set_ui_scale(self.ui_scale.zoom_in(), cx),
            Action::ZoomOut => self.set_ui_scale(self.ui_scale.zoom_out(), cx),
            Action::ZoomReset => self.set_ui_scale(self.ui_scale.reset(), cx),
            Action::FocusFilter => {
                // Tabs with a filter input claim this; the rest have nothing to focus
                // yet and it stays a no-op there. SSH Home's quick-connect box doubles
                // as the list filter, so `Ctrl+F` lands in it.
                match self.active_tab {
                    Tab::Network => {
                        self.network.focus_filter(window, cx);
                        cx.notify();
                    }
                    Tab::Ssh if self.active_session.is_none() => {
                        self.ssh_home.focus_filter(window, cx);
                        cx.notify();
                    }
                    _ => {}
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
        let (accent, surface, border, muted, selection) =
            (t.accent, t.surface, t.border, t.muted, t.selection);
        let bindings = self.effective_bindings();
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
                    .child(div().text_body(t).child(action.label()))
                    .child(div().text_body(t).text_color(rgb(accent)).child(shortcut))
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
                                .w(scaled(420.))
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
                                        .child(div().text_title(t).child("Keyboard Shortcuts"))
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
    /// aligned) the scope switcher and the software-rendering badge. One bar, not
    /// the previous two stacked ones — a whole row of chrome bought nothing but
    /// vertical clutter, and scope-switching is an occasional act that belongs at the
    /// edge, not on its own strip above everything.
    ///
    /// Two things used to go wrong at the right-hand end. `Global` and the workspace
    /// name were **two chips**, so a switch between mutually exclusive layers read as
    /// two unrelated buttons; they are one [`SegmentedControl`] now, the same control
    /// System and Network use for their sub-views, and the same one the per-item origin
    /// badge ([`ScopeChip`]) is deliberately *not*. And at 700px the six tab words ran
    /// under the chips and "System" simply vanished behind `Global`; the tabs collapse
    /// to their icons below [`LABELLED_TABS_MIN_VIEWPORT`] instead — see [`TabChrome`].
    fn tab_strip(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::active(cx).clone();
        let (surface, border, accent) = (t.surface, t.border, t.accent);
        let chrome = TabChrome::for_window(window.viewport_size().width, window.rem_size());

        let active = self.active_tab;
        let tabs: Vec<_> = Tab::ALL
            .iter()
            .enumerate()
            .map(|(ix, &tab)| {
                let is_active = tab == active;
                chrome_tab(("tab", ix), is_active, &t)
                    // `flex_none` on the glyph: it is the one part of an icon-only tab
                    // that may never shrink.
                    .child(div().flex_none().child(tab.icon().small()))
                    .when(chrome.shows_label(), |this| this.child(tab.label()))
                    // Only when the word is gone: a tooltip repeating a label the user
                    // can already read is noise, and gpui debug-asserts on a second
                    // `tooltip()` call for the same element anyway.
                    .when(!chrome.shows_label(), |this| this.tip(tab.label()))
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
        let selected_scope = self
            .scopes
            .iter()
            .position(|choice| choice.scope == current)
            .unwrap_or(0);
        let scope_switcher = SegmentedControl::new("scope-switcher")
            .segments(
                self.scopes
                    .iter()
                    .map(|choice| Segment::new(choice.label.clone())),
            )
            .selected(selected_scope)
            .on_select(cx.listener(|this, ev: &SegmentSelect, _win, cx| {
                let Some(choice) = this.scopes.get(ev.index) else {
                    return;
                };
                let target = choice.scope.clone();
                this.set_scope(target);
                cx.notify();
            }));

        div()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h(scaled(TAB_STRIP_H))
            // The chrome's height is not negotiable. Without this the strip is an
            // ordinary shrinkable flex item in the window's column, so any tab whose
            // content reports a taller intrinsic height than the window has left
            // (Settings' four stacked sections measure ~1053px at 1080p) takes the
            // difference out of the bar above it: the whole navbar visibly rode up —
            // 42px tall on SSH/Network, 27px on Settings — as you switched tabs.
            .flex_shrink_0()
            .px_3()
            .gap_1()
            .bg(rgb(surface))
            .border_b_1()
            .border_color(rgb(border))
            .child(
                // The wordmark is the one Title in the chrome. It used to be the
                // only *unsized* element in the bar, which meant it rendered at gpui's
                // 16px default by accident and shouted with BOLD to make up for having
                // no rung of its own.
                div()
                    .flex_none()
                    .pr_2()
                    .text_title(&t)
                    .text_color(rgb(accent))
                    .child("✦ sid"),
            )
            // The tab list is the bar's elastic member, and it is the only one that may
            // be cut short. Six labels measure ~540px; a 620px window has ~590px of bar
            // after the padding, so the tabs used to eat the row whole and push the scope
            // chips and the status badges clean off the right edge — with no clip, the
            // pills simply painted outside the window. (`flex_1` alone would not have
            // saved them: gpui reports a text element's min-content width as its full
            // string, so a row of text tabs refuses to shrink.)
            //
            // `flex_1 + min_w(0)` makes this strip the thing that gives, `overflow_x_scroll`
            // keeps every tab *reachable* while it gives, and each tab's `flex_none` keeps
            // labels whole inside it. It also replaces the old spacer div: the strip is
            // what absorbs the free space on a wide window, so the chips still sit at the
            // right edge and the 2000px layout is unchanged.
            .child(
                div()
                    .id("primary-tab-strip")
                    .flex()
                    .flex_row()
                    .items_center()
                    .h_full()
                    .gap_1()
                    .flex_1()
                    .min_w(px(0.))
                    // No trailing padding here: a scroll container clips at its padding
                    // box, so `pr_2` buys nothing visible (measured — the 620px capture is
                    // identical with and without it). The 4px the cut edge gets from the
                    // bar's own `gap_1` is the whole separation, same as a browser's tab
                    // strip against its toolbar.
                    .overflow_x_scroll()
                    .children(tabs),
            )
            // The switcher and the badge hold the right edge. The badge never shrinks —
            // a software-rendering warning that scrolls out of the window is a warning
            // that was not delivered. (The secrets warning left this bar entirely: it
            // is a word in the status bar now, see `status_bar`.)
            //
            // The switcher is capped and clipped, because a segment carries a
            // *workspace name* the chrome has no say in:
            // `platform-infrastructure-monorepo` is a 260px word on its own. The
            // control's segments are already `min_w_0` + clamped, so the cap is what
            // makes them actually elide instead of pushing the badge off the bar.
            .child(
                div()
                    .flex_none()
                    .min_w(px(0.))
                    .max_w(scaled(360.))
                    .overflow_hidden()
                    .child(scope_switcher),
            )
            .children(self.gpu_status_badge(cx))
    }

    /// The app-wide status bar (`sid_ui::StatusBar`): the strip under the active tab
    /// that says what sid is holding, and what the view is doing.
    ///
    /// **Left**, in order: the secrets backend *in words* — `keyring` in success ink, or
    /// `secrets in memory` in warning ink with a glyph. This is where the top bar's lone
    /// yellow `!` pill went, and the reason it went is the reason it was never good: a
    /// warning mark that needs a click to say what it means is not a warning. Clicking
    /// still opens the same `secret_status_message` detail (backend, warning,
    /// recommendation), now anchored above the strip it came from. Then the open SSH
    /// session count, absent at zero — an ops bar reports what *is*. Then the selected
    /// Database connection with the same dot its own row draws (`db_status`).
    ///
    /// **Right**: facts about the view rather than the work — the zoom readout while it
    /// is not 100% (click resets, exactly as ctrl+0 does) and, under `SID_PERF`, the
    /// last frame's cost, which until now existed only as stderr spam.
    ///
    /// The workspace scope is deliberately **not** repeated here. It is already a chip
    /// in the top bar, and "one list per fact" governs single facts too.
    fn status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::active(cx);
        let (border, surface, fg) = (t.border, t.surface, t.fg);
        let degraded = self.secrets_degraded;
        let (word, tone) = secrets_fact(degraded);

        // The popover rides beside the item rather than inside it: `StatusItem` has no
        // child slot on purpose (it is a word, a mark and a click), and an `anchored()`
        // element is `Position::Absolute`, so a wrapper costs the strip no width.
        let popover = self.secrets_detail_open.then(|| {
            deferred(
                anchored()
                    .anchor(Anchor::BottomLeft)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        div()
                            .id("secret-status-popover")
                            .occlude()
                            .mb_1()
                            .max_w(scaled(360.))
                            .p_3()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(border))
                            .bg(rgb(surface))
                            .text_body(t)
                            .text_color(rgb(fg))
                            .child(self.secrets_status_detail.clone()),
                    ),
            )
            .with_priority(2)
        });

        let secrets = div()
            .flex()
            .flex_row()
            .items_center()
            .flex_none()
            .child(
                StatusItem::new("status-secrets", word)
                    .tone(tone)
                    // The glyph is spent only on the degraded state. A healthy backend
                    // gets one quiet word: chrome that decorates the good news trains
                    // the eye to skip the bad.
                    .when(degraded, |item| item.icon(Icon::Warning))
                    .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| {
                        this.secrets_detail_open = !this.secrets_detail_open;
                        cx.notify();
                    })),
            )
            .children(popover);

        let mut bar = StatusBar::new().left(secrets);
        if let Some(label) = ssh_session_fact(self.ssh_sessions.len()) {
            bar = bar.left(StatusItem::new("status-ssh", label));
        }
        if let Some((name, state)) = self.db_status() {
            bar = bar.left(StatusItem::new("status-db", format!("db: {name}")).dot(state));
        }
        if !self.ui_scale.is_default() {
            bar = bar.right(
                // Click is ctrl+0: the readout only exists while it has something to
                // say, so the one thing it can usefully do is make itself go away.
                StatusItem::new("status-zoom", self.ui_scale.label()).on_click(cx.listener(
                    |this, _ev: &ClickEvent, _window, cx| this.set_ui_scale(UiScale::DEFAULT, cx),
                )),
            );
        }
        let frame_us = LAST_FRAME_US.load(std::sync::atomic::Ordering::Relaxed);
        if frame_us > 0 {
            bar = bar.right(StatusItem::new(
                "status-perf",
                format!("{:.1} ms", frame_us as f64 / 1000.),
            ));
        }
        bar
    }

    /// The GPU pre-flight's software-rendering badge: a small `sw` pill at the tab
    /// strip's right end, rendered only while the render path is degraded (hardware
    /// rendering — the overwhelmingly common case — shows nothing at all). Click
    /// toggles a trigger-anchored popover with the why, plus the `sid --gpu-report`
    /// pointer for the full evidence. The last pill in the top bar: the secrets warning
    /// that used to sit beside it is now a word in the status bar (`status_bar`).
    fn gpu_status_badge(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let reason = self.render_soft_reason.as_deref()?;
        let t = theme::active(cx);
        let (warning, border, surface, fg) = (t.warning, t.border, t.surface, t.fg);
        let badge = div()
            .id("gpu-status-badge")
            .px_2()
            .py(scaled(2.))
            .rounded_full()
            .text_meta(t)
            .cursor_pointer()
            .bg(rgb(warning))
            // Deliberately not a theme token: a near-black label reads clearly against
            // every theme's amber `warning` tone, which a theme-following text color
            // could not guarantee (cosmos-light's `bg` is a light off-white —
            // unreadable on the same amber pill).
            .text_color(rgb(0x1a1a1a))
            .child("sw")
            .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| {
                this.gpu_badge_open = !this.gpu_badge_open;
                // The secrets popover no longer needs closing here. The two used to
                // snap to the same window corner and the later-drawn one won; the
                // secrets detail now rises from the status bar's bottom-left, so both
                // can be open at once and neither hides the other.
                cx.notify();
            }));

        let detail: SharedString =
            format!("software rendering — {reason}\n\nrun `sid --gpu-report` for details").into();
        let popover = self.gpu_badge_open.then(|| {
            deferred(
                anchored()
                    .anchor(Anchor::TopRight)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        div()
                            .id("gpu-status-popover")
                            .occlude()
                            .mt_1()
                            .max_w(scaled(360.))
                            .p_3()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(border))
                            .bg(rgb(surface))
                            .text_body(t)
                            .text_color(rgb(fg))
                            .child(detail),
                    ),
            )
            .with_priority(2)
        });

        Some(
            div()
                .flex()
                .flex_row()
                .items_center()
                // Never shrinks: see `tab_strip`'s right-edge comment.
                .flex_none()
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
    /// strip and the connections surface. `clamp_one_line` (clip + a real ellipsis, not
    /// wrap — see [`sid_ui::StyledExt::clamp_one_line`] for why gpui's own `truncate()`
    /// silently doesn't) keeps it to one line no matter how long the message is. `None` —
    /// the common case with no error — renders nothing.
    ///
    /// Round-D §A dropped the startup secrets-backend notice this bar used to double as
    /// (a persistent "secrets: …" line) — the backend is a word at the foot of the
    /// window now, on every tab (see `AppState::status_bar`).
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
                .text_meta(t)
                .text_color(rgb(danger))
                .clamp_one_line()
                .child(text),
        )
    }

    /// The SSH session strip: `home` (leftmost, always goes Home) · one tab per live
    /// session (its [`StatusDot`], the host's alias, and a close button that appears
    /// under the pointer) · a `+` [`IconButton`].
    ///
    /// It is a **tab bar**, at the same height and with the same active treatment as the
    /// chrome above it ([`chrome_tab`]) — before this it was a row of 30px rounded chips
    /// with a hairline box each, a Unicode `●` and a Unicode `×`, which read as an
    /// unfinished sketch of a tab bar sitting directly under a real one. The one
    /// deliberate difference is the fill: the chrome bar is `surface`, this sits on the
    /// canvas `bg`, so the two are legible as *chrome* and *this tab's content* rather
    /// than as two navigations of equal rank.
    fn session_tab_strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::active(cx).clone();
        let home_selected = self.active_session.is_none();
        // No glyph: every icon in the registry that would fit "home" is already the mark
        // of one of the six tabs above, and a glyph that means two things is worse than
        // a word that means one.
        let home = chrome_tab("ssh-tab-home", home_selected, &t)
            .child("home")
            .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| this.go_home(window, cx)));

        let tabs: Vec<_> = self
            .ssh_sessions
            .iter()
            .enumerate()
            .map(|(ix, tab)| {
                let selected = self.active_session == Some(ix);
                let state =
                    crate::ui::ssh_home::connection_state(Some(tab.session.read(cx).status()));
                // The alias when the session came from a saved record, which is the name
                // the user picked and the one the card on Home shows; the dialled
                // `user@host` only when there is no record behind it.
                let label: SharedString = tab
                    .source
                    .as_ref()
                    .map_or_else(|| tab.label.clone(), |(alias, _)| alias.clone().into());
                // A group per tab, so the close button can appear for *this* tab under
                // the pointer without a second piece of state anywhere.
                let group = SharedString::from(format!("ssh-session-tab-{ix}"));
                chrome_tab(("ssh-session-tab", ix), selected, &t)
                    .group(group.clone())
                    .child(
                        div()
                            .flex_none()
                            .child(StatusDot::new(("ssh-session-dot", ix), state)),
                    )
                    // A session tab is only ever as wide as a name you can read: an
                    // unclamped label (`deploy@prod-eu-west-1-application-server-01
                    // .internal.acme-api.example.com`) grew one tab to 570px and pushed
                    // the strip's own controls off the window. `min_w(0)` drops the
                    // text's content-sized minimum so the cap can bite, and the clamp
                    // cuts it with a real `…`.
                    .child(
                        div()
                            .min_w(px(0.))
                            .max_w(scaled(200.))
                            .clamp_one_line()
                            .child(label),
                    )
                    .child(
                        // Present in the layout at all times so nothing reflows when the
                        // pointer arrives — only its opacity changes. The active tab
                        // keeps it lit, because the tab you are on is the one you close.
                        div()
                            .flex_none()
                            .opacity(if selected { 1. } else { 0. })
                            .group_hover(group, |s| s.opacity(1.))
                            .child(
                                IconButton::new(
                                    ("ssh-session-tab-close", ix),
                                    Icon::Close,
                                    "Close this session (Ctrl+W)",
                                )
                                .small()
                                .on_click(cx.listener(
                                    move |this, _ev: &ClickEvent, window, cx| {
                                        this.close_session(ix, window, cx);
                                    },
                                )),
                            ),
                    )
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                        this.activate_session(ix, window, cx);
                    }))
            })
            .collect();

        let add = IconButton::new("ssh-tab-add", Icon::Add, "New SSH session (Ctrl+T)")
            .small()
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
            .items_center()
            .w_full()
            .h(scaled(TAB_STRIP_H))
            .flex_shrink_0()
            .px_2()
            .gap_1()
            .bg(rgb(t.bg))
            .hairline_b(&t)
            .child(home)
            // The elastic member, same arrangement as the chrome strip above with one
            // difference: it shrinks but does not *grow*. The session tabs are what
            // gives on a narrow window and they stay reachable by scrolling while they
            // give — but `+` belongs immediately after the last tab, the way a browser
            // draws it, not stranded against the far edge of the window.
            .child(
                div()
                    .id("ssh-session-strip")
                    .flex()
                    .flex_row()
                    .items_center()
                    .h_full()
                    .gap_1()
                    .flex_shrink(1.)
                    .min_w(px(0.))
                    .overflow_x_scroll()
                    .children(tabs),
            )
            .child(div().flex_none().child(add))
            // Everything to the right of `+` is empty strip.
            .child(div().flex_1())
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
                            .text_body(t)
                            .cursor_pointer()
                            .bg(rgb(selection))
                            .text_color(rgb(fg_strong))
                            .child("← close tab")
                            .on_click(cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                                this.close_session(ix, window, cx);
                            })),
                    )
                    .child(
                        // `min_w(0)` + one clamped line: gpui measures a text element's
                        // MIN-content width as its full single-line width (see
                        // `sid_ui::StyledExt::clamp_one_line`), so `flex_1` alone leaves
                        // this header with a minimum it can never shrink below and a long
                        // `user@host · status` walks straight out of the strip.
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_body(t)
                            .text_color(rgb(muted))
                            .clamp_one_line()
                            .child(header),
                    ),
            )
            .child(div().flex().flex_col().flex_1().child(session))
    }

    /// Switch the active primary tab from outside `app.rs` — the Workspaces tab's
    /// jump-to-scope-tab affordance (Overview's scope-items rows) needs this exact
    /// three-step sequence, otherwise identical to `dispatch_action`'s `PrimaryTab`/
    /// `Settings` arms and `tab_strip`'s own click handler.
    pub(crate) fn switch_to_tab(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        self.active_tab = tab;
        self.close_palette(cx);
        self.refocus_stable_target(window, cx);
        cx.notify();
    }
}

impl Render for AppState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // SID_PERF=1: time the frame. See `perf_probe` — `build=` is this function's own
        // cost (element construction), `total=` is the whole frame including gpui's
        // layout/prepaint/paint, which is where scroll lag actually lives.
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
            // `workspaces_tab` needs `window` for the same reason (the Umbrella
            // fleet's `TableState::new`).
            Tab::Workspaces => self.workspaces_tab(window, cx),
            // `settings_tab` needs no lazy widget construction, so it takes `&self`
            // rather than `&mut self` like the tabs above.
            Tab::Settings => self.settings_tab(cx),
        };

        // The three modal scrims. `sid_ui::modal::overlay` owns the shape — viewport
        // sized, occluding, deferred above everything, `SCRIM`-washed — which this
        // function used to spell out three times in eighteen identical lines each.
        let overlay = self.form.clone().map(|form| modal::overlay(window, form));
        let db_overlay = self
            .db
            .form
            .clone()
            .map(|form| modal::overlay(window, form));
        let password_prompt_overlay = self
            .password_prompt
            .clone()
            .map(|prompt| modal::overlay(window, prompt));

        // Keyboard-driven system (2026-07-02 plan): the palette + cheat-sheet overlays,
        // and the root-level key handler that opens/dispatches them. `capture_key_down`
        // runs *before* any descendant (the terminal included) sees the keystroke — see
        // `handle_root_key_down`'s doc comment for why that ordering is load-bearing.
        let palette_overlay = self.palette_overlay(window, cx);
        let cheat_sheet_overlay = self.cheat_sheet_overlay(window, cx);

        // THE lever. Everything authored in rems — gpui's own `.p_2()`/`.gap_1()`/
        // `.h_8()`/`.rounded_md()` shorthands, `sid-ui`'s type scale, every `px(..)`
        // length — resolves against this, so one assignment per frame scales the entire
        // UI. Set in `render` rather than once at startup so it survives a window
        // recreation and cannot drift from `self.ui_scale`.
        window.set_rem_size(self.ui_scale.rem_size());

        let t = theme::active(cx);
        let (bg, fg) = (t.bg, t.fg);
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(bg))
            .text_color(rgb(fg))
            .track_focus(&self.root_focus)
            .capture_key_down(cx.listener(Self::handle_root_key_down))
            .child(self.tab_strip(window, cx))
            // `min_h(0)` is the other half of pinning the chrome. A flex item's automatic
            // minimum height is its content's intrinsic height, and gpui's `flex_1` sets a
            // `0%` basis — a percentage that is indefinite during the intrinsic pass, so it
            // falls back to content-based sizing. A tall tab therefore claimed the height it
            // wanted (Settings: 1053 of 1080px) instead of the height it was given, and the
            // strip above absorbed the difference. With the floor dropped, the active tab
            // gets exactly what is left under the chrome and scrolls inside it.
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.))
                    .child(content),
            )
            // Under the tab, above every overlay. It shrinks the tab's box rather than
            // overlaying it, which is what makes the SSH terminal reflow to fewer rows
            // (`ui::session::grid_size` measures the pane it is actually given) and the
            // scrolling tabs stop scrolling at a line that is still on screen.
            .child(self.status_bar(cx))
            .children(overlay)
            .children(db_overlay)
            .children(password_prompt_overlay)
            .children(palette_overlay)
            .children(cheat_sheet_overlay)
            // Last child, so its paint closure runs at the very end of the paint phase.
            .children(perf_start.map(|start| perf_probe(self.active_tab, start)))
    }
}

/// The last frame's total cost in microseconds — written by [`perf_probe`]'s paint
/// closure, read by [`AppState::status_bar`] on the *next* frame.
///
/// A static rather than a field on [`AppState`] for two reasons. [`perf_probe`] is a free
/// function holding no entity handle, so it has nothing to write into; and a value that
/// is one frame stale needs no `cx.notify()`, while a `notify` issued from inside paint
/// on every frame is an infinite render loop that would make the instrument change what
/// it measures. Zero means "no frame has ever been timed", which is exactly the state
/// when `SID_PERF` is unset — so it doubles as the readout's visibility gate and the bar
/// needs no second env lookup.
static LAST_FRAME_US: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The secrets item's word and ink: the whole of what the retired `!` badge was trying
/// to say, said.
///
/// Pure, because this is the single place sid decides whether the secret backend is
/// *reported* as healthy, and a warning that silently flips to the reassuring wording is
/// the one failure here worth a test.
fn secrets_fact(degraded: bool) -> (&'static str, BadgeTone) {
    if degraded {
        ("secrets in memory", BadgeTone::Warning)
    } else {
        ("keyring", BadgeTone::Success)
    }
}

/// The open-SSH-session count for the status bar, or `None` at zero.
///
/// Hidden rather than rendered as `0 ssh sessions`: the bar reports what sid is holding,
/// and a count of nothing is not a fact about the app — it is a fact about the bar.
fn ssh_session_fact(open: usize) -> Option<String> {
    (open > 0).then(|| count_label(open, "ssh session"))
}

/// The frame timer, as an element.
///
/// `SID_PERF` used to report only the elapsed time of `AppState::render` — element
/// *build*. That is a small and, for scrolling, actively misleading fraction of a frame:
/// building the System tab's process table costs ~0.3ms while the frame it belongs to can
/// cost 20ms, because everything expensive (taffy layout, text shaping, prepaint of every
/// visible cell, paint) happens *after* `render` returns. Scroll lag was therefore
/// invisible to the instrument that existed.
///
/// A [`canvas`] fixes that with no new machinery: its paint closure runs in the paint
/// phase, and placed last in the root's child list it runs at the end of it, so
/// `start.elapsed()` there is essentially the frame's whole CPU cost. `absolute()` +
/// `size_full()` keeps it out of the flex layout; a canvas paints nothing and takes no
/// hitbox, so it cannot perturb what it measures beyond its own `eprintln!` (which lands
/// in the *next* frame's budget, not this one's).
///
/// The same canvas also splits the frame in two, because its *prepaint* closure runs at
/// the end of the prepaint phase: `layout=` is build + taffy + prepaint (where text is
/// measured and every element's bounds are resolved), `total=` adds paint. Which of the
/// two moves under a fix is the difference between "we build too many elements" and "we
/// re-shape too much text".
///
/// One line per frame — not a threshold — because the number that matters is a p95 over a
/// sustained scroll, and that needs the whole distribution:
///
/// ```text
/// sid-perf: frame System build=0.31ms layout=14.90ms total=18.42ms
/// ```
fn perf_probe(tab: Tab, start: std::time::Instant) -> impl IntoElement {
    let build = start.elapsed();
    canvas(
        move |_, _, _| start.elapsed(),
        move |_, layout: std::time::Duration, _, _| {
            let total = start.elapsed();
            LAST_FRAME_US.store(
                total.as_micros() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            eprintln!(
                "sid-perf: frame {:?} build={:.2}ms layout={:.2}ms total={:.2}ms",
                tab,
                build.as_secs_f64() * 1e3,
                layout.as_secs_f64() * 1e3,
                total.as_secs_f64() * 1e3,
            );
        },
    )
    .absolute()
    .size_full()
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
/// degraded (memory — feeds `AppState::secrets_degraded`, which picks the status bar's
/// secrets wording), and the full status text for that item's popover: which backend is
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

    // ---- startup zoom (GitHub #4) ------------------------------------------------

    #[test]
    fn the_app_opens_at_the_persisted_zoom() {
        assert_eq!(startup_scale(150, None).percent(), 150);
        assert_eq!(startup_scale(100, None), UiScale::DEFAULT);
        // A store row nobody's build wrote still opens a usable window.
        assert_eq!(startup_scale(0, None).percent(), 50);
        assert_eq!(startup_scale(140, None).percent(), 150);
    }

    #[test]
    fn sid_ui_scale_overrides_the_persisted_zoom_for_one_run() {
        assert_eq!(startup_scale(100, Some("150")).percent(), 150);
        assert_eq!(startup_scale(150, Some("100")), UiScale::DEFAULT);
        assert_eq!(startup_scale(100, Some(" 200 ")).percent(), 200);
        // Junk (or an empty var) is ignored, not fatal — the persisted value stands.
        assert_eq!(startup_scale(125, Some("huge")).percent(), 125);
        assert_eq!(startup_scale(125, Some("")).percent(), 125);
        // …and an override past the ladder clamps like any other percent.
        assert_eq!(startup_scale(100, Some("9000")).percent(), 200);
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

    // ---- status bar: which items are on it, given what is true --------------------

    /// Every visibility rule the bottom bar has, in one place.
    ///
    /// These fail silently by nature — nothing errors, an item simply stops appearing
    /// (or, worse, a degraded backend starts reading as a healthy one) — which is the
    /// whole reason they are pure functions rather than `if`s buried in `status_bar`.
    #[test]
    fn the_status_bar_says_what_is_true_and_hides_what_is_not() {
        // The fact the retired `!` badge could not state without a click.
        assert_eq!(secrets_fact(false), ("keyring", BadgeTone::Success));
        assert_eq!(
            secrets_fact(true),
            ("secrets in memory", BadgeTone::Warning)
        );

        // Sessions: absent at zero, pluralised after one.
        assert_eq!(ssh_session_fact(0), None);
        assert_eq!(ssh_session_fact(1).as_deref(), Some("1 ssh session"));
        assert_eq!(ssh_session_fact(3).as_deref(), Some("3 ssh sessions"));

        // Zoom rides a predicate `UiScale` already owns — asserted here so the bar's
        // "only when it has something to say" rule is written down beside the others.
        assert!(UiScale::DEFAULT.is_default());
        assert!(!UiScale::from_percent(125).is_default());
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
    fn every_tab_has_its_own_mark() {
        // The whole premise of the narrow-window bar: with the words gone, the glyph is
        // the only thing telling one tab from another. Two tabs sharing an icon would
        // make the collapsed bar unreadable in a way the wide one never shows.
        let icons: std::collections::HashSet<_> = Tab::ALL.iter().map(|t| t.icon()).collect();
        assert_eq!(icons.len(), Tab::ALL.len(), "two tabs share a glyph");
    }

    #[test]
    fn a_narrow_window_drops_the_tab_words_and_keeps_the_tabs() {
        // The defect: at 700px the six labels ran under the scope chips and "System"
        // vanished behind `Global`. Nothing may clip, so the words go first.
        let rem = px(16.);
        assert_eq!(TabChrome::for_window(px(1920.), rem), TabChrome::Labelled);
        assert_eq!(
            TabChrome::for_window(px(LABELLED_TABS_MIN_VIEWPORT), rem),
            TabChrome::Labelled,
            "the breakpoint itself"
        );
        assert_eq!(TabChrome::for_window(px(700.), rem), TabChrome::IconOnly);
        assert_eq!(TabChrome::for_window(px(620.), rem), TabChrome::IconOnly);
    }

    #[test]
    fn the_tab_breakpoint_is_measured_in_design_pixels_not_device_ones() {
        // At 150% a 1280px window has only ~853 design px of bar — less than the six
        // words plus the wordmark and the scope switcher — so it must collapse even
        // though 1280 > 900. Same rule as Settings' `rail_fits`.
        assert_eq!(
            TabChrome::for_window(px(1280.), px(16. * 1.5)),
            TabChrome::IconOnly
        );
        assert_eq!(
            TabChrome::for_window(px(1280.), px(16.)),
            TabChrome::Labelled,
            "the same window at 100% keeps its words"
        );
    }

    #[test]
    fn only_the_labelled_chrome_draws_a_word() {
        assert!(TabChrome::Labelled.shows_label());
        assert!(!TabChrome::IconOnly.shows_label());
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
