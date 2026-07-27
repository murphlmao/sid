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
    Context, IntoElement, ParentElement, Render, SharedString, Styled, Window, div, px, rgb,
};

use crate::badge::{ALL_BADGE_FILLS, ALL_BADGE_TONES, Badge, BadgeFill, BadgeTone};
use crate::button::{ALL_BUTTON_VARIANTS, Button, ButtonSize, ButtonVariant, IconButton};
use crate::card::Card;
use crate::elevation::Elevation;
use crate::empty_state::EmptyState;
use crate::icon::Icon;
use crate::kbd::Kbd;
use crate::list::{List, Row};
use crate::scope_chip::ScopeChip;
use crate::status_dot::{ALL_CONNECTION_STATES, ConnectionState, StatusDot, StatusLegend};
use crate::styled::{StyledExt as _, h_flex, v_flex};
use crate::theme::{self, Theme};
use crate::toolbar::Toolbar;
use crate::typography::{ALL_TYPE_ROLES, TypeRole, Typography};

/// The gallery screen. Holds no state: everything on it is a fresh element per frame.
pub struct Gallery;

impl Gallery {
    /// Mount with `cx.new(|_| Gallery::new())`.
    pub fn new() -> Self {
        Self
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        v_flex()
            .size_full()
            .bg(rgb(theme.bg))
            .text_body(&theme)
            .child(chrome(&theme))
            // The specimen is a full-width band rather than a fifth column: the samples
            // are sentences, and a fifth of 1920px is not a line of text.
            .child(type_specimen(&theme))
            .child(
                h_flex()
                    .flex_1()
                    .items_start()
                    .gap_4()
                    .px_4()
                    .pb_4()
                    .overflow_hidden()
                    .child(v_flex().flex_1().gap_4().children(buttons(&theme)))
                    .child(v_flex().flex_1().gap_4().children(chips(&theme)))
                    .child(v_flex().flex_1().gap_4().children(structure(&theme)))
                    .child(v_flex().flex_1().gap_4().children(rows(&theme))),
            )
    }
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
fn structure(theme: &Theme) -> Vec<gpui::AnyElement> {
    vec![
        Card::new()
            .title("toolbar")
            .child(
                Toolbar::new()
                    .filter(fake_filter(theme, "filter processes"))
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
                    .truncate()
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

/// A stand-in for the search field. `TextInput`/`SearchInput` land in a later commit;
/// until then the toolbar needs *something* filter-shaped to hold its left edge.
fn fake_filter(theme: &Theme, placeholder: &'static str) -> impl IntoElement + use<> {
    h_flex()
        .w_full()
        .gap_2()
        .px_2()
        .py_1()
        .rounded_md()
        .elevation(Elevation::Well, theme)
        .child(Icon::Search.el().text_color(rgb(theme.faint)))
        .child(div().text_meta(theme).child(placeholder))
}
