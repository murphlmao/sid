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

use gpui::{AnyElement, ClickEvent, Context, Keystroke, div, prelude::*, px, rgb};
use sid_store::{DefaultScope, KeyBinding, PanelSide, Settings, Store};

use crate::app::AppState;
use crate::keymap::{self, Action, Chord, RebindOutcome};
use sid_ui::Typography as _;
use sid_ui::{Kbd, theme};

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
    /// The persisted keybinding overrides, cached under the same rule as [`Self::cached`]
    /// (loaded once, refreshed after every successful write — render never reads the
    /// store). The effective registry is recomposed from these on every keystroke, which
    /// is why they are held here rather than as a `Vec<Binding>`: overrides are the
    /// small, authoritative thing; the registry is derived.
    keybindings: Vec<KeyBinding>,
    /// The action whose next keystroke is being captured, if any. Capture mode is
    /// **self-terminating**: any decisive keystroke ends it (see
    /// [`AppState::capture_rebind`]), so it can never sit armed swallowing keys.
    capturing: Option<Action>,
    /// The last rebind attempt's inline verdict, shown under its own row. Only refusals
    /// land here — a successful rebind speaks for itself by changing the chip.
    notice: Option<(Action, String)>,
}

impl SettingsTabState {
    pub(crate) fn new(store: &Store) -> Self {
        Self {
            cached: store.settings().unwrap_or_default(),
            error: None,
            keybindings: load_overrides(store),
            capturing: None,
            notice: None,
        }
    }

    /// Whether a rebind capture is armed. `app.rs`'s root key handler asks this before
    /// it resolves anything — capture mode has to see chords the registry would
    /// otherwise claim (`Ctrl+2` switches tabs).
    pub(crate) fn is_capturing(&self) -> bool {
        self.capturing.is_some()
    }

    /// The stored overrides, parsed into the keymap's validated form. Rows this build
    /// can't trust (unknown action id, key outside the allowlist) are dropped here, so
    /// that action simply keeps its default.
    fn overrides(&self) -> Vec<keymap::KeyOverride> {
        self.keybindings
            .iter()
            .filter_map(|b| keymap::parse_override(&b.action, &b.key, b.ctrl, b.shift))
            .collect()
    }

    /// Whether `action` currently carries a user override (drives the "custom" marker
    /// and whether a per-row reset is offered at all).
    fn is_overridden(&self, action: Action) -> bool {
        self.keybindings.iter().any(|b| b.action == action.id())
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

// ---- keymap round-trip (pure-of-gpui, unit-tested) -------------------------

/// Read every stored override. A read failure degrades to "no overrides" rather than
/// blocking the whole Settings screen: a broken keymap table must not cost the user
/// their theme and storage panes too. The defaults still work, which is exactly the
/// state a user with no overrides is in.
fn load_overrides(store: &Store) -> Vec<KeyBinding> {
    store.keybindings().unwrap_or_default()
}

/// The stored shape of a captured chord. `shift_held` is the physical modifier state,
/// used only for symbol keys — those match shift-agnostically (see `Chord::shift`), so
/// the chord itself has no shift to record and the keystroke is the honest thing to
/// store.
fn keybinding_record(action: Action, chord: &Chord, shift_held: bool) -> KeyBinding {
    KeyBinding {
        action: action.id().to_string(),
        key: chord.key.to_string(),
        ctrl: chord.ctrl,
        shift: chord.shift.unwrap_or(shift_held),
    }
}

/// Persist one accepted rebind. Unlike the `persist_*` helpers above this is *not* a
/// read-modify-write of a blob — the keybindings table is keyed by action id, so one
/// rebind writes exactly one row and can't disturb another action's.
fn persist_keybinding(
    store: &Store,
    action: Action,
    chord: &Chord,
    shift_held: bool,
) -> sid_store::Result<()> {
    store.set_keybinding(&keybinding_record(action, chord, shift_held))
}

/// Drop one action's override, returning whether one was set.
fn persist_reset_binding(store: &Store, action: Action) -> sid_store::Result<bool> {
    store.clear_keybinding(action.id())
}

/// Drop every override, returning how many were dropped.
fn persist_reset_all(store: &Store) -> sid_store::Result<usize> {
    store.clear_all_keybindings()
}

/// The inline message for a refused rebind — the one place an outcome becomes prose.
/// `None` for [`RebindOutcome::Applied`]: a rebind that worked needs no words, the chip
/// already changed.
fn refusal_message(outcome: &RebindOutcome) -> Option<String> {
    match outcome {
        RebindOutcome::Applied { .. } => None,
        // Naming the owner is the whole point of refusing instead of stealing: it turns
        // a dead end into a two-step fix (reset that action, then rebind this one).
        RebindOutcome::Conflict { with } => Some(format!(
            "already bound to {} — reset that first",
            with.label()
        )),
        RebindOutcome::Reserved { reason } | RebindOutcome::Invalid { reason } => {
            Some((*reason).to_string())
        }
    }
}

// ---- render pieces (free functions — no `self` needed) ---------------------

/// A section header: the [`Label`] role, uppercased, with `mb_2`.
///
/// [`Label`]: sid_ui::TypeRole::Label
fn section_header(chrome: &theme::Theme, label: &str) -> impl IntoElement {
    div().text_label(chrome).mb_2().child(label.to_uppercase())
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
        .child(div().text_meta(chrome).child(label))
        .child(content)
}

/// A small inline control (rebind / reset / cancel) — the row-scale button this screen
/// needs and `sid_ui` doesn't have a variant of. Deliberately built here rather than in
/// the component crate: it is one row's affordance, and `sid_ui::Button` is a
/// full-height form button that would out-shout the binding chip beside it.
fn row_button(
    chrome: &theme::Theme,
    id: (&'static str, usize),
    label: &'static str,
    emphasis: u32,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_2()
        .py_0p5()
        .rounded_md()
        .cursor_pointer()
        .border_1()
        .border_color(rgb(chrome.border))
        .bg(rgb(chrome.surface))
        .text_meta(chrome)
        .text_color(rgb(emphasis))
        .hover(|s| s.bg(rgb(chrome.selection)))
        .child(label)
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
                .text_mono(chrome)
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
        .child(div().text_meta(chrome).text_color(rgb(chrome.faint)).child(
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
                .text_meta(&chrome)
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
            .bg(rgb(chrome.bg))
            .text_color(rgb(chrome.fg))
            // Width-capped, centered content column: full-bleed settings rows on a
            // wide window put a label on the left and its value/shortcut a full
            // screen-width away (design review — same rule as the SSH home surface).
            .items_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(880.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children(error)
                    .child(theme_section)
                    .child(behavior_section)
                    .child(self.keymap_section(&chrome, cx))
                    .child(storage_section(&chrome)),
            )
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
                    .text_body(chrome)
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
        // Re-project the new palette onto gpui-component's theme so the borrowed
        // widgets (SQL editor, tables, popup menus) follow the switch LIVE — see
        // `sid_ui::bridge`. The `cx.refresh_windows()` below repaints everything.
        sid_ui::bridge::sync(None, cx);
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
                    .text_meta(chrome)
                    .child(self.secrets_status_detail.clone()),
            )
            .child(
                div()
                    .text_meta(chrome)
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
                    .text_body(chrome)
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
                    .text_body(chrome)
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
                    .text_body(chrome)
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

    // ---- Keyboard section (the rebinding editor) ---------------------------

    /// **The** answer to "what is bound right now", for every consumer: the root key
    /// handler, the command palette, the `?` cheat sheet and this screen all resolve
    /// through here, so a rebind is live everywhere the moment it is written — no
    /// second copy of the registry to keep in step.
    pub(crate) fn effective_bindings(&self) -> Vec<keymap::Binding> {
        keymap::effective_bindings(&self.settings.overrides())
    }

    /// The Keyboard section: one row per [`keymap::Action`], its effective binding as a
    /// `Kbd` chip, and the affordances to change it.
    fn keymap_section(
        &self,
        chrome: &theme::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let bindings = self.effective_bindings();
        let any_custom = !self.settings.keybindings.is_empty();
        let rows: Vec<_> = keymap::ALL_ACTIONS
            .iter()
            .enumerate()
            .map(|(ix, &action)| self.keymap_row(ix, action, &bindings, chrome, cx))
            .collect();

        let reset_all = any_custom.then(|| {
            row_button(chrome, ("keymap-reset-all", 0), "reset all", chrome.muted).on_click(
                cx.listener(|this, _ev: &ClickEvent, _window, cx| this.reset_all_bindings(cx)),
            )
        });

        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(section_header(chrome, "keyboard"))
                    .children(reset_all),
            )
            .child(div().flex().flex_col().children(rows))
            .child(
                div()
                    .mt_2()
                    .text_meta(chrome)
                    .text_color(rgb(chrome.faint))
                    .child(
                        "shortcuts carry Ctrl; inside a focused terminal a letter chord \
                         reaches sid as Ctrl+Shift+<key> and the shell keeps the plain one",
                    ),
            )
    }

    /// One action's row: label, current binding, and — depending on state — the rebind /
    /// cancel / reset affordances, with any refusal message underneath it. The message
    /// sits on the row it belongs to rather than in the screen-level error strip:
    /// "already bound to Command Palette" is only meaningful next to the row that asked.
    fn keymap_row(
        &self,
        ix: usize,
        action: Action,
        bindings: &[keymap::Binding],
        chrome: &theme::Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let capturing = self.settings.capturing == Some(action);
        let custom = self.settings.is_overridden(action);
        let shortcut = keymap::primary_shortcut(action, bindings);
        let notice = self
            .settings
            .notice
            .as_ref()
            .filter(|(a, _)| *a == action)
            .map(|(_, message)| message.clone());

        // While capturing, the chip is replaced by the prompt: the row is asking a
        // question, and showing the old binding next to "press a chord" reads as if the
        // old one were still an answer.
        let binding_cell = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(if capturing {
                div()
                    .text_meta(chrome)
                    .text_color(rgb(chrome.accent))
                    .child("press a chord — Esc to cancel")
                    .into_any_element()
            } else {
                match shortcut {
                    Some(spec) => Kbd::new(spec).into_any_element(),
                    None => div().text_meta(chrome).child("—").into_any_element(),
                }
            });

        let custom_marker = (custom && !capturing).then(|| {
            div()
                .text_meta(chrome)
                .text_color(rgb(chrome.faint))
                .child("custom")
        });

        let rebind = if capturing {
            row_button(chrome, ("keymap-cancel", ix), "cancel", chrome.muted)
                .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| this.cancel_rebind(cx)))
        } else {
            row_button(chrome, ("keymap-rebind", ix), "rebind", chrome.accent).on_click(
                cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                    this.begin_rebind(action, cx)
                }),
            )
        };

        let reset = (custom && !capturing).then(|| {
            row_button(chrome, ("keymap-reset", ix), "reset", chrome.muted).on_click(cx.listener(
                move |this, _ev: &ClickEvent, _window, cx| this.reset_binding(action, cx),
            ))
        });

        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(if capturing {
                        chrome.selection
                    } else {
                        chrome.bg
                    }))
                    .hover(|s| s.bg(rgb(chrome.selection)))
                    .child(div().flex_1().text_body(chrome).child(action.label()))
                    .children(custom_marker)
                    .child(binding_cell)
                    .child(rebind)
                    .children(reset),
            )
            .children(notice.map(|message| {
                div()
                    .px_3()
                    .pb_1()
                    .text_meta(chrome)
                    .text_color(rgb(chrome.danger))
                    .child(message)
            }))
    }

    /// Arm capture for `action`. Only one row captures at a time — arming a second
    /// silently disarms the first, which is what clicking a different row means.
    pub(crate) fn begin_rebind(&mut self, action: Action, cx: &mut Context<Self>) {
        self.settings.capturing = Some(action);
        self.settings.notice = None;
        cx.notify();
    }

    /// Disarm capture, leaving the binding as it was.
    pub(crate) fn cancel_rebind(&mut self, cx: &mut Context<Self>) {
        self.settings.capturing = None;
        self.settings.notice = None;
        cx.notify();
    }

    /// Consume the captured keystroke: validate it, ask [`keymap::resolve_rebind`], and
    /// either persist it or show why not.
    ///
    /// Capture is **self-terminating** — every path but a bare modifier disarms it. An
    /// armed capture swallows keystrokes app-wide (that is the point: it has to see
    /// `Ctrl+2`), so it must never be able to stay armed while the user's attention has
    /// moved on; at most one keystroke is ever consumed.
    pub(crate) fn capture_rebind(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) {
        let Some(action) = self.settings.capturing else {
            return;
        };
        // Holding Ctrl before the key is how a chord is typed, not a failed attempt.
        if matches!(
            keystroke.key.as_str(),
            "control" | "shift" | "alt" | "platform" | "function"
        ) {
            return;
        }
        self.settings.capturing = None;
        if keystroke.key.eq_ignore_ascii_case("escape") {
            self.settings.notice = None;
            cx.notify();
            return;
        }

        let chord = match keymap::chord_from_keystroke(keystroke) {
            Ok(chord) => chord,
            Err(why) => {
                self.settings.notice = Some((action, why.to_string()));
                cx.notify();
                return;
            }
        };
        let outcome = keymap::resolve_rebind(&self.effective_bindings(), action, chord);
        match refusal_message(&outcome) {
            Some(message) => self.settings.notice = Some((action, message)),
            None => {
                match persist_keybinding(&self.store, action, &chord, keystroke.modifiers.shift) {
                    Ok(()) => {
                        self.settings.keybindings = load_overrides(&self.store);
                        self.settings.notice = None;
                        self.settings.error = None;
                    }
                    Err(e) => self.settings.error = Some(format!("failed to save keybinding: {e}")),
                }
            }
        }
        cx.notify();
    }

    /// Reset one action to its default binding.
    pub(crate) fn reset_binding(&mut self, action: Action, cx: &mut Context<Self>) {
        match persist_reset_binding(&self.store, action) {
            Ok(_) => {
                self.settings.keybindings = load_overrides(&self.store);
                self.settings.notice = None;
                self.settings.error = None;
            }
            Err(e) => self.settings.error = Some(format!("failed to reset keybinding: {e}")),
        }
        cx.notify();
    }

    /// Reset the whole keymap to its defaults.
    pub(crate) fn reset_all_bindings(&mut self, cx: &mut Context<Self>) {
        match persist_reset_all(&self.store) {
            Ok(_) => {
                self.settings.keybindings = load_overrides(&self.store);
                self.settings.capturing = None;
                self.settings.notice = None;
                self.settings.error = None;
            }
            Err(e) => self.settings.error = Some(format!("failed to reset keymap: {e}")),
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

    // ---- the rebinding editor's store seam ---------------------------------

    fn chord(key: &'static str) -> Chord {
        Chord {
            key,
            ctrl: true,
            shift: Some(false),
        }
    }

    #[test]
    fn persist_keybinding_round_trips() {
        let (_dir, store) = tmp_store();
        persist_keybinding(&store, Action::CommandPalette, &chord("g"), false).expect("persist");
        let stored = store.keybindings().expect("read");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].action, "command_palette");
        assert_eq!(stored[0].key, "g");
        assert!(stored[0].ctrl);
        assert!(!stored[0].shift);
    }

    #[test]
    fn a_symbol_chord_records_the_shift_that_was_actually_held() {
        // The chord matches shift-agnostically, so the keystroke is the honest record.
        let (_dir, store) = tmp_store();
        let symbol = Chord {
            key: "?",
            ctrl: true,
            shift: None,
        };
        persist_keybinding(&store, Action::Settings, &symbol, true).expect("persist");
        assert!(store.keybindings().expect("read")[0].shift);
    }

    #[test]
    fn a_persisted_override_is_what_the_effective_registry_resolves() {
        // The whole seam end to end, minus gpui: write -> reload -> parse -> compose.
        let (_dir, store) = tmp_store();
        persist_keybinding(&store, Action::CommandPalette, &chord("g"), false).expect("persist");
        let state = SettingsTabState::new(&store);
        let bindings = keymap::effective_bindings(&state.overrides());
        assert_eq!(
            keymap::primary_shortcut(Action::CommandPalette, &bindings).as_deref(),
            Some("Ctrl+G"),
            "the stored override, not the Ctrl+K default"
        );
        assert!(state.is_overridden(Action::CommandPalette));
        assert!(!state.is_overridden(Action::Settings));
    }

    #[test]
    fn resetting_one_binding_leaves_the_others_and_restores_the_default() {
        let (_dir, store) = tmp_store();
        persist_keybinding(&store, Action::CommandPalette, &chord("g"), false).expect("p1");
        persist_keybinding(&store, Action::CheatSheet, &chord("j"), false).expect("p2");
        assert!(persist_reset_binding(&store, Action::CommandPalette).expect("reset"));

        let state = SettingsTabState::new(&store);
        assert!(!state.is_overridden(Action::CommandPalette));
        assert!(state.is_overridden(Action::CheatSheet));
        let bindings = keymap::effective_bindings(&state.overrides());
        assert_eq!(
            keymap::primary_shortcut(Action::CommandPalette, &bindings).as_deref(),
            Some("Ctrl+K"),
            "a reset action is back on its default chord"
        );
    }

    #[test]
    fn resetting_everything_restores_the_default_registry_exactly() {
        let (_dir, store) = tmp_store();
        persist_keybinding(&store, Action::CommandPalette, &chord("g"), false).expect("p1");
        persist_keybinding(&store, Action::CheatSheet, &chord("j"), false).expect("p2");
        assert_eq!(persist_reset_all(&store).expect("reset all"), 2);

        let state = SettingsTabState::new(&store);
        assert!(state.keybindings.is_empty());
        assert_eq!(
            keymap::effective_bindings(&state.overrides()),
            keymap::default_bindings()
        );
    }

    #[test]
    fn a_refusal_names_the_action_that_owns_the_chord() {
        let message = refusal_message(&RebindOutcome::Conflict {
            with: Action::CommandPalette,
        })
        .expect("a conflict has a message");
        assert!(
            message.contains(Action::CommandPalette.label()),
            "the user must be told WHICH action to reset: {message}"
        );
    }

    #[test]
    fn a_refused_rebind_says_why_and_an_applied_one_says_nothing() {
        assert_eq!(
            refusal_message(&RebindOutcome::Reserved {
                reason: "Ctrl+C is copy"
            })
            .as_deref(),
            Some("Ctrl+C is copy")
        );
        assert_eq!(
            refusal_message(&RebindOutcome::Invalid {
                reason: "a shortcut must include Ctrl"
            })
            .as_deref(),
            Some("a shortcut must include Ctrl")
        );
        assert_eq!(
            refusal_message(&RebindOutcome::Applied {
                bindings: Vec::new()
            }),
            None
        );
    }

    #[test]
    fn a_store_row_this_build_cannot_parse_falls_back_to_the_default() {
        // Forward compatibility: a keymap written by a newer sid must not brick this
        // one's keyboard.
        let (_dir, store) = tmp_store();
        store
            .set_keybinding(&KeyBinding {
                action: "teleport".into(),
                key: "g".into(),
                ctrl: true,
                shift: false,
            })
            .expect("write a row from the future");
        let state = SettingsTabState::new(&store);
        assert!(state.overrides().is_empty());
        assert_eq!(
            keymap::effective_bindings(&state.overrides()),
            keymap::default_bindings()
        );
    }

    #[test]
    fn a_fresh_state_is_not_capturing() {
        let (_dir, store) = tmp_store();
        let state = SettingsTabState::new(&store);
        assert!(!state.is_capturing(), "capture is never armed on load");
    }
}
