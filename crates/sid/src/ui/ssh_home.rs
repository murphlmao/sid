//! SSH tab, Home state (ssh-v3): the saved-connections tree sidebar, quick-connect box,
//! and per-row inline rename / folder assignment.
//!
//! [`HomeTabState`] is a sibling cache to `AppState`'s own SSH fields, exactly like
//! `ui::db_tab`'s `DbTabState` — see that module's doc comment for the pattern this
//! mirrors: a second `impl AppState` block here reaches back into `AppState`'s
//! `pub(crate)` fields (`hosts`, `armed_delete`, `ssh_sessions`, `store`, `error`) rather
//! than app.rs growing a tree-rendering section of its own. The Home tab's MAIN pane
//! (the connection-manager host list) stays in `app.rs` (`AppState::ssh_connections_main`)
//! — unchanged from the pre-ssh-v3 single-session SSH tab, just relocated next to this
//! new sidebar.
//!
//! Pure/unit-tested: [`group_by_folder`] (the tree's grouping transform), [`filter_hosts`]
//! (the quick-connect box's search filter), [`parse_quick_connect`] (the `user@host[:port]`
//! shorthand). Rendering is observation-gated, per the plan's pragmatic-TDD rule.

use std::collections::{BTreeMap, HashSet};

use gpui::{
    ClickEvent, Context, Entity, IntoElement, MouseButton, MouseDownEvent, Pixels, SharedString,
    Window, actions, div, prelude::*, px, rgb,
};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use sid_store::{Attributed, Host, Scope};

use crate::app::{AppState, delete_click_executes};
use crate::ui::TextInput;
use crate::ui::theme;

const MONO: &str = "DejaVu Sans Mono";

/// The saved-connections tree's fixed sidebar width (Home state only — the session
/// state's file browser has its own `SIDEBAR_WIDTH` in `session.rs`).
const TREE_WIDTH: Pixels = px(260.);

actions!(
    ssh_home,
    [
        /// Commit the in-place rename/folder edit (bound to `enter`).
        InlineEditCommit,
        /// Cancel the in-place rename/folder edit (bound to `escape`).
        InlineEditCancel,
    ]
);

/// The key context [`InlineEditCommit`]/[`InlineEditCancel`] are scoped to — set on the
/// row wrapper around whichever `TextInput` is mid-edit, exactly like `HostForm`'s own
/// `"HostForm"` context (see that module's doc comment: the binding sits on an ancestor
/// of the focused field, so it fires no matter which nested input has focus).
pub(crate) const INLINE_EDIT_CONTEXT: &str = "SshInlineEdit";

/// An in-place edit box replacing a tree row's label: either renaming the host (VS
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

/// SSH tab, Home state: the tree sidebar's collapse/search/inline-edit UI state. The
/// composed host list itself lives on `AppState` (`self.hosts`) — this only holds
/// view-local state that isn't part of the layered store.
pub(crate) struct HomeTabState {
    /// Folder names collapsed in the tree. Ungrouped hosts (`folder: None`) render
    /// flat at the top and are never collapsible, so this only ever holds named
    /// folders.
    collapsed_folders: HashSet<String>,
    /// Quick-connect / filter box: as-you-type substring filter over the tree, and
    /// (the `⏎` button) a `user@host[:port]` parse for an ephemeral, unsaved connect.
    search: Entity<TextInput>,
    /// The row currently mid-rename or mid-folder-edit, if any.
    edit: Option<InlineEdit>,
    /// The quick-connect box's last parse failure, shown under it until the next
    /// attempt or edit.
    quick_error: Option<String>,
    /// Which row (if any) the tree's last right-click landed on — `None` reads as
    /// "empty space" (or a folder header). Feeds the tree's *single* `context_menu`
    /// (attached to the whole scroll container, in [`AppState::ssh_home_sidebar`]),
    /// which decides row-menu vs. "+ Add connection" from this.
    ///
    /// This indirection exists because `gpui_component::menu::ContextMenuExt` can't be
    /// attached once per row: its convenience method hardcodes the wrapper's element id
    /// to the literal string `"context-menu"`, and none of the tree ancestors between
    /// the scroll container and any given row differ per row — every row's wrapper
    /// would collide on the exact same `GlobalElementId`, sharing (and clobbering) one
    /// `ContextMenuState`. `gpui-component`'s own `Table` sidesteps this the same way:
    /// one `context_menu` on the table body, fed by a `right_clicked_row` field set
    /// from each row's own `on_mouse_down(MouseButton::Right, ..)` (see
    /// `gpui-component`'s `table/state.rs`) — this mirrors that exact pattern.
    ///
    /// Reset to `None` on every right-click via the tree container's
    /// `capture_any_mouse_down` (capture phase, always runs first — see that call
    /// site's doc comment), then set back to `Some((host, origin))` by whichever row's
    /// ordinary bubble-phase `on_mouse_down(Right, ..)` fires next, if any. Emptiness is
    /// therefore the default for any right-click that isn't on a row — no dedicated
    /// "empty space" element needs its own clearing logic, and a short host list (empty
    /// space below the last row) works exactly like an overflowing one (empty space is
    /// merely unreachable by the mouse) or right-clicking a folder header.
    right_click_target: Option<(Host, Scope)>,
}

impl HomeTabState {
    pub(crate) fn new(cx: &mut Context<AppState>) -> Self {
        Self {
            collapsed_folders: HashSet::new(),
            search: cx.new(|cx| TextInput::new(cx, "user@host[:port] — quick connect / filter")),
            edit: None,
            quick_error: None,
            right_click_target: None,
        }
    }
}

// ---- pure tree/search logic (unit-tested) -------------------------------------------

/// One visual group in the saved-connections tree. `folder: None` is the flat,
/// always-expanded top-level group (no header rendered for it); every other value is a
/// named, collapsible folder.
pub(crate) struct FolderGroup<'a> {
    pub folder: Option<&'a str>,
    pub hosts: Vec<&'a Attributed<Host>>,
}

/// Group `hosts` by their `folder` field for the tree: the `None` group always comes
/// first (flat, no header — only emitted if non-empty), followed by named folders in
/// alphabetical order. Within each group, hosts are sorted by alias (case-insensitive)
/// for a stable, predictable order — the composed list's own order just reflects read
/// order, not anything a user picked. Pure; the tree's collapse state is applied by the
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
/// the tree's normal, unfiltered view.
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

/// Parse the quick-connect box's `user@host[:port]` shorthand. `None` if the text
/// doesn't look like that shape at all — the same box doubles as a plain tree filter,
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
    /// The Home tab's SIDEBAR (ssh-v3): quick-connect/filter box above the
    /// folder-grouped saved-connections tree.
    pub(crate) fn ssh_home_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::active(cx);
        let (bg, border) = (t.bg, t.border);
        let query = self.ssh_home.search.read(cx).content().to_string();
        let filtered = filter_hosts(&self.hosts, &query);
        let owned: Vec<Attributed<Host>> = filtered.into_iter().cloned().collect();
        let groups = group_by_folder(&owned);

        let mut rows = Vec::new();
        for (gix, group) in groups.into_iter().enumerate() {
            if let Some(folder) = group.folder {
                let collapsed = self.ssh_home.collapsed_folders.contains(folder);
                rows.push(
                    self.folder_header(gix, folder, collapsed, cx)
                        .into_any_element(),
                );
                if collapsed {
                    continue;
                }
            }
            for a in group.hosts {
                rows.push(self.host_tree_row(a, cx).into_any_element());
            }
        }

        div()
            .w(TREE_WIDTH)
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(bg))
            .border_r_1()
            .border_color(rgb(border))
            .child(self.sidebar_header(cx))
            .child(self.quick_connect_box(cx))
            .child(
                div()
                    .id("ssh-home-tree")
                    .flex_1()
                    .overflow_y_scroll()
                    .py_1()
                    // Right-click *anywhere* in the tree defaults to "no row" —
                    // `capture_any_mouse_down` fires during the CAPTURE phase, which
                    // completes in full before any BUBBLE-phase handler runs (see
                    // `dispatch_mouse_event` in gpui's `window.rs`: capture is one full
                    // pass over every listener, then bubble is a second full pass, in
                    // reverse/child-first order). So this always resets the target
                    // first; a specific row's own `on_mouse_down(Right, ..)` (an
                    // ordinary BUBBLE-phase handler, see `host_tree_row`) then fires
                    // straight after and overrides it back to `Some(row)` — but only
                    // when the click actually landed on that row. Reaching for this
                    // instead of a plain bubble-phase clear on this same container:
                    // bubble fires child-before-parent, so a bubble-phase clear here
                    // would run AFTER (and stomp) a row's bubble-phase set, not before.
                    .capture_any_mouse_down(cx.listener(
                        |this, ev: &MouseDownEvent, _window, cx| {
                            if ev.button == MouseButton::Right {
                                this.ssh_home.right_click_target = None;
                                cx.notify();
                            }
                        },
                    ))
                    .children(rows)
                    // Trailing empty space below the last row, so "right-click empty
                    // space → Add connection" has somewhere to land even when the list
                    // is short — purely a layout spacer; the capture-phase reset above
                    // is what actually makes empty-space right-clicks correct.
                    .child(div().flex_1().min_h(px(48.)))
                    // ONE context menu for the whole tree — see `right_click_target`'s
                    // doc comment on why this can't be attached per-row.
                    .context_menu(self.tree_context_menu(cx)),
            )
    }

    /// Builds the tree's single [`ContextMenuExt::context_menu`]: "+ Add connection"
    /// when nothing more specific was right-clicked, or the target row's actions
    /// (mirrors the plan's "Rename / Edit / Assign folder / Delete", plus `Connect` for
    /// parity with the row's hover icons) when [`HomeTabState::right_click_target`] says
    /// a row was hit. Reads `right_click_target` fresh at build time — set by whichever
    /// row/filler's own `on_mouse_down(MouseButton::Right, ..)` fired first for this
    /// same click, always before this builder runs.
    fn tree_context_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + use<> {
        let this = cx.entity();
        move |menu, _window, cx| {
            let target = this.read(cx).ssh_home.right_click_target.clone();
            match target {
                Some((host, origin)) => Self::row_context_menu(menu, this.clone(), host, origin),
                None => Self::add_connection_menu_item(menu, this.clone(), "+ Add connection"),
            }
        }
    }

    /// A menu with just the single "+ Add connection" item — the empty-space/no-target
    /// case shared by [`Self::tree_context_menu`]'s `None` arm.
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
    /// icons already offer, a full [`crate::ui::host_form::HostForm`] edit, and delete —
    /// the plan's "Rename / Edit / Assign folder / Delete", plus `Connect`.
    fn row_context_menu(
        menu: PopupMenu,
        this: Entity<AppState>,
        host: Host,
        origin: Scope,
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

    /// The sidebar's header row: a title plus the `+ Add connection` affordance —
    /// without this the Home sidebar had no visible way to add a host at all (the
    /// `main` pane's own `+ Add host` button was easy to miss, and the tree itself gave
    /// no hint). Opens the exact same [`HostForm::new_add`] path as every other
    /// add-connection entry point (`main`'s button, the tab-strip `+`, this tree's
    /// empty-space context menu) — see `AppState::open_add_form`.
    fn sidebar_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme::active(cx);
        let (border, muted, selection, fg_strong) =
            (t.border, t.muted, t.selection, t.fg_strong);
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(rgb(border))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(muted))
                    .child("CONNECTIONS"),
            )
            .child(
                div()
                    .id("ssh-home-add-connection")
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .text_xs()
                    .cursor_pointer()
                    .bg(rgb(selection))
                    .text_color(rgb(fg_strong))
                    .child("+ Add connection")
                    .on_click(cx.listener(|this, _ev: &ClickEvent, window, cx| {
                        this.open_add_form(window, cx);
                    })),
            )
    }

    fn quick_connect_box(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::active(cx);
        let (border, muted, accent, fg_strong, danger) =
            (t.border, t.muted, t.accent, t.fg_strong, t.danger);
        let search = self.ssh_home.search.clone();
        // Fixed-width, never squeezed by a long placeholder/typed value — see the `go`
        // doc comment on the sibling input wrapper below for why the input side needs
        // its own clip.
        let go = div()
            .id("ssh-quick-connect-go")
            .w(px(36.))
            .h(px(34.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .text_xs()
            .cursor_pointer()
            .bg(rgb(accent))
            .text_color(rgb(fg_strong))
            .child("⏎")
            .on_click(
                cx.listener(|this, _ev: &ClickEvent, window, cx| this.quick_connect(window, cx)),
            );

        let mut col = div()
            .flex()
            .flex_col()
            .gap_1()
            .px_2()
            .py_2()
            .border_b_1()
            .border_color(rgb(border))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_1()
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
                        div().flex_1().min_w(px(0.)).overflow_hidden().child(search),
                    )
                    .child(go),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(muted))
                    .child("saved hosts below · double-click a name to rename"),
            );
        if let Some(err) = &self.ssh_home.quick_error {
            col = col.child(div().text_xs().text_color(rgb(danger)).child(err.clone()));
        }
        col
    }

    /// `⏎`: parse the quick-connect box as `user@host[:port]` and, if it parses, open an
    /// ephemeral (unsaved) session for it — `source: None`, so the tree's live-dot only
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

    fn folder_header(
        &self,
        gix: usize,
        folder: &str,
        collapsed: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme::active(cx);
        let (muted, selection) = (t.muted, t.selection);
        let caret = if collapsed { "▸" } else { "▾" };
        let owned = folder.to_string();
        div()
            .id(("ssh-folder", gix))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .cursor_pointer()
            .text_xs()
            .text_color(rgb(muted))
            .hover(|s| s.bg(rgb(selection)))
            .child(caret)
            .child(folder.to_string())
            .on_click(cx.listener(move |this, _ev: &ClickEvent, _window, cx| {
                this.toggle_folder(owned.clone(), cx);
            }))
    }

    fn toggle_folder(&mut self, folder: String, cx: &mut Context<Self>) {
        if !self.ssh_home.collapsed_folders.remove(&folder) {
            self.ssh_home.collapsed_folders.insert(folder);
        }
        cx.notify();
    }

    /// One tree row: either the normal (icon, name, live-dot, hover actions) row, or —
    /// while this exact (alias, origin) is mid-edit — the inline rename/folder box.
    fn host_tree_row(
        &self,
        a: &Attributed<Host>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme::active(cx);
        let (fg, muted, selection, accent, danger, success) =
            (t.fg, t.muted, t.selection, t.accent, t.danger, t.success);
        let host = a.item.clone();
        let origin = a.origin.clone();

        if let Some(edit) = &self.ssh_home.edit
            && edit.key() == (host.alias.as_str(), &origin)
        {
            return self.inline_edit_row(edit, cx).into_any_element();
        }

        let key = (host.alias.clone(), origin.clone());
        let is_live = self
            .ssh_sessions
            .iter()
            .any(|t| t.source.as_ref() == Some(&key));
        let armed = delete_click_executes(self.armed_delete.as_ref(), &key);
        let alias: SharedString = host.alias.clone().into();
        let addr: SharedString = format!("{}@{}:{}", host.user, host.host, host.port).into();

        let action = |id: (&'static str, u64), label: SharedString, color: u32| {
            div()
                .id(id)
                .px_1()
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .text_color(rgb(color))
                .hover(|s| s.bg(rgb(selection)))
                .child(label)
        };
        let row_id = row_hash(&host.alias, &origin);

        let connect = {
            let host = host.clone();
            let key = key.clone();
            action(("ssh-tree-connect", row_id), "»".into(), accent).on_click(cx.listener(
                move |this, _ev: &ClickEvent, window, cx| {
                    this.connect_host(host.clone(), Some(key.clone()), window, cx);
                },
            ))
        };
        let rename = {
            let alias = host.alias.clone();
            let origin = origin.clone();
            action(("ssh-tree-rename", row_id), "✎".into(), muted).on_click(cx.listener(
                move |this, _ev: &ClickEvent, window, cx| {
                    this.start_rename(alias.clone(), origin.clone(), window, cx);
                },
            ))
        };
        let folder_btn = {
            let alias = host.alias.clone();
            let origin = origin.clone();
            let current = host.folder.clone();
            action(("ssh-tree-folder", row_id), "folder".into(), muted).on_click(cx.listener(
                move |this, _ev: &ClickEvent, window, cx| {
                    this.start_folder_edit(
                        alias.clone(),
                        origin.clone(),
                        current.clone(),
                        window,
                        cx,
                    );
                },
            ))
        };
        let delete = {
            let secret_ref = host.secret_ref.clone();
            let key = key.clone();
            let (label, color): (SharedString, u32) = if armed {
                ("✕?".into(), danger)
            } else {
                ("✕".into(), muted)
            };
            action(("ssh-tree-delete", row_id), label, color).on_click(cx.listener(
                move |this, _ev: &ClickEvent, _window, cx| {
                    if delete_click_executes(this.armed_delete.as_ref(), &key) {
                        this.delete_row(&key.0, &key.1, secret_ref.as_deref(), cx);
                    } else {
                        this.armed_delete = Some(key.clone());
                        cx.notify();
                    }
                },
            ))
        };

        let label_click = {
            let alias_for_rename = host.alias.clone();
            let origin_for_rename = origin.clone();
            let host_for_connect = host.clone();
            let key_for_connect = key.clone();
            cx.listener(move |this, ev: &ClickEvent, window, cx| {
                if ev.click_count() >= 2 {
                    this.start_rename(
                        alias_for_rename.clone(),
                        origin_for_rename.clone(),
                        window,
                        cx,
                    );
                } else {
                    this.connect_host(
                        host_for_connect.clone(),
                        Some(key_for_connect.clone()),
                        window,
                        cx,
                    );
                }
            })
        };

        div()
            .id(("ssh-tree-host", row_id))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .rounded_md()
            .hover(|s| s.bg(rgb(selection)))
            // Records this row as the tree's right-click target — read by the tree's
            // single `context_menu` (see `HomeTabState::right_click_target`'s doc
            // comment for why every row can't just have its own `.context_menu`).
            .on_mouse_down(MouseButton::Right, {
                let host = host.clone();
                let origin = origin.clone();
                cx.listener(move |this, _ev: &MouseDownEvent, _window, cx| {
                    this.ssh_home.right_click_target = Some((host.clone(), origin.clone()));
                    cx.notify();
                })
            })
            .child(
                div()
                    .w(px(12.))
                    .text_xs()
                    .text_color(rgb(if is_live { success } else { muted }))
                    .child(if is_live { "●" } else { "○" }),
            )
            .child(
                div()
                    .id(("ssh-tree-host-label", row_id))
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.))
                    .cursor_pointer()
                    .on_click(label_click)
                    .child(div().text_xs().text_color(rgb(fg)).truncate().child(alias))
                    .child(
                        div()
                            .font_family(MONO)
                            .text_color(rgb(muted))
                            .truncate()
                            .child(addr)
                            .text_size(px(10.)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(connect)
                    .child(rename)
                    .child(folder_btn)
                    .child(delete),
            )
            .into_any_element()
    }

    fn inline_edit_row(
        &self,
        edit: &InlineEdit,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let accent = theme::active(cx).accent;
        let (input, flag): (Entity<TextInput>, &'static str) = match edit {
            InlineEdit::Rename { input, .. } => (input.clone(), "renaming · Enter/Esc"),
            InlineEdit::Folder { input, .. } => (input.clone(), "folder · Enter/Esc"),
        };
        div()
            .key_context(INLINE_EDIT_CONTEXT)
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .on_action(cx.listener(|this, _ev: &InlineEditCommit, _window, cx| {
                this.commit_inline_edit(cx);
            }))
            .on_action(cx.listener(|this, _ev: &InlineEditCancel, _window, cx| {
                this.cancel_inline_edit(cx);
            }))
            // Same `min_w(0) + overflow_hidden` clip as the quick-connect box — a long
            // in-progress rename/folder value must not bleed into the "renaming ·
            // Enter/Esc" flag beside it.
            .child(div().flex_1().min_w(px(0.)).overflow_hidden().child(input))
            .child(div().text_xs().text_color(rgb(accent)).child(flag))
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
/// share a gpui element id — extremely unlikely for the small lists this tree holds, and
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
