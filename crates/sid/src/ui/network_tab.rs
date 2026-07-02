//! Network tab (inc-1): listening ports + interfaces, sourced live from `sid_core::sys`.
//!
//! [`NetworkTabState`] is deliberately **live/ephemeral** — CLAUDE.md's layered-scope
//! invariant (global store + per-workspace `.sid/config.toml`) does not apply here.
//! There is no store, no scope, no secrets, nothing committed; every render reflects
//! the machine's current state and a refresh simply re-probes it. `crates/sid` is the
//! one crate allowed to name `sid_sysinfo`'s concrete `SysinfoProvider::new()`
//! constructor — every call through it after construction goes back out via the
//! `sid_core::sys::SysProvider` trait, matching `sid-db`'s `DbClient`/`db_registry`
//! seam.
//!
//! Ports are rendered with `gpui-component`'s `Table`/`TableDelegate` (cribbed from
//! `db_tab.rs`'s `ResultDelegate`), reused on the shared `session::ssh_runtime()` Tokio
//! runtime for the same reason `db_tab.rs` does: `sysinfo`/`netstat2`/`nix` calls are
//! synchronous OS calls, not async-native, but keeping them off gpui's own executor
//! avoids blocking `render`. Unlike `ResultDelegate`, [`PortsDelegate`] is interactive
//! (per-row kill) — `TableDelegate::render_td`'s `cx: &mut Context<TableState<Self>>` is
//! scoped to the table's own entity, so the two-click kill-confirm state lives on the
//! delegate itself rather than routed back through `AppState`.

use std::sync::{Arc, Mutex};

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, FontWeight, IntoElement, SharedString, Window,
    div, prelude::*, px, rgb,
};
use gpui_component::table::{Column, Table, TableDelegate, TableState};
use sid_core::sys::{ListeningPort, NetInterface, Pid, Protocol, Signal, SysProvider};
use sid_sysinfo::SysinfoProvider;

use crate::app::AppState;
use crate::ui::session::ssh_runtime;

// Dark-theme palette, aligned with `app.rs`/`db_tab.rs`. Kept local so `ui` stays
// self-contained (same convention as `db_tab.rs`).
const BORDER: u32 = 0x2c2c30;
const FG: u32 = 0xdcdce0;
const FG_DIM: u32 = 0x8a8a90;
const ACTIVE_BG: u32 = 0x33343a;
const BRAND: u32 = 0x5a9ad0;
const DANGER: u32 = 0xd08a8a;

/// Network tab state. See the module doc comment for why this holds no store/scope.
pub struct NetworkTabState {
    /// The one seam this crate constructs concretely (`SysinfoProvider::new()`).
    /// Shared (via `Arc<Mutex<_>>`) between the refresh task and the ports table's own
    /// kill task, both of which run on `session::ssh_runtime()`.
    provider: Arc<Mutex<SysinfoProvider>>,
    /// Set once the tab has triggered its first refresh (on first paint) — guards
    /// against re-triggering it on every subsequent render.
    loaded: bool,
    /// True while a refresh task is in flight — guards re-entrant ⟳ clicks.
    refreshing: bool,
    interfaces: Vec<NetInterface>,
    /// Name of the interface holding the default route, if any — sorted first in the
    /// interfaces strip.
    default_route: Option<String>,
    error: Option<String>,
    /// The ports table. Lazily built by `ensure_network_table` (needs `window`, which
    /// isn't available from `AppState::new`) — mirrors `DbTabState::results`.
    table: Option<Entity<TableState<PortsDelegate>>>,
}

impl NetworkTabState {
    pub(crate) fn new() -> Self {
        Self {
            provider: Arc::new(Mutex::new(SysinfoProvider::new())),
            loaded: false,
            refreshing: false,
            interfaces: Vec::new(),
            default_route: None,
            error: None,
            table: None,
        }
    }
}

/// Backs the ports [`Table`]. Constructed empty by `ensure_network_table`, then
/// mutated in place (`set_ports`) on every refresh — mirrors `db_tab.rs`'s
/// `ResultDelegate`. Unlike `ResultDelegate`, this delegate is interactive: it owns the
/// two-click kill-confirm state and spawns its own kill task, since `render_td`'s `cx`
/// is scoped to `TableState<Self>`, not the outer `AppState`.
struct PortsDelegate {
    provider: Arc<Mutex<SysinfoProvider>>,
    ports: Vec<ListeningPort>,
    /// The pid whose kill button has been clicked once — the second click on the same
    /// pid sends the signal.
    armed_kill: Option<Pid>,
    /// Outcome of the last kill attempt, if it failed (e.g. `SysError::PermissionDenied`
    /// on a root-owned process). Cleared on the next refresh, arm, or successful kill.
    kill_error: Option<String>,
    columns: Vec<Column>,
}

impl PortsDelegate {
    fn new(provider: Arc<Mutex<SysinfoProvider>>) -> Self {
        Self {
            provider,
            ports: Vec::new(),
            armed_kill: None,
            kill_error: None,
            columns: vec![
                Column::new("proto", "Proto").width(px(64.)),
                Column::new("port", "Port").width(px(72.)),
                Column::new("pid", "PID").width(px(80.)),
                Column::new("process", "Process").width(px(240.)),
                Column::new("kill", "").width(px(72.)),
            ],
        }
    }

    /// Replace the cached rows after a refresh. Disarms any pending kill confirmation —
    /// the row set just changed underneath it (mirrors `DbTabState::refresh` disarming
    /// a pending delete).
    fn set_ports(&mut self, ports: Vec<ListeningPort>) {
        self.ports = ports;
        self.armed_kill = None;
    }

    /// Second click on an armed row: send SIGTERM to `pid` on the shared runtime. On
    /// success the row is dropped from the cache immediately (rather than waiting on
    /// the next refresh); on failure the error (esp. `SysError::PermissionDenied` for a
    /// root-owned process) is surfaced via `kill_error`.
    fn kill(&mut self, pid: Pid, cx: &mut Context<TableState<Self>>) {
        self.armed_kill = None;
        self.kill_error = None;
        let provider = self.provider.clone();
        cx.spawn(async move |this, cx| {
            let handle = ssh_runtime().spawn(async move {
                provider
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .kill_process(pid, Signal::Term)
            });
            let outcome = handle.await;
            let _ = this.update(cx, |state, cx| {
                match outcome {
                    Ok(Ok(())) => {
                        state.delegate_mut().ports.retain(|p| p.pid != Some(pid));
                    }
                    Ok(Err(e)) => state.delegate_mut().kill_error = Some(e.to_string()),
                    Err(join_err) => {
                        state.delegate_mut().kill_error =
                            Some(format!("kill task panicked: {join_err}"));
                    }
                }
                state.refresh(cx);
                cx.notify();
            });
        })
        .detach();
    }
}

impl TableDelegate for PortsDelegate {
    fn columns_count(&self, _cx: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _cx: &App) -> usize {
        self.ports.len()
    }

    fn column(&self, col_ix: usize, _cx: &App) -> &Column {
        &self.columns[col_ix]
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let port = self.ports[row_ix].clone();
        // `ElementId` has no `From<(&str, usize, usize)>` impl — fold (row, col) into a
        // single index (5 columns, generous multiplier) instead of a 3-tuple.
        let cell_id = ("net-cell", row_ix * 8 + col_ix);
        match col_ix {
            0 => {
                let label = match port.protocol {
                    Protocol::Tcp => "tcp",
                    Protocol::Udp => "udp",
                };
                div()
                    .id(cell_id)
                    .px_2()
                    .text_xs()
                    .text_color(rgb(FG))
                    .child(label)
            }
            1 => div()
                .id(cell_id)
                .px_2()
                .text_xs()
                .text_color(rgb(FG))
                .child(port.port.to_string()),
            2 => {
                let label: SharedString = port
                    .pid
                    .map(|p| p.as_u32().to_string())
                    .unwrap_or_else(|| "—".to_string())
                    .into();
                div()
                    .id(cell_id)
                    .px_2()
                    .text_xs()
                    .text_color(rgb(FG_DIM))
                    .child(label)
            }
            3 => {
                let label: SharedString = if port.command.is_empty() {
                    "—".into()
                } else {
                    port.command.clone().into()
                };
                div()
                    .id(cell_id)
                    .px_2()
                    .text_xs()
                    .text_color(rgb(FG))
                    .child(label)
            }
            _ => {
                let Some(pid) = port.pid else {
                    return div()
                        .id(cell_id)
                        .px_2()
                        .text_xs()
                        .text_color(rgb(FG_DIM))
                        .child("—");
                };
                let armed = kill_click_executes(self.armed_kill, pid);
                let (label, color) = if armed {
                    ("kill?", DANGER)
                } else {
                    ("kill", FG_DIM)
                };
                div()
                    .id(cell_id)
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .text_xs()
                    .cursor_pointer()
                    .text_color(rgb(color))
                    .hover(|s| s.bg(rgb(ACTIVE_BG)))
                    .child(label)
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                        if kill_click_executes(this.delegate().armed_kill, pid) {
                            this.delegate_mut().kill(pid, cx);
                        } else {
                            this.delegate_mut().armed_kill = Some(pid);
                            cx.notify();
                        }
                    }))
            }
        }
    }
}

impl AppState {
    pub(crate) fn network_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_network_table(window, cx);
        if !self.network.loaded {
            self.network.loaded = true;
            self.refresh_network(cx);
        }

        let table = self.network.table.clone();
        let port_count = table
            .as_ref()
            .map(|t| t.read(cx).delegate().ports.len())
            .unwrap_or(0);
        let kill_error = table
            .as_ref()
            .and_then(|t| t.read(cx).delegate().kill_error.clone());

        let sub: SharedString = match &self.network.error {
            Some(e) => format!("error: {e}").into(),
            None if self.network.refreshing => "refreshing…".into(),
            None => format!("{port_count} listening ports").into(),
        };
        let refresh_label = if self.network.refreshing {
            "…"
        } else {
            "⟳ refresh"
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
                            .id("net-refresh")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .text_sm()
                            .cursor_pointer()
                            .text_color(rgb(BRAND))
                            .hover(|s| s.bg(rgb(ACTIVE_BG)))
                            .child(refresh_label)
                            .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| {
                                this.refresh_network(cx);
                            })),
                    ),
            )
            .child(self.interfaces_strip())
            .children(table.map(|t| div().flex_1().w_full().child(Table::new(&t).stripe(true))))
            .children(kill_error.map(|e| {
                div()
                    .px_4()
                    .py_1()
                    .text_xs()
                    .text_color(rgb(DANGER))
                    .child(format!("✗ {e}"))
            }))
            .into_any_element()
    }

    /// Interfaces summary strip: name · addrs · up/down · rx/tx (humanized), default
    /// route interface first (`sort_interfaces_default_first`, applied on refresh).
    fn interfaces_strip(&self) -> impl IntoElement + use<> {
        let default_name = self.network.default_route.clone();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(rgb(BORDER))
            .children(self.network.interfaces.iter().map(|iface| {
                let is_default = default_name.as_deref() == Some(iface.name.as_str());
                let addrs: SharedString = if iface.addrs.is_empty() {
                    "no addrs".into()
                } else {
                    iface.addrs.join(", ").into()
                };
                let (status_label, status_color) = if iface.is_up {
                    ("up", BRAND)
                } else {
                    ("down", FG_DIM)
                };
                let throughput: SharedString = format!(
                    "↓{} ↑{}",
                    humanize_bytes(iface.rx_bytes),
                    humanize_bytes(iface.tx_bytes)
                )
                .into();
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .w(px(120.))
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(FG))
                            .child(iface.name.clone()),
                    )
                    .when(is_default, |el| {
                        el.child(div().text_xs().text_color(rgb(BRAND)).child("★"))
                    })
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(rgb(FG_DIM))
                            .child(addrs),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(status_color))
                            .child(status_label),
                    )
                    .child(div().text_xs().text_color(rgb(FG_DIM)).child(throughput))
            }))
    }

    /// Lazily build the ports table on first paint of the Network tab. Idempotent
    /// (checked every render) — mirrors `db_tab.rs`'s `ensure_query_widgets`.
    fn ensure_network_table(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.network.table.is_some() {
            return;
        }
        let provider = self.network.provider.clone();
        let table = cx.new(|cx| TableState::new(PortsDelegate::new(provider), window, cx));
        self.network.table = Some(table);
    }

    /// ⟳ refresh: re-probe ports, interfaces, and the default route on the shared
    /// runtime, then apply the results. No blocking in `render` — this only ever runs
    /// from a click or the lazy first-paint trigger in `network_tab`.
    pub(crate) fn refresh_network(&mut self, cx: &mut Context<Self>) {
        if self.network.refreshing {
            return;
        }
        self.network.refreshing = true;
        self.network.error = None;
        cx.notify();

        let provider = self.network.provider.clone();
        let table = self.network.table.clone();

        cx.spawn(async move |this, cx| {
            let handle = ssh_runtime().spawn(async move {
                let mut guard = provider.lock().unwrap_or_else(|e| e.into_inner());
                (
                    guard.list_listening_ports(),
                    guard.list_interfaces(),
                    guard.default_route_iface_name(),
                )
            });
            let outcome = handle.await;
            let _ = this.update(cx, |this, cx| {
                this.network.refreshing = false;
                match outcome {
                    Ok((ports_res, ifaces_res, route_res)) => {
                        let mut err = None;
                        match ports_res {
                            Ok(ports) => {
                                if let Some(table) = &table {
                                    table.update(cx, |state, cx| {
                                        state.delegate_mut().set_ports(ports);
                                        state.refresh(cx);
                                    });
                                }
                            }
                            Err(e) => err = Some(e.to_string()),
                        }
                        match ifaces_res {
                            Ok(mut ifaces) => {
                                let default_name = route_res.unwrap_or(None);
                                sort_interfaces_default_first(&mut ifaces, default_name.as_deref());
                                this.network.default_route = default_name;
                                this.network.interfaces = ifaces;
                            }
                            Err(e) => {
                                if err.is_none() {
                                    err = Some(e.to_string());
                                }
                            }
                        }
                        this.network.error = err;
                    }
                    Err(join_err) => {
                        this.network.error =
                            Some(format!("network probe task panicked: {join_err}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

// ---- pure helpers (unit-tested) ---------------------------------------------------

/// Two-click kill confirm: `true` when `clicked` is the pid already armed. Mirrors
/// `app::delete_click_executes`, keyed on `Pid` since kill targets a process, not a row.
fn kill_click_executes(armed: Option<Pid>, clicked: Pid) -> bool {
    armed == Some(clicked)
}

/// Human-readable byte count (binary units, one decimal place above `B`) — e.g. "340 B",
/// "1.2 MB". Pure so it's unit-tested without touching a real interface counter.
fn humanize_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit_ix = 0;
    while value >= 1024.0 && unit_ix < UNITS.len() - 1 {
        value /= 1024.0;
        unit_ix += 1;
    }
    if unit_ix == 0 {
        format!("{bytes} {}", UNITS[unit_ix])
    } else {
        format!("{value:.1} {}", UNITS[unit_ix])
    }
}

/// Put the default-route interface first (if present among `interfaces`), then sort the
/// rest by name. Pure so it's unit-tested without touching a real routing table.
fn sort_interfaces_default_first(interfaces: &mut [NetInterface], default_name: Option<&str>) {
    interfaces.sort_by(|a, b| {
        let a_is_default = default_name == Some(a.name.as_str());
        let b_is_default = default_name == Some(b.name.as_str());
        b_is_default
            .cmp(&a_is_default)
            .then_with(|| a.name.cmp(&b.name))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(name: &str) -> NetInterface {
        NetInterface {
            name: name.to_string(),
            addrs: Vec::new(),
            rx_bytes: 0,
            tx_bytes: 0,
            is_up: true,
        }
    }

    #[test]
    fn kill_needs_two_clicks_on_the_same_pid() {
        let pid = Pid::from_u32(123);
        let other = Pid::from_u32(456);
        assert!(!kill_click_executes(None, pid));
        assert!(kill_click_executes(Some(pid), pid));
        assert!(!kill_click_executes(Some(pid), other));
    }

    #[test]
    fn humanize_bytes_picks_the_right_unit() {
        assert_eq!(humanize_bytes(0), "0 B");
        assert_eq!(humanize_bytes(340), "340 B");
        assert_eq!(humanize_bytes(1024), "1.0 KB");
        assert_eq!(humanize_bytes(1_536), "1.5 KB");
        assert_eq!(humanize_bytes(1_048_576), "1.0 MB");
    }

    #[test]
    fn default_route_iface_sorts_first_then_by_name() {
        let mut ifaces = vec![iface("wlan0"), iface("eth0"), iface("lo")];
        sort_interfaces_default_first(&mut ifaces, Some("wlan0"));
        let names: Vec<&str> = ifaces.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["wlan0", "eth0", "lo"]);
    }

    #[test]
    fn no_default_route_sorts_all_by_name() {
        let mut ifaces = vec![iface("wlan0"), iface("eth0")];
        sort_interfaces_default_first(&mut ifaces, None);
        let names: Vec<&str> = ifaces.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["eth0", "wlan0"]);
    }
}
