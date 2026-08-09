//! The component gallery — the observation gate for this crate.
//!
//! CLAUDE.md: *"a rendering spike is gated by observation, not unit tests; that is
//! correct, not a shortcut."* The unit tests in the modules around this one cover the
//! colour and state *decisions*; nothing they can assert tells you whether a button
//! looks like a button. This screen is what does — every component, in every variant
//! and every state, on one canvas, so a `scripts/sid-cap.sh` capture in each of the four
//! palettes is a complete before/after record of the crate.
//!
//! It is **dev-only by env gate**: `SID_GALLERY=1` makes `main.rs` mount this instead of
//! the app shell. There is no navigation to it, no menu entry and no keybinding, so it
//! cannot be reached by accident — but it is compiled into every build, which is the
//! point: a component that stops compiling breaks the build rather than rotting behind
//! a feature flag nobody enables.
//!
//! Capture it with:
//!
//! ```text
//! scripts/sid-cap.sh --env SID_GALLERY=1 --env SID_THEME=cosmos --out /tmp/g-cosmos.png
//! ```

use gpui::{
    Context, Entity, InteractiveElement as _, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement as _, Styled, Window, div, px, rgb, rgba,
};
use gpui_component::input::InputState;

use crate::badge::{ALL_BADGE_FILLS, ALL_BADGE_TONES, Badge, BadgeFill, BadgeTone};
use crate::bridge::SCRIM;
use crate::button::{ALL_BUTTON_VARIANTS, Button, ButtonSize, ButtonVariant, IconButton};
use crate::card::Card;
use crate::elevation::Elevation;
use crate::empty_state::EmptyState;
use crate::grid::{CardGrid, GridCard};
use crate::icon::Icon;
use crate::input::{self, FieldWidth, SearchInput, TextInput};
use crate::kbd::Kbd;
use crate::list::{List, Row};
use crate::modal::Modal;
use crate::notice::{caveat_line, error_line};
use crate::radio::Radio;
use crate::scope_chip::ScopeChip;
use crate::segmented::SegmentedControl;
use crate::status_dot::{ALL_CONNECTION_STATES, ConnectionState, StatusDot, StatusLegend};
use crate::styled::{StyledExt as _, h_flex, v_flex};
use crate::theme::{self, Theme};
use crate::toast::{ALL_TOAST_TONES, Toast};
use crate::toolbar::Toolbar;
use crate::typography::{ALL_TYPE_ROLES, TypeRole, Typography};

/// The sample objects the card grid is drawn with — a host grid, because that is the
/// grid this shape was built for.
const GRID_CARDS: [(&str, &str, ConnectionState); 5] = [
    ("home-server", "you@192.168.1.10:22", ConnectionState::Live),
    ("vps-1", "root@5.5.5.5:22", ConnectionState::Connecting),
    (
        "staging",
        "deploy@staging.acme:22",
        ConnectionState::Offline,
    ),
    ("prod", "deploy@prod.acme:22", ConnectionState::Failed),
    ("bastion", "ops@bastion.acme:2222", ConnectionState::Offline),
];

/// The four live fields the gallery draws.
///
/// A field's text is an entity, not an element, so unlike everything else on this screen
/// it has to persist between frames. Building one needs a `&mut Window`, which
/// `Gallery::new` does not have (`main.rs` mounts the gallery with `cx.new(|_| ..)`), so
/// they are built on the first render instead — see [`Gallery::fields`].
struct Fields {
    filter: Entity<InputState>,
    alias: Entity<InputState>,
    port: Entity<InputState>,
    unhelpful: Entity<InputState>,
    off: Entity<InputState>,
}

/// The gallery screen. Holds the field states and nothing else: every other element on
/// it is fresh per frame.
pub struct Gallery {
    fields: Option<Fields>,
}

impl Gallery {
    /// Mount with `cx.new(|_| Gallery::new())`.
    pub fn new() -> Self {
        Self { fields: None }
    }

    /// The field states, built on first use.
    fn fields(&mut self, window: &mut Window, cx: &mut Context<Self>) -> &Fields {
        self.fields.get_or_insert_with(|| Fields {
            filter: input::field(window, cx, "filter processes"),
            alias: input::field(window, cx, "prod-eu-west-1"),
            port: input::field(window, cx, "22"),
            unhelpful: input::field(window, cx, "/path/to/go"),
            off: input::field(window, cx, "read-only"),
        })
    }
}

impl Default for Gallery {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether the env gate is open. `main.rs` calls this; nothing else should.
pub fn requested() -> bool {
    std::env::var("SID_GALLERY").is_ok_and(|v| v == "1")
}

impl Render for Gallery {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        let fields = self.fields(window, cx);
        let filter = fields.filter.clone();
        let field_band = fields_band(&theme, fields);
        v_flex()
            .id("gallery")
            .size_full()
            // The screen has outgrown 1080px, and it should: it is an inventory, not a
            // dashboard. Scrolling the root is what keeps it *usable* at a normal window
            // size — before this, the bands took the height and the four component
            // columns (a `flex_1` under an `overflow_hidden`) were squeezed to nothing
            // and simply could not be reached. Captures pass `--size 1920x2400` and get
            // the whole thing in one frame.
            .overflow_y_scroll()
            .bg(rgb(theme.bg))
            .text_body(&theme)
            .child(chrome(&theme))
            // The specimen is a full-width band rather than a fifth column: the samples
            // are sentences, and a fifth of 1920px is not a line of text.
            .child(type_specimen(&theme))
            // Fields get a band of their own for the same reason plus one more: the
            // width proof needs a parent that offers no width, and a gallery column is
            // the opposite of that.
            .child(field_band)
            // The overlays are a band for the same reason, plus one of their own: a
            // modal panel is 460px wide at its real size, and a fifth column that wide
            // squeezed the other four until the host rows wrapped mid-word.
            .child(overlays(&theme))
            .child(
                h_flex()
                    // `flex_none`, not `flex_1`: inside a scrolling parent an item that
                    // grows to fill takes the *viewport's* leftover height instead of
                    // its own, which is another way of saying it disappears.
                    .flex_none()
                    .items_start()
                    .gap_4()
                    .px_4()
                    .pb_4()
                    // `min_w_0` on every column: a flex item's default minimum is its
                    // content width, so without it the widest card (the grid) pins the
                    // row wider than the window and the last column is clipped off the
                    // right edge.
                    .child(column().children(buttons(&theme)))
                    .child(column().children(chips(&theme)))
                    .child(column().children(structure(&theme, &filter)))
                    .child(column().children(rows(&theme))),
            )
    }
}

/// One elastic gallery column: an equal share of the row, allowed to shrink below its
/// content width.
fn column() -> gpui::Div {
    v_flex().flex_1().min_w_0().gap_4()
}

/// The top bar: what this screen is, and which palette is on — so a capture identifies
/// itself without needing the filename.
fn chrome(theme: &Theme) -> impl IntoElement + use<> {
    h_flex()
        .w_full()
        .justify_between()
        .px_4()
        .py_2()
        .elevation(Elevation::Surface, theme)
        .child(
            h_flex()
                .gap_2()
                .child(div().text_title(theme).child("sid-ui"))
                .child(div().hint_text(theme).child("component gallery")),
        )
        .child(
            h_flex()
                .gap_2()
                .child(Badge::new(theme.name).outline())
                .child(Badge::new("SID_GALLERY=1")),
        )
}

/// A labelled row inside a card — the gallery's own caption style.
fn row<C: IntoElement>(
    theme: &Theme,
    label: &'static str,
    content: C,
) -> impl IntoElement + use<C> {
    v_flex()
        .gap_1()
        .child(div().hint_text(theme).child(label))
        .child(h_flex().flex_wrap().gap_2().child(content))
}

/// Column 1 — every button variant, size and state.
fn buttons(theme: &Theme) -> Vec<gpui::AnyElement> {
    let variants = |suffix: &'static str, size: ButtonSize, icon: bool| {
        h_flex().flex_wrap().gap_2().children(
            ALL_BUTTON_VARIANTS
                .iter()
                .map(move |&variant| labelled(variant, suffix, size, icon)),
        )
    };

    vec![
        Card::new()
            .title("button")
            .child(row(theme, "medium", variants("md", ButtonSize::Md, false)))
            .child(row(theme, "small", variants("sm", ButtonSize::Sm, false)))
            .child(row(
                theme,
                "leading icon",
                variants("icon", ButtonSize::Md, true),
            ))
            .into_any_element(),
        Card::new()
            .title("button states")
            .child(row(
                theme,
                "disabled",
                h_flex().flex_wrap().gap_2().children(
                    ALL_BUTTON_VARIANTS
                        .iter()
                        .map(|&v| labelled(v, "off", ButtonSize::Md, false).disabled(true)),
                ),
            ))
            .child(row(
                theme,
                "loading",
                h_flex().flex_wrap().gap_2().children(
                    ALL_BUTTON_VARIANTS
                        .iter()
                        .map(|&v| labelled(v, "busy", ButtonSize::Md, false).loading(true)),
                ),
            ))
            .child(row(
                theme,
                "full width",
                Button::new("gallery-btn-full", "connect")
                    .primary()
                    .icon(Icon::Terminal)
                    .full_width(),
            ))
            .child(row(
                theme,
                "trailing icon — a menu trigger says so",
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("gallery-btn-split", "Export").trailing_icon(Icon::ChevronDown),
                    )
                    .child(
                        Button::new("gallery-btn-split-sm", "Export")
                            .small()
                            .trailing_icon(Icon::ChevronDown),
                    )
                    .child(
                        Button::new("gallery-btn-split-both", "Run")
                            .primary()
                            .icon(Icon::Terminal)
                            .trailing_icon(Icon::ChevronDown),
                    ),
            ))
            .into_any_element(),
        Card::new()
            .title("icon button")
            .action(
                IconButton::new("gallery-card-action", Icon::More, "more actions")
                    .small()
                    .into_any_element(),
            )
            .child(row(
                theme,
                "medium — every one carries a tooltip",
                h_flex()
                    .gap_2()
                    .child(IconButton::new("gib-1", Icon::Refresh, "refresh"))
                    .child(IconButton::new("gib-2", Icon::Search, "filter"))
                    .child(IconButton::new("gib-3", Icon::Copy, "copy").primary())
                    .child(IconButton::new("gib-4", Icon::Trash, "delete").danger()),
            ))
            .child(row(
                theme,
                "small",
                h_flex()
                    .gap_2()
                    .child(IconButton::new("gib-5", Icon::Refresh, "refresh").small())
                    .child(IconButton::new("gib-6", Icon::Settings, "settings").small())
                    .child(IconButton::new("gib-7", Icon::Close, "close").small()),
            ))
            .child(row(
                theme,
                "disabled · loading",
                h_flex()
                    .gap_2()
                    .child(IconButton::new("gib-8", Icon::Trash, "delete").disabled(true))
                    .child(IconButton::new("gib-9", Icon::Refresh, "refreshing").loading(true)),
            ))
            .into_any_element(),
    ]
}

/// One button, labelled after its own variant so the capture is self-describing.
fn labelled(variant: ButtonVariant, suffix: &str, size: ButtonSize, icon: bool) -> Button {
    let id: SharedString = format!("gallery-btn-{}-{suffix}", variant.label()).into();
    let button = Button::new(id, variant.label()).variant(variant).size(size);
    if icon { button.icon(Icon::Add) } else { button }
}

/// Column 2 — badges, pills and keybinding chips.
fn chips(theme: &Theme) -> Vec<gpui::AnyElement> {
    let tone_row = |fill: BadgeFill| {
        h_flex().flex_wrap().gap_2().children(
            ALL_BADGE_TONES
                .iter()
                .map(move |&tone| Badge::new(tone.label()).tone(tone).fill(fill)),
        )
    };

    vec![
        Card::new()
            .title("badge")
            .children(ALL_BADGE_FILLS.iter().map(|&fill| {
                let label: &'static str = match fill {
                    BadgeFill::Soft => "soft — the default",
                    BadgeFill::Solid => "solid",
                    BadgeFill::Outline => "outline",
                };
                row(theme, label, tone_row(fill)).into_any_element()
            }))
            .child(row(
                theme,
                "pill · counts · origin chips",
                h_flex()
                    .gap_2()
                    .child(Badge::new("connected").pill().tone(BadgeTone::Success))
                    .child(Badge::new("connecting").pill().tone(BadgeTone::Warning))
                    .child(Badge::count(12).pill())
                    // Origin chips stay orientation, per the design system: two
                    // weights of neutral, not a spent accent.
                    .child(Badge::new("global"))
                    .child(Badge::new("workspace").solid()),
            ))
            .into_any_element(),
        Card::new()
            .title("kbd")
            .child(row(
                theme,
                "display syntax · gpui syntax · chord · unparseable",
                h_flex()
                    .gap_3()
                    .child(Kbd::new("Ctrl+K"))
                    .child(Kbd::new("ctrl-shift-t"))
                    .child(Kbd::new("ctrl-k ctrl-s"))
                    .child(Kbd::new("double-click")),
            ))
            .child(row(
                theme,
                "in a settings row",
                h_flex()
                    .w_full()
                    .justify_between()
                    .gap_4()
                    .child(div().child("command palette"))
                    .child(Kbd::new("Ctrl+P")),
            ))
            .into_any_element(),
        Card::new()
            .title("icon")
            .count(Icon::ALL.len())
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .text_color(rgb(theme.muted))
                    .children(Icon::ALL.iter().map(|&icon| icon.el())),
            )
            .into_any_element(),
    ]
}

/// Column 3 — the containers.
fn structure(theme: &Theme, filter: &Entity<InputState>) -> Vec<gpui::AnyElement> {
    vec![
        Card::new()
            .title("toolbar")
            .child(
                Toolbar::new()
                    .filter(SearchInput::new(filter).small())
                    .count(132, "process")
                    .action(
                        Button::new("gallery-tb-refresh", "refresh")
                            .small()
                            .icon(Icon::Refresh),
                    ),
            )
            .child(
                Toolbar::new()
                    .count_label("no filter — count and actions hold the right edge")
                    .action(IconButton::new("gallery-tb-add", Icon::Add, "add host").small()),
            )
            .child(
                // The slot's real worst case: six call sites feed it an OS error rather
                // than a count. It has to elide instead of pushing the action off-screen.
                Toolbar::new()
                    .count_label(
                        "error: ssh: handshake failed: no supported authentication \
                         methods remain after the agent refused every identity",
                    )
                    .action(IconButton::new("gallery-tb-retry", Icon::Refresh, "retry").small()),
            )
            .into_any_element(),
        Card::new()
            .title("inline notice")
            .child(div().hint_text(theme).child(
                "a property of the thing above it, not an event floating over the \
                 screen — it stays until that thing changes",
            ))
            .child(error_line("kill: operation not permitted (pid 1)"))
            .child(caveat_line("sorted within this page only"))
            .child(error_line(
                "connect: ssh: handshake failed: no supported authentication methods \
                 remain after the agent refused every identity offered for this host",
            ))
            .into_any_element(),
        Card::new()
            .title("card · panel")
            .child(div().hint_text(theme).child(
                "a panel's body takes its height from the container, so a list inside \
                 it can scroll instead of growing past the card",
            ))
            .child(
                Card::panel("saved connections")
                    .count(9)
                    .h(px(150.))
                    .action(
                        IconButton::new("gallery-panel-add", Icon::Add, "add connection").small(),
                    )
                    .child(
                        v_flex()
                            .id("gallery-panel-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .p_1()
                            .gap_1()
                            .children((0..9).map(|ix| {
                                h_flex()
                                    .w_full()
                                    .justify_between()
                                    .gap_2()
                                    .row_padding()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .clamp_one_line()
                                            .text_body(theme)
                                            .child(format!("connection-{ix:02}")),
                                    )
                                    .child(Badge::new(if ix % 3 == 0 { "global" } else { "ws" }))
                            })),
                    ),
            )
            .into_any_element(),
        Card::new()
            .title("segmented control")
            .child(div().hint_text(theme).child(
                "in a column, so the track has to refuse the parent's stretch — and a \
                 long context name has to elide rather than widen the strip",
            ))
            .child(
                SegmentedControl::new("gallery-seg")
                    .segments(["Ports", "Services", "Interfaces"])
                    .selected(1),
            )
            .child(
                SegmentedControl::new("gallery-seg-long")
                    .segments(["gke_acme-prod_europe-west1_cluster-alpha", "minikube"])
                    .selected(0),
            )
            .into_any_element(),
        Card::new()
            .title("card")
            .count(2)
            .action(
                IconButton::new("gallery-card-refresh", Icon::Refresh, "refresh")
                    .small()
                    .into_any_element(),
            )
            .child(div().hint_text(theme).child(
                "a raised card: surface fill, hairline, uppercase muted header, an \
                 optional count and header actions",
            ))
            .child(
                Card::section("section")
                    .count(3)
                    .child(div().hint_text(theme).child(
                        "a flat section: the same header with no chrome, for titled \
                         blocks inside a reading column",
                    )),
            )
            .into_any_element(),
        Card::new()
            .title("card grid")
            .child(div().hint_text(theme).child(
                "cards wrap into as many columns as the width allows, and stretch to \
                 fill a short line — the SSH home's host grid, at a narrower column \
                 here so the wrap is visible inside one gallery pane",
            ))
            .child(
                CardGrid::new()
                    .min_col(px(150.))
                    .max_col(px(210.))
                    .children(
                        GRID_CARDS
                            .iter()
                            .enumerate()
                            .map(|(ix, (name, addr, state))| {
                                GridCard::new(("gallery-grid-card", ix))
                                    // The middle one is picked: `selection` fill plus the
                                    // brighter outline, against its resting neighbours.
                                    .selected(ix == 1)
                                    .on_click(|_, _, _| {})
                                    .child(
                                        h_flex()
                                            .w_full()
                                            .gap_2()
                                            .child(StatusDot::new(("gallery-grid-dot", ix), *state))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .clamp_one_line()
                                                    .text_color(rgb(theme.fg_strong))
                                                    .child(*name),
                                            ),
                                    )
                                    .child(div().hint_text(theme).clamp_one_line().child(*addr))
                                    .child(
                                        h_flex().w_full().gap_1().pt_1().child(
                                            Button::new(("gallery-grid-connect", ix), "connect")
                                                .small()
                                                .icon(Icon::Terminal),
                                        ),
                                    )
                            }),
                    ),
            )
            .into_any_element(),
        Card::new()
            .title("empty state")
            .child(
                div().h_48().child(
                    EmptyState::new("no workspaces yet")
                        .icon(Icon::Folder)
                        .guidance(
                            "add a git repository and sid tracks its branches, hosts \
                             and saved connections",
                        )
                        .action(
                            Button::new("gallery-empty-add", "add workspace")
                                .primary()
                                .icon(Icon::Add),
                        ),
                ),
            )
            .into_any_element(),
    ]
}

/// Column 4 — the list row and the two chips that live in it.
fn rows(theme: &Theme) -> Vec<gpui::AnyElement> {
    let sub = |id: &'static str, slot: &'static str| SharedString::from(format!("{id}-{slot}"));
    let host_row =
        |id: &'static str, alias: &'static str, addr: &'static str, state, chip, selected| {
            Row::new(id)
                .selected(selected)
                .leading(StatusDot::new(sub(id, "dot"), state))
                .on_click(|_, _, _| {})
                .child(div().text_body(theme).child(alias))
                .child(div().hint_text(theme).child(addr))
                .meta(chip)
                .action(
                    Button::new(sub(id, "connect"), "connect")
                        .primary()
                        .small()
                        .icon(Icon::Terminal),
                )
                .action(IconButton::new(sub(id, "files"), Icon::Folder, "browse files").small())
                .action(
                    IconButton::new(sub(id, "delete"), Icon::Trash, "delete")
                        .small()
                        .danger(),
                )
        };

    vec![
        Card::new()
            .title("status dot")
            .child(row(
                theme,
                "legend — every dot names itself",
                StatusLegend::new("gallery-legend"),
            ))
            .child(row(
                theme,
                "the mark alone (hover for the name)",
                h_flex()
                    .gap_3()
                    .children(ALL_CONNECTION_STATES.iter().map(|&s| {
                        StatusDot::new(SharedString::from(format!("gallery-dot-{}", s.label())), s)
                    })),
            ))
            .into_any_element(),
        Card::new()
            .title("radio")
            .child(div().hint_text(theme).child(
                "the mark only — the label, the click target and the selected fill \
                 belong to the Row it leads, because a save-to option is a whole row",
            ))
            .child(
                List::stack()
                    .child(
                        Row::new("gallery-radio-1")
                            .selected(true)
                            .leading(Radio::new(true))
                            .on_click(|_, _, _| {})
                            .child(div().text_body(theme).child("workspace"))
                            .child(
                                div()
                                    .hint_text(theme)
                                    .child("committed to .sid/config.toml"),
                            ),
                    )
                    .child(
                        Row::new("gallery-radio-2")
                            .leading(Radio::new(false))
                            .on_click(|_, _, _| {})
                            .child(div().text_body(theme).child("global"))
                            .child(
                                div()
                                    .hint_text(theme)
                                    .child("this machine, every workspace"),
                            ),
                    )
                    .child(
                        Row::new("gallery-radio-3")
                            .leading(Radio::new(false).enabled(false))
                            .child(
                                div()
                                    .text_body(theme)
                                    .text_color(rgb(theme.faint))
                                    .child("workspace"),
                            )
                            .child(div().hint_text(theme).child("no workspace is focused")),
                    ),
            )
            .into_any_element(),
        Card::new()
            .title("scope chip")
            .child(row(
                theme,
                "origin — weight, never hue",
                h_flex()
                    .gap_2()
                    .child(ScopeChip::global())
                    .child(ScopeChip::workspace("acme-api"))
                    .child(ScopeChip::global().duplicate(true))
                    .child(ScopeChip::workspace("acme-api").duplicate(true)),
            ))
            .into_any_element(),
        Card::new()
            .title("list · row")
            .count(3)
            .child(
                List::stack()
                    .child(host_row(
                        "gallery-row-1",
                        "home-server",
                        "you@192.168.1.10:22",
                        ConnectionState::Live,
                        ScopeChip::global(),
                        false,
                    ))
                    .child(host_row(
                        "gallery-row-2",
                        "staging",
                        "deploy@staging.acme-api.internal:22",
                        ConnectionState::Offline,
                        ScopeChip::workspace("acme-api"),
                        true,
                    ))
                    .child(host_row(
                        "gallery-row-3",
                        "vps-1",
                        "root@5.5.5.5:22",
                        ConnectionState::Failed,
                        ScopeChip::global().duplicate(true),
                        false,
                    )),
            )
            .into_any_element(),
    ]
}

/// Column 5 — the overlays: a modal over its scrim, and every notice tone.
///
/// The modal is drawn *in place* over a stand-in canvas rather than through
/// [`crate::modal::overlay`], which anchors to the window and would black out the rest of
/// the gallery. Everything inside the panel is the real thing: the real header, the real
/// scrim colour, the real footer buttons, a real [`Toast`].
fn overlays(theme: &Theme) -> impl IntoElement + use<> {
    h_flex()
        .w_full()
        .items_start()
        .gap_4()
        .px_4()
        .child(
            Card::new()
                .title("modal")
                .w(px(620.))
                .flex_none()
                .child(div().hint_text(theme).child(
                    "a panel over the scrim: title, close button, a body that scrolls \
                     when the fields outrun the window, and a footer with the keyboard \
                     contract on the left and the primary action on the right",
                ))
                .child(
                    div()
                        .relative()
                        // Taller than the panel by design: the scrim has to be visible
                        // above and below it, or the demo reads as a docked pane.
                        .h(px(380.))
                        .w_full()
                        .overflow_hidden()
                        .rounded_md()
                        .elevation(Elevation::Bg, theme)
                        // The app underneath, so the scrim has something to darken.
                        .child(
                            v_flex()
                                .p_2()
                                .gap_1()
                                .child(div().text_label(theme).child("SAVED CONNECTIONS · 3"))
                                .children(GRID_CARDS.iter().take(3).map(|(name, addr, _)| {
                                    h_flex()
                                        .w_full()
                                        .justify_between()
                                        .gap_2()
                                        .row_padding()
                                        .child(div().text_body(theme).child(*name))
                                        .child(div().text_mono_meta(theme).child(*addr))
                                })),
                        )
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(rgba(SCRIM))
                                .child(
                                    // At its real width, so the gallery shows the panel
                                    // the forms render rather than a scaled-down one.
                                    Modal::new("gallery-modal", "Add host")
                                        .submit_hint("saves")
                                        .on_dismiss(|_, _, _| {})
                                        .child(fake_field(theme, "alias", "prod-eu-west-1"))
                                        .child(fake_field(theme, "user", "deploy"))
                                        .child(Toast::danger("port must be a number in 1-65535"))
                                        .footer(
                                            Button::new("gallery-modal-cancel", "Cancel").ghost(),
                                        )
                                        .footer(
                                            Button::new("gallery-modal-save", "Save").primary(),
                                        ),
                                ),
                        ),
                ),
        )
        .child(
            Card::new()
                .title("toast")
                .count(ALL_TOAST_TONES.len())
                .flex_1()
                .min_w_0()
                .child(div().hint_text(theme).child(
                    "the tone frames the notice; the sentence stays fg so it is readable \
                     in every palette",
                ))
                .children(
                    ALL_TOAST_TONES
                        .iter()
                        .zip(TOAST_SAMPLES)
                        .map(|(&tone, sample)| Toast::new(tone, sample)),
                )
                .child(
                    Toast::warning(
                        "the workspace config was written, but the keyring refused the \
                         secret — it is held in memory for this session",
                    )
                    .title("saved with a warning")
                    .action(Button::new("gallery-toast-retry", "retry").small())
                    .on_dismiss(|_, _, _| {}),
                ),
        )
}

/// One sample sentence per tone, in [`ALL_TOAST_TONES`] order.
const TOAST_SAMPLES: [&str; 4] = [
    "no OS keyring — this password is held for this session only",
    "connection saved to the workspace",
    "sid is running without a GPU adapter; rendering is on llvmpipe",
    "alias exists in global — edit it instead",
];

/// A stand-in for a labelled text field. The real one is a `TextInput`, which is an
/// entity rather than an element and so cannot be built from this stateless screen.
fn fake_field(theme: &Theme, label: &'static str, value: &'static str) -> impl IntoElement + use<> {
    v_flex()
        .gap_1()
        .child(div().text_meta(theme).child(label))
        .child(
            div()
                .w_full()
                .px_2()
                .py_1p5()
                .rounded_md()
                .elevation(Elevation::Well, theme)
                .text_body(theme)
                .child(value),
        )
}

/// The type specimen — every role on the scale, once, with its own measurements
/// printed beside it.
///
/// This is the band that answers "is the hierarchy obvious?" by eye rather than by
/// argument. Each sample renders *in* its role and prints the numbers straight off
/// [`TypeRole::spec`], so the specimen cannot drift from the scale: there is no literal
/// size anywhere in it to fall out of date.
fn type_specimen(theme: &Theme) -> impl IntoElement + use<> {
    let sample = |role: TypeRole| {
        let spec = role.spec(theme);
        let measure = format!(
            "{}px · {} · {}",
            f32::from(spec.size),
            if spec.weight == gpui::FontWeight::MEDIUM {
                "medium"
            } else {
                "normal"
            },
            spec.family.unwrap_or("system"),
        );
        h_flex()
            .w_full()
            .gap_4()
            .items_baseline()
            .child(
                div()
                    .w(px(96.))
                    .flex_none()
                    .text_label(theme)
                    .child(role.name().to_uppercase()),
            )
            .child(
                div()
                    .w(px(212.))
                    .flex_none()
                    .text_meta(theme)
                    .child(measure),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .clamp_one_line()
                    .text_role(role, theme)
                    .child("Sphinx of black quartz, judge my vow — 0123456789"),
            )
    };

    h_flex()
        .w_full()
        .items_start()
        .gap_4()
        .p_4()
        .child(
            Card::new()
                .title("type scale")
                .count(ALL_TYPE_ROLES.len())
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_meta(theme)
                        .child("three sizes, two weights, one mono family"),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .children(ALL_TYPE_ROLES.iter().map(|&r| sample(r))),
                ),
        )
        .child(
            // The same six roles doing their day job, so the ladder can be judged as a
            // screen and not only as a chart.
            Card::new()
                .title("hierarchy in place")
                .w(px(420.))
                .flex_none()
                .child(div().text_title(theme).child("Processes"))
                .child(
                    div()
                        .text_body(theme)
                        .child("34 running, sorted by resident memory"),
                )
                .child(div().text_label(theme).child("FILTERS"))
                .child(div().text_meta(theme).child("user · system · all"))
                .child(div().text_mono(theme).child("/usr/lib/systemd/systemd"))
                .child(div().text_mono_meta(theme).child("pid 1 · 0.0% · 12.4 MB")),
        )
}

/// The fields band — every field shape, and the proof that a field sizes itself.
///
/// The right-hand card is the important one and it is deliberately unflattering: the
/// same field is drawn twice inside a parent that offers **no width at all**, once as
/// [`TextInput`] and once in the shape the widget it replaces had. The replacement holds
/// its 160px floor; the old shape collapses to padding and border, which is the ~20px
/// stub that swallowed clicks on the SFTP go-to-path field for a whole release.
fn fields_band(theme: &Theme, fields: &Fields) -> impl IntoElement + use<> {
    h_flex()
        .w_full()
        .items_start()
        .gap_4()
        .px_4()
        .pb_4()
        .child(
            Card::new()
                .title("text input")
                .flex_1()
                .min_w_0()
                .child(div().hint_text(theme).child(
                    "over gpui-component's Input: ctrl-backspace deletes a word, \
                     ctrl-shift-arrow extends the selection by one, and Tab leaves the \
                     field instead of being swallowed",
                ))
                // Explicit, increasing tab indices — the point of the demo. Every other
                // tab stop in the window (and there are dozens: each Button is one) sits
                // at the default 0, so these three sort after all of them and Tab walks
                // them in order. Left at the default they would be three more index-0
                // stops and Tab would visit them in whatever order the frame was built.
                .child(labelled_field(
                    theme,
                    "Fill — a form field · tab 1",
                    TextInput::new(&fields.alias).tab_index(1),
                ))
                .child(labelled_field(
                    theme,
                    "Fixed(90px) — a port · tab 2",
                    TextInput::new(&fields.port).fixed(px(90.)).tab_index(2),
                ))
                .child(labelled_field(
                    theme,
                    "disabled · tab 3 (still a stop — see the module docs)",
                    TextInput::new(&fields.off).disabled(true).tab_index(3),
                )),
        )
        .child(
            Card::new()
                .title("search input")
                .flex_1()
                .min_w_0()
                .child(div().hint_text(theme).child(
                    "Grow width, the registry's search glyph, and a clear affordance \
                     that appears once there is something to clear",
                ))
                .child(
                    Toolbar::new()
                        .filter(SearchInput::new(&fields.filter).small())
                        .count(132, "process")
                        .action(
                            Button::new("gallery-field-refresh", "refresh")
                                .small()
                                .icon(Icon::Refresh),
                        ),
                )
                .child(labelled_field(
                    theme,
                    "at Md, filling its line · tab 4",
                    SearchInput::new(&fields.filter)
                        .width(FieldWidth::Fill)
                        .tab_index(4),
                )),
        )
        .child(
            Card::new()
                .title("a field declares its own width")
                .flex_1()
                .min_w_0()
                .child(div().hint_text(theme).child(
                    "both of these sit in a parent that offers no width. the left one \
                     declares a 160px floor; the right one is the shape the old widget \
                     had — percentages all the way down, so it collapses to padding and \
                     border and eats the clicks aimed at it",
                ))
                .child(
                    // A content-sized row: `flex_none` so it does not take the card's
                    // width, `items_start` so it does not stretch its children. This is
                    // the unhelpful parent.
                    h_flex()
                        .flex_none()
                        .items_start()
                        .gap_4()
                        .child(
                            // `items_start` on the caption stack as well, or the caption
                            // stretches the field under it and the demo ends up
                            // measuring the caption.
                            v_flex()
                                .items_start()
                                .gap_1()
                                .child(div().hint_text(theme).child("TextInput"))
                                .child(TextInput::new(&fields.unhelpful)),
                        )
                        .child(
                            v_flex()
                                .items_start()
                                .gap_1()
                                .child(div().hint_text(theme).child("the old shape"))
                                .child(no_floor_field(theme)),
                        ),
                ),
        )
}

/// A field under its own caption — the shape a form row has.
fn labelled_field<F: IntoElement>(
    theme: &Theme,
    label: &'static str,
    field: F,
) -> impl IntoElement + use<F> {
    v_flex()
        .gap_1()
        .child(div().hint_text(theme).child(label))
        .child(field)
}

/// The widget this crate replaces, reduced to the one property that broke it: a box with
/// padding, a border and **no width of its own**. Rendered beside the real thing so the
/// capture carries its own before/after.
fn no_floor_field(theme: &Theme) -> impl IntoElement + use<> {
    div()
        .flex()
        .items_center()
        .px_2()
        .py_1()
        .rounded_md()
        .elevation(Elevation::Well, theme)
        .child(div().text_meta(theme).child(""))
}
