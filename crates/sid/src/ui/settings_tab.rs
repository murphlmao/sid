//! Settings screen (round-E §C): a left section rail — Appearance, Behaviour,
//! Keyboard, Storage — beside a content pane holding one panel at a time.
//!
//! It used to be one 880px column centred in the window, so a 2000px window was
//! 56% empty on *both* sides and every section stacked into one long scroll with
//! bare headers as the only structure. The column is still capped at 880px for
//! reading comfort, but it is anchored to the content gutter, and the width that
//! buys goes to the rail. Below ~900 design px (see [`rail_fits`]) the rail becomes
//! a `SegmentedControl` above the content, so nothing has to overflow sideways.
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

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, Context, KeyDownEvent, Keystroke, Pixels, Window, div, prelude::*,
    px, rgb,
};
use sid_store::{DefaultScope, KeyBinding, PanelSide, Settings, Store};

use crate::app::AppState;
use crate::keymap::{self, Action, Chord, RebindOutcome};
use sid_ui::{Card, Kbd, SegmentSelect, SegmentedControl, caveat_line, error_line, scaled, theme};
use sid_ui::{StyledExt as _, Typography as _};

// ---- the screen's shape -----------------------------------------------------

/// The section rail's width. Wide enough for the longest label at 150% zoom,
/// narrow enough that it reads as navigation serving the content rather than as a
/// peer of it.
const RAIL_W: f32 = 220.;

/// The widest a reading column gets, per the design system. Unlike the old
/// centred column this is a *cap*, not a centring rule — the pane starts at the
/// content gutter, so a wide window grows the empty space on one side only.
const CONTENT_MAX_W: f32 = 880.;

/// Below this window width the rail costs more than it orients: 220 + 880 no
/// longer fit side by side, and a squeezed column under a rail reads worse than a
/// full one under a segmented strip.
const RAIL_MIN_VIEWPORT: f32 = 900.;

/// Whether the window is wide enough for the section rail.
///
/// Both sides are *design* pixels: the breakpoint goes through [`scaled`] so a
/// 1920px window at 150% zoom (1280 design px of room) collapses to the strip,
/// which is the whole point of measuring in the same currency the layout is
/// authored in. Pure, so the rule is testable without a window.
fn rail_fits(viewport_width: Pixels, rem_size: Pixels) -> bool {
    viewport_width >= scaled(RAIL_MIN_VIEWPORT).to_pixels(rem_size)
}

/// The Settings screen's sections, in nav order.
///
/// The whole navigation state of this screen: which one of these is showing. There
/// is no scroll position to restore and no per-section state — a section is a
/// filter over what the content pane builds, nothing more.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SettingsSection {
    #[default]
    Appearance,
    Behaviour,
    Keyboard,
    Storage,
}

impl SettingsSection {
    /// Every section, in the order the nav lists them.
    const ALL: [Self; 4] = [
        Self::Appearance,
        Self::Behaviour,
        Self::Keyboard,
        Self::Storage,
    ];

    /// The nav item's label, and — uppercased by [`Card`] — its panel's header. One
    /// string for both so the two can never disagree about what a section is called.
    fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Behaviour => "Behaviour",
            Self::Keyboard => "Keyboard",
            Self::Storage => "Storage",
        }
    }

    /// This section's position in [`Self::ALL`] — the segmented strip's index.
    fn index(self) -> usize {
        Self::ALL.iter().position(|&s| s == self).unwrap_or(0)
    }

    /// The section at `index`, falling back to the default rather than panicking: the
    /// index comes from a click on a control built from the same list, so an
    /// out-of-range value can only mean the two drifted, and a wrong-but-showing
    /// screen beats a crashed one.
    fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }
}

/// How a nav click reaches `AppState`. `Rc` because both nav shapes hand the same
/// handler to every item.
type SectionSelect = Rc<dyn Fn(SettingsSection, &mut Window, &mut App)>;

/// The Settings frame: the section nav beside the content pane on a wide window,
/// above it on a narrow one.
///
/// A `RenderOnce` element rather than a plain function because the breakpoint needs
/// [`Window::viewport_size`], and `settings_tab` deliberately takes no `&mut Window`
/// (it builds no lazy widget, unlike every other tab). An element is handed the
/// window at render time, which is the cheapest way in and costs `app.rs` nothing.
#[derive(IntoElement)]
struct SettingsFrame {
    active: SettingsSection,
    on_select: SectionSelect,
    content: AnyElement,
}

impl RenderOnce for SettingsFrame {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme::active(cx).clone();
        let rail = rail_fits(window.viewport_size().width, window.rem_size());
        let nav = if rail {
            section_rail(&t, self.active, &self.on_select)
        } else {
            section_strip(&t, self.active, &self.on_select)
        };

        div()
            .flex()
            .map(|this| {
                if rail {
                    this.flex_row()
                } else {
                    this.flex_col()
                }
            })
            .flex_1()
            .min_h(px(0.))
            .bg(rgb(t.bg))
            .text_color(rgb(t.fg))
            .child(nav)
            .child(
                // The content gutter: capped for reading comfort, but anchored to the
                // pane's left edge. The old screen centred this column, so a 2000px
                // window put 560px of nothing on *both* sides of it.
                div()
                    .id("settings-content")
                    .flex_1()
                    .min_w(px(0.))
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .p_3()
                    .child(
                        div()
                            .w_full()
                            .max_w(scaled(CONTENT_MAX_W))
                            .child(self.content),
                    ),
            )
    }
}

/// The wide-window nav: a fixed column of sections.
///
/// Shares the canvas fill and separates with a hairline, per the design system's
/// sidebar rule — a second background colour here would split the screen into two
/// worlds instead of one.
fn section_rail(
    t: &theme::Theme,
    active: SettingsSection,
    on_select: &SectionSelect,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .flex_none()
        .w(scaled(RAIL_W))
        .gap_1()
        // `py_4` matches the content pane's own top padding, so the first nav label
        // lines up with the first panel's header instead of floating above it.
        .px_2()
        .py_4()
        .border_r_1()
        .border_color(rgb(t.border))
        .children(
            SettingsSection::ALL
                .iter()
                .enumerate()
                .map(|(ix, &section)| {
                    nav_item(t, ix, section, section == active, on_select.clone())
                }),
        )
        .into_any_element()
}

/// One rail row. Keyboard-reachable through gpui's own tab-stop ring —
/// `Root` (`sid_ui::component::Root`, this window's root) binds Tab to `focus_next`, and
/// `tab_index` both makes the element focusable and enrols it, with gpui keeping the
/// focus handle in element state. No second focus system, and no `FocusHandle` of
/// this screen's own.
fn nav_item(
    t: &theme::Theme,
    ix: usize,
    section: SettingsSection,
    active: bool,
    on_select: SectionSelect,
) -> AnyElement {
    let by_key = on_select.clone();
    div()
        .id(("settings-nav", ix))
        .tab_index(ix as isize)
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .px_2()
        .py_1p5()
        .rounded_md()
        .cursor_pointer()
        // The shared focus ring: a transparent hairline at rest, `accent` when focused,
        // so Tab reaching this row never resizes it. See `StyledExt::focus_ring`.
        .focus_ring(t)
        .when(active, |this| this.bg(rgb(t.selection)))
        .hover(|s| s.bg(rgb(t.selection)))
        .text_body(t)
        .text_color(rgb(if active { t.fg_strong } else { t.muted }))
        .child(
            // The active marker: a short accent rule, painted transparent when the
            // section is not active so the label never shifts sideways.
            div()
                .flex_none()
                .w(scaled(2.))
                .h(scaled(16.))
                .rounded_md()
                .when(active, |this| this.bg(rgb(t.accent))),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .clamp_one_line()
                .child(section.label()),
        )
        .on_click(move |_ev: &ClickEvent, window, cx| on_select(section, window, cx))
        .on_key_down(move |ev: &KeyDownEvent, window, cx| {
            if matches!(ev.keystroke.key.as_str(), "enter" | "space") {
                cx.stop_propagation();
                by_key(section, window, cx);
            }
        })
        .into_any_element()
}

/// The narrow-window nav: the same four sections as a segmented strip above the
/// content, so nothing has to overflow sideways.
fn section_strip(
    t: &theme::Theme,
    active: SettingsSection,
    on_select: &SectionSelect,
) -> AnyElement {
    let on_select = on_select.clone();
    div()
        .flex_none()
        .px_4()
        .py_2()
        .border_b_1()
        .border_color(rgb(t.border))
        .child(
            SegmentedControl::new("settings-nav")
                .segments(SettingsSection::ALL.map(SettingsSection::label))
                .selected(active.index())
                .on_select(move |ev: &SegmentSelect, window, cx| {
                    on_select(SettingsSection::from_index(ev.index), window, cx);
                }),
        )
        .into_any_element()
}

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
    /// Which section the nav is on. The only navigation state this screen has.
    section: SettingsSection,
}

impl SettingsTabState {
    pub(crate) fn new(store: &Store) -> Self {
        Self {
            cached: store.settings().unwrap_or_default(),
            error: None,
            keybindings: load_overrides(store),
            capturing: None,
            notice: None,
            section: SettingsSection::default(),
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

/// One labeled control inside a section panel: a muted label above, the
/// interactive content below.
///
/// `min_w(0)` because gpui reports a text element's min-content width as its whole
/// string: without it a long path in the Storage panel sets the row's minimum to
/// its own width and paints straight out of the card.
fn labeled_row(
    chrome: &theme::Theme,
    label: &'static str,
    content: impl IntoElement,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .min_w(px(0.))
        .gap_1()
        .child(div().text_meta(chrome).child(label))
        .child(content)
}

/// A small inline control (rebind / reset / cancel) — the row-scale button this screen
/// needs and `sid_ui` doesn't have a variant of. Deliberately built here rather than in
/// the component crate: it is one row's affordance, and `sid_ui::Button` is a
/// full-height form button that would out-shout the binding chip beside it.
///
/// `emphasis` is always `muted` today, and on purpose. Sixteen accent-red "rebind"
/// buttons would be the very failure `sid_ui::kbd`'s doc comment describes — accent
/// means *engage*, and a column of it teaches the eye to ignore red everywhere. Hover
/// (a `selection` fill) is what says "clickable" here; accent is spent on the one row
/// that is actually asking for a keystroke.
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

/// The Behaviour panel's option lists, each as one `(value, label)` table. One list
/// per control so the labels and the values cannot drift: the segmented control
/// reports an index back into the same array it was built from.
const SCOPES: [(DefaultScope, &str); 3] = [
    (DefaultScope::Global, "global"),
    (DefaultScope::Workspace, "workspace"),
    (DefaultScope::Ask, "ask"),
];
const SIDES: [(PanelSide, &str); 2] = [(PanelSide::Left, "left"), (PanelSide::Right, "right")];
const KEYRING: [(bool, &str); 2] = [(true, "enabled"), (false, "disabled")];

/// Where `current` sits in an option table. Falls back to the first option rather
/// than to "nothing selected", which would read as the app having forgotten the
/// setting.
fn selected_index<T: PartialEq>(options: &[(T, &str)], current: &T) -> usize {
    options
        .iter()
        .position(|(value, _)| value == current)
        .unwrap_or(0)
}

/// `default_scope`, as the app's own segmented control.
///
/// These three were hand-rolled chip strips — three near-identical copies of a
/// bordered, `surface`-filled row that predates [`SegmentedControl`] and got the
/// track/chip elevation backwards on every palette. The component owns that
/// decision now (`sid_ui::segmented::segment_paint`), and it is tested against all
/// four themes.
fn default_scope_selector(current: DefaultScope, cx: &mut Context<AppState>) -> SegmentedControl {
    SegmentedControl::new("settings-default-scope")
        .segments(SCOPES.map(|(_, label)| label))
        .selected(selected_index(&SCOPES, &current))
        .on_select(cx.listener(|this, ev: &SegmentSelect, _window, cx| {
            if let Some(&(scope, _)) = SCOPES.get(ev.index) {
                this.set_default_scope(scope, cx);
            }
        }))
}

/// Which side the SFTP file panel docks on.
fn file_browser_side_selector(current: PanelSide, cx: &mut Context<AppState>) -> SegmentedControl {
    SegmentedControl::new("settings-file-browser-side")
        .segments(SIDES.map(|(_, label)| label))
        .selected(selected_index(&SIDES, &current))
        .on_select(cx.listener(|this, ev: &SegmentSelect, _window, cx| {
            if let Some(&(side, _)) = SIDES.get(ev.index) {
                this.set_file_browser_side_pref(side, cx);
            }
        }))
}

/// Whether secrets go to the OS keyring.
fn secret_keyring_selector(enabled: bool, cx: &mut Context<AppState>) -> SegmentedControl {
    SegmentedControl::new("settings-secret-keyring")
        .segments(KEYRING.map(|(_, label)| label))
        .selected(selected_index(&KEYRING, &enabled))
        .on_select(cx.listener(|this, ev: &SegmentSelect, _window, cx| {
            if let Some(&(value, _)) = KEYRING.get(ev.index) {
                this.set_secret_keyring_enabled(value, cx);
            }
        }))
}

/// Split `app::secret_status_message`'s composed line into the sentence ("what is
/// happening") and, when there is one, the trailing detail ("why", plus the pacman
/// recommendation).
///
/// That function builds the string as `"secrets: {effective}[ — {warning}][
/// ({recommendation})]"` — the recommendation, when present, is always the LAST
/// top-level `(...)` group, appended after everything else with nothing following it.
/// Depth-counting from the end finds exactly that group even though both `effective`
/// (e.g. `"in-memory (no persistence)"`) and the recommendation itself (`"...(e.g.
/// `sudo pacman -S gnome-keyring`)..."`) contain their own, unrelated parens that a
/// first-`(`/last-`)` split would catch instead.
fn split_status_detail(message: &str) -> (&str, Option<&str>) {
    if !message.ends_with(')') {
        return (message, None);
    }
    let mut depth = 0i32;
    for (i, c) in message.char_indices().rev() {
        match c {
            ')' => depth += 1,
            '(' => {
                depth -= 1;
                if depth == 0 {
                    let sentence = message[..i].trim_end();
                    let detail = &message[i + 1..message.len() - 1];
                    return (sentence, Some(detail));
                }
            }
            _ => {}
        }
    }
    (message, None)
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
                .min_w(px(0.))
                .clamp_one_line()
                .text_mono(chrome)
                .child(path.to_string_lossy().into_owned()),
        )
    };

    Card::new()
        .title(SettingsSection::Storage.label())
        .child(path_row("data directory", data_dir))
        .child(path_row("store file", store_path))
        .child(path_row("demo database", demo_db_path))
        .child(caveat_line(
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
        let active = self.settings.section;

        // One listener, shared by the rail and the strip: the index round-trips through
        // `SettingsSection` so the two navs cannot disagree about what they selected.
        let select = cx.listener(|this, index: &usize, _window, cx| {
            this.settings.section = SettingsSection::from_index(*index);
            cx.notify();
        });
        let on_select: SectionSelect =
            Rc::new(move |section, window, cx| select(&section.index(), window, cx));

        let content = div()
            .flex()
            .flex_col()
            .gap_3()
            .children(self.settings.error.clone().map(error_line))
            .child(match active {
                // The theme panel's active marker follows the LIVE theme (not the
                // persisted name): identical in normal use (set_theme installs +
                // persists together), and honest under the SID_THEME per-run override,
                // where the persisted value deliberately differs.
                SettingsSection::Appearance => self
                    .theme_section(&chrome, chrome.name, cx)
                    .into_any_element(),
                SettingsSection::Behaviour => self
                    .behavior_section(&chrome, &settings, cx)
                    .into_any_element(),
                SettingsSection::Keyboard => self.keymap_section(&chrome, cx).into_any_element(),
                SettingsSection::Storage => storage_section(&chrome).into_any_element(),
            })
            .into_any_element();

        SettingsFrame {
            active,
            on_select,
            content,
        }
        .into_any_element()
    }

    // ---- Theme section --------------------------------------------------------

    fn theme_section(
        &self,
        chrome: &theme::Theme,
        applied: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        Card::new()
            .title(SettingsSection::Appearance.label())
            .count(theme::THEME_NAMES.len())
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
                .w(scaled(16.))
                .h(scaled(16.))
                .rounded_md()
                .bg(rgb(color))
                .border_1()
                // `chrome.border` (the row's own hairline) is a token step from
                // `selection`/`well` by design — right for separating panels, too
                // close to read as a ring once a near-black `bg`/`surface` swatch
                // (void: 0x000000/0x0a0a0a) sits on the `selection`-filled active
                // row (0x161616): a 1px, 12-of-255 step disappears at that size.
                // `faint` is a token step further from both row fills in every
                // built-in, dark or light, so the ring reads without a per-palette
                // branch.
                .border_color(rgb(chrome.faint))
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
            // `well`, not `surface`: the row lives inside a `surface` panel now, and
            // surface-on-surface is an invisible row.
            .bg(rgb(if active {
                chrome.selection
            } else {
                chrome.well
            }))
            // `selection` fill (above) plus the accent dot below are the row's two
            // active markers; an accent hairline on top of both was a third, and
            // `accent` means "engage" — spent here on a static row, it stopped
            // meaning anything. Every row keeps the same hairline `border`.
            .border_1()
            .border_color(rgb(chrome.border))
            .child(
                div()
                    .w(scaled(14.))
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
        // The backend sid actually ended up with, as a notice rather than a paragraph
        // of muted prose: red when the keyring is missing and secrets are degraded,
        // muted when it is only telling you which backend is in use. Split into the
        // sentence (what happened) and, when there is one, a `detail` line (why, plus
        // the pacman recommendation) — `secret_status_message` composes both into one
        // string, and at ~140 characters for the degraded case that used to be one
        // line clamped down to an ellipsis.
        let (sentence, detail) = split_status_detail(&self.secrets_status_detail);
        let backend = if self.secrets_degraded {
            error_line(sentence.to_string())
        } else {
            caveat_line(sentence.to_string())
        };
        let backend = match detail {
            Some(detail) => backend.detail(detail.to_string()),
            None => backend,
        };

        Card::new()
            .title(SettingsSection::Behaviour.label())
            .child(labeled_row(
                chrome,
                "default scope for new items",
                default_scope_selector(settings.default_scope, cx),
            ))
            .child(labeled_row(
                chrome,
                "file browser side",
                file_browser_side_selector(settings.file_browser_side, cx),
            ))
            .child(labeled_row(
                chrome,
                "secret keyring",
                div()
                    .flex()
                    .flex_col()
                    .min_w(px(0.))
                    .gap_1p5()
                    .child(secret_keyring_selector(settings.secret_keyring_enabled, cx))
                    // The restart caveat belongs to THIS control and to nothing else on
                    // the screen: a theme switch is live, a scope default applies to the
                    // next item, and the dock side fans out to every open session at
                    // once. Only the secret backend is chosen once, at startup. It used
                    // to sit at the bottom of the section, where it read as a warning
                    // about all three.
                    .child(
                        div()
                            .text_meta(chrome)
                            .child("changes take effect on restart"),
                    )
                    .child(backend),
            ))
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

        Card::new()
            .title(SettingsSection::Keyboard.label())
            .count(rows.len())
            .when_some(reset_all, Card::action)
            .child(div().flex().flex_col().children(rows))
            .child(div().text_meta(chrome).child(
                "shortcuts carry Ctrl; inside a focused terminal a letter chord \
                         reaches sid as Ctrl+Shift+<key> and the shell keeps the plain one",
            ))
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

        let custom_marker = (custom && !capturing).then(|| div().text_meta(chrome).child("custom"));

        let rebind = if capturing {
            row_button(chrome, ("keymap-cancel", ix), "cancel", chrome.muted)
                .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| this.cancel_rebind(cx)))
        } else {
            row_button(chrome, ("keymap-rebind", ix), "rebind", chrome.muted).on_click(
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
                    .when(capturing, |this| this.bg(rgb(chrome.selection)))
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
    fn split_status_detail_finds_the_outer_recommendation_past_nested_parens() {
        // The real degraded message (`app::secret_status_message`, keyring unavailable):
        // both `effective` and the recommendation itself carry their own parens, so a
        // naive first-`(`/last-`)` split would cut the sentence at "in-memory (" and
        // leave "no persistence)" dangling in front of the warning.
        let msg = "secrets: in-memory (no persistence) — OS keyring unavailable (no \
                    Secret Service provider is running); secrets will not persist \
                    across restarts (install a Secret Service provider (e.g. `sudo \
                    pacman -S gnome-keyring`) so secrets persist across restarts)";
        let (sentence, detail) = split_status_detail(msg);
        assert_eq!(
            sentence,
            "secrets: in-memory (no persistence) — OS keyring unavailable (no Secret \
             Service provider is running); secrets will not persist across restarts"
        );
        assert_eq!(
            detail,
            Some(
                "install a Secret Service provider (e.g. `sudo pacman -S gnome-keyring`) \
                 so secrets persist across restarts"
            )
        );
    }

    #[test]
    fn split_status_detail_is_sentence_only_when_the_backend_is_healthy() {
        // `secret_status_message("OS keyring", None, None)` — no warning, no
        // recommendation, nothing to split off.
        assert_eq!(
            split_status_detail("secrets: OS keyring"),
            ("secrets: OS keyring", None)
        );
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

    // ---- the screen's shape ------------------------------------------------

    #[test]
    fn a_fresh_state_opens_on_the_first_section() {
        let (_dir, store) = tmp_store();
        assert_eq!(
            SettingsTabState::new(&store).section,
            SettingsSection::ALL[0]
        );
    }

    #[test]
    fn every_section_round_trips_through_its_nav_index() {
        // The rail and the segmented strip both speak indices; a section that does not
        // survive the round trip would light the wrong nav item.
        for section in SettingsSection::ALL {
            assert_eq!(SettingsSection::from_index(section.index()), section);
        }
    }

    #[test]
    fn an_index_past_the_last_section_falls_back_instead_of_panicking() {
        assert_eq!(
            SettingsSection::from_index(SettingsSection::ALL.len()),
            SettingsSection::default()
        );
    }

    #[test]
    fn the_rail_needs_room_for_itself_and_a_reading_column() {
        // The defect this screen was rebuilt for: at 2000px there is room for both, so
        // the rail shows and the column is left-aligned beside it.
        let rem = px(16.);
        assert!(rail_fits(px(2000.), rem), "2000px is a rail window");
        assert!(
            rail_fits(px(RAIL_MIN_VIEWPORT), rem),
            "the breakpoint itself"
        );
        assert!(!rail_fits(px(700.), rem), "700px collapses to the strip");
    }

    #[test]
    fn the_breakpoint_is_measured_in_design_pixels_not_device_ones() {
        // At 150% a 1280px window has only ~853 design px of room — less than the rail
        // plus a column — so it must collapse even though 1280 > 900.
        let zoomed = px(16. * 1.5);
        assert!(!rail_fits(px(1280.), zoomed));
        assert!(
            rail_fits(px(1280.), px(16.)),
            "the same window at 100% fits"
        );
    }

    #[test]
    fn an_option_table_reports_the_index_of_the_current_value() {
        assert_eq!(selected_index(&SCOPES, &DefaultScope::Ask), 2);
        assert_eq!(selected_index(&SIDES, &PanelSide::Right), 1);
        assert_eq!(selected_index(&KEYRING, &false), 1);
    }

    #[test]
    fn a_fresh_state_is_not_capturing() {
        let (_dir, store) = tmp_store();
        let state = SettingsTabState::new(&store);
        assert!(!state.is_capturing(), "capture is never armed on load");
    }
}
