//! The sid application: state + rendering (P3.1).
//!
//! [`AppState`] is the single gpui entity. It owns the open [`Store`], the current
//! [`Scope`], the active tab, and a **cached** composed host list. Events mutate the state
//! and call `cx.notify()`; `render` paints from the cache and never does I/O (the store's
//! reads return `Result` and touch redb + the filesystem, so they run on events only).
//!
//! P3.1 wires the SSH tab's host list to `Store::read_hosts` — the first time the store and
//! the GUI meet on screen. Other tabs are placeholders for later slices.

use gpui::{
    ClickEvent, Context, FontWeight, SharedString, Window, div, prelude::*, px, rgb, uniform_list,
};
use sid_store::{Attributed, Host, Scope, Store, ViewFilters, WorkspaceId, WorkspaceMeta};

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
    scope: Scope,
    active_tab: Tab,
    filters: ViewFilters,
    scopes: Vec<ScopeChoice>,
    hosts: Vec<Attributed<Host>>,
    error: Option<String>,
}

impl AppState {
    /// Build the app state over an open store and load the initial (Global) view.
    pub fn new(store: Store) -> Self {
        let mut state = Self {
            store,
            scope: Scope::Global,
            active_tab: Tab::Ssh,
            filters: ViewFilters::default(),
            scopes: Vec::new(),
            hosts: Vec::new(),
            error: None,
        };
        state.reload_scopes();
        state.refresh();
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
                    .px_4()
                    .py_2()
                    .text_sm()
                    .text_color(rgb(FG_DIM))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(sub),
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.active_tab {
            Tab::Ssh => self.ssh_tab(cx).into_any_element(),
            other => self.placeholder(other).into_any_element(),
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(BG))
            .text_color(rgb(FG))
            .child(self.titlebar(cx))
            .child(self.tab_strip(cx))
            .child(div().flex().flex_col().flex_1().child(content))
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

fn seed_if_empty(store: &Store, dir: &std::path::Path) {
    let no_hosts = store.global().list_hosts().map(|h| h.is_empty()).unwrap_or(false);
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
    };
    let _ = store.write_host(&global("home-server", "you", "192.168.1.10"), &Scope::Global);
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
    let _ = store.write_host(&global("staging", "deploy", "staging.acme-api.internal"), &ws);
    let _ = store.write_host(&global("prod", "deploy", "prod.acme-api.internal"), &ws);
    let _ = store.write_host(&global("vps-1", "admin", "5.5.5.5"), &ws); // duplicates global vps-1
}
