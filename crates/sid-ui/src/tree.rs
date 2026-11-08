//! Trees — nested navigation with chevrons, arrow keys and lazy children.
//!
//! Three screens hand-roll the same thing: SSH home's folder carets, the Database
//! schema list (schema → table → column, with its own `ChevronDown`/`ChevronRight`
//! toggle per table) and the Database connection folders. Each spells its own expansion
//! set, its own indent, its own caret, and none of them can be driven from the keyboard.
//! [`TreeState`] is that, once:
//!
//! - **Expand / collapse** by clicking the chevron, double-clicking a branch, or
//!   Right / Left / Space / Enter.
//! - **Keyboard navigation** — Up / Down move the selection through the *visible* rows,
//!   Right steps into a branch (expanding it first), Left steps out to the parent
//!   (collapsing first), Home / End jump to the ends. The arithmetic is the pure
//!   [`navigate`], unit-tested without a window.
//! - **Selection** — one selected row, drawn the way a selected [`crate::Row`] is: the
//!   `selection` fill and a 3px accent marker on the leading edge.
//! - **Lazy children** — a node built with [`TreeNode::lazy`] has children that do not
//!   exist yet. Expanding it marks it loading, shows a "Loading…" row under it, and
//!   emits [`TreeEvent::LoadChildren`]; the owner fetches and calls
//!   [`TreeState::set_children`] (or [`TreeState::set_failed`], which shows the error in
//!   place and retries on the next expand).
//!
//! Rows are 32px ([`Density::Dense`]), rendered through `gpui`'s `uniform_list`, so a
//! schema with thousands of columns only lays out what is on screen.
//!
//! A tree is an `Entity` its owner holds: `cx.new(|cx| TreeState::new("db-schema", cx))`,
//! `cx.subscribe(&tree, ..)` for its [`TreeEvent`]s, and `.child(tree.clone())` to render
//! it. For a right-click menu, wrap it — `div().child(tree.clone()).context_menu(..)` —
//! and read [`TreeState::selected`] in the builder: a right-click selects the row under
//! the pointer first.

use std::collections::HashSet;

use gpui::{
    ClickEvent, Context, EventEmitter, FocusHandle, InteractiveElement as _, IntoElement,
    KeyDownEvent, MouseButton, ParentElement as _, Render, ScrollStrategy, SharedString,
    StatefulInteractiveElement as _, Styled as _, UniformListScrollHandle, Window, div,
    prelude::FluentBuilder as _, px, rgb, uniform_list,
};

use crate::bridge::hover_of;
use crate::icon::Icon;
use crate::metrics::{Density, Radius, Rounded as _};
use crate::scale::scaled;
use crate::styled::{StyledExt as _, h_flex};
use crate::theme::{self, Theme};
use crate::typography::Typography as _;

/// The indent per depth level, in design pixels.
pub const INDENT_PX: f32 = 16.;

/// A node's children.
#[derive(Clone, Debug, PartialEq)]
pub enum TreeChildren {
    /// Not expandable.
    Leaf,
    /// Present.
    Loaded(Vec<TreeNode>),
    /// Exist, but not fetched yet — expanding emits [`TreeEvent::LoadChildren`].
    Unloaded,
    /// Being fetched.
    Loading,
    /// The fetch failed with this message; expanding again retries.
    Failed(SharedString),
}

/// One node of a tree.
#[derive(Clone, Debug, PartialEq)]
pub struct TreeNode {
    /// A stable key, unique across the whole tree — selection and expansion are keyed
    /// by it, so it must survive a refresh (`"public.users.email"`, not an index).
    pub key: SharedString,
    /// What the row says.
    pub label: SharedString,
    /// A leading glyph.
    pub icon: Option<Icon>,
    /// Quiet text at the row's right edge — a column type, a count.
    pub meta: Option<SharedString>,
    /// The children.
    pub children: TreeChildren,
}

impl TreeNode {
    /// A node with no children.
    pub fn leaf(key: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            icon: None,
            meta: None,
            children: TreeChildren::Leaf,
        }
    }

    /// A node whose children are already known.
    pub fn branch(
        key: impl Into<SharedString>,
        label: impl Into<SharedString>,
        children: Vec<TreeNode>,
    ) -> Self {
        Self {
            children: TreeChildren::Loaded(children),
            ..Self::leaf(key, label)
        }
    }

    /// A node whose children are fetched the first time it is expanded.
    pub fn lazy(key: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            children: TreeChildren::Unloaded,
            ..Self::leaf(key, label)
        }
    }

    /// Set the leading glyph.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Set the right-edge meta text.
    pub fn meta(mut self, meta: impl Into<SharedString>) -> Self {
        self.meta = Some(meta.into());
        self
    }

    /// Whether this node can be expanded at all.
    pub fn is_expandable(&self) -> bool {
        !matches!(self.children, TreeChildren::Leaf)
    }
}

/// What a visible row shows.
#[derive(Clone, Debug, PartialEq)]
pub enum RowKind {
    /// A node.
    Node {
        /// The label.
        label: SharedString,
        /// The glyph.
        icon: Option<Icon>,
        /// The right-edge text.
        meta: Option<SharedString>,
        /// Whether it has (or may have) children.
        expandable: bool,
        /// Whether it is currently open.
        expanded: bool,
    },
    /// A status line under an expanded lazy node: loading, or the failure message.
    Status {
        /// The words.
        text: SharedString,
        /// Whether it is an error.
        failed: bool,
    },
}

/// One row of the flattened, currently visible tree.
#[derive(Clone, Debug, PartialEq)]
pub struct VisibleRow {
    /// The node's key (a status row borrows its parent's key with a `::status` suffix).
    pub key: SharedString,
    /// The parent's key, `None` at the root.
    pub parent: Option<SharedString>,
    /// Nesting depth, 0 at the root.
    pub depth: usize,
    /// What it shows.
    pub kind: RowKind,
}

impl VisibleRow {
    /// Whether keyboard and pointer selection may land on this row.
    pub fn is_selectable(&self) -> bool {
        matches!(self.kind, RowKind::Node { .. })
    }

    fn is_expandable(&self) -> bool {
        matches!(
            self.kind,
            RowKind::Node {
                expandable: true,
                ..
            }
        )
    }

    fn is_expanded(&self) -> bool {
        matches!(self.kind, RowKind::Node { expanded: true, .. })
    }
}

/// Flatten the visible part of `roots` given the `expanded` set — the pure model the
/// renderer and the keyboard both read.
pub fn flatten(roots: &[TreeNode], expanded: &HashSet<SharedString>) -> Vec<VisibleRow> {
    fn walk(
        nodes: &[TreeNode],
        parent: Option<&SharedString>,
        depth: usize,
        expanded: &HashSet<SharedString>,
        out: &mut Vec<VisibleRow>,
    ) {
        for node in nodes {
            let open = node.is_expandable() && expanded.contains(&node.key);
            out.push(VisibleRow {
                key: node.key.clone(),
                parent: parent.cloned(),
                depth,
                kind: RowKind::Node {
                    label: node.label.clone(),
                    icon: node.icon,
                    meta: node.meta.clone(),
                    expandable: node.is_expandable(),
                    expanded: open,
                },
            });
            if !open {
                continue;
            }
            let status = |text: SharedString, failed: bool| VisibleRow {
                key: SharedString::from(format!("{}::status", node.key)),
                parent: Some(node.key.clone()),
                depth: depth + 1,
                kind: RowKind::Status { text, failed },
            };
            match &node.children {
                TreeChildren::Loaded(children) => {
                    walk(children, Some(&node.key), depth + 1, expanded, out)
                }
                TreeChildren::Loading | TreeChildren::Unloaded => {
                    out.push(status("Loading…".into(), false))
                }
                TreeChildren::Failed(message) => out.push(status(
                    SharedString::from(format!("Couldn't load: {message}")),
                    true,
                )),
                TreeChildren::Leaf => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(roots, None, 0, expanded, &mut out);
    out
}

/// A key the tree responds to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeKey {
    /// Previous visible row.
    Up,
    /// Next visible row.
    Down,
    /// Collapse, or step out to the parent.
    Left,
    /// Expand, or step into the first child.
    Right,
    /// First row.
    Home,
    /// Last row.
    End,
    /// Enter: open a leaf, toggle a branch.
    Activate,
    /// Space: toggle a branch.
    Toggle,
}

impl TreeKey {
    /// Map a `gpui` key name to a tree key. Modified keystrokes are not the tree's.
    pub fn from_keystroke(key: &str, modified: bool) -> Option<Self> {
        if modified {
            return None;
        }
        Some(match key {
            "up" => TreeKey::Up,
            "down" => TreeKey::Down,
            "left" => TreeKey::Left,
            "right" => TreeKey::Right,
            "home" => TreeKey::Home,
            "end" => TreeKey::End,
            "enter" => TreeKey::Activate,
            "space" => TreeKey::Toggle,
            _ => return None,
        })
    }
}

/// What a key press should do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeMove {
    /// Move the selection here.
    Select(SharedString),
    /// Open this branch.
    Expand(SharedString),
    /// Close this branch.
    Collapse(SharedString),
    /// The leaf was activated.
    Activate(SharedString),
    /// Nothing to do.
    Stay,
}

/// The keyboard arithmetic, pure over the visible rows.
pub fn navigate(rows: &[VisibleRow], selected: Option<&SharedString>, key: TreeKey) -> TreeMove {
    let selectable: Vec<usize> = (0..rows.len())
        .filter(|&i| rows[i].is_selectable())
        .collect();
    let Some(&first) = selectable.first() else {
        return TreeMove::Stay;
    };
    let last = *selectable.last().unwrap_or(&first);
    let current = selected.and_then(|key| rows.iter().position(|r| &r.key == key));
    let select = |ix: usize| TreeMove::Select(rows[ix].key.clone());

    let Some(cur) = current else {
        // Nothing selected yet: any movement lands on an end.
        return match key {
            TreeKey::Up | TreeKey::End => select(last),
            TreeKey::Down | TreeKey::Home | TreeKey::Right | TreeKey::Left => select(first),
            TreeKey::Activate | TreeKey::Toggle => TreeMove::Stay,
        };
    };
    let row = &rows[cur];
    match key {
        TreeKey::Down => selectable
            .iter()
            .find(|&&i| i > cur)
            .map_or(TreeMove::Stay, |&i| select(i)),
        TreeKey::Up => selectable
            .iter()
            .rev()
            .find(|&&i| i < cur)
            .map_or(TreeMove::Stay, |&i| select(i)),
        TreeKey::Home => select(first),
        TreeKey::End => select(last),
        TreeKey::Right => {
            if row.is_expandable() && !row.is_expanded() {
                TreeMove::Expand(row.key.clone())
            } else if row.is_expanded() {
                // Into the first child — if it is a status row there is nowhere to go.
                rows.get(cur + 1)
                    .filter(|next| next.parent.as_ref() == Some(&row.key) && next.is_selectable())
                    .map_or(TreeMove::Stay, |next| TreeMove::Select(next.key.clone()))
            } else {
                TreeMove::Stay
            }
        }
        TreeKey::Left => {
            if row.is_expanded() {
                TreeMove::Collapse(row.key.clone())
            } else {
                row.parent.clone().map_or(TreeMove::Stay, TreeMove::Select)
            }
        }
        TreeKey::Activate => {
            if row.is_expandable() {
                toggle(row)
            } else {
                TreeMove::Activate(row.key.clone())
            }
        }
        TreeKey::Toggle => {
            if row.is_expandable() {
                toggle(row)
            } else {
                TreeMove::Stay
            }
        }
    }
}

fn toggle(row: &VisibleRow) -> TreeMove {
    if row.is_expanded() {
        TreeMove::Collapse(row.key.clone())
    } else {
        TreeMove::Expand(row.key.clone())
    }
}

/// What a tree tells its owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeEvent {
    /// The selection moved to this key.
    Selected(SharedString),
    /// A leaf was opened (Enter, or a double-click).
    Activated(SharedString),
    /// A branch opened.
    Expanded(SharedString),
    /// A branch closed.
    Collapsed(SharedString),
    /// A lazy branch opened and needs its children: fetch them and call
    /// [`TreeState::set_children`] / [`TreeState::set_failed`].
    LoadChildren(SharedString),
}

fn find_mut<'a>(nodes: &'a mut [TreeNode], key: &str) -> Option<&'a mut TreeNode> {
    for node in nodes {
        if node.key.as_ref() == key {
            return Some(node);
        }
        if let TreeChildren::Loaded(children) = &mut node.children
            && let Some(found) = find_mut(children, key)
        {
            return Some(found);
        }
    }
    None
}

/// A tree's state. See the module docs.
pub struct TreeState {
    id: SharedString,
    roots: Vec<TreeNode>,
    expanded: HashSet<SharedString>,
    selected: Option<SharedString>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    /// The rows as last rendered — what the list's item builder and the keyboard read.
    visible: Vec<VisibleRow>,
    /// When set, Enter / double-click on a *branch* also emits
    /// [`TreeEvent::Activated`] (it still toggles). For trees whose branches are
    /// themselves openable things — a database table that also owns columns.
    activate_branches: bool,
}

/// Whether an activation gesture on a row should additionally emit `Activated`.
/// Leaves always do (via [`navigate`] / the click handler); branches only opt in.
pub fn branch_activates(activate_branches: bool, expandable: bool) -> bool {
    activate_branches && expandable
}

impl EventEmitter<TreeEvent> for TreeState {}

impl TreeState {
    /// An empty tree. `id` must be unique within the window.
    pub fn new(id: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            id: id.into(),
            roots: Vec::new(),
            expanded: HashSet::new(),
            selected: None,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            visible: Vec::new(),
            activate_branches: false,
        }
    }

    /// Opt in to [`TreeEvent::Activated`] on branches too (see [`branch_activates`]).
    pub fn set_activate_branches(&mut self, on: bool) {
        self.activate_branches = on;
    }

    /// Replace the whole tree. Expansion and selection are kept by key, so a refresh
    /// that returns the same keys leaves the user where they were.
    pub fn set_roots(&mut self, roots: Vec<TreeNode>, cx: &mut Context<Self>) {
        self.roots = roots;
        self.refresh_visible();
        cx.notify();
    }

    /// The roots.
    pub fn roots(&self) -> &[TreeNode] {
        &self.roots
    }

    /// Fill in a lazy node's children.
    pub fn set_children(&mut self, key: &str, children: Vec<TreeNode>, cx: &mut Context<Self>) {
        if let Some(node) = find_mut(&mut self.roots, key) {
            node.children = TreeChildren::Loaded(children);
            self.refresh_visible();
            cx.notify();
        }
    }

    /// Record that a lazy node's fetch failed. The message shows under it; expanding it
    /// again retries.
    pub fn set_failed(
        &mut self,
        key: &str,
        message: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        if let Some(node) = find_mut(&mut self.roots, key) {
            node.children = TreeChildren::Failed(message.into());
            self.refresh_visible();
            cx.notify();
        }
    }

    /// The selected key.
    pub fn selected(&self) -> Option<&SharedString> {
        self.selected.as_ref()
    }

    /// Whether `key` is expanded.
    pub fn is_expanded(&self, key: &str) -> bool {
        self.expanded.iter().any(|k| k.as_ref() == key)
    }

    /// The handle to focus for keyboard navigation.
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// Select `key`, scroll it into view, and emit [`TreeEvent::Selected`].
    pub fn select(&mut self, key: SharedString, cx: &mut Context<Self>) {
        if self.selected.as_ref() == Some(&key) {
            return;
        }
        if let Some(ix) = self.visible.iter().position(|r| r.key == key) {
            self.scroll.scroll_to_item(ix, ScrollStrategy::Top);
        }
        self.selected = Some(key.clone());
        cx.emit(TreeEvent::Selected(key));
        cx.notify();
    }

    /// Open `key`. A lazy (or previously failed) node turns to loading and emits
    /// [`TreeEvent::LoadChildren`].
    pub fn expand(&mut self, key: SharedString, cx: &mut Context<Self>) {
        let mut load = false;
        if let Some(node) = find_mut(&mut self.roots, &key) {
            if !node.is_expandable() {
                return;
            }
            if matches!(
                node.children,
                TreeChildren::Unloaded | TreeChildren::Failed(_)
            ) {
                node.children = TreeChildren::Loading;
                load = true;
            }
        }
        self.expanded.insert(key.clone());
        self.refresh_visible();
        cx.emit(TreeEvent::Expanded(key.clone()));
        if load {
            cx.emit(TreeEvent::LoadChildren(key));
        }
        cx.notify();
    }

    /// Close `key`. If the selection was inside it, the selection moves up to `key`.
    pub fn collapse(&mut self, key: SharedString, cx: &mut Context<Self>) {
        if !self.expanded.remove(&key) {
            return;
        }
        let was_inside = self
            .selected
            .as_ref()
            .is_some_and(|sel| self.is_descendant(sel, &key));
        self.refresh_visible();
        cx.emit(TreeEvent::Collapsed(key.clone()));
        if was_inside {
            self.select(key, cx);
        }
        cx.notify();
    }

    /// Open a closed branch or close an open one.
    pub fn toggle(&mut self, key: SharedString, cx: &mut Context<Self>) {
        if self.expanded.contains(&key) {
            self.collapse(key, cx);
        } else {
            self.expand(key, cx);
        }
    }

    /// Carry out a [`TreeMove`].
    pub fn apply(&mut self, mv: TreeMove, cx: &mut Context<Self>) {
        match mv {
            TreeMove::Select(key) => self.select(key, cx),
            TreeMove::Expand(key) => self.expand(key, cx),
            TreeMove::Collapse(key) => self.collapse(key, cx),
            TreeMove::Activate(key) => cx.emit(TreeEvent::Activated(key)),
            TreeMove::Stay => {}
        }
    }

    fn is_descendant(&self, key: &SharedString, ancestor: &SharedString) -> bool {
        fn contains(nodes: &[TreeNode], key: &SharedString) -> bool {
            nodes.iter().any(|n| {
                &n.key == key || matches!(&n.children, TreeChildren::Loaded(c) if contains(c, key))
            })
        }
        let mut found = false;
        let mut stack: Vec<&TreeNode> = self.roots.iter().collect();
        while let Some(node) = stack.pop() {
            if &node.key == ancestor {
                found = matches!(&node.children, TreeChildren::Loaded(c) if contains(c, key));
                break;
            }
            if let TreeChildren::Loaded(c) = &node.children {
                stack.extend(c.iter());
            }
        }
        found
    }

    fn refresh_visible(&mut self) {
        self.visible = flatten(&self.roots, &self.expanded);
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) =
            TreeKey::from_keystroke(&ev.keystroke.key, ev.keystroke.modifiers.modified())
        else {
            return;
        };
        cx.stop_propagation();
        let mv = navigate(&self.visible, self.selected.as_ref(), key);
        let branch = match (&key, &self.selected) {
            (TreeKey::Activate, Some(sel)) => self
                .visible
                .iter()
                .find(|r| &r.key == sel)
                .filter(|r| branch_activates(self.activate_branches, r.is_expandable()))
                .map(|r| r.key.clone()),
            _ => None,
        };
        self.apply(mv, cx);
        if let Some(k) = branch {
            cx.emit(TreeEvent::Activated(k));
        }
    }

    fn render_row(&self, ix: usize, theme: &Theme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let row = &self.visible[ix];
        let selected = self.selected.as_ref() == Some(&row.key);
        let indent = 8. + INDENT_PX * row.depth as f32;
        let key = row.key.clone();
        let base = h_flex()
            .id((self.id.clone(), ix))
            .relative()
            .w_full()
            .h(Density::Dense.row_len())
            .pl(scaled(indent))
            .pr_2()
            .gap_1p5()
            .corners(Radius::Md)
            .text_body(theme);

        match &row.kind {
            RowKind::Status { text, failed } => base
                .child(div().w(scaled(16.)).flex_none())
                .child(
                    div()
                        .min_w_0()
                        .clamp_one_line()
                        .text_meta(theme)
                        .when_failed(*failed, theme)
                        .child(text.clone()),
                )
                .into_any_element(),
            RowKind::Node {
                label,
                icon,
                meta,
                expandable,
                expanded,
            } => {
                let hover = hover_of(
                    theme,
                    if selected {
                        theme.selection
                    } else {
                        theme.surface
                    },
                );
                let chevron_key = key.clone();
                let click_key = key.clone();
                let right_key = key.clone();
                let expandable = *expandable;
                let chevron = div()
                    .id((SharedString::from(format!("{}-chevron", self.id)), ix))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(scaled(16.))
                    .when(expandable, |this| {
                        let glyph = if *expanded {
                            Icon::ChevronDown
                        } else {
                            Icon::ChevronRight
                        };
                        this.child(glyph.small().text_color(rgb(theme.muted)))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                window.focus(&this.focus, cx);
                                this.toggle(chevron_key.clone(), cx);
                            }))
                    });
                base.cursor_pointer()
                    .when(selected, |this| this.bg(rgb(theme.selection)))
                    .hover(move |s| s.bg(rgb(hover)))
                    .when(selected, |this| {
                        this.child(
                            div()
                                .absolute()
                                .left(px(2.))
                                .top(px(7.))
                                .bottom(px(7.))
                                .w(px(3.))
                                .rounded_full()
                                .bg(rgb(theme.accent)),
                        )
                    })
                    .child(chevron)
                    .when_some(*icon, |this, icon| {
                        this.child(
                            div()
                                .flex_none()
                                .child(icon.small().text_color(rgb(theme.muted))),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .clamp_one_line()
                            .when(selected, |this| this.text_color(rgb(theme.fg_strong)))
                            .child(label.clone()),
                    )
                    .when_some(meta.clone(), |this, meta| {
                        this.child(div().flex_none().text_mono_meta(theme).child(meta))
                    })
                    .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                        window.focus(&this.focus, cx);
                        this.select(click_key.clone(), cx);
                        if ev.click_count() >= 2 {
                            if expandable {
                                this.toggle(click_key.clone(), cx);
                                if branch_activates(this.activate_branches, true) {
                                    cx.emit(TreeEvent::Activated(click_key.clone()));
                                }
                            } else {
                                cx.emit(TreeEvent::Activated(click_key.clone()));
                            }
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _, window, cx| {
                            window.focus(&this.focus, cx);
                            this.select(right_key.clone(), cx);
                        }),
                    )
                    .into_any_element()
            }
        }
    }
}

/// A small private helper so a failed status line reads in the danger ink.
trait WhenFailed: gpui::Styled + Sized {
    fn when_failed(self, failed: bool, theme: &Theme) -> Self {
        if failed {
            self.text_color(rgb(theme.danger))
        } else {
            self
        }
    }
}

impl<T: gpui::Styled + Sized> WhenFailed for T {}

impl Render for TreeState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::active(cx).clone();
        self.refresh_visible();
        let count = self.visible.len();
        div()
            .id(self.id.clone())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .size_full()
            .min_h_0()
            .child(
                uniform_list(
                    SharedString::from(format!("{}-rows", self.id)),
                    count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                        let theme = theme.clone();
                        range
                            .filter(|&ix| ix < this.visible.len())
                            .map(|ix| this.render_row(ix, &theme, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.scroll)
                .size_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<TreeNode> {
        vec![
            TreeNode::branch(
                "public",
                "public",
                vec![
                    TreeNode::lazy("public.users", "users").meta("table"),
                    TreeNode::branch(
                        "public.orders",
                        "orders",
                        vec![
                            TreeNode::leaf("public.orders.id", "id").meta("int8"),
                            TreeNode::leaf("public.orders.total", "total").meta("numeric"),
                        ],
                    ),
                ],
            ),
            TreeNode::leaf("readme", "readme"),
        ]
    }

    fn keys(rows: &[VisibleRow]) -> Vec<&str> {
        rows.iter().map(|r| r.key.as_ref()).collect()
    }

    fn open(keys: &[&str]) -> HashSet<SharedString> {
        keys.iter()
            .map(|k| SharedString::from(k.to_string()))
            .collect()
    }

    #[test]
    fn a_collapsed_tree_shows_only_its_roots() {
        let rows = flatten(&sample(), &HashSet::new());
        assert_eq!(keys(&rows), vec!["public", "readme"]);
        assert!(rows.iter().all(|r| r.depth == 0));
    }

    #[test]
    fn expanding_reveals_children_in_order_and_one_level_deeper() {
        let rows = flatten(&sample(), &open(&["public", "public.orders"]));
        assert_eq!(
            keys(&rows),
            vec![
                "public",
                "public.users",
                "public.orders",
                "public.orders.id",
                "public.orders.total",
                "readme"
            ]
        );
        assert_eq!(rows[3].depth, 2);
        assert_eq!(rows[3].parent.as_deref(), Some("public.orders"));
    }

    #[test]
    fn an_expanded_lazy_node_shows_a_loading_row_and_a_failed_one_its_error() {
        let rows = flatten(&sample(), &open(&["public", "public.users"]));
        let status = &rows[2];
        assert!(!status.is_selectable());
        assert!(matches!(
            &status.kind,
            RowKind::Status { failed: false, .. }
        ));

        let mut failed = sample();
        if let TreeChildren::Loaded(c) = &mut failed[0].children {
            c[0].children = TreeChildren::Failed("permission denied".into());
        }
        let rows = flatten(&failed, &open(&["public", "public.users"]));
        match &rows[2].kind {
            RowKind::Status { text, failed } => {
                assert!(*failed);
                assert!(text.contains("permission denied"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn up_and_down_walk_the_visible_rows_and_skip_status_lines() {
        let rows = flatten(&sample(), &open(&["public", "public.users"]));
        // public, users, (loading), orders, readme
        let users = SharedString::from("public.users");
        assert_eq!(
            navigate(&rows, Some(&users), TreeKey::Down),
            TreeMove::Select("public.orders".into()),
            "the loading line is not a stop"
        );
        let orders = SharedString::from("public.orders");
        assert_eq!(
            navigate(&rows, Some(&orders), TreeKey::Up),
            TreeMove::Select(users.clone())
        );
        let readme = SharedString::from("readme");
        assert_eq!(
            navigate(&rows, Some(&readme), TreeKey::Down),
            TreeMove::Stay,
            "no wrap"
        );
        assert_eq!(
            navigate(&rows, None, TreeKey::Down),
            TreeMove::Select("public".into())
        );
        assert_eq!(
            navigate(&rows, None, TreeKey::Up),
            TreeMove::Select(readme.clone())
        );
        assert_eq!(
            navigate(&rows, Some(&users), TreeKey::Home),
            TreeMove::Select("public".into())
        );
        assert_eq!(
            navigate(&rows, Some(&users), TreeKey::End),
            TreeMove::Select(readme)
        );
    }

    #[test]
    fn right_expands_then_steps_in_and_left_collapses_then_steps_out() {
        let public = SharedString::from("public");
        let closed = flatten(&sample(), &HashSet::new());
        assert_eq!(
            navigate(&closed, Some(&public), TreeKey::Right),
            TreeMove::Expand(public.clone())
        );

        let opened = flatten(&sample(), &open(&["public"]));
        assert_eq!(
            navigate(&opened, Some(&public), TreeKey::Right),
            TreeMove::Select("public.users".into())
        );
        assert_eq!(
            navigate(&opened, Some(&public), TreeKey::Left),
            TreeMove::Collapse(public.clone())
        );
        let orders = SharedString::from("public.orders");
        assert_eq!(
            navigate(&opened, Some(&orders), TreeKey::Left),
            TreeMove::Select(public)
        );
        // A leaf has nowhere to go right.
        let readme = SharedString::from("readme");
        assert_eq!(
            navigate(&opened, Some(&readme), TreeKey::Right),
            TreeMove::Stay
        );
    }

    #[test]
    fn branches_activate_only_when_opted_in() {
        assert!(branch_activates(true, true));
        assert!(!branch_activates(false, true));
        assert!(!branch_activates(true, false));
    }

    #[test]
    fn enter_opens_a_leaf_and_toggles_a_branch() {
        let rows = flatten(&sample(), &open(&["public"]));
        let readme = SharedString::from("readme");
        assert_eq!(
            navigate(&rows, Some(&readme), TreeKey::Activate),
            TreeMove::Activate(readme.clone())
        );
        let public = SharedString::from("public");
        assert_eq!(
            navigate(&rows, Some(&public), TreeKey::Activate),
            TreeMove::Collapse(public.clone())
        );
        assert_eq!(
            navigate(&rows, Some(&public), TreeKey::Toggle),
            TreeMove::Collapse(public)
        );
        assert_eq!(
            navigate(&rows, Some(&readme), TreeKey::Toggle),
            TreeMove::Stay
        );
    }

    #[test]
    fn an_empty_tree_goes_nowhere() {
        assert_eq!(navigate(&[], None, TreeKey::Down), TreeMove::Stay);
    }

    #[test]
    fn only_unmodified_navigation_keys_belong_to_the_tree() {
        assert_eq!(TreeKey::from_keystroke("down", false), Some(TreeKey::Down));
        assert_eq!(
            TreeKey::from_keystroke("enter", false),
            Some(TreeKey::Activate)
        );
        assert_eq!(
            TreeKey::from_keystroke("down", true),
            None,
            "ctrl-down is the app's"
        );
        assert_eq!(TreeKey::from_keystroke("a", false), None);
    }

    #[test]
    fn find_mut_reaches_nested_nodes() {
        let mut roots = sample();
        let node = find_mut(&mut roots, "public.orders.total").expect("nested");
        assert_eq!(node.label.as_ref(), "total");
        assert!(find_mut(&mut roots, "nope").is_none());
    }
}
