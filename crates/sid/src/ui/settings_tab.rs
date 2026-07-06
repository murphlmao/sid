//! Settings screen (round-E §C): a single, spacey scrollable column with four
//! sections — Theme, Behavior, Keyboard, Storage.
//!
//! Follows the `ui::db_tab`/`ui::network_tab`/`ui::systems_tab` module convention:
//! state lives in [`SettingsTabState`], and every render/mutation method is a
//! second `impl AppState` block here rather than in `app.rs`. Unlike those tabs,
//! there is no lazy widget construction (no `Table`/`Input` to build on first
//! paint) and no live/ephemeral probing — this screen is a thin, direct view over
//! `Store::settings()`/`set_settings`, so `settings_tab` itself takes `&self`.
//!
//! Every mutating control (`set_theme`/`set_default_scope`/
//! `set_file_browser_side_pref`/`set_secret_keyring_enabled`) is a full
//! read-modify-write of the whole `Settings` struct — see the `persist_*` free
//! functions below, which are the actual store round-trip, extracted gpui-free so
//! they're unit-tested against a tmp `Store` directly (round-E §C.4). Where
//! `AppState` already caches a mirror of a setting (`file_browser_side`, fanned
//! out to every live SSH session), the click handler updates that mirror the same
//! way the file panel's own `⇄ dock` toggle does (`AppState::toggle_dock_side`).
//!
//! The theme switch additionally installs the new palette as the process-wide
//! `Theme` global, syncs gpui-component's own `ThemeMode` (via `theme::
//! component_mode` — added here since no other track had landed it yet at the
//! time this file was written; a parallel theme-sweep track owns wiring the same
//! helper into `main.rs`'s startup path), and refreshes every window so the
//! switch is visible immediately, per round-E §C.1.

use gpui::{AnyElement, ClickEvent, Context, div, prelude::*, px, rgb};
use sid_store::{DefaultScope, PanelSide, Settings, Store};

use crate::app::AppState;
use crate::keymap;
use crate::ui::theme;

/// Monospace family for the Storage section's paths — aligned with `app.rs`'s own
/// `MONO` const. Kept local so `ui` stays self-contained (same convention as
/// `network_tab.rs`/`systems_tab.rs`'s local color consts).
const MONO: &str = "DejaVu Sans Mono";

/// Settings tab state: a cached snapshot of the persisted [`Settings`] (loaded
/// once in `AppState::new`, refreshed after every successful write — never
/// re-read from `render` itself, per this crate's "render never does I/O" rule;
/// see `app.rs`'s module doc) plus the one thing that can go wrong: a failed
/// `Store::set_settings` write surfaces here instead of silently no-opping.
/// No form, no modal — every control on this screen is a direct,
/// read-modify-write control.
pub struct SettingsTabState {
    cached: Settings,
    error: Option<String>,
}

impl SettingsTabState {
    pub(crate) fn new(store: &Store) -> Self {
        Self {
            cached: store.settings().unwrap_or_default(),
            error: None,
        }
    }
}

// ---- store round-trip (pure-of-gpui, unit-tested) --------------------------

/// Read-modify-write `Settings::theme`. `AppState::set_theme` wraps this with the
/// live-switch side effects (palette install, gpui-component mode sync, window
/// refresh) — this function is only the persisted half.
fn persist_theme(store: &Store, name: &str) -> sid_store::Result<()> {
    let mut settings = store.settings()?;
    settings.theme = name.to_string();
    store.set_settings(&settings)
}

/// Read-modify-write `Settings::default_scope`.
fn persist_default_scope(store: &Store, scope: DefaultScope) -> sid_store::Result<()> {
    let mut settings = store.settings()?;
    settings.default_scope = scope;
    store.set_settings(&settings)
}

/// Read-modify-write `Settings::file_browser_side`.
fn persist_file_browser_side(store: &Store, side: PanelSide) -> sid_store::Result<()> {
    let mut settings = store.settings()?;
    settings.file_browser_side = side;
    store.set_settings(&settings)
}

/// Read-modify-write `Settings::secret_keyring_enabled`.
fn persist_secret_keyring_enabled(store: &Store, enabled: bool) -> sid_store::Result<()> {
    let mut settings = store.settings()?;
    settings.secret_keyring_enabled = enabled;
    store.set_settings(&settings)
}

// ---- render pieces (free functions — no `self` needed) ---------------------

/// A section header — spacey pass §B.2: `text_xs` UPPERCASE `muted` label with
/// `mb_2`.
fn section_header(chrome: &theme::Theme, label: &str) -> impl IntoElement {
    div()
        .text_xs()
        .text_color(rgb(chrome.muted))
        .mb_2()
        .child(label.to_uppercase())
}

/// One labeled control inside a section: a muted label above, the interactive
/// content below.
fn labeled_row(
    chrome: &theme::Theme,
    label: &'static str,
    content: impl IntoElement,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_xs().text_color(rgb(chrome.muted)).child(label))
        .child(content)
}

/// The Keyboard section: a read-only two-column list of `keymap::ALL_ACTIONS` +
/// their `primary_shortcut`, mirroring `app.rs`'s `?` cheat-sheet overlay content
/// (round-E §C.2) — "rebinding comes later" per the plan.
fn keyboard_section(chrome: &theme::Theme) -> impl IntoElement {
    let bindings = keymap::default_bindings();
    let rows = keymap::ALL_ACTIONS.iter().map(|&action| {
        let shortcut = keymap::primary_shortcut(action, &bindings).unwrap_or_else(|| "—".into());
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_4()
            .py_1()
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(chrome.fg))
                    .child(action.label()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(chrome.accent))
                    .child(shortcut),
            )
    });

    div()
        .flex()
        .flex_col()
        .child(section_header(chrome, "keyboard"))
        .child(div().flex().flex_col().gap_1().children(rows))
        .child(
            div()
                .mt_2()
                .text_xs()
                .text_color(rgb(chrome.faint))
                .child("rebinding comes later"),
        )
}

/// The Storage section: the global data dir + the two files that live under it,
/// plus a note that the encrypted-file secret vault (round-D §A) is dormant.
/// Paths are recomputed here rather than exposed from `app.rs` — `data_dir()` is
/// already `pub`, and `store.redb`/`demo.db` are the exact literal filenames
/// `app::open_store`/`app::seed_if_empty` join onto it.
fn storage_section(chrome: &theme::Theme) -> impl IntoElement {
    let data_dir = crate::app::data_dir();
    let store_path = data_dir.join("store.redb");
    let demo_db_path = data_dir.join("demo.db");

    let path_row = |label: &'static str, path: std::path::PathBuf| {
        labeled_row(
            chrome,
            label,
            div()
                .text_sm()
                .font_family(MONO)
                .text_color(rgb(chrome.fg))
                .child(path.to_string_lossy().into_owned()),
        )
    };

    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(section_header(chrome, "storage"))
        .child(path_row("data directory", data_dir))
        .child(path_row("store file", store_path))
        .child(path_row("demo database", demo_db_path))
        .child(div().text_xs().text_color(rgb(chrome.faint)).child(
            "the encrypted-file secret vault is dormant (round D §A) — keyring or \
                 in-memory only",
        ))
}

impl AppState {
    /// Round-E §C: the Settings screen. Takes `&self` (not `&mut self`) — there is
    /// no lazy widget to build on first paint, unlike `db_tab`/`network_tab`/
    /// `systems_tab`; every mutation happens through a control's own click
    /// handler (the `set_*` methods below), never from render itself.
    pub(crate) fn settings_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let chrome = theme::active(cx).clone();
        let settings = self.settings.cached.clone();

        let error = self.settings.error.clone().map(|e| {
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .text_xs()
                .text_color(rgb(chrome.danger))
                .child(format!("error: {e}"))
        });

        // The active marker follows the LIVE theme (not the persisted name): identical
        // in normal use (set_theme installs + persists together), and honest under the
        // SID_THEME per-run override, where the persisted value deliberately differs.
        let theme_section = self.theme_section(&chrome, chrome.name, cx);
        let behavior_section = self.behavior_section(&chrome, &settings, cx);

        div()
            .id("settings-tab")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .p_4()
            .gap_3()
            .bg(rgb(chrome.bg))
            .text_color(rgb(chrome.fg))
            .children(error)
            .child(theme_section)
            .child(behavior_section)
            .child(keyboard_section(&chrome))
            .child(storage_section(&chrome))
            .into_any_element()
    }

    // ---- Theme section --------------------------------------------------------

    fn theme_section(
        &self,
        chrome: &theme::Theme,
        applied: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(section_header(chrome, "theme"))
            .children(
                theme::THEME_NAMES
                    .iter()
                    .enumerate()
                    .map(|(ix, &name)| self.theme_row(ix, name, chrome, applied, cx)),
            )
    }

    /// One row per `theme::THEME_NAMES` entry: an active marker, the theme's name,
    /// and five small swatches previewing its own palette (bg, surface, accent,
    /// success, danger). Clicking anywhere on the row applies it.
    fn theme_row(
        &self,
        ix: usize,
        name: &'static str,
        chrome: &theme::Theme,
        applied: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let palette = theme::by_name(name);
        let active = name == applied;
        let swatch = |color: u32| {
            div()
                .w(px(16.))
                .h(px(16.))
                .rounded_md()
                .bg(rgb(color))
                .border_1()
                .border_color(rgb(chrome.border))
        };

        div()
            .id(("theme-row", ix))
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .px_3()
            .py_2()
            .rounded_md()
            .cursor_pointer()
            .bg(rgb(if active {
                chrome.selection
            } else {
                chrome.surface
            }))
            .border_1()
            .border_color(rgb(if active { chrome.accent } else { chrome.border }))
            .child(
                div()
                    .w(px(14.))
                    .text_color(rgb(if active { chrome.accent } else { chrome.faint }))
                    .child(if active { "●" } else { "○" }),
            )
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .text_color(rgb(if active { chrome.fg_strong } else { chrome.fg }))
                    .child(name),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(swatch(palette.bg))
                    .child(swatch(palette.surface))
                    .child(swatch(palette.accent))
                    .child(swatch(palette.success))
                    .child(swatch(palette.danger)),
            )
            .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                this.set_theme(name, cx);
            }))
    }

    /// Switch the active theme (round-E §C.1): install it as the process-wide
    /// palette, sync gpui-component's own chrome mode so its widgets (the SQL
    /// editor, tables, …) never end up mismatched against the active sid
    /// palette, persist the choice, and refresh every window so the switch is
    /// LIVE. `pub(crate)` so this module's tests can drive it directly, and so a
    /// future command-palette "switch theme" entry could reuse it.
    pub(crate) fn set_theme(&mut self, name: &'static str, cx: &mut Context<Self>) {
        theme::install(name, cx);
        let mode = theme::component_mode(theme::active(cx));
        gpui_component::Theme::change(mode, None, cx);
        match persist_theme(&self.store, name) {
            Ok(()) => {
                self.settings.cached.theme = name.to_string();
                self.settings.error = None;
            }
            Err(e) => self.settings.error = Some(format!("failed to save theme: {e}")),
        }
        cx.refresh_windows();
        cx.notify();
    }

    // ---- Behavior section -------------------------------------------------

    fn behavior_section(
        &self,
        chrome: &theme::Theme,
        settings: &Settings,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let default_scope = self.default_scope_selector(chrome, settings.default_scope, cx);
        let file_browser_side =
            self.file_browser_side_selector(chrome, settings.file_browser_side, cx);
        let keyring = self.secret_keyring_selector(chrome, settings.secret_keyring_enabled, cx);

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(section_header(chrome, "behavior"))
            .child(labeled_row(
                chrome,
                "default scope for new items",
                default_scope,
            ))
            .child(labeled_row(chrome, "file browser side", file_browser_side))
            .child(labeled_row(chrome, "secret keyring", keyring))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(chrome.muted))
                    .child(self.secrets_status_detail.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(chrome.faint))
                    .child("changes take effect on restart"),
            )
    }

    fn default_scope_selector(
        &self,
        chrome: &theme::Theme,
        current: DefaultScope,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let options = [
            (DefaultScope::Global, "global"),
            (DefaultScope::Workspace, "workspace"),
            (DefaultScope::Ask, "ask"),
        ];
        div()
            .flex()
            .flex_row()
            .gap_2()
            .children(options.into_iter().enumerate().map(|(ix, (scope, label))| {
                let active = current == scope;
                div()
                    .id(("default-scope", ix))
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .bg(rgb(if active {
                        chrome.selection
                    } else {
                        chrome.surface
                    }))
                    .border_1()
                    .border_color(rgb(if active { chrome.accent } else { chrome.border }))
                    .text_color(rgb(if active {
                        chrome.fg_strong
                    } else {
                        chrome.muted
                    }))
                    .child(label)
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                        this.set_default_scope(scope, cx);
                    }))
            }))
    }

    pub(crate) fn set_default_scope(&mut self, scope: DefaultScope, cx: &mut Context<Self>) {
        match persist_default_scope(&self.store, scope) {
            Ok(()) => {
                self.settings.cached.default_scope = scope;
                self.settings.error = None;
            }
            Err(e) => self.settings.error = Some(format!("failed to save default scope: {e}")),
        }
        cx.notify();
    }

    fn file_browser_side_selector(
        &self,
        chrome: &theme::Theme,
        current: PanelSide,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let options = [(PanelSide::Left, "left"), (PanelSide::Right, "right")];
        div()
            .flex()
            .flex_row()
            .gap_2()
            .children(options.into_iter().enumerate().map(|(ix, (side, label))| {
                let active = current == side;
                div()
                    .id(("file-browser-side", ix))
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .bg(rgb(if active {
                        chrome.selection
                    } else {
                        chrome.surface
                    }))
                    .border_1()
                    .border_color(rgb(if active { chrome.accent } else { chrome.border }))
                    .text_color(rgb(if active {
                        chrome.fg_strong
                    } else {
                        chrome.muted
                    }))
                    .child(label)
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                        this.set_file_browser_side_pref(side, cx);
                    }))
            }))
    }

    /// Persist `Settings::file_browser_side`, then fan the new value out to every
    /// live SSH session and the cached `AppState::file_browser_side` mirror —
    /// exactly what the file panel's own `⇄ dock` toggle does
    /// (`AppState::toggle_dock_side`); this is just a second entry point to the
    /// identical cached-mirror + broadcast.
    pub(crate) fn set_file_browser_side_pref(&mut self, side: PanelSide, cx: &mut Context<Self>) {
        match persist_file_browser_side(&self.store, side) {
            Ok(()) => {
                self.settings.cached.file_browser_side = side;
                self.settings.error = None;
                self.file_browser_side = side;
                for tab in &self.ssh_sessions {
                    tab.session
                        .update(cx, |session, cx| session.set_dock_side(side, cx));
                }
            }
            Err(e) => self.settings.error = Some(format!("failed to save file browser side: {e}")),
        }
        cx.notify();
    }

    fn secret_keyring_selector(
        &self,
        chrome: &theme::Theme,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let options = [(true, "enabled"), (false, "disabled")];
        div()
            .flex()
            .flex_row()
            .gap_2()
            .children(options.into_iter().enumerate().map(|(ix, (value, label))| {
                let active = enabled == value;
                div()
                    .id(("secret-keyring", ix))
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .bg(rgb(if active {
                        chrome.selection
                    } else {
                        chrome.surface
                    }))
                    .border_1()
                    .border_color(rgb(if active { chrome.accent } else { chrome.border }))
                    .text_color(rgb(if active {
                        chrome.fg_strong
                    } else {
                        chrome.muted
                    }))
                    .child(label)
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                        this.set_secret_keyring_enabled(value, cx);
                    }))
            }))
    }

    pub(crate) fn set_secret_keyring_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        match persist_secret_keyring_enabled(&self.store, enabled) {
            Ok(()) => {
                self.settings.cached.secret_keyring_enabled = enabled;
                self.settings.error = None;
            }
            Err(e) => self.settings.error = Some(format!("failed to save keyring setting: {e}")),
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().expect("tmp dir");
        let store = Store::open(&dir.path().join("store.redb")).expect("open tmp store");
        (dir, store)
    }

    #[test]
    fn persist_theme_round_trips() {
        let (_dir, store) = tmp_store();
        persist_theme(&store, "void").expect("persist theme");
        assert_eq!(store.settings().unwrap().theme, "void");
    }

    #[test]
    fn persist_default_scope_round_trips() {
        let (_dir, store) = tmp_store();
        persist_default_scope(&store, DefaultScope::Workspace).expect("persist scope");
        assert_eq!(
            store.settings().unwrap().default_scope,
            DefaultScope::Workspace
        );
    }

    #[test]
    fn persist_file_browser_side_round_trips() {
        let (_dir, store) = tmp_store();
        persist_file_browser_side(&store, PanelSide::Right).expect("persist side");
        assert_eq!(
            store.settings().unwrap().file_browser_side,
            PanelSide::Right
        );
    }

    #[test]
    fn persist_secret_keyring_enabled_round_trips() {
        let (_dir, store) = tmp_store();
        persist_secret_keyring_enabled(&store, false).expect("persist keyring toggle");
        assert!(!store.settings().unwrap().secret_keyring_enabled);
    }

    #[test]
    fn persisting_one_field_preserves_the_others() {
        let (_dir, store) = tmp_store();
        persist_default_scope(&store, DefaultScope::Global).expect("persist scope");
        persist_theme(&store, "dusk").expect("persist theme");
        let settings = store.settings().unwrap();
        assert_eq!(settings.theme, "dusk");
        assert_eq!(
            settings.default_scope,
            DefaultScope::Global,
            "an earlier write must survive a later, unrelated write"
        );
    }

    #[test]
    fn settings_tab_state_caches_the_stores_settings_at_construction() {
        let (_dir, store) = tmp_store();
        persist_theme(&store, "dusk").expect("persist theme");
        let state = SettingsTabState::new(&store);
        assert_eq!(state.cached.theme, "dusk");
        assert!(state.error.is_none());
    }
}
