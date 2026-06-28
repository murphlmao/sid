//! sid GPUI app-shell spike.
//!
//! Renders a static, non-interactive "app shell" to prove that gpui builds and
//! paints on Wayland. Three stacked regions:
//!   1. a titlebar row: `✦ sid` brand on the left, then three scope buttons
//!      (`Global` active, `acme-api`, `payments-svc`);
//!   2. a tab strip: `SSH / SFTP` (active), `Database`, `Network`,
//!      `Workspaces`, `System`;
//!   3. a content area filling the rest: a scrollable `uniform_list` of fake
//!      SSH hosts, each row showing an alias and a dim monospace
//!      `user@host:port` subtitle.
//!
//! Styling is deliberately flat grayscale — theming is deferred. The host
//! subtitles use an installed monospace family to validate monospace rendering.
//!
//! This is a single-file spike against the published gpui 0.2.2 API. Every
//! method used here was verified against the crate source (examples
//! `hello_world.rs`, `uniform_list.rs`, `scrollable.rs`, and `src/styled.rs`).

use gpui::{
    App, Application, Bounds, Context, FontWeight, SharedString, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, rgb, size, uniform_list,
};

// ---- neutral grayscale palette (theming deferred) --------------------------
const BG: u32 = 0x1b1b1d;
const TITLEBAR_BG: u32 = 0x141416;
const TABSTRIP_BG: u32 = 0x202023;
const ROW_ALT_BG: u32 = 0x1f1f22;
const HOVER_BG: u32 = 0x2c2c31;
const ACTIVE_BG: u32 = 0x35353a;
const BORDER: u32 = 0x2c2c30;
const ACCENT: u32 = 0xc0c0c6;
const BRAND_FG: u32 = 0xf2f2f4;
const FG: u32 = 0xd8d8da;
const FG_DIM: u32 = 0x8a8a90;
const ACTIVE_FG: u32 = 0xffffff;

/// Monospace family for host subtitles. gpui selects a font purely by family
/// NAME and silently falls back to a proportional system font if the name is
/// missing, so we name a concrete, near-universally-installed Linux mono family
/// rather than gpui's bundled `.ZedMono` (which maps to the unbundled "Lilex").
const MONO: &str = "DejaVu Sans Mono";

/// One fake SSH host: a short alias plus a precomputed `user@host:port` line.
struct Host {
    alias: SharedString,
    subtitle: SharedString,
}

impl Host {
    fn new(alias: &str, user: &str, host: &str, port: u16) -> Self {
        Self {
            alias: alias.to_owned().into(),
            subtitle: format!("{user}@{host}:{port}").into(),
        }
    }
}

/// Root view: holds the static shell state and renders all three regions.
struct AppShell {
    scopes: Vec<SharedString>,
    active_scope: usize,
    tabs: Vec<SharedString>,
    active_tab: usize,
    hosts: Vec<Host>,
}

impl AppShell {
    fn new() -> Self {
        Self {
            scopes: vec!["Global".into(), "acme-api".into(), "payments-svc".into()],
            active_scope: 0,
            tabs: vec![
                "SSH / SFTP".into(),
                "Database".into(),
                "Network".into(),
                "Workspaces".into(),
                "System".into(),
            ],
            active_tab: 0,
            hosts: vec![
                Host::new("prod", "deploy", "prod.acme-api.internal", 22),
                Host::new("staging", "deploy", "staging.acme-api.internal", 22),
                Host::new("home-server", "murphy", "home-server.lan", 22),
                Host::new("db-primary", "postgres", "db-primary.payments-svc.internal", 2222),
                Host::new("bastion", "ops", "bastion.acme.io", 2222),
                Host::new("ci-runner", "runner", "ci-runner.ci.internal", 22),
                Host::new("edge-eu", "root", "edge-eu.payments-svc.net", 22),
                Host::new("backup", "backup", "backup.home.lan", 2200),
            ],
        }
    }

    /// Titlebar: brand on the left, then the scope buttons (active one filled).
    fn titlebar(&self) -> impl IntoElement {
        let buttons = self
            .scopes
            .iter()
            .enumerate()
            .map(|(ix, label)| {
                let active = ix == self.active_scope;
                div()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .text_sm()
                    .bg(rgb(if active { ACTIVE_BG } else { TITLEBAR_BG }))
                    .text_color(rgb(if active { ACTIVE_FG } else { FG_DIM }))
                    .child(label.clone())
            })
            .collect::<Vec<_>>();

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .w_full()
            .h(px(44.))
            .px_4()
            .bg(rgb(TITLEBAR_BG))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .text_color(rgb(BRAND_FG))
                    .font_weight(FontWeight::BOLD)
                    .child("✦ sid"),
            )
            .children(buttons)
    }

    /// Tab strip: active tab is brighter with an accent underline.
    fn tab_strip(&self) -> impl IntoElement {
        let tabs = self
            .tabs
            .iter()
            .enumerate()
            .map(|(ix, label)| {
                let active = ix == self.active_tab;
                div()
                    .px_4()
                    .py_2()
                    .text_sm()
                    .text_color(rgb(if active { ACTIVE_FG } else { FG_DIM }))
                    .border_b_2()
                    .border_color(rgb(if active { ACCENT } else { TABSTRIP_BG }))
                    .child(label.clone())
            })
            .collect::<Vec<_>>();

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

    /// A single host row: bold alias over a dim monospace subtitle. Clickable
    /// (the one bit of interactivity) — logs the selected alias to stdout.
    fn host_row(&self, ix: usize) -> impl IntoElement + use<> {
        let host = &self.hosts[ix];
        let alias = host.alias.clone();
        let subtitle = host.subtitle.clone();
        let clicked = alias.clone();

        div()
            .id(ix)
            .flex()
            .flex_col()
            .gap_1()
            .w_full()
            .px_4()
            .py_2()
            .bg(rgb(if ix % 2 == 0 { BG } else { ROW_ALT_BG }))
            .border_b_1()
            .border_color(rgb(BORDER))
            .cursor_pointer()
            .hover(|this| this.bg(rgb(HOVER_BG)))
            .on_click(move |_event, _window, _cx| {
                println!("sid: selected host {clicked}");
            })
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(FG))
                    .child(alias),
            )
            .child(
                div()
                    .font_family(MONO)
                    .text_xs()
                    .text_color(rgb(FG_DIM))
                    .child(subtitle),
            )
    }
}

impl Render for AppShell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let host_count = self.hosts.len();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(BG))
            .text_color(rgb(FG))
            .child(self.titlebar())
            .child(self.tab_strip())
            .child(
                // Content area fills the remaining vertical space; the
                // uniform_list virtualizes rows and scrolls within it.
                div().flex_1().w_full().child(
                    uniform_list(
                        "ssh-hosts",
                        host_count,
                        cx.processor(|this, range: std::ops::Range<usize>, _window, _cx| {
                            range.map(|ix| this.host_row(ix)).collect::<Vec<_>>()
                        }),
                    )
                    .h_full(),
                ),
            )
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(880.0), px(620.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| AppShell::new()),
        )
        .unwrap();
        cx.activate(true);
    });
}
