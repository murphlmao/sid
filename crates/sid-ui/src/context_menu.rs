//! Context menus — a sid-styled builder over `gpui-component`'s `PopupMenu`.
//!
//! Every right-click menu in the app was a hand-chained run of `PopupMenuItem::new(..)
//! .icon(Icon::X.el()).on_click(..)`, which left three decisions to each call site: the
//! casing of the labels (`"connect"` beside `"Promote to global"` in the same SSH menu),
//! where a destructive item goes, and whether it looks destructive at all (`delete`
//! rendered exactly like `rename`). [`ContextMenu`] makes those the component's:
//!
//! - **Labels are sentence-cased** ([`crate::copy::sentence_case`]).
//! - **A danger item is drawn in the `danger` ink** — icon and label — and
//!   [`ContextMenu::danger_item`] puts a separator before the first one, so destructive
//!   actions sit at the bottom, fenced off, the way every platform menu does it.
//! - **Icons come from [`Icon`]**, never a raw library icon.
//!
//! It is still the library's menu underneath — positioning, keyboard navigation, the
//! dismiss-on-click-outside, and (since v2 turns the bridge's `shadow` flag on) the
//! floating shadow all come from `PopupMenu`. This type only builds the items.
//!
//! ```ignore
//! .context_menu(move |menu, _window, cx| {
//!     ContextMenu::new()
//!         .item(MenuItem::new("connect").icon(Icon::Terminal).on_select(connect))
//!         .item(MenuItem::new("edit…").icon(Icon::Settings).on_select(edit))
//!         .danger_item(MenuItem::new("delete").icon(Icon::Trash).on_select(delete))
//!         .apply(menu, cx)
//! })
//! ```

use std::rc::Rc;

use gpui::{App, ParentElement as _, SharedString, Styled as _, Window, div, rgb};
use gpui_component::menu::{PopupMenu, PopupMenuItem};

use crate::copy::sentence_case;
use crate::icon::Icon;
use crate::styled::h_flex;
use crate::theme;
use crate::typography::Typography as _;

/// What a menu item does when chosen.
type SelectHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// One actionable line in a [`ContextMenu`].
pub struct MenuItem {
    label: SharedString,
    icon: Option<Icon>,
    danger: bool,
    disabled: bool,
    checked: bool,
    on_select: Option<SelectHandler>,
}

impl MenuItem {
    /// An item labelled `label` (sentence-cased when shown).
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            danger: false,
            disabled: false,
            checked: false,
            on_select: None,
        }
    }

    /// A leading glyph.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Draw this item in the `danger` ink. Prefer [`ContextMenu::danger_item`], which
    /// also fences it off below a separator.
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    /// Grey the item out and ignore it.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Show a check beside the item (a toggled view option).
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    /// What choosing the item does.
    pub fn on_select(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }

    /// The label as it will be shown.
    pub fn display_label(&self) -> String {
        sentence_case(&self.label)
    }

    /// Whether this item is drawn as destructive.
    pub fn is_danger(&self) -> bool {
        self.danger
    }

    fn into_popup_item(self, danger_ink: u32) -> PopupMenuItem {
        let label = self.display_label();
        let item = if self.danger {
            let icon = self.icon;
            let label = SharedString::from(label);
            let mut item = PopupMenuItem::element(move |_window, cx| {
                let t = theme::active(cx).clone();
                h_flex().child(
                    div()
                        .text_body(&t)
                        .text_color(rgb(t.danger))
                        .child(label.clone()),
                )
            });
            if let Some(icon) = icon {
                // Coloured on the icon element itself, so the library's icon column
                // (which keeps the danger item aligned with its neighbours) cannot
                // repaint it in the menu's neutral ink.
                item = item.icon(icon.el().text_color(rgb(danger_ink)));
            }
            item
        } else {
            let mut item = PopupMenuItem::new(label);
            if let Some(icon) = self.icon {
                item = item.icon(icon.el());
            }
            item
        };
        let item = item.disabled(self.disabled).checked(self.checked);
        match self.on_select {
            Some(handler) => item.on_click(move |_, window, cx| handler(window, cx)),
            None => item,
        }
    }
}

/// One line of a menu.
enum Entry {
    Item(MenuItem),
    Separator,
    Heading(SharedString),
}

/// A right-click menu, built declaratively. See the module docs.
#[derive(Default)]
pub struct ContextMenu {
    entries: Vec<Entry>,
}

impl ContextMenu {
    /// An empty menu.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append an item.
    pub fn item(mut self, item: MenuItem) -> Self {
        self.entries.push(Entry::Item(item));
        self
    }

    /// Append a destructive item, fenced below a separator if it is the first one.
    pub fn danger_item(mut self, item: MenuItem) -> Self {
        let has_danger = self
            .entries
            .iter()
            .any(|e| matches!(e, Entry::Item(i) if i.danger));
        let ends_in_separator = matches!(self.entries.last(), Some(Entry::Separator) | None);
        if !has_danger && !ends_in_separator {
            self.entries.push(Entry::Separator);
        }
        self.entries.push(Entry::Item(item.danger()));
        self
    }

    /// Append a separator. Consecutive and leading separators are dropped at build time.
    pub fn separator(mut self) -> Self {
        self.entries.push(Entry::Separator);
        self
    }

    /// Append a small non-interactive heading (a group label inside the menu).
    pub fn heading(mut self, label: impl Into<SharedString>) -> Self {
        self.entries.push(Entry::Heading(label.into()));
        self
    }

    /// How many entries will be built, after separator tidying.
    pub fn len(&self) -> usize {
        keep_mask(&self.entries).iter().filter(|k| **k).count()
    }

    /// Whether the menu has nothing in it.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The tidied entry kinds, for tests.
    #[cfg(test)]
    fn tidy_kinds(&self) -> Vec<&'static str> {
        self.entries
            .iter()
            .zip(keep_mask(&self.entries))
            .filter(|(_, keep)| *keep)
            .map(|(e, _)| match e {
                Entry::Item(_) => "item",
                Entry::Separator => "sep",
                Entry::Heading(_) => "heading",
            })
            .collect()
    }

    /// Build the items into the library's `menu`, in the active palette.
    pub fn apply(self, menu: PopupMenu, cx: &App) -> PopupMenu {
        let danger_ink = theme::active(cx).danger;
        let keep = keep_mask(&self.entries);
        self.entries
            .into_iter()
            .zip(keep)
            .filter(|(_, keep)| *keep)
            .fold(menu, |menu, (entry, _)| match entry {
                Entry::Separator => menu.separator(),
                Entry::Heading(label) => menu.item(PopupMenuItem::label(sentence_case(&label))),
                Entry::Item(item) => menu.item(item.into_popup_item(danger_ink)),
            })
    }
}

/// Which entries survive tidying: no leading, trailing or doubled separators.
fn keep_mask(entries: &[Entry]) -> Vec<bool> {
    let mut keep = vec![false; entries.len()];
    let mut last_kept_sep = true; // a leading separator counts as "after a separator"
    for (ix, e) in entries.iter().enumerate() {
        let is_sep = matches!(e, Entry::Separator);
        if is_sep && last_kept_sep {
            continue;
        }
        keep[ix] = true;
        last_kept_sep = is_sep;
    }
    // Trailing separators.
    for ix in (0..entries.len()).rev() {
        if !keep[ix] {
            continue;
        }
        if matches!(entries[ix], Entry::Separator) {
            keep[ix] = false;
        } else {
            break;
        }
    }
    keep
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_sentence_cased() {
        assert_eq!(
            MenuItem::new("assign folder…").display_label(),
            "Assign folder…"
        );
        assert_eq!(
            MenuItem::new("Promote to global").display_label(),
            "Promote to global"
        );
    }

    #[test]
    fn the_first_danger_item_is_fenced_off_below_a_separator() {
        let menu = ContextMenu::new()
            .item(MenuItem::new("connect"))
            .item(MenuItem::new("rename"))
            .danger_item(MenuItem::new("delete"))
            .danger_item(MenuItem::new("forget key"));
        assert_eq!(
            menu.tidy_kinds(),
            vec!["item", "item", "sep", "item", "item"]
        );
        assert!(matches!(&menu.entries[3], Entry::Item(i) if i.is_danger()));
    }

    #[test]
    fn a_danger_item_alone_needs_no_fence() {
        let menu = ContextMenu::new().danger_item(MenuItem::new("kill"));
        assert_eq!(menu.tidy_kinds(), vec!["item"]);
    }

    #[test]
    fn stray_separators_are_tidied_away() {
        let menu = ContextMenu::new()
            .separator()
            .item(MenuItem::new("a"))
            .separator()
            .separator()
            .item(MenuItem::new("b"))
            .separator();
        assert_eq!(menu.tidy_kinds(), vec!["item", "sep", "item"]);
        assert_eq!(menu.len(), 3);
        assert!(ContextMenu::new().separator().is_empty());
    }
}
