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
//! [`home_empty`] (which nothing the grid is showing). Issue #2 adds the decisions
//! behind "add what I just typed": [`saved_match`] (does this text *name* a saved
//! connection?), [`add_seed`]/[`seed_display`] (what an unmatched query hands the form),
//! [`add_row_visible`] (when the inline add row is there) and [`quick_enter`] (what Enter
//! does — never an ad-hoc dial any more). Rendering is observation-gated, per the plan's
//! pragmatic-TDD rule.

use std::collections::BTreeMap;
use std::rc::Rc;

use gpui::{
    AnyElement, ClickEvent, Context, Entity, IntoElement, MouseButton, MouseDownEvent,
    SharedString, Subscription, Window, actions, div, prelude::*, px, rgb,
};
use sid_store::{Attributed, AuthMethod, Host, Scope};

use crate::app::{AppState, can_demote, can_promote};
use crate::keymap::{Action, default_bindings, primary_shortcut};
use crate::ui::ssh_import::ImportSlot;
use crate::ui::{SessionStatus, host_form};
use sid_ui::component::{ContextMenuExt, PopupMenu};
use sid_ui::input::FieldWidth;
use sid_ui::{
    Badge, Button, ButtonSize, Card, CardGrid, ConnectionState, ContextMenu, EmptyState, GridCard,
    Icon, IconButton, InputState, Kbd, List, MenuItem, Row, StatusDot, StyledExt as _, TextInput,
    TreeNode, TreeState, Typography as _, caveat_line, h_flex, scaled, theme, v_flex,
};

actions!(
    ssh_home,
    [
        /// Commit the in-place rename/folder edit (bound to `enter`).
        InlineEditCommit,
        /// Cancel the in-place rename/folder edit (bound to `escape`).
        InlineEditCancel,
        /// Fire the quick-connect box (bound to `enter` in its key context) — found by
        /// the capture-harness shakedown: Enter did nothing, only the Go button worked.
        /// Since issue #2 it opens a *saved* connection, or offers to add an unsaved one.
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
        input: Entity<InputState>,
    },
    Folder {
        alias: String,
        origin: Scope,
        input: Entity<InputState>,
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
    /// The folders tree beside the grid (shown only when a folder exists). Its
    /// expansion and selection live in the entity, so they survive refreshes by key
    /// (they are not persisted across launches).
    tree: Entity<TreeState>,
    _tree_subscription: Subscription,
    /// The folder the tree last selected: the grid shows only its hosts. `None` = all.
    folder_filter: Option<String>,
    /// The "Import from SSH config" dialog (see `ui::ssh_import`).
    pub(crate) import: ImportSlot,
    /// Quick-connect / filter box: as-you-type substring filter over the grid, and — on
    /// Enter — either the saved connection the text *names* or an invitation to add it.
    search: Entity<InputState>,
    /// The row currently mid-rename or mid-folder-edit, if any.
    edit: Option<InlineEdit>,
    /// What the box's last Enter had to say, shown under it until the next attempt or
    /// edit. Only ever [`missing_message`] now: with ad-hoc dialling retired there is no
    /// longer any such thing as a quick-connect parse *failure* — every non-empty query
    /// either names a connection or seeds one.
    quick_note: Option<String>,
    /// Which card (if any) the grid's last right-click landed on — `None` reads as
    /// "empty space" (or a folder header). Feeds the grid's *single* `context_menu`
    /// (attached to the whole scroll container, in [`AppState::ssh_home_main`]),
    /// which decides card-menu vs. "+ Add connection" from this.
    ///
    /// This indirection exists because `ContextMenuExt` (`sid_ui::component::ContextMenuExt`)
    /// can't be attached once per card: its convenience method hardcodes the wrapper's element id
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
    pub(crate) fn focus_filter(&self, window: &mut Window, cx: &mut gpui::App) {
        self.search.update(cx, |state, cx| state.focus(window, cx));
    }

    /// Whether the import dialog is up (it blocks the root key dispatcher like the forms).
    pub(crate) fn import_open(&self) -> bool {
        self.import.modal.is_some()
    }

    /// The import dialog's scrim, for `AppState::render` to stack with the other overlays.
    pub(crate) fn import_overlay(&self, window: &Window) -> Option<impl IntoElement + use<>> {
        self.import
            .modal
            .clone()
            .map(|modal| sid_ui::modal::overlay(window, modal))
    }

    pub(crate) fn new(window: &mut Window, cx: &mut Context<AppState>) -> Self {
        let tree = cx.new(|cx| TreeState::new("ssh-groups", cx));
        let subscription = cx.subscribe_in(
            &tree,
            window,
            |this: &mut AppState, _tree, ev, window, cx| {
                this.on_groups_tree_event(ev, window, cx);
            },
        );
        Self {
            tree,
            _tree_subscription: subscription,
            folder_filter: None,
            import: ImportSlot::default(),
            search: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("user@host[:port] — filter, connect, or add")
            }),
            edit: None,
            quick_note: None,
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
const UNGROUPED: &str = "Ungrouped";

/// Parse the quick-connect box's `user@host[:port]` shorthand. `None` if the text
/// doesn't look like that shape at all — the same box doubles as a plain grid filter, so
/// a partial query (most keystrokes) must not read as a failed connect attempt.
///
/// Since issue #2 a `None` is not an error anywhere: [`add_seed`] wraps this, and answers
/// a rejection by seeding the add form with the raw text instead of throwing it away.
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

/// The fields an unmatched quick-connect query hands to the add form (issue #2).
///
/// Everything typed survives the trip. A query that is not the strict `user@host[:port]`
/// shape still produces a seed — the form's own validation is where a bad hostname gets
/// argued with, and silently dropping half of what someone typed is a worse answer than
/// showing it back to them in a field they can fix.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AddSeed {
    /// The record's proposed name.
    pub alias: String,
    /// The login, empty when the query named no user.
    pub user: String,
    /// The hostname or address.
    pub host: String,
    /// The port; 22 unless the query said otherwise.
    pub port: u16,
}

/// What the quick-connect box's Enter (and its `connect` button) does with what is
/// currently typed.
///
/// **Ad-hoc dialling is deliberately gone.** Enter used to build a throwaway [`Host`] out
/// of whatever parsed and connect to it with `source: None` — a session with no record
/// behind it, no dot on any card, and nothing to come back to tomorrow. Issue #2 retires
/// that: an unknown target is an invitation to save one, not a connection that evaporates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum QuickEnter {
    /// Nothing typed — Enter has nothing to act on.
    Nothing,
    /// The text names a saved connection: dial that record.
    Connect {
        /// The matched record's alias.
        alias: String,
        /// The layer it was read from.
        origin: Scope,
    },
    /// The text names something no layer holds: say so, and open the add form filled in
    /// from it.
    AddMissing(AddSeed),
}

/// The saved record `query` *names*, if any — the question "is this already a
/// connection?", which is a different question from the one [`filter_hosts`] answers.
///
/// Exact, case-insensitive, and tried in the order a person means them: the record's own
/// name first, then the address in the three shapes the address is written in. A
/// substring is deliberately not a match — `web` narrows the grid to `web-1`, `web-2` and
/// `web-3`, and Enter must not pick one of them for you.
pub(crate) fn saved_match<'a>(
    hosts: &'a [Attributed<Host>],
    query: &str,
) -> Option<&'a Attributed<Host>> {
    let q = query.trim().to_ascii_lowercase();
    if q.is_empty() {
        return None;
    }
    let keys: [fn(&Host) -> String; 4] = [
        |h| h.alias.to_ascii_lowercase(),
        |h| format!("{}@{}", h.user, h.host).to_ascii_lowercase(),
        |h| format!("{}@{}:{}", h.user, h.host, h.port).to_ascii_lowercase(),
        |h| h.host.to_ascii_lowercase(),
    ];
    keys.into_iter()
        .find_map(|key| hosts.iter().find(|a| key(&a.item) == q))
}

/// Fill an [`AddSeed`] from whatever is in the box.
///
/// Reuses [`parse_quick_connect`] for the strict shape and falls back to a forgiving
/// split, so `deploy@web-1:notaport` seeds a *visible* broken host rather than quietly
/// becoming `deploy@web-1`.
pub(crate) fn add_seed(query: &str) -> AddSeed {
    let s = query.trim();
    if let Some((user, host, port)) = parse_quick_connect(s) {
        return AddSeed {
            alias: format!("{user}@{host}"),
            user,
            host,
            port,
        };
    }
    let (user, host) = match s.split_once('@') {
        Some((user, host)) => (user.trim().to_string(), host.trim().to_string()),
        None => (String::new(), s.to_string()),
    };
    let alias = match user.is_empty() {
        true => host.clone(),
        false => format!("{user}@{host}"),
    };
    AddSeed {
        alias,
        user,
        host,
        port: 22,
    }
}

/// How a seed is written on the add row and in the "not saved" message — the same string
/// in both, so the row you clicked and the sentence you get name the same thing.
///
/// The default port is left off: `deploy@web-1` is what the user typed and what they will
/// recognise; `deploy@web-1:22` is the machine talking back.
pub(crate) fn seed_display(seed: &AddSeed) -> String {
    let base = match seed.user.is_empty() {
        true => seed.host.clone(),
        false => format!("{}@{}", seed.user, seed.host),
    };
    match seed.port {
        22 => base,
        port => format!("{base}:{port}"),
    }
}

/// Whether the inline "add …" row belongs under the quick-connect box.
///
/// Non-empty and unmatched: the row is the answer to "what I typed isn't here", so it has
/// nothing to say about an empty box, and offering to *add* a connection that already
/// exists would be offering the one thing the add-mode guard is about to refuse.
pub(crate) fn add_row_visible(query: &str, matched: bool) -> bool {
    !query.trim().is_empty() && !matched
}

/// What Enter does, from the box's text and the composed host list.
pub(crate) fn quick_enter(hosts: &[Attributed<Host>], query: &str) -> QuickEnter {
    if query.trim().is_empty() {
        return QuickEnter::Nothing;
    }
    match saved_match(hosts, query) {
        Some(a) => QuickEnter::Connect {
            alias: a.item.alias.clone(),
            origin: a.origin.clone(),
        },
        None => QuickEnter::AddMissing(add_seed(query)),
    }
}

/// The line shown under the box when Enter hit nothing — issue #2's *"must SAY it does
/// not exist"*.
///
/// It names the target rather than the mistake, because there is no mistake: the form
/// opening over this row is the rest of the sentence.
pub(crate) fn missing_message(seed: &AddSeed) -> String {
    format!(
        "{} is not a saved connection — add it, and sid will open it once you do",
        seed_display(seed)
    )
}

// ---- tree / get-started / card-info logic (unit-tested) ----------------------------------

/// Below this many saved connections the home shows its "Get started" cards.
pub(crate) const GET_STARTED_BELOW: usize = 6;

/// Whether the "Get started" surface fills the free space: only while the fleet is small.
pub(crate) fn show_get_started(total: usize) -> bool {
    total < GET_STARTED_BELOW
}

/// The folder filter that still applies: a filter naming a folder no host is in any more
/// (renamed, emptied) is ignored rather than showing an empty grid.
pub(crate) fn effective_folder<'a>(
    filter: Option<&'a str>,
    hosts: &[Attributed<Host>],
) -> Option<&'a str> {
    filter.filter(|f| hosts.iter().any(|a| a.item.folder.as_deref() == Some(*f)))
}

/// One line of auth info for a card.
pub(crate) fn auth_summary(auth: &AuthMethod) -> String {
    match auth {
        AuthMethod::Agent => "Agent".into(),
        AuthMethod::Password => "Password".into(),
        AuthMethod::Key { path } => {
            let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
            format!("Key {name}")
        }
    }
}

/// What a groups-tree key refers to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TreeTarget {
    All,
    Folder(String),
    Host(u64),
}

const TREE_ALL: &str = "all";

/// Decode a tree key built by [`tree_nodes`].
pub(crate) fn parse_tree_key(key: &str) -> TreeTarget {
    if let Some(f) = key.strip_prefix("folder:") {
        return TreeTarget::Folder(f.to_string());
    }
    if let Some(id) = key.strip_prefix("host:").and_then(|n| n.parse().ok()) {
        return TreeTarget::Host(id);
    }
    TreeTarget::All
}

/// The groups tree: `All connections`, then one branch per named folder holding its
/// hosts. A store with no folders yields only the `All` node (the caller hides the tree).
pub(crate) fn tree_nodes(hosts: &[Attributed<Host>]) -> Vec<TreeNode> {
    let mut roots = vec![
        TreeNode::leaf(TREE_ALL, "All connections")
            .icon(Icon::Globe)
            .meta(hosts.len().to_string()),
    ];
    for group in group_by_folder(hosts) {
        let Some(folder) = group.folder else { continue };
        let children = group
            .hosts
            .iter()
            .map(|a| {
                TreeNode::leaf(
                    format!("host:{}", row_hash(&a.item.alias, &a.origin)),
                    a.item.alias.clone(),
                )
                .icon(Icon::Terminal)
            })
            .collect();
        roots.push(
            TreeNode::branch(format!("folder:{folder}"), folder, children)
                .icon(Icon::Folder)
                .meta(group.hosts.len().to_string()),
        );
    }
    roots
}

// ---- rendering + row actions (observation-gated) ------------------------------------

impl AppState {
    /// Home's connections surface: a folders `Tree` (when any folder exists) beside a
    /// **card-grid** panel. Quick-connect field, `Import` and `Add connection` live in the
    /// panel header; the grid below groups hosts under folder headings and flows into as
    /// many columns as the window has room for. With only a few hosts the cards grow and a
    /// "Get started" row of cards fills the rest of the panel (see [`show_get_started`]) so
    /// a wide window is never a small cluster beside a void.
    ///
    /// This IS the connection manager; there is no second list anywhere.
    pub(crate) fn ssh_home_main(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::active(cx).clone();
        let query = self.ssh_home.search.read(cx).value().to_string();
        let folder = effective_folder(self.ssh_home.folder_filter.as_deref(), &self.hosts);
        let in_folder: Vec<Attributed<Host>> = self
            .hosts
            .iter()
            .filter(|a| folder.is_none_or(|f| a.item.folder.as_deref() == Some(f)))
            .cloned()
            .collect();
        let owned: Vec<Attributed<Host>> = filter_hosts(&in_folder, &query)
            .into_iter()
            .cloned()
            .collect();
        let empty = home_empty(self.hosts.len(), owned.len());
        let groups = group_by_folder(&owned);
        let named = groups.iter().filter(|g| g.folder.is_some()).count();
        let few = show_get_started(self.hosts.len()) && query.trim().is_empty();

        // Keep the tree's rows in step with the store. Guarded by equality so a steady
        // state never notifies (this runs in render).
        let nodes = tree_nodes(&self.hosts);
        let tree = self.ssh_home.tree.clone();
        if tree.read(cx).roots() != nodes.as_slice() {
            tree.update(cx, |tree, cx| tree.set_roots(nodes.clone(), cx));
        }
        let has_folders = nodes.len() > 1;

        let mut sections = Vec::new();
        for group in groups {
            let mut section = v_flex().w_full().gap_3();
            if let Some(title) = section_title(group.folder, named) {
                let count = group.hosts.len();
                section = section.child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_heading(&t)
                                .clamp_one_line()
                                .child(SharedString::from(title.to_string())),
                        )
                        .child(div().flex_none().child(Badge::count(count).pill())),
                );
            }
            let cards: Vec<_> = group.hosts.iter().map(|a| self.host_card(a, cx)).collect();
            section = section.child(
                CardGrid::new()
                    .min_col(px(320.))
                    .max_col(match few {
                        true => px(800.),
                        false => px(440.),
                    })
                    .children(cards),
            );
            sections.push(section.into_any_element());
        }

        let panel = Card::panel("connections")
            .count(self.hosts.len())
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .action(self.quick_connect_field(cx))
            .action(
                Button::new("ssh-home-import", "Import")
                    .icon(Icon::File)
                    .tooltip("Import connections from ~/.ssh/config")
                    .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                        this.open_ssh_import(window, cx);
                    })),
            )
            .action(
                Button::new("ssh-home-add-connection", "Add connection")
                    .primary()
                    .icon(Icon::Add)
                    .tooltip("Save a new SSH connection")
                    .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                        this.open_add_form(window, cx);
                    })),
            );

        let panel = panel.children(self.quick_connect_answer(folder, cx)).child(
            List::scrolling("ssh-home-grid")
                .gap_5()
                .p(sid_ui::CARD_PADDING.len())
                // Right-click anywhere in the grid defaults to "no card": this
                // capture-phase handler runs before any card's bubble-phase one, which
                // then sets the target back if the click landed on a card.
                .capture_any_mouse_down(cx.listener(|this, ev: &MouseDownEvent, _window, cx| {
                    if ev.button == MouseButton::Right {
                        this.ssh_home.right_click_target = None;
                        cx.notify();
                    }
                }))
                .children(sections)
                .when_some(empty, |this, empty| {
                    this.child(self.home_empty_state(empty, &query, cx))
                })
                .when(few && empty.is_none(), |this| {
                    this.child(self.get_started(cx))
                })
                .when(empty.is_none(), |this| {
                    this.child(div().flex_1().min_h(scaled(48.)))
                })
                // ONE context menu for the whole grid — see `right_click_target`.
                .context_menu(self.grid_context_menu(cx)),
        );

        let groups_card = has_folders.then(|| {
            Card::panel("groups")
                .w(scaled(260.))
                .flex_none()
                .child(div().flex_1().min_h(px(0.)).p_2().child(tree))
        });

        div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h(px(0.))
            .gap(sid_ui::SECTION_GAP.len())
            .p(sid_ui::PAGE_GUTTER.len())
            .children(groups_card)
            .child(panel)
    }

    /// The "Get started" surface: three cards that occupy the free width while there are
    /// few connections — add one, import from `~/.ssh/config`, and the keys worth
    /// knowing (real bindings from the keymap). It collapses away at
    /// [`GET_STARTED_BELOW`] hosts.
    fn get_started(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::active(cx).clone();
        let bindings = default_bindings();
        let key = |action: Action| primary_shortcut(action, &bindings);
        let start_card =
            |id: &'static str, icon: Icon, title: &'static str, body: &'static str| -> GridCard {
                GridCard::new(id)
                    .child(
                        div()
                            .size(scaled(40.))
                            .flex_none()
                            .rounded_full()
                            .bg(rgb(t.selection))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon.el().size(scaled(20.)).text_color(rgb(t.accent))),
                    )
                    .child(div().text_heading(&t).child(title))
                    .child(div().text_body(&t).text_color(rgb(t.muted)).child(body))
            };

        let know_card =
            |id: &'static str, icon: Icon, title: &'static str, body: &'static str| -> GridCard {
                GridCard::new(id)
                    .child(icon.el().size(scaled(20.)).text_color(rgb(t.accent)))
                    .child(div().text_heading(&t).child(title))
                    .child(div().text_body(&t).text_color(rgb(t.muted)).child(body))
            };

        let add = start_card(
            "ssh-start-add",
            Icon::Add,
            "Add a connection",
            "Save a host once, then reach it with one click, Enter, or the palette.",
        )
        .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
            this.open_add_form(window, cx);
        }));
        let import = start_card(
            "ssh-start-import",
            Icon::File,
            "Import from ~/.ssh/config",
            "Bring your existing hosts along. You review them first; nothing is overwritten.",
        )
        .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
            this.open_ssh_import(window, cx);
        }));

        let tip = |label: &'static str, spec: Option<String>| {
            h_flex()
                .w_full()
                .justify_between()
                .child(div().text_body(&t).child(label))
                .children(spec.map(Kbd::new))
        };
        let keys = start_card(
            "ssh-start-keys",
            Icon::Terminal,
            "Keyboard tips",
            "Everything here works without the mouse.",
        )
        .child(
            v_flex()
                .w_full()
                .gap_1()
                .child(tip("Filter or quick-connect", key(Action::FocusFilter)))
                .child(tip("Command palette", key(Action::CommandPalette)))
                .child(tip("New session", key(Action::NewSession)))
                .child(tip("Keyboard cheat sheet", key(Action::CheatSheet))),
        );

        v_flex()
            .w_full()
            .gap_3()
            .child(div().text_heading(&t).child("Get started"))
            .child(
                CardGrid::new()
                    .min_col(px(320.))
                    .max_col(px(640.))
                    .child(add)
                    .child(import)
                    .child(keys),
            )
            .child(
                div()
                    .mt(sid_ui::SECTION_GAP.len())
                    .text_heading(&t)
                    .child("What a session gives you"),
            )
            .child(
                CardGrid::new()
                    .min_col(px(320.))
                    .max_col(px(640.))
                    .child(know_card(
                        "ssh-know-files",
                        Icon::Folder,
                        "Files beside the terminal",
                        "Every session opens with an SFTP panel next to the shell. Browse, preview, upload and rename without leaving the tab.",
                    ))
                    .child(know_card(
                        "ssh-know-edit",
                        Icon::Rename,
                        "Edit remote files locally",
                        "Open a remote file in your own editor; saving writes it back to the server.",
                    ))
                    .child(know_card(
                        "ssh-know-secrets",
                        Icon::User,
                        "Secrets stay in your keyring",
                        "Passwords and key passphrases live in the OS keyring. Saved connections only hold a reference to them.",
                    )),
            )
            .into_any_element()
    }

    /// The list's empty state: what is missing, what to do, and the buttons that do it.
    fn home_empty_state(
        &self,
        empty: HomeEmpty,
        query: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = match empty {
            HomeEmpty::NoHosts => EmptyState::new("No saved connections")
                .icon(Icon::Globe)
                .guidance("Add one, or type user@host above and press Enter.")
                .action(
                    Button::new("ssh-empty-add", "Add connection")
                        .size(ButtonSize::Lg)
                        .primary()
                        .icon(Icon::Add)
                        .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                            this.open_add_form(window, cx);
                        })),
                )
                .secondary(
                    Button::new("ssh-empty-import", "Import from ~/.ssh/config")
                        .size(ButtonSize::Lg)
                        .icon(Icon::File)
                        .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                            this.open_ssh_import(window, cx);
                        })),
                ),
            HomeEmpty::NoMatch => EmptyState::new(format!("Nothing matches \"{}\"", query.trim()))
                .icon(Icon::Search)
                .guidance(
                    "Clear the filter to see every connection, or use the add row above to \
                     save what you typed.",
                )
                .action(
                    Button::new("ssh-empty-clear", "Clear filter")
                        .size(ButtonSize::Lg)
                        .icon(Icon::Close)
                        .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                            this.ssh_home
                                .search
                                .update(cx, |input, cx| input.set_value("", window, cx));
                            this.ssh_home.quick_note = None;
                            this.ssh_home.folder_filter = None;
                            cx.notify();
                        })),
                ),
        };
        div().w_full().flex_1().min_h(scaled(320.)).child(state)
    }

    /// The groups tree's events: a folder narrows the grid to it, "All connections"
    /// widens it again, a host selects its card, activating a host connects.
    fn on_groups_tree_event(
        &mut self,
        ev: &sid_ui::tree::TreeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use sid_ui::tree::TreeEvent;
        match ev {
            TreeEvent::Selected(key) => match parse_tree_key(key) {
                TreeTarget::All => self.ssh_home.folder_filter = None,
                TreeTarget::Folder(f) => self.ssh_home.folder_filter = Some(f),
                TreeTarget::Host(id) => {
                    if let Some(a) = self.host_by_tree_id(id) {
                        self.ssh_home.selected = Some((a.item.alias.clone(), a.origin.clone()));
                    }
                }
            },
            TreeEvent::Activated(key) => {
                if let TreeTarget::Host(id) = parse_tree_key(key)
                    && let Some(a) = self.host_by_tree_id(id)
                {
                    let (host, key) = (a.item.clone(), (a.item.alias.clone(), a.origin.clone()));
                    self.row_primary_action(host, key, window, cx);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn host_by_tree_id(&self, id: u64) -> Option<&Attributed<Host>> {
        self.hosts
            .iter()
            .find(|a| row_hash(&a.item.alias, &a.origin) == id)
    }

    /// Builds the grid's single [`ContextMenuExt::context_menu`]: add/import when nothing
    /// more specific was right-clicked, or the target card's actions when
    /// [`HomeTabState::right_click_target`] says a card was hit.
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
                    Self::row_context_menu(menu, this.clone(), host, origin, scope, cx)
                }
                None => {
                    let (add, import) = (this.clone(), this.clone());
                    ContextMenu::new()
                        .item(MenuItem::new("Add connection").icon(Icon::Add).on_select(
                            move |window, cx| {
                                add.update(cx, |state, cx| state.open_add_form(window, cx));
                            },
                        ))
                        .item(
                            MenuItem::new("Import from ~/.ssh/config")
                                .icon(Icon::File)
                                .on_select(move |window, cx| {
                                    import
                                        .update(cx, |state, cx| state.open_ssh_import(window, cx));
                                }),
                        )
                        .apply(menu, cx)
                }
            }
        }
    }

    /// The per-row menu: connect, rename, a full-form edit, folder assignment, layer moves
    /// and — last, after a separator, as a danger item — delete.
    fn row_context_menu(
        menu: PopupMenu,
        this: Entity<AppState>,
        host: Host,
        origin: Scope,
        scope: Scope,
        cx: &gpui::App,
    ) -> PopupMenu {
        let key = (host.alias.clone(), origin.clone());
        let mut items = ContextMenu::new()
            .item(MenuItem::new("Connect").icon(Icon::Terminal).on_select({
                let (this, host, key) = (this.clone(), host.clone(), key.clone());
                move |window, cx| {
                    let (host, key) = (host.clone(), key.clone());
                    this.update(cx, |state, cx| {
                        state.connect_host(host, Some(key), window, cx)
                    });
                }
            }))
            .item(MenuItem::new("Rename").icon(Icon::Rename).on_select({
                let (this, alias, origin) = (this.clone(), host.alias.clone(), origin.clone());
                move |window, cx| {
                    let (alias, origin) = (alias.clone(), origin.clone());
                    this.update(cx, |state, cx| {
                        state.start_rename(alias, origin, window, cx)
                    });
                }
            }))
            .item(MenuItem::new("Edit…").icon(Icon::Settings).on_select({
                let (this, host, origin) = (this.clone(), host.clone(), origin.clone());
                move |window, cx| {
                    let (host, origin) = (host.clone(), origin.clone());
                    this.update(cx, |state, cx| {
                        state.open_edit_form(host, origin, window, cx)
                    });
                }
            }))
            .item(
                MenuItem::new("Assign folder…")
                    .icon(Icon::Folder)
                    .on_select({
                        let (this, alias, origin, folder) = (
                            this.clone(),
                            host.alias.clone(),
                            origin.clone(),
                            host.folder.clone(),
                        );
                        move |window, cx| {
                            let (alias, origin, folder) =
                                (alias.clone(), origin.clone(), folder.clone());
                            this.update(cx, |state, cx| {
                                state.start_folder_edit(alias, origin, folder, window, cx)
                            });
                        }
                    }),
            );
        if can_promote(&origin) {
            items = items.item(
                MenuItem::new("Promote to global")
                    .icon(Icon::ArrowUp)
                    .on_select({
                        let (this, alias, origin) =
                            (this.clone(), host.alias.clone(), origin.clone());
                        move |_window, cx| {
                            let (alias, origin) = (alias.clone(), origin.clone());
                            this.update(cx, |state, cx| state.promote_row(&alias, &origin, cx));
                        }
                    }),
            );
        }
        if can_demote(&origin, &scope) {
            items = items.item(
                MenuItem::new("Demote to workspace")
                    .icon(Icon::ArrowDown)
                    .on_select({
                        let (this, alias) = (this.clone(), host.alias.clone());
                        move |_window, cx| {
                            let alias = alias.clone();
                            this.update(cx, |state, cx| state.demote_row(&alias, cx));
                        }
                    }),
            );
        }
        items
            .separator()
            .danger_item(MenuItem::new("Delete").icon(Icon::Trash).on_select({
                let secret_ref = host.secret_ref.clone();
                move |_window, cx| {
                    let (alias, origin) = key.clone();
                    let secret_ref = secret_ref.clone();
                    this.update(cx, |state, cx| {
                        state.delete_row(&alias, &origin, secret_ref.as_deref(), cx)
                    });
                }
            }))
            .apply(menu, cx)
    }

    /// The quick-connect box, as the panel header's **filter** slot. Enter connects (or
    /// offers to add), and typing narrows the grid.
    fn quick_connect_field(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        // `min_w(0)` + `overflow_hidden` clips the input's own paint to the box so the
        // placeholder/typed text cannot bleed into the buttons beside it.
        div()
            .key_context(QUICK_CONNECT_CONTEXT)
            .on_action(cx.listener(|this, _: &QuickConnectGo, window, cx| {
                this.quick_connect(window, cx);
            }))
            .flex_none()
            .w(scaled(380.))
            .min_w(px(0.))
            .overflow_hidden()
            .child(TextInput::new(&self.ssh_home.search).width(FieldWidth::Fixed(px(380.))))
    }

    /// What the box has to say back, if anything: the inline add row and the note left
    /// by an Enter that named nothing. Both live under the header, above the grid.
    fn quick_connect_answer(
        &self,
        folder: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let t = theme::active(cx).clone();
        let query = self.ssh_home.search.read(cx).value().to_string();
        let matched = saved_match(&self.hosts, &query).is_some();
        let mut out: Vec<AnyElement> = Vec::new();
        if let Some(folder) = folder {
            out.push(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .px_4()
                    .pt_2()
                    .child(
                        div()
                            .text_meta(&t)
                            .child(SharedString::from(format!("Showing folder {folder}"))),
                    )
                    .child(
                        Button::new("ssh-folder-clear", "Show all")
                            .size(ButtonSize::Sm)
                            .ghost()
                            .on_click(cx.listener(|this, _ev: &ClickEvent, _w, cx| {
                                this.ssh_home.folder_filter = None;
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        // The inline add row — issue #2's headline. It appears the moment what is typed
        // stops naming anything saved, which is also the moment the grid below has
        // nothing left to show; before this, that state was a dead end with a "clear the
        // filter" button on it. See `add_row_visible` for when, and `card_click_action`
        // for why a *single* click is enough here and not on a card: this row is 20px
        // tall, says exactly what it will do, and opens a form rather than a socket.
        if add_row_visible(&query, matched) {
            let seed = add_seed(&query);
            let label: SharedString =
                format!("Add {} as a connection…", seed_display(&seed)).into();
            out.push(
                div()
                    .flex_none()
                    .px_2()
                    .pt_1()
                    .child(
                        Row::new("ssh-quick-add")
                            .leading(
                                Icon::Add
                                    .el()
                                    .size(px(14.))
                                    .text_color(rgb(t.accent))
                                    .into_any_element(),
                            )
                            .child(div().text_body(&t).clamp_one_line().child(label))
                            .on_click(cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                                this.open_seeded_add_form(seed.clone(), window, cx);
                            })),
                    )
                    .into_any_element(),
            );
        }
        // What used to live beside this: "saved connections below · double-click a name
        // to rename · right-click for more" — 12px of muted prose doing the job of three
        // controls. Every one of those interactions still works; none of them is
        // documentation-only any more. What lives here now is the answer to an Enter that
        // hit nothing, and it is a `caveat` rather than an error: typing the name of a
        // machine you have not saved yet is not a mistake.
        if let Some(note) = &self.ssh_home.quick_note {
            out.push(
                div()
                    .flex_none()
                    .px_3()
                    .pt_1()
                    .child(caveat_line(note.clone()))
                    .into_any_element(),
            );
        }
        out
    }

    /// `⏎` / the `connect` button: open the saved connection the box *names*, or — when
    /// it names nothing — say so and offer to add it.
    ///
    /// **This no longer dials an unsaved host.** It used to: any text that parsed as
    /// `user@host[:port]` became a throwaway [`Host`] and a session with `source: None`,
    /// which meant a connection with no record behind it, no dot on any card, and nothing
    /// to come back to. Issue #2 retires that deliberately — every connection sid opens
    /// is one it can open again tomorrow. The cost is one dialog on first use of a host;
    /// the price it replaces is a session you cannot find twice.
    fn quick_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.ssh_home.search.read(cx).value().to_string();
        match quick_enter(&self.hosts, &text) {
            QuickEnter::Nothing => {
                self.ssh_home.quick_note = None;
                cx.notify();
            }
            QuickEnter::Connect { alias, origin } => {
                self.quick_connect_saved(&alias, &origin, window, cx)
            }
            QuickEnter::AddMissing(seed) => {
                self.ssh_home.quick_note = Some(missing_message(&seed));
                self.open_seeded_add_form(seed, window, cx);
            }
        }
    }

    /// Quick-connect's hit path: open (or switch to) the record the box named, then clear
    /// the box — the filter has done its job once you are in the session.
    ///
    /// Goes through [`Self::row_primary_action`], not `connect_host`, so typing the name
    /// of a host that is *already* connected switches to its tab instead of dialling a
    /// second session — the same agreement [`row_primary`] enforces for the card's own
    /// button.
    fn quick_connect_saved(
        &mut self,
        alias: &str,
        origin: &Scope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = self
            .hosts
            .iter()
            .find(|a| a.item.alias == alias && &a.origin == origin)
            .map(|a| a.item.clone())
        else {
            return;
        };
        self.ssh_home.quick_note = None;
        self.ssh_home
            .search
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.row_primary_action(host, (alias.to_string(), origin.clone()), window, cx);
    }

    /// Open the add form already holding `seed`, and dial the record once it is saved.
    ///
    /// The same path for the inline add row and for Enter-on-an-unknown-host: the row is
    /// the mouse's spelling of that key, so they must not diverge into "one of these also
    /// connects".
    fn open_seeded_add_form(&mut self, seed: AddSeed, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity();
        let after_save: host_form::AfterSave = Rc::new(move |host, target, window, cx| {
            let (host, target) = (host.clone(), target.clone());
            this.update(cx, |state, cx| {
                state.connect_after_add(&host, &target, window, cx)
            });
        });
        host_form::queue_add_prefill(
            cx,
            host_form::AddPrefill {
                alias: seed.alias,
                user: seed.user,
                host: seed.host,
                port: seed.port,
                after_save: Some(after_save),
            },
        );
        self.open_add_form(window, cx);
    }

    /// The follow-up to a save that came from quick-connect: dial what was just written.
    ///
    /// Re-reads the composed list rather than trusting the submission it is handed, for
    /// two reasons. A save the add-mode guard refused never reached the store, and this
    /// must do nothing rather than connect — so the *address* is compared too, not just
    /// the alias, or a refused add would dial whichever record already owned that name.
    /// And the stored record is the one carrying the `secret_ref` the secret lifecycle
    /// just minted; the submitted one never has it.
    fn connect_after_add(
        &mut self,
        submitted: &Host,
        target: &Scope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let saved = self.hosts.iter().find(|a| {
            &a.origin == target
                && a.item.alias == submitted.alias
                && a.item.user == submitted.user
                && a.item.host == submitted.host
                && a.item.port == submitted.port
        });
        let Some(host) = saved.map(|a| a.item.clone()) else {
            return;
        };
        self.ssh_home.quick_note = None;
        self.ssh_home
            .search
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.connect_host(
            host,
            Some((submitted.alias.clone(), target.clone())),
            window,
            cx,
        );
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
                .primary()
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
            IconButton::new(slot("ssh-card-files"), Icon::File, "Browse files (SFTP)").on_click(
                cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                    this.row_browse_files(host.clone(), key.clone(), window, cx);
                }),
            )
        };
        let edit = {
            let host = host.clone();
            let origin = origin.clone();
            IconButton::new(slot("ssh-card-edit-btn"), Icon::Settings, "Edit connection").on_click(
                cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                    this.open_edit_form(host.clone(), origin.clone(), window, cx);
                }),
            )
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
                            .text_heading(&t)
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
                            .max_w(scaled(120.))
                            .overflow_hidden()
                            .clamp_one_line()
                            .child(self.scope_chip(a)),
                    ),
            )
            .child(div().w_full().text_mono(&t).clamp_one_line().child(addr))
            .child(
                div()
                    .w_full()
                    .text_meta(&t)
                    .clamp_one_line()
                    .child(SharedString::from(auth_summary(&host.auth))),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .pt_2()
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
        let (input, flag): (Entity<InputState>, &'static str) = match edit {
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
                    .child(
                        div()
                            .w_full()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .child(TextInput::new(&input)),
                    )
                    .child(div().text_meta(&t).text_color(rgb(t.accent)).child(flag)),
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
            let mut state = InputState::new(window, cx).placeholder("alias");
            state.set_value(seed, window, cx);
            state
        });
        input.update(cx, |state, cx| state.focus(window, cx));
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
            let mut state = InputState::new(window, cx).placeholder("folder (blank = none)");
            if let Some(f) = current {
                state.set_value(f, window, cx);
            }
            state
        });
        input.update(cx, |state, cx| state.focus(window, cx));
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
                let new_alias = input.read(cx).value().trim().to_string();
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
                let folder = input.read(cx).value().trim().to_string();
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
    fn get_started_collapses_as_the_fleet_grows() {
        assert!(show_get_started(0));
        assert!(show_get_started(GET_STARTED_BELOW - 1));
        assert!(!show_get_started(GET_STARTED_BELOW));
    }

    #[test]
    fn tree_has_only_all_without_folders() {
        let hosts = vec![attributed(host("a", None))];
        assert_eq!(tree_nodes(&hosts).len(), 1);
    }

    #[test]
    fn tree_nests_hosts_under_their_folder_and_keys_round_trip() {
        let hosts = vec![
            attributed(host("a", Some("work"))),
            attributed(host("b", Some("work"))),
            attributed(host("c", None)),
        ];
        let nodes = tree_nodes(&hosts);
        assert_eq!(nodes.len(), 2);
        assert_eq!(parse_tree_key(&nodes[0].key), TreeTarget::All);
        assert_eq!(
            parse_tree_key(&nodes[1].key),
            TreeTarget::Folder("work".into())
        );
        let sid_ui::TreeChildren::Loaded(kids) = &nodes[1].children else {
            panic!("folder must be a loaded branch");
        };
        assert_eq!(kids.len(), 2);
        assert_eq!(
            parse_tree_key(&kids[0].key),
            TreeTarget::Host(row_hash("a", &Scope::Global))
        );
    }

    #[test]
    fn a_stale_folder_filter_is_ignored() {
        let hosts = vec![attributed(host("a", Some("work")))];
        assert_eq!(effective_folder(Some("work"), &hosts), Some("work"));
        assert_eq!(effective_folder(Some("gone"), &hosts), None);
        assert_eq!(effective_folder(None, &hosts), None);
    }

    #[test]
    fn auth_summary_names_the_key_file_not_the_path() {
        assert_eq!(auth_summary(&AuthMethod::Agent), "Agent");
        assert_eq!(auth_summary(&AuthMethod::Password), "Password");
        assert_eq!(
            auth_summary(&AuthMethod::Key {
                path: "/home/me/.ssh/id_ed25519".into()
            }),
            "Key id_ed25519"
        );
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

    // ---- "is what I typed already a connection?" (issue #2) -------------------------

    fn addressed(alias: &str, user: &str, address: &str, port: u16) -> Attributed<Host> {
        attributed(Host {
            user: user.into(),
            host: address.into(),
            port,
            ..host(alias, None)
        })
    }

    #[test]
    fn a_saved_alias_is_matched_whatever_case_or_padding_it_is_typed_in() {
        let hosts = vec![addressed("Web-1", "deploy", "10.0.0.1", 22)];
        assert!(saved_match(&hosts, "web-1").is_some());
        assert!(saved_match(&hosts, "  WEB-1  ").is_some());
    }

    #[test]
    fn the_address_matches_in_every_shape_it_is_written_in() {
        let hosts = vec![addressed("prod", "deploy", "prod.acme.io", 2222)];
        for query in [
            "deploy@prod.acme.io",
            "deploy@prod.acme.io:2222",
            "prod.acme.io",
        ] {
            assert!(
                saved_match(&hosts, query).is_some(),
                "{query} should name this record"
            );
        }
    }

    #[test]
    fn a_record_name_wins_over_another_records_address() {
        // Someone who names a record after a hostname means *that record* when they type
        // it, not the unrelated host that happens to answer on that address.
        let hosts = vec![
            addressed("db.internal", "root", "10.0.0.9", 22),
            addressed("other", "deploy", "db.internal", 22),
        ];
        let found = saved_match(&hosts, "db.internal").expect("a match");
        assert_eq!(found.item.alias, "db.internal");
    }

    #[test]
    fn an_explicit_port_picks_out_the_record_on_that_port() {
        let hosts = vec![
            addressed("web-22", "deploy", "web.acme.io", 22),
            addressed("web-alt", "deploy", "web.acme.io", 2222),
        ];
        let found = saved_match(&hosts, "deploy@web.acme.io:2222").expect("a match");
        assert_eq!(found.item.alias, "web-alt");
    }

    #[test]
    fn a_substring_narrows_the_grid_without_naming_a_connection() {
        // The box does two jobs and this is the seam between them: `web` is a perfectly
        // good filter and a terrible thing to dial. Pinned against the real filter so the
        // two rules cannot drift into agreeing.
        let hosts = vec![
            addressed("web-1", "deploy", "10.0.0.1", 22),
            addressed("web-2", "deploy", "10.0.0.2", 22),
        ];
        assert_eq!(filter_hosts(&hosts, "web").len(), 2);
        assert_eq!(saved_match(&hosts, "web"), None);
    }

    #[test]
    fn a_blank_box_names_nothing() {
        let hosts = vec![addressed("web-1", "deploy", "10.0.0.1", 22)];
        assert_eq!(saved_match(&hosts, ""), None);
        assert_eq!(saved_match(&hosts, "   "), None);
    }

    // ---- seeding the add form -------------------------------------------------------

    #[test]
    fn a_full_target_seeds_every_field() {
        assert_eq!(
            add_seed("deploy@web-1.acme.io:2222"),
            AddSeed {
                alias: "deploy@web-1.acme.io".into(),
                user: "deploy".into(),
                host: "web-1.acme.io".into(),
                port: 2222,
            }
        );
    }

    #[test]
    fn a_target_without_a_port_seeds_the_default_one() {
        assert_eq!(add_seed("root@10.1.50.2").port, 22);
    }

    #[test]
    fn a_bare_hostname_seeds_the_host_and_leaves_the_user_to_be_filled_in() {
        let seed = add_seed("web-1");
        assert_eq!(seed.host, "web-1");
        assert_eq!(seed.user, "");
        assert_eq!(seed.alias, "web-1");
    }

    #[test]
    fn an_unparseable_port_stays_visible_instead_of_being_dropped() {
        // The strict parser rejects this outright; the seed must not answer by silently
        // connecting to `deploy@web-1` on 22 — the user has to be able to see what they
        // typed in order to fix it.
        let seed = add_seed("deploy@web-1:notaport");
        assert_eq!(seed.user, "deploy");
        assert_eq!(seed.host, "web-1:notaport");
    }

    #[test]
    fn the_seed_is_written_the_way_it_was_typed() {
        assert_eq!(seed_display(&add_seed("deploy@web-1")), "deploy@web-1");
        assert_eq!(
            seed_display(&add_seed("deploy@web-1:2222")),
            "deploy@web-1:2222"
        );
        assert_eq!(seed_display(&add_seed("web-1")), "web-1");
    }

    // ---- the add row, and what Enter does -------------------------------------------

    #[test]
    fn the_add_row_stays_away_from_an_empty_box() {
        assert!(!add_row_visible("", false));
        assert!(!add_row_visible("   ", false));
    }

    #[test]
    fn the_add_row_appears_for_something_that_is_not_saved_yet() {
        assert!(add_row_visible("deploy@web-9", false));
    }

    #[test]
    fn the_add_row_stands_down_once_the_text_names_a_real_connection() {
        assert!(!add_row_visible("web-1", true));
    }

    #[test]
    fn enter_on_a_saved_connection_dials_that_record() {
        let hosts = vec![addressed("web-1", "deploy", "10.0.0.1", 22)];
        assert_eq!(
            quick_enter(&hosts, "web-1"),
            QuickEnter::Connect {
                alias: "web-1".into(),
                origin: Scope::Global,
            }
        );
    }

    #[test]
    fn enter_on_an_unknown_host_offers_to_add_it_instead_of_dialling_it() {
        // The behaviour issue #2 replaces: this used to build a throwaway `Host` and
        // connect to it, leaving a session with no record behind it.
        let hosts = vec![addressed("web-1", "deploy", "10.0.0.1", 22)];
        let QuickEnter::AddMissing(seed) = quick_enter(&hosts, "deploy@web-9") else {
            panic!("an unknown host must not resolve to a connect");
        };
        assert_eq!(seed.host, "web-9");
        assert_eq!(seed.user, "deploy");
    }

    #[test]
    fn enter_on_an_empty_box_does_nothing_at_all() {
        let hosts = vec![addressed("web-1", "deploy", "10.0.0.1", 22)];
        assert_eq!(quick_enter(&hosts, "  "), QuickEnter::Nothing);
    }

    #[test]
    fn the_add_row_and_the_enter_key_never_disagree() {
        // The row is the mouse's spelling of Enter. Pinned to each other rather than to a
        // table, so a change to the matching rules can't leave one of them offering an
        // add the other refuses.
        let hosts = vec![
            addressed("web-1", "deploy", "10.0.0.1", 22),
            addressed("prod", "root", "prod.acme.io", 2222),
        ];
        for query in [
            "",
            "   ",
            "web",
            "web-1",
            "WEB-1",
            "deploy@10.0.0.1",
            "root@prod.acme.io:2222",
            "deploy@web-9",
            "something else entirely",
        ] {
            let matched = saved_match(&hosts, query).is_some();
            let offers_add = matches!(quick_enter(&hosts, query), QuickEnter::AddMissing(_));
            assert_eq!(
                add_row_visible(query, matched),
                offers_add,
                "{query:?}: the row and Enter disagree"
            );
        }
    }

    #[test]
    fn the_missing_message_names_the_target_and_says_it_is_not_saved() {
        let msg = missing_message(&add_seed("deploy@web-9"));
        assert!(msg.contains("deploy@web-9"), "{msg}");
        assert!(msg.contains("not a saved connection"), "{msg}");
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
        assert_eq!(section_title(None, 1), Some("Ungrouped"));
        assert_eq!(section_title(None, 5), Some("Ungrouped"));
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
        assert_eq!(titles, vec![Some("Ungrouped"), Some("work")]);

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
