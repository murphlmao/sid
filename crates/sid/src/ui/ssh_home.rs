//! SSH tab, Home state: THE connections surface — a **card-grid dashboard**. Header and
//! quick-connect across the top, then every saved host as a card, grouped under
//! full-width folder headings and flowing into as many columns as the window has room
//! for (status dots, inline rename/folder edit, right-click menu incl. promote/demote,
//! origin chips).
//!
//! Two layouts died to get here. The design review killed the original [tree sidebar |
//! host-card list] split that showed the same hosts twice with two vocabularies; the
//! single width-capped column that replaced it was rejected in turn — *"a shitload of
//! empty space where there shouldn't be"* at 2000px, and *"make the connections easier
//! to work with"*. A connection is an object, not a line: as a card it carries its own
//! name, address, origin and controls inside 300px, which is what lets the grid spend
//! the full window width without ever putting an action a screen away from its subject.
//! See `sid_ui::grid` for the layout mechanism and its tested column contract.
//!
//! [`HomeTabState`] is a sibling cache to `AppState`'s own SSH fields, exactly like
//! `ui::db_tab`'s `DbTabState` — see that module's doc comment for the pattern this
//! mirrors: a second `impl AppState` block here reaches back into `AppState`'s
//! `pub(crate)` fields (`hosts`, `ssh_sessions`, `store`, `error`).
//!
//! Pure/unit-tested: [`group_by_folder`] (the grid's grouping transform),
//! [`section_title`] (which groups get a heading), [`filter_hosts`] (the quick-connect
//! box's search filter), [`parse_quick_connect`] (the `user@host[:port]` shorthand),
//! [`connection_state`] (what a card's dot says), [`row_primary`] (what its primary
//! button does), [`card_click_action`] (what clicking the card body itself does) and
//! [`home_empty`] (which nothing the grid is showing). Rendering is observation-gated,
//! per the plan's pragmatic-TDD rule.

use std::collections::{BTreeMap, HashSet};

use gpui::{
    AnyElement, ClickEvent, Context, Entity, IntoElement, MouseButton, MouseDownEvent,
    SharedString, Window, actions, div, prelude::*, px, rgb,
};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use sid_store::{Attributed, Host, Scope};

use crate::app::{AppState, can_demote, can_promote};
use crate::ui::{SessionStatus, TextInput};
use sid_ui::{
    Button, CardGrid, ConnectionState, EmptyState, GridCard, Icon, IconButton, List, StatusDot,
    StatusLegend, StyledExt as _, card::header_text, h_flex, theme, v_flex,
};

const MONO: &str = "DejaVu Sans Mono";

actions!(
    ssh_home,
    [
        /// Commit the in-place rename/folder edit (bound to `enter`).
        InlineEditCommit,
        /// Cancel the in-place rename/folder edit (bound to `escape`).
        InlineEditCancel,
        /// Fire the quick-connect box (bound to `enter` in its key context) — found by
        /// the capture-harness shakedown: Enter did nothing, only the Go button worked.
        QuickConnectGo,
    ]
);

/// The key context [`QuickConnectGo`] is scoped to — set on the quick-connect box's
/// wrapper, an ancestor of the focused `TextInput`, same trick as the inline edits.
pub(crate) const QUICK_CONNECT_CONTEXT: &str = "SshQuickConnect";

/// The key context [`InlineEditCommit`]/[`InlineEditCancel`] are scoped to — set on the
/// row wrapper around whichever `TextInput` is mid-edit, exactly like `HostForm`'s own
/// `"HostForm"` context (see that module's doc comment: the binding sits on an ancestor
/// of the focused field, so it fires no matter which nested input has focus).
pub(crate) const INLINE_EDIT_CONTEXT: &str = "SshInlineEdit";

/// An in-place edit box replacing a card's name line: either renaming the host (VS
/// Code style — F2/double-click, per the plan) or reassigning its folder.
enum InlineEdit {
    Rename {
        alias: String,
        origin: Scope,
        input: Entity<TextInput>,
    },
    Folder {
        alias: String,
        origin: Scope,
        input: Entity<TextInput>,
    },
}

impl InlineEdit {
    fn key(&self) -> (&str, &Scope) {
        match self {
            InlineEdit::Rename { alias, origin, .. } => (alias.as_str(), origin),
            InlineEdit::Folder { alias, origin, .. } => (alias.as_str(), origin),
        }
    }
}

/// SSH tab, Home state: the grid's collapse/search/selection/inline-edit UI state. The
/// composed host list itself lives on `AppState` (`self.hosts`) — this only holds
/// view-local state that isn't part of the layered store.
pub(crate) struct HomeTabState {
    /// Folder names collapsed in the grid. The unfiled group (`folder: None`) is not a
    /// folder anyone created and is never collapsible, so this only ever holds named
    /// folders.
    collapsed_folders: HashSet<String>,
    /// Quick-connect / filter box: as-you-type substring filter over the grid, and
    /// (the `⏎` button) a `user@host[:port]` parse for an ephemeral, unsaved connect.
    search: Entity<TextInput>,
    /// The row currently mid-rename or mid-folder-edit, if any.
    edit: Option<InlineEdit>,
    /// The quick-connect box's last parse failure, shown under it until the next
    /// attempt or edit.
    quick_error: Option<String>,
    /// Which card (if any) the grid's last right-click landed on — `None` reads as
    /// "empty space" (or a folder header). Feeds the grid's *single* `context_menu`
    /// (attached to the whole scroll container, in [`AppState::ssh_home_main`]),
    /// which decides card-menu vs. "+ Add connection" from this.
    ///
    /// This indirection exists because `gpui_component::menu::ContextMenuExt` can't be
    /// attached once per card: its convenience method hardcodes the wrapper's element id
    /// to the literal string `"context-menu"`, and none of the ancestors between the
    /// scroll container and any given card differ per card — every card's wrapper
    /// would collide on the exact same `GlobalElementId`, sharing (and clobbering) one
    /// `ContextMenuState`. `gpui-component`'s own `Table` sidesteps this the same way:
    /// one `context_menu` on the table body, fed by a `right_clicked_row` field set
    /// from each row's own `on_mouse_down(MouseButton::Right, ..)` (see
    /// `gpui-component`'s `table/state.rs`) — this mirrors that exact pattern.
    ///
    /// Reset to `None` on every right-click via the grid container's
    /// `capture_any_mouse_down` (capture phase, always runs first — see that call
    /// site's doc comment), then set back to `Some((host, origin))` by whichever card's
    /// ordinary bubble-phase `on_mouse_down(Right, ..)` fires next, if any. Emptiness is
    /// therefore the default for any right-click that isn't on a card — no dedicated
    /// "empty space" element needs its own clearing logic, and a short grid (empty space
    /// below the last row of cards) works exactly like an overflowing one (empty space
    /// is merely unreachable by the mouse) or right-clicking a folder header.
    right_click_target: Option<(Host, Scope)>,
    /// The card the user last picked, by (alias, origin) — the grid's visual anchor.
    ///
    /// A grid needs one and a list did not: in a list the pointer is always on the row
    /// it is about to act on, but in a six-column grid the eye loses its place between
    /// the card it chose and the context menu it opened over it. Selection is set by a
    /// single click on a card body and by a right-click (so the menu visibly belongs to
    /// something), and it is deliberately *only* a visual state — see
    /// [`card_click_action`], which keeps the first click on a 300px target harmless.
    ///
    /// A stale key (the host was renamed or deleted while selected) simply matches no
    /// card and paints nothing; there is nothing to clean up.
    selected: Option<(String, Scope)>,
}

impl HomeTabState {
    /// Move keyboard focus into the quick-connect/filter box — `Action::FocusFilter`'s
    /// (`Ctrl+F` / `Ctrl+/`) handler for the SSH tab, which used to be a no-op here
    /// because Network was the only tab with a filter wired up.
    pub(crate) fn focus_filter(&self, window: &mut Window, cx: &gpui::App) {
        self.search.read(cx).focus(window);
    }

    pub(crate) fn new(cx: &mut Context<AppState>) -> Self {
        Self {
            collapsed_folders: HashSet::new(),
            search: cx.new(|cx| TextInput::new(cx, "user@host[:port] — quick connect / filter")),
            edit: None,
            quick_error: None,
            right_click_target: None,
            selected: None,
        }
    }
}

// ---- pure grouping/search logic (unit-tested) ---------------------------------------

/// One visual group in the saved-connections grid. `folder: None` is the unfiled group,
/// always expanded (see [`section_title`] for whether it is even named); every other
/// value is a named, collapsible folder.
pub(crate) struct FolderGroup<'a> {
    pub folder: Option<&'a str>,
    pub hosts: Vec<&'a Attributed<Host>>,
}

/// Group `hosts` by their `folder` field for the grid: the `None` group always comes
/// first (flat, no header — only emitted if non-empty), followed by named folders in
/// alphabetical order. Within each group, hosts are sorted by alias (case-insensitive)
/// for a stable, predictable order — the composed list's own order just reflects read
/// order, not anything a user picked. Pure; the grid's collapse state is applied by the
/// caller when deciding whether to render a group's rows.
pub(crate) fn group_by_folder(hosts: &[Attributed<Host>]) -> Vec<FolderGroup<'_>> {
    let mut top: Vec<&Attributed<Host>> = Vec::new();
    let mut named: BTreeMap<&str, Vec<&Attributed<Host>>> = BTreeMap::new();
    for a in hosts {
        match a.item.folder.as_deref() {
            None => top.push(a),
            Some(f) => named.entry(f).or_default().push(a),
        }
    }
    let by_alias = |v: &mut Vec<&Attributed<Host>>| {
        v.sort_by(|a, b| {
            a.item
                .alias
                .to_ascii_lowercase()
                .cmp(&b.item.alias.to_ascii_lowercase())
        });
    };
    by_alias(&mut top);

    let mut groups = Vec::new();
    if !top.is_empty() {
        groups.push(FolderGroup {
            folder: None,
            hosts: top,
        });
    }
    for (folder, mut hosts) in named {
        by_alias(&mut hosts);
        groups.push(FolderGroup {
            folder: Some(folder),
            hosts,
        });
    }
    groups
}

/// Filter hosts for the quick-connect box: case-insensitive substring match against
/// `alias` or `user@host`. An empty (or whitespace-only) query matches everything —
/// the grid's normal, unfiltered view.
pub(crate) fn filter_hosts<'a>(
    hosts: &'a [Attributed<Host>],
    query: &str,
) -> Vec<&'a Attributed<Host>> {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return hosts.iter().collect();
    }
    hosts
        .iter()
        .filter(|a| {
            let addr = format!("{}@{}", a.item.user, a.item.host).to_ascii_lowercase();
            a.item.alias.to_ascii_lowercase().contains(&q) || addr.contains(&q)
        })
        .collect()
}

/// What a saved row's dot says, given the live session (if any) that row opened.
///
/// The tree used to ask one question — "is there a tab whose `source` is this row?" —
/// and paint `●` or `○` from the answer, which folded *dialling*, *authenticated* and
/// *failed* into one indistinguishable mark. Four states now, straight off the session's
/// own lifecycle.
///
/// A `Closed` session reads as [`ConnectionState::Offline`], not `Failed`: the tab is
/// still open so the user can read its scrollback, but nothing is connected and the row
/// is ready to dial again. A `Failed` one keeps its colour until the tab is closed —
/// that is the one state the old dot could never show, and the one worth seeing.
pub(crate) fn connection_state(status: Option<&SessionStatus>) -> ConnectionState {
    match status {
        None | Some(SessionStatus::Closed) => ConnectionState::Offline,
        Some(SessionStatus::Connecting) => ConnectionState::Connecting,
        Some(SessionStatus::Connected) => ConnectionState::Live,
        Some(SessionStatus::Failed(_)) => ConnectionState::Failed,
    }
}

/// Why the connection list is showing nothing, when it is.
///
/// Two different nothings that used to render identically — as literally nothing, an
/// empty scroll area under a header saying `CONNECTIONS · 0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HomeEmpty {
    /// Nothing is saved in either layer. A first-run screen: it needs a way in.
    NoHosts,
    /// Hosts exist, but the filter excludes all of them. A dead end the user typed
    /// themselves, so the way out is to undo it — not to add a host.
    NoMatch,
}

/// Which empty state (if either) the list should show.
///
/// `matched` is the count *after* filtering, `total` the composed list before it. A
/// non-empty result is never an empty state, whatever the query is.
pub(crate) fn home_empty(total: usize, matched: usize) -> Option<HomeEmpty> {
    match (matched, total) {
        (1.., _) => None,
        (0, 0) => Some(HomeEmpty::NoHosts),
        (0, _) => Some(HomeEmpty::NoMatch),
    }
}

/// What a row's primary button does when clicked.
///
/// The tab's primary verb used to be documented in a hint string — *"double-click a name
/// to rename · right-click for more"* — rather than rendered as a control, which is the
/// whole of the user's "doesn't feel intuitive". It is a button now, and a button has to
/// say what it will do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowPrimary {
    /// Dial this host: open a new session.
    Connect,
    /// Switch to the session this row already opened.
    Focus,
}

impl RowPrimary {
    /// The button's label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            RowPrimary::Connect => "connect",
            RowPrimary::Focus => "focus",
        }
    }

    /// The button's tooltip — the label is a verb, this says what it acts on.
    pub(crate) fn tooltip(self) -> &'static str {
        match self {
            RowPrimary::Connect => "open a session on this host",
            RowPrimary::Focus => "switch to this host's open session",
        }
    }
}

/// Which verb a row's primary button offers, from the state its dot is already showing.
///
/// Clicking "connect" on a row that is *already connected* used to open a **second**
/// session to the same host, silently, because `connect_host` always pushes a new tab.
/// The live dot said a session existed and the button ignored it. Now the two agree: if
/// something is up (or coming up), the primary verb goes to it. Opening a deliberate
/// second session is still available from the row's right-click menu.
///
/// `Failed` reads as [`RowPrimary::Connect`], not `Focus`: retrying is what the user
/// wants from a failed row, and the failed tab stays open behind it so its error text is
/// still readable.
pub(crate) fn row_primary(state: ConnectionState) -> RowPrimary {
    match state {
        ConnectionState::Connecting | ConnectionState::Live => RowPrimary::Focus,
        ConnectionState::Offline | ConnectionState::Failed => RowPrimary::Connect,
    }
}

/// What a click on a host card's *body* does — the card-grid rebuild's one genuinely
/// new interaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CardAction {
    /// Mark this card as the picked one. Changes nothing but the pixels.
    Select,
    /// Dial this host: open a new session.
    Connect,
    /// Switch to the session this card already opened.
    Focus,
}

/// Which of the three a body click resolves to, from the click count and the card's live
/// state.
///
/// A card is a *big* target — roughly 300x110px, against the ~24px square the old row
/// gave its primary verb — and that is the point: the screen's job is to make
/// connections easy to work with. But a target that large cannot also be a hair trigger.
/// One click therefore only picks the card up; the second one opens it. Every card still
/// carries an explicit `connect`/`focus` button that acts on the first click, so the
/// cheap gesture is always available *deliberately* and never by accident.
///
/// The open verb is not decided here — it defers to [`row_primary`], the same function
/// the card's own button reads, so the two can never disagree about whether an already
/// connected host should be dialled again or simply switched to.
pub(crate) fn card_click_action(click_count: usize, state: ConnectionState) -> CardAction {
    if click_count < 2 {
        return CardAction::Select;
    }
    match row_primary(state) {
        RowPrimary::Connect => CardAction::Connect,
        RowPrimary::Focus => CardAction::Focus,
    }
}

/// The heading a folder group renders above its grid, if it renders one.
///
/// In the list this replaced, the unfiled hosts were the *absence* of a header: they sat
/// flat at the top and the folders came after. A grid cannot borrow that trick — cards
/// under no header read as belonging to whichever header is nearest, and with the
/// folders below them that reading is wrong. So the unfiled group is named, but only
/// when there is a named folder to be told apart from: on the common store where nothing
/// is filed at all, a lone `UNGROUPED` rule across the whole screen is a header for
/// nothing.
pub(crate) fn section_title(folder: Option<&str>, named_folders: usize) -> Option<&str> {
    match (folder, named_folders) {
        (Some(folder), _) => Some(folder),
        (None, 0) => None,
        (None, _) => Some(UNGROUPED),
    }
}

/// The implicit final/first group's name — see [`section_title`].
const UNGROUPED: &str = "ungrouped";

/// Parse the quick-connect box's `user@host[:port]` shorthand. `None` if the text
/// doesn't look like that shape at all — the same box doubles as a plain grid filter,
/// so a partial query (most keystrokes) must not read as a failed connect attempt; only
/// the `⏎` action treats a `None` as an actual error to surface.
pub(crate) fn parse_quick_connect(input: &str) -> Option<(String, String, u16)> {
    let s = input.trim();
    let (user, rest) = s.split_once('@')?;
    if user.is_empty() || rest.is_empty() {
        return None;
    }
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() => (h, p.parse::<u16>().ok()?),
        _ => (rest, 22),
    };
    Some((user.to_string(), host.to_string(), port))
}

// ---- rendering + row actions (observation-gated) ------------------------------------

impl AppState {
    /// Home's single connections surface: a **card-grid dashboard**. Header and
    /// quick-connect across the top, then every saved host as a card, grouped under
    /// full-width folder headings and flowing into as many columns as the window has
    /// room for.
    ///
    /// This replaces a left-anchored 880px column, and the verdict that killed it was
    /// *"a shitload of empty space where there shouldn't be"* plus *"make the
    /// connections easier to work with"*. Both come from the same root cause: a
    /// connection is an **object**, and the screen was rendering it as a **line**. A
    /// line has to be capped at a reading width or its actions end up a screen away from
    /// its name — which is exactly why 1120px of a 2000px window was dead canvas. A card
    /// carries its own name, address, origin and controls inside one 300px box, so the
    /// reading-width problem is solved *per card* and the layout is free to use the
    /// whole width. See `sid_ui::grid` for the mechanism and the tested column contract.
    ///
    /// This IS the connection manager; there is no second list anywhere (the old
    /// sidebar/main split showed the same hosts twice).
    pub(crate) fn ssh_home_main(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.ssh_home.search.read(cx).content().to_string();
        let filtered = filter_hosts(&self.hosts, &query);
        let owned: Vec<Attributed<Host>> = filtered.into_iter().cloned().collect();
        let empty = home_empty(self.hosts.len(), owned.len());
        let groups = group_by_folder(&owned);
        // Whether the unfiled group needs a heading of its own — see `section_title`.
        let named = groups.iter().filter(|g| g.folder.is_some()).count();

        let mut sections = Vec::new();
        for (gix, group) in groups.into_iter().enumerate() {
            let collapsed = group
                .folder
                .is_some_and(|f| self.ssh_home.collapsed_folders.contains(f));
            let mut section = v_flex().w_full().gap_2();
            if let Some(title) = section_title(group.folder, named) {
                let count = group.hosts.len();
                section = section.child(self.section_header(
                    gix,
                    title,
                    group.folder,
                    count,
                    collapsed,
                    cx,
                ));
            }
            if !collapsed {
                let cards: Vec<_> = group.hosts.iter().map(|a| self.host_card(a, cx)).collect();
                section = section.child(CardGrid::new().children(cards));
            }
            sections.push(section.into_any_element());
        }

        v_flex()
            .flex_1()
            .min_h(px(0.))
            .child(self.home_header(cx))
            .child(self.quick_connect_box(cx))
            .child(
                List::scrolling("ssh-home-grid")
                    // The grid's own gutter and the gap between folder sections. `p_4`
                    // matches the status bar directly above, so the first card's left
                    // edge lines up with the chrome instead of floating.
                    .gap_4()
                    .p_4()
                    // Right-click *anywhere* in the grid defaults to "no card" —
                    // `capture_any_mouse_down` fires during the CAPTURE phase, which
                    // completes in full before any BUBBLE-phase handler runs (see
                    // `dispatch_mouse_event` in gpui's `window.rs`: capture is one full
                    // pass over every listener, then bubble is a second full pass, in
                    // reverse/child-first order). So this always resets the target
                    // first; a specific card's own `on_mouse_down(Right, ..)` (an
                    // ordinary BUBBLE-phase handler, see `host_card`) then fires
                    // straight after and overrides it back to `Some(host)` — but only
                    // when the click actually landed on that card. Reaching for this
                    // instead of a plain bubble-phase clear on this same container:
                    // bubble fires child-before-parent, so a bubble-phase clear here
                    // would run AFTER (and stomp) a card's bubble-phase set, not before.
                    .capture_any_mouse_down(cx.listener(
                        |this, ev: &MouseDownEvent, _window, cx| {
                            if ev.button == MouseButton::Right {
                                this.ssh_home.right_click_target = None;
                                cx.notify();
                            }
                        },
                    ))
                    .children(sections)
                    // Nothing to show: say which nothing this is, and hand back a way
                    // out of it. See `home_empty`. It takes the free height rather than
                    // a fixed 280px, so an empty screen is centred in the window instead
                    // of pinned under the toolbar.
                    .when_some(empty, |this, empty| {
                        this.child(self.home_empty_state(empty, cx))
                    })
                    // Trailing empty space below the last row of cards, so "right-click
                    // empty space → Add connection" has somewhere to land even when the
                    // grid is short — purely a layout spacer; the capture-phase reset
                    // above is what actually makes empty-space right-clicks correct.
                    // Skipped when an empty state is up, since that already claims the
                    // free height.
                    .when(empty.is_none(), |this| {
                        this.child(div().flex_1().min_h(px(48.)))
                    })
                    // ONE context menu for the whole grid — see `right_click_target`'s
                    // doc comment on why this can't be attached per-card.
                    .context_menu(self.grid_context_menu(cx)),
            )
    }

    /// The list's empty state: a headline, one line of what to do next, and a real
    /// control that does it.
    ///
    /// `.interface-design/system.md` says empty states "say what to do next, in muted
    /// text, without a box" — right about the box, incomplete about the rest. The
    /// zero-host case used to render *nothing at all* under a `CONNECTIONS · 0` header,
    /// which on a wide window is indistinguishable from a failed render; and telling a
    /// first-run user what to do while giving them no way to do it is the same mistake
    /// as documenting the tab's primary verb in a hint string.
    fn home_empty_state(&self, empty: HomeEmpty, cx: &mut Context<Self>) -> impl IntoElement {
        let state = match empty {
            HomeEmpty::NoHosts => EmptyState::new("no saved connections")
                .icon(Icon::Globe)
                .guidance(
                    "add a host to keep it here, or type user@host above to connect once \
                     without saving",
                )
                .action(
                    Button::new("ssh-empty-add", "add connection")
                        .primary()
                        .icon(Icon::Add)
                        .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                            this.open_add_form(window, cx);
                        })),
                ),
            // A dead end the user typed, so the way out is to undo it — offering "add a
            // connection" here would answer a search with a form.
            HomeEmpty::NoMatch => EmptyState::new("no connection matches the filter")
                .icon(Icon::Search)
                .guidance(
                    "clear the filter to see every saved connection, or press Enter to \
                     connect to what you typed",
                )
                .action(
                    Button::new("ssh-empty-clear", "clear filter")
                        .icon(Icon::Close)
                        .on_click(cx.listener(|this, _ev: &ClickEvent, _window, cx| {
                            this.ssh_home.search.update(cx, |input, cx| input.reset(cx));
                            this.ssh_home.quick_error = None;
                            cx.notify();
                        })),
                ),
        };
        // The scroll body is a plain stack, so the panel needs a height of its own to
        // have something to centre itself in. `flex_1` claims the body's free height —
        // on a 1200px window that centres the panel in the canvas, where a fixed 280px
        // box would leave it hanging under the toolbar with 700px of nothing below it.
        div().w_full().flex_1().min_h(px(320.)).child(state)
    }

    /// Builds the grid's single [`ContextMenuExt::context_menu`]: "+ Add connection"
    /// when nothing more specific was right-clicked, or the target card's actions when
    /// [`HomeTabState::right_click_target`] says a card was hit.
    ///
    /// This menu carries more than it used to, and deliberately: the card shows
    /// `connect`, `files` and `edit`, so rename, folder assignment, the layer moves and
    /// **delete** live only here. Putting delete on a 300px card would have been one
    /// stray click from destroying a record.
    ///
    /// Reads `right_click_target` fresh at build time — set by whichever card's own
    /// `on_mouse_down(MouseButton::Right, ..)` fired first for this same click, always
    /// before this builder runs.
    fn grid_context_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + use<> {
        let this = cx.entity();
        move |menu, _window, cx| {
            let target = this.read(cx).ssh_home.right_click_target.clone();
            let scope = this.read(cx).scope.clone();
            match target {
                Some((host, origin)) => {
                    Self::row_context_menu(menu, this.clone(), host, origin, scope)
                }
                None => Self::add_connection_menu_item(menu, this.clone(), "+ Add connection"),
            }
        }
    }

    /// A menu with just the single "+ Add connection" item — the empty-space/no-target
    /// case shared by [`Self::grid_context_menu`]'s `None` arm.
    fn add_connection_menu_item(
        menu: PopupMenu,
        this: Entity<AppState>,
        label: &'static str,
    ) -> PopupMenu {
        menu.item(PopupMenuItem::new(label).on_click(move |_ev, window, cx| {
            this.update(cx, |state, cx| state.open_add_form(window, cx));
        }))
    }

    /// The per-row menu: connect, the same in-place rename/folder-assign the row's hover
    /// icons already offer, a full [`crate::ui::host_form::HostForm`] edit, layer moves
    /// (promote to global / demote into the focused workspace — carried over from the
    /// deleted second host list), and delete.
    fn row_context_menu(
        menu: PopupMenu,
        this: Entity<AppState>,
        host: Host,
        origin: Scope,
        scope: Scope,
    ) -> PopupMenu {
        let key = (host.alias.clone(), origin.clone());
        menu.item(PopupMenuItem::new("Connect").on_click({
            let this = this.clone();
            let host = host.clone();
            let key = key.clone();
            move |_ev, window, cx| {
                let host = host.clone();
                let key = key.clone();
                this.update(cx, |state, cx| {
                    state.connect_host(host, Some(key), window, cx)
                });
            }
        }))
        .item(PopupMenuItem::new("Rename").on_click({
            let this = this.clone();
            let alias = host.alias.clone();
            let origin = origin.clone();
            move |_ev, window, cx| {
                let alias = alias.clone();
                let origin = origin.clone();
                this.update(cx, |state, cx| {
                    state.start_rename(alias, origin, window, cx)
                });
            }
        }))
        .item(PopupMenuItem::new("Edit…").on_click({
            let this = this.clone();
            let host = host.clone();
            let origin = origin.clone();
            move |_ev, window, cx| {
                let host = host.clone();
                let origin = origin.clone();
                this.update(cx, |state, cx| {
                    state.open_edit_form(host, origin, window, cx)
                });
            }
        }))
        .item(PopupMenuItem::new("Assign folder…").on_click({
            let this = this.clone();
            let alias = host.alias.clone();
            let origin = origin.clone();
            let folder = host.folder.clone();
            move |_ev, window, cx| {
                let alias = alias.clone();
                let origin = origin.clone();
                let folder = folder.clone();
                this.update(cx, |state, cx| {
                    state.start_folder_edit(alias, origin, folder, window, cx)
                });
            }
        }))
        .when(can_promote(&origin), |menu| {
            menu.item(PopupMenuItem::new("Promote to global").on_click({
                let this = this.clone();
                let alias = host.alias.clone();
                let origin = origin.clone();
                move |_ev, _window, cx| {
                    let alias = alias.clone();
                    let origin = origin.clone();
                    this.update(cx, |state, cx| state.promote_row(&alias, &origin, cx));
                }
            }))
        })
        .when(can_demote(&origin, &scope), |menu| {
            menu.item(PopupMenuItem::new("Demote to workspace").on_click({
                let this = this.clone();
                let alias = host.alias.clone();
                move |_ev, _window, cx| {
                    let alias = alias.clone();
                    this.update(cx, |state, cx| state.demote_row(&alias, cx));
                }
            }))
        })
        .separator()
        .item(PopupMenuItem::new("Delete").on_click({
            let secret_ref = host.secret_ref.clone();
            move |_ev, _window, cx| {
                let (alias, origin) = key.clone();
                let secret_ref = secret_ref.clone();
                this.update(cx, |state, cx| {
                    state.delete_row(&alias, &origin, secret_ref.as_deref(), cx)
                });
            }
        }))
    }

    /// The screen's header row: the count, the status legend, and the `+ add connection`
    /// affordance on the right edge. Opens the exact same [`HostForm::new_add`] path as
    /// every other add-connection entry point (the tab-strip `+`, the empty state's
    /// button, the grid's empty-space context menu) — see `AppState::open_add_form`.
    fn home_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme::active(cx).clone();
        let count = self.hosts.len();
        let label: SharedString = header_text("connections", Some(count)).into();
        h_flex()
            .w_full()
            .gap_4()
            .px_4()
            .py_2()
            .hairline_b(&t)
            .child(div().flex_none().section_label(&t).child(label))
            // The dot vocabulary, spelled out once, beside the count it qualifies — the
            // grid paints a 6px circle per card and nothing else on screen says what it
            // means. `flex_1` parks it in the middle so the add button keeps the right
            // edge to itself; the legend wraps inside this box on a narrow window.
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .child(StatusLegend::new("ssh-home-legend")),
            )
            .child(
                Button::new("ssh-home-add-connection", "add connection")
                    .small()
                    .icon(Icon::Add)
                    .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                        this.open_add_form(window, cx);
                    })),
            )
    }

    fn quick_connect_box(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::active(cx).clone();
        let danger = t.danger;
        let search = self.ssh_home.search.clone();
        // A red filled square with a `⏎` in it used to sit here: sid's most emphatic
        // affordance, spent on "submit this text field", while the tab's actual primary
        // verb had no button at all. Now it is a labelled button and says which of the
        // box's two jobs it performs — the same box is also the list filter, and typing
        // in it never needs this.
        let go = Button::new("ssh-quick-connect-go", "connect")
            .primary()
            .icon(Icon::Terminal)
            .tooltip("connect to the typed user@host[:port] without saving it — or press Enter")
            .on_click(
                cx.listener(|this, _ev: &ClickEvent, window, cx| this.quick_connect(window, cx)),
            );

        let mut col = div()
            .key_context(QUICK_CONNECT_CONTEXT)
            .on_action(cx.listener(|this, _: &QuickConnectGo, window, cx| {
                this.quick_connect(window, cx);
            }))
            .flex()
            .flex_col()
            .gap_1()
            .px_4()
            .py_2()
            .hairline_b(&t)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        // `TextInput` paints its shaped line at its own natural width,
                        // ignoring the box's flex-assigned bounds (see `TextElement::
                        // paint` in `text_input.rs` — it calls `line.paint` with no
                        // content mask). Left unclipped, the placeholder/typed text
                        // bled straight through the `go` button beside it and into
                        // whatever sat past it. `min_w(0)` lets this flex item actually
                        // shrink to the row's available width (the flexbox default
                        // min-width is content-sized, which would fight `flex_1` here);
                        // `overflow_hidden` then clips the input's own paint (including
                        // its nested `TextElement`) to that width — a content mask
                        // gpui's `Div` establishes around all of its descendants'
                        // painting, not just its own quad, so it fixes the child
                        // without needing to touch the shared `TextInput` element.
                        // Capped, unlike the grid below it: a 2000px-wide text field is
                        // not a better text field, and letting it run the full width
                        // would drag the `connect` button it submits to the far edge of
                        // the screen — the exact "action a screen-width from its
                        // subject" the grid exists to stop doing.
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .max_w(px(640.))
                            .overflow_hidden()
                            .child(search),
                    )
                    .child(go),
            );
        // What used to live here: "saved connections below · double-click a name to
        // rename · right-click for more" — 12px of muted prose doing the job of three
        // controls. Every one of those interactions still works; none of them is
        // documentation-only any more.
        if let Some(err) = &self.ssh_home.quick_error {
            col = col.child(div().text_xs().text_color(rgb(danger)).child(err.clone()));
        }
        col
    }

    /// `⏎`: parse the quick-connect box as `user@host[:port]` and, if it parses, open an
    /// ephemeral (unsaved) session for it — `source: None`, so the grid's live-dot only
    /// ever tracks saved hosts. A non-matching query (most partial input, since the same
    /// box doubles as a filter) surfaces a short error instead of silently doing nothing.
    fn quick_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.ssh_home.search.read(cx).content().to_string();
        match parse_quick_connect(&text) {
            Some((user, host, port)) => {
                self.ssh_home.quick_error = None;
                let alias = format!("{user}@{host}");
                let host_rec = Host {
                    alias,
                    user,
                    host,
                    port,
                    secret_ref: None,
                    auth: sid_store::AuthMethod::default(),
                    folder: None,
                };
                self.connect_host(host_rec, None, window, cx);
                self.ssh_home.search.update(cx, |input, cx| input.reset(cx));
            }
            None => {
                self.ssh_home.quick_error = Some("expected user@host[:port]".to_string());
                cx.notify();
            }
        }
    }

    /// One group's full-width heading: `WORK · 3` over a hairline, with a disclosure
    /// caret when the group is a real folder.
    ///
    /// The list this replaced made a folder a *row* — same box, same padding and the
    /// same hover as the hosts inside it, distinguished only by a caret. In a grid the
    /// distinction is structural instead: a heading spans the full width and the cards
    /// flow beneath it, which is the `Card::section` language (`text_xs` UPPERCASE
    /// `muted`, via the shared [`header_text`]) applied to a group of objects rather
    /// than a block of text.
    ///
    /// The implicit `ungrouped` heading is not a folder and gets no caret and no click:
    /// there is nothing to collapse it *into*, and a group the user never created should
    /// not offer to hide itself. The 14px spacer keeps its title on the same left edge as
    /// every folder's.
    fn section_header(
        &self,
        gix: usize,
        title: &str,
        folder: Option<&str>,
        count: usize,
        collapsed: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme::active(cx).clone();
        let caret = match collapsed {
            true => Icon::ChevronRight,
            false => Icon::ChevronDown,
        };
        let header = h_flex()
            .w_full()
            .gap_1p5()
            .py_1()
            .rounded_sm()
            .hairline_b(&t)
            .child(match folder {
                Some(_) => caret
                    .el()
                    .size(px(14.))
                    .text_color(rgb(t.muted))
                    .into_any_element(),
                None => div().w(px(14.)).flex_none().into_any_element(),
            })
            .child(
                div()
                    .section_label(&t)
                    .child(header_text(title, Some(count))),
            );
        match folder {
            // A folder heading is the whole collapse target, not just its caret — a 14px
            // triangle is a worse click target than the 1900px rule beside it.
            Some(folder) => {
                let owned = folder.to_string();
                header
                    .id(("ssh-folder", gix))
                    .cursor_pointer()
                    .hover_fill(&t)
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                        this.toggle_folder(owned.clone(), cx);
                    }))
                    .into_any_element()
            }
            None => header.into_any_element(),
        }
    }

    fn toggle_folder(&mut self, folder: String, cx: &mut Context<Self>) {
        if !self.ssh_home.collapsed_folders.remove(&folder) {
            self.ssh_home.collapsed_folders.insert(folder);
        }
        cx.notify();
    }

    /// The live session this row opened, if one is still in the tab strip.
    fn session_for_row(&self, key: &(String, Scope)) -> Option<usize> {
        self.ssh_sessions
            .iter()
            .position(|t| t.source.as_ref() == Some(key))
    }

    /// A row's primary button: connect, or go to the session this row already has —
    /// see [`row_primary`] for which, and why clicking "connect" on a connected row
    /// used to silently open a second session.
    fn row_primary_action(
        &mut self,
        host: Host,
        key: (String, Scope),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = self.session_for_row(&key);
        let state = self.session_state(&key, cx);
        match (row_primary(state), existing) {
            (RowPrimary::Focus, Some(ix)) => self.activate_session(ix, window, cx),
            _ => self.connect_host(host, Some(key), window, cx),
        }
    }

    /// A row's `browse files` button: land in this host's SFTP panel.
    ///
    /// A fresh session already opens with the file panel showing, so the only extra work
    /// is for a session that is *already* open with the panel collapsed — reveal it
    /// before switching, or the click would look like it did nothing.
    fn row_browse_files(
        &mut self,
        host: Host,
        key: (String, Scope),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.session_for_row(&key) {
            Some(ix) => {
                if let Some(tab) = self.ssh_sessions.get(ix) {
                    tab.session
                        .update(cx, |session, cx| session.reveal_files(cx));
                }
                self.activate_session(ix, window, cx);
            }
            None => self.connect_host(host, Some(key), window, cx),
        }
    }

    /// The live state of the session this (alias, origin) opened, if any — the fact both
    /// the card's dot and its primary verb are drawn from.
    ///
    /// Read fresh at *click* time as well as at render time: a session can come up (or
    /// fall over) between the two, and a card that dials a second connection because it
    /// was painted a second ago is the exact bug [`row_primary`] exists to prevent.
    fn session_state(&self, key: &(String, Scope), cx: &gpui::App) -> ConnectionState {
        connection_state(
            self.ssh_sessions
                .iter()
                .find(|t| t.source.as_ref() == Some(key))
                .map(|t| t.session.read(cx).status()),
        )
    }

    /// One host card: status, name, address, origin and the two verbs worth a control of
    /// their own — or, while this exact (alias, origin) is mid-edit, the same card with
    /// its name line replaced by the rename/folder box.
    ///
    /// **What is on the card and what is not.** The row this replaces carried five
    /// affordances (`connect ✎ folder ⌫ ×`) crammed into 150px, three of which were
    /// glyphs. A card has room, which makes it tempting to put everything on it — and
    /// tempting is the wrong instinct: `delete` on a big soft target is a hazard, and
    /// rename/promote/demote are rare. The card gets `connect` (the tab's whole point),
    /// `files` (the other half of the tab's name, and unreachable from this screen until
    /// recently) and `edit`; everything else stays one right-click away.
    fn host_card(&self, a: &Attributed<Host>, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::active(cx).clone();
        let host = a.item.clone();
        let origin = a.origin.clone();
        let key = (host.alias.clone(), origin.clone());
        let card_id = row_hash(&host.alias, &origin);
        // A short, stable per-card element id for each control. gpui wants ids unique
        // within the window, and every one of these repeats across the grid.
        let slot = |name: &'static str| (name, card_id);

        if let Some(edit) = &self.ssh_home.edit
            && edit.key() == (host.alias.as_str(), &origin)
        {
            return self.inline_edit_card(edit, slot("ssh-card-edit"), cx);
        }

        let state = self.session_state(&key, cx);
        let selected = self.ssh_home.selected.as_ref() == Some(&key);
        let alias: SharedString = host.alias.clone().into();
        let addr: SharedString = format!("{}@{}:{}", host.user, host.host, host.port).into();

        // THE primary verb of the tab, as a control instead of a sentence.
        //
        // `Secondary`, not `Primary`, and the reason is the design system's "one accent,
        // used sparingly": this button repeats once per card, so an accent fill would
        // paint a constellation of red across the grid and train the eye to ignore it —
        // the exact failure the audit flagged on Settings' 16 accent-red keybindings. It
        // is still the loudest thing in the card's action row because it is the only one
        // carrying a word. The screen's single accent goes to quick-connect.
        let primary = row_primary(state);
        let connect = {
            let host = host.clone();
            let key = key.clone();
            Button::new(slot("ssh-card-connect"), primary.label())
                .small()
                .icon(Icon::Terminal)
                .tooltip(primary.tooltip())
                .on_click(cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                    this.row_primary_action(host.clone(), key.clone(), window, cx);
                }))
        };
        // The other half of the tab's name. SFTP was unreachable from this screen: no
        // browse affordance, no split hint, nothing saying files were even available.
        let files = {
            let host = host.clone();
            let key = key.clone();
            IconButton::new(slot("ssh-card-files"), Icon::File, "browse files (SFTP)")
                .small()
                .on_click(cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                    this.row_browse_files(host.clone(), key.clone(), window, cx);
                }))
        };
        let edit = {
            let host = host.clone();
            let origin = origin.clone();
            IconButton::new(
                slot("ssh-card-edit-btn"),
                Icon::Settings,
                "edit this connection",
            )
            .small()
            .on_click(cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                this.open_edit_form(host.clone(), origin.clone(), window, cx);
            }))
        };

        // The body click. One click picks the card up, two open it — see
        // `card_click_action` for why a target this big cannot dial on the first.
        let body_click = {
            let host = host.clone();
            let key = key.clone();
            cx.listener(move |this, ev: &ClickEvent, window, cx| {
                let state = this.session_state(&key, cx);
                match card_click_action(ev.click_count(), state) {
                    CardAction::Select => {
                        this.ssh_home.selected = Some(key.clone());
                        cx.notify();
                    }
                    CardAction::Connect => {
                        this.connect_host(host.clone(), Some(key.clone()), window, cx)
                    }
                    CardAction::Focus => {
                        if let Some(ix) = this.session_for_row(&key) {
                            this.activate_session(ix, window, cx);
                        }
                    }
                }
            })
        };

        GridCard::new(slot("ssh-card"))
            .selected(selected)
            .on_click(body_click)
            // Records this card as the grid's right-click target — read by the grid's
            // single `context_menu` (see `HomeTabState::right_click_target`'s doc
            // comment for why every card can't just have its own `.context_menu`). It
            // also *selects*, so the menu that opens visibly belongs to a card rather
            // than appearing over an anonymous one.
            .on_secondary_mouse_down({
                let host = host.clone();
                let origin = origin.clone();
                let key = key.clone();
                cx.listener(move |this, _ev: &MouseDownEvent, _window, cx| {
                    this.ssh_home.right_click_target = Some((host.clone(), origin.clone()));
                    this.ssh_home.selected = Some(key.clone());
                    cx.notify();
                })
            })
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(StatusDot::new(slot("ssh-card-dot"), state))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_sm()
                            .text_color(rgb(t.fg_strong))
                            .clamp_one_line()
                            .child(alias),
                    )
                    // Where this record lives (global vs a workspace, `· dup` when
                    // shadowing) — the attributive store's one per-card fact nothing
                    // else on screen shows.
                    //
                    // Bounded, because the workspace layer's chip carries a *workspace
                    // name* the card has no say in. `flex_none` alone means the chip
                    // never gives up a pixel, and `GridCard` has no clip of its own, so
                    // a workspace called `platform-infrastructure-monorepo` pushed the
                    // chip clean out through the card's right border — the alias beside
                    // it is already `flex_1 + min_w(0) + clamp_one_line` and so was
                    // busy shrinking to make room for something that would not stop
                    // growing. A third of the card is all a provenance mark gets.
                    .child(
                        div()
                            .flex_none()
                            .max_w(px(120.))
                            .overflow_hidden()
                            .clamp_one_line()
                            .child(self.scope_chip(a)),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .font_family(MONO)
                    .text_xs()
                    .text_color(rgb(t.muted))
                    .clamp_one_line()
                    .child(addr),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_1()
                    .pt_1()
                    .child(connect)
                    // The word on the left, the glyphs on the right: the same
                    // engagement-last ordering `sid_ui::Row` fixes for rows, held to
                    // inside a card so a grid of them scans as one column of verbs.
                    .child(div().flex_1().min_w(px(0.)))
                    .child(files)
                    .child(edit),
            )
            .into_any_element()
    }

    /// A card mid-rename / mid-folder-assign: the same box, with its name line replaced
    /// by the edit field. Replacing the *card* rather than dropping a full-width row into
    /// the grid keeps the geometry still while the user types — the neighbouring cards
    /// do not reflow around the edit.
    fn inline_edit_card(
        &self,
        edit: &InlineEdit,
        id: (&'static str, u64),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme::active(cx).clone();
        let (input, flag): (Entity<TextInput>, &'static str) = match edit {
            InlineEdit::Rename { input, .. } => (input.clone(), "renaming · Enter/Esc"),
            InlineEdit::Folder { input, .. } => (input.clone(), "folder · Enter/Esc"),
        };
        div()
            .key_context(INLINE_EDIT_CONTEXT)
            .w_full()
            .on_action(cx.listener(|this, _ev: &InlineEditCommit, _window, cx| {
                this.commit_inline_edit(cx);
            }))
            .on_action(cx.listener(|this, _ev: &InlineEditCancel, _window, cx| {
                this.cancel_inline_edit(cx);
            }))
            .child(
                GridCard::new(id)
                    .selected(true)
                    // Same `min_w(0) + overflow_hidden` clip as the quick-connect box —
                    // a long in-progress rename/folder value must not bleed out of the
                    // card it belongs to.
                    .child(div().w_full().min_w(px(0.)).overflow_hidden().child(input))
                    .child(div().text_xs().text_color(rgb(t.accent)).child(flag)),
            )
            .into_any_element()
    }

    /// F2/double-click: start renaming `alias`'s label in place (VS Code style).
    fn start_rename(
        &mut self,
        alias: String,
        origin: Scope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let seed = alias.clone();
        let input = cx.new(|cx| {
            let mut ti = TextInput::new(cx, "alias");
            ti.set_content(seed, cx);
            ti
        });
        input.read(cx).focus(window);
        self.ssh_home.edit = Some(InlineEdit::Rename {
            alias,
            origin,
            input,
        });
        cx.notify();
    }

    /// `folder`: start reassigning `alias`'s folder in place — blank commits to "no folder".
    fn start_folder_edit(
        &mut self,
        alias: String,
        origin: Scope,
        current: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| {
            let mut ti = TextInput::new(cx, "folder (blank = none)");
            if let Some(f) = current {
                ti.set_content(f, cx);
            }
            ti
        });
        input.read(cx).focus(window);
        self.ssh_home.edit = Some(InlineEdit::Folder {
            alias,
            origin,
            input,
        });
        cx.notify();
    }

    fn cancel_inline_edit(&mut self, cx: &mut Context<Self>) {
        self.ssh_home.edit = None;
        cx.notify();
    }

    /// Enter: commit whichever inline edit is open. A rename to an existing alias in the
    /// same layer (or a no-op/empty rename) surfaces `Store::rename_host`'s `Conflict`
    /// (or is silently dropped, for a genuine no-op) rather than ever clobbering a row —
    /// same guard `Store::rename_host` already enforces.
    fn commit_inline_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.ssh_home.edit.take() else {
            return;
        };
        match edit {
            InlineEdit::Rename {
                alias,
                origin,
                input,
            } => {
                let new_alias = input.read(cx).content().trim().to_string();
                if new_alias.is_empty() || new_alias == alias {
                    cx.notify();
                    return;
                }
                match self.store.rename_host(&origin, &alias, &new_alias) {
                    Ok(()) => self.refresh(),
                    Err(e) => self.error = Some(e.to_string()),
                }
                cx.notify();
            }
            InlineEdit::Folder {
                alias,
                origin,
                input,
            } => {
                let folder = input.read(cx).content().trim().to_string();
                let folder = if folder.is_empty() {
                    None
                } else {
                    Some(folder)
                };
                match self.store.set_host_folder(&origin, &alias, folder) {
                    Ok(()) => self.refresh(),
                    Err(e) => self.error = Some(e.to_string()),
                }
                cx.notify();
            }
        }
    }
}

/// A short, stable per-row element-id disambiguator: gpui `id()` tuples need a `Hash`
/// key, and (alias, origin) — the row's real identity — isn't `Copy`, so this collapses
/// it to a `u64` via a plain FNV-1a over the debug-formatted key. Display-only (element
/// ids, not persisted/compared data), so a hash collision would only mean two rows
/// share a gpui element id — extremely unlikely for the small fleets this grid holds, and
/// harmless (gpui just needs ids unique enough to preserve per-row interaction state).
fn row_hash(alias: &str, origin: &Scope) -> u64 {
    let s = format!("{alias:?}{origin:?}");
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use sid_store::{AuthMethod, WorkspaceId};

    fn host(alias: &str, folder: Option<&str>) -> Host {
        Host {
            alias: alias.into(),
            user: "u".into(),
            host: "h".into(),
            port: 22,
            secret_ref: None,
            auth: AuthMethod::default(),
            folder: folder.map(str::to_string),
        }
    }

    fn attributed(h: Host) -> Attributed<Host> {
        Attributed {
            item: h,
            origin: Scope::Global,
            duplicate: false,
        }
    }

    #[test]
    fn group_by_folder_puts_ungrouped_hosts_first_flat() {
        let hosts = vec![
            attributed(host("zeta", Some("work"))),
            attributed(host("alpha", None)),
            attributed(host("beta", None)),
        ];
        let groups = group_by_folder(&hosts);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].folder, None);
        let names: Vec<&str> = groups[0]
            .hosts
            .iter()
            .map(|a| a.item.alias.as_str())
            .collect();
        assert_eq!(names, vec!["alpha", "beta"]);
        assert_eq!(groups[1].folder, Some("work"));
    }

    #[test]
    fn group_by_folder_sorts_named_folders_alphabetically() {
        let hosts = vec![
            attributed(host("h1", Some("zeta-folder"))),
            attributed(host("h2", Some("alpha-folder"))),
        ];
        let groups = group_by_folder(&hosts);
        assert_eq!(groups[0].folder, Some("alpha-folder"));
        assert_eq!(groups[1].folder, Some("zeta-folder"));
    }

    #[test]
    fn group_by_folder_omits_the_none_group_when_empty() {
        let hosts = vec![attributed(host("h1", Some("work")))];
        let groups = group_by_folder(&hosts);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].folder, Some("work"));
    }

    #[test]
    fn filter_hosts_matches_alias_or_address_case_insensitively() {
        let hosts = vec![
            attributed(host("web-1", None)),
            attributed(Host {
                user: "deploy".into(),
                host: "prod.acme.internal".into(),
                ..host("prod", None)
            }),
        ];
        let found = filter_hosts(&hosts, "ACME");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].item.alias, "prod");

        let found = filter_hosts(&hosts, "web");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].item.alias, "web-1");

        assert_eq!(filter_hosts(&hosts, "").len(), 2);
        assert_eq!(filter_hosts(&hosts, "   ").len(), 2);
    }

    #[test]
    fn parse_quick_connect_reads_user_host_port() {
        assert_eq!(
            parse_quick_connect("deploy@web-1.acme.io:2222"),
            Some(("deploy".to_string(), "web-1.acme.io".to_string(), 2222))
        );
    }

    #[test]
    fn parse_quick_connect_defaults_port_22() {
        assert_eq!(
            parse_quick_connect("root@10.1.50.2"),
            Some(("root".to_string(), "10.1.50.2".to_string(), 22))
        );
    }

    #[test]
    fn parse_quick_connect_rejects_non_matching_or_partial_input() {
        assert_eq!(parse_quick_connect("just typing"), None);
        assert_eq!(parse_quick_connect("@host"), None);
        assert_eq!(parse_quick_connect("user@"), None);
        assert_eq!(parse_quick_connect("user@host:"), None);
        assert_eq!(parse_quick_connect("user@host:notaport"), None);
    }

    #[test]
    fn a_row_with_no_session_reads_as_not_connected() {
        assert_eq!(connection_state(None), ConnectionState::Offline);
    }

    #[test]
    fn the_dot_now_separates_dialling_from_connected_from_failed() {
        // The whole point of the four-state mark: the old `●`/`○` could only say
        // "there is a tab for this row" or "there isn't".
        assert_eq!(
            connection_state(Some(&SessionStatus::Connecting)),
            ConnectionState::Connecting
        );
        assert_eq!(
            connection_state(Some(&SessionStatus::Connected)),
            ConnectionState::Live
        );
        assert_eq!(
            connection_state(Some(&SessionStatus::Failed("no route to host".into()))),
            ConnectionState::Failed
        );
    }

    #[test]
    fn a_closed_session_frees_the_row_rather_than_marking_it_failed() {
        // The tab stays open so its scrollback is still readable, but nothing is
        // connected — the row is ready to dial again and should look it.
        assert_eq!(
            connection_state(Some(&SessionStatus::Closed)),
            ConnectionState::Offline
        );
    }

    #[test]
    fn a_row_with_nothing_open_offers_connect() {
        assert_eq!(row_primary(ConnectionState::Offline), RowPrimary::Connect);
        assert_eq!(row_primary(ConnectionState::Offline).label(), "connect");
    }

    #[test]
    fn a_row_that_already_has_a_session_goes_to_it_instead_of_dialling_twice() {
        // The bug this pins: `connect_host` always pushes a NEW tab, so clicking
        // connect on an already-connected row opened a second session to the same host
        // while the row's own dot sat there saying one was already up.
        assert_eq!(row_primary(ConnectionState::Live), RowPrimary::Focus);
        assert_eq!(row_primary(ConnectionState::Connecting), RowPrimary::Focus);
        assert_eq!(row_primary(ConnectionState::Live).label(), "focus");
    }

    #[test]
    fn a_failed_row_offers_a_retry_not_a_trip_to_the_wreckage() {
        // Retrying is what a failed row is for. Its tab stays open behind the retry so
        // the error text is still readable.
        assert_eq!(row_primary(ConnectionState::Failed), RowPrimary::Connect);
    }

    #[test]
    fn every_primary_verb_names_itself_and_its_object() {
        // The button replaces a hint string; it cannot itself be cryptic.
        for verb in [RowPrimary::Connect, RowPrimary::Focus] {
            assert!(!verb.label().is_empty(), "{verb:?}: no label");
            assert!(!verb.tooltip().is_empty(), "{verb:?}: no tooltip");
            assert_ne!(verb.label(), verb.tooltip());
        }
        assert_ne!(RowPrimary::Connect.label(), RowPrimary::Focus.label());
    }

    #[test]
    fn one_click_on_a_card_only_picks_it_up() {
        // The card body is a ~300x110px target — twenty times the area of the old row's
        // connect glyph. A single click on that much surface must never dial a machine:
        // "easier to work with" cannot mean "easier to SSH somewhere by accident".
        for state in [
            ConnectionState::Offline,
            ConnectionState::Connecting,
            ConnectionState::Live,
            ConnectionState::Failed,
        ] {
            assert_eq!(
                card_click_action(1, state),
                CardAction::Select,
                "{state:?}: a single click resolved to something that talks to a host"
            );
        }
    }

    #[test]
    fn a_double_click_opens_the_card_the_same_way_its_own_button_would() {
        // Two verbs, one decision: the card's double-click and the card's connect button
        // must never disagree about what "open this" means, so both read `row_primary`.
        assert_eq!(
            card_click_action(2, ConnectionState::Offline),
            CardAction::Connect
        );
        assert_eq!(
            card_click_action(2, ConnectionState::Failed),
            CardAction::Connect
        );
        assert_eq!(
            card_click_action(2, ConnectionState::Live),
            CardAction::Focus
        );
        assert_eq!(
            card_click_action(2, ConnectionState::Connecting),
            CardAction::Focus
        );
    }

    #[test]
    fn the_card_and_its_button_never_disagree() {
        // Pins the two together rather than to a hand-written table, so a change to
        // `row_primary` cannot leave the double-click dialling a second session.
        for state in [
            ConnectionState::Offline,
            ConnectionState::Connecting,
            ConnectionState::Live,
            ConnectionState::Failed,
        ] {
            let expected = match row_primary(state) {
                RowPrimary::Connect => CardAction::Connect,
                RowPrimary::Focus => CardAction::Focus,
            };
            assert_eq!(card_click_action(2, state), expected, "{state:?}");
        }
    }

    #[test]
    fn a_triple_click_is_still_an_open_not_a_second_dial() {
        // gpui keeps counting while the mouse keeps clicking; a fast triple-click on an
        // offline card must not be read as "not a double-click" and fall back to select.
        assert_eq!(
            card_click_action(3, ConnectionState::Offline),
            CardAction::Connect
        );
        assert_eq!(
            card_click_action(9, ConnectionState::Live),
            CardAction::Focus
        );
        // A keyboard-dispatched click reports a count of 1 — select, never connect.
        assert_eq!(
            card_click_action(0, ConnectionState::Offline),
            CardAction::Select
        );
    }

    #[test]
    fn a_named_folder_always_titles_its_own_section() {
        assert_eq!(section_title(Some("work"), 2), Some("work"));
        assert_eq!(section_title(Some("work"), 1), Some("work"));
    }

    #[test]
    fn ungrouped_hosts_are_only_named_when_there_is_something_to_tell_them_apart_from() {
        // With no folders at all, every card is in the one implicit group, and a lone
        // "UNGROUPED" rule above the whole screen is a header for nothing.
        assert_eq!(section_title(None, 0), None);
        // As soon as a named folder exists, the unfiled cards need a name of their own —
        // otherwise they read as belonging to whatever header is nearest.
        assert_eq!(section_title(None, 1), Some("ungrouped"));
        assert_eq!(section_title(None, 5), Some("ungrouped"));
    }

    #[test]
    fn the_section_titles_agree_with_the_grouping_they_describe() {
        // Ties the heading rule to the real grouping transform, so the two cannot drift.
        let hosts = vec![
            attributed(host("alpha", None)),
            attributed(host("zeta", Some("work"))),
        ];
        let groups = group_by_folder(&hosts);
        let named = groups.iter().filter(|g| g.folder.is_some()).count();
        let titles: Vec<Option<&str>> = groups
            .iter()
            .map(|g| section_title(g.folder, named))
            .collect();
        assert_eq!(titles, vec![Some("ungrouped"), Some("work")]);

        // The all-unfiled store — the shape a first-run user actually has.
        let flat = vec![attributed(host("alpha", None))];
        let groups = group_by_folder(&flat);
        let named = groups.iter().filter(|g| g.folder.is_some()).count();
        assert_eq!(section_title(groups[0].folder, named), None);
    }

    #[test]
    fn a_list_with_rows_in_it_is_never_an_empty_state() {
        assert_eq!(home_empty(2, 2), None);
        assert_eq!(
            home_empty(2, 1),
            None,
            "a filtered-down list still has rows"
        );
        assert_eq!(home_empty(500, 1), None);
    }

    #[test]
    fn a_first_run_screen_asks_for_a_host() {
        assert_eq!(home_empty(0, 0), Some(HomeEmpty::NoHosts));
    }

    #[test]
    fn a_filter_that_matches_nothing_is_a_different_nothing() {
        // The two used to render identically — as literally nothing under a
        // `CONNECTIONS · 0` header. They need different words and different exits:
        // answering a search with "add a connection" would be a non-sequitur.
        assert_eq!(home_empty(3, 0), Some(HomeEmpty::NoMatch));
        assert_ne!(home_empty(3, 0), home_empty(0, 0));
    }

    #[test]
    fn the_empty_states_agree_with_the_filter_they_describe() {
        // Ties `home_empty` to the real filter rather than to a hand-written count, so
        // the two cannot drift: a blank query matches everything, so a non-empty store
        // can never land in `NoHosts`.
        let hosts = vec![attributed(host("web-1", None))];
        let matched = filter_hosts(&hosts, "").len();
        assert_eq!(home_empty(hosts.len(), matched), None);

        let matched = filter_hosts(&hosts, "nothing-matches-this").len();
        assert_eq!(home_empty(hosts.len(), matched), Some(HomeEmpty::NoMatch));

        let none: Vec<Attributed<Host>> = Vec::new();
        let matched = filter_hosts(&none, "").len();
        assert_eq!(home_empty(none.len(), matched), Some(HomeEmpty::NoHosts));
    }

    #[test]
    fn workspace_scope_hosts_still_filter_and_group() {
        // Sanity check the pure helpers don't assume `Scope::Global` — a workspace-origin
        // row groups/filters identically.
        let ws = Scope::Workspace(WorkspaceId("/w".to_string()));
        let hosts = vec![Attributed {
            item: host("staging", Some("prod")),
            origin: ws,
            duplicate: false,
        }];
        assert_eq!(group_by_folder(&hosts)[0].folder, Some("prod"));
        assert_eq!(filter_hosts(&hosts, "staging").len(), 1);
    }
}
