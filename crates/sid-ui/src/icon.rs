//! The icon registry — named, monochrome, bundled.
//!
//! `gpui-component-assets` has been wired into the app since the Database tab landed
//! (`main.rs`'s `with_assets(..)`) and shipped 86 Lucide monochrome SVGs that nothing
//! referenced. Meanwhile the UI stood in 18 distinct ad-hoc Unicode glyphs — `⟳` 19
//! times, `→` 46, `✎` 7, `»` 3 — which render in whatever the font happens to have,
//! at whatever weight it happens to be, and carry no label.
//!
//! This enum is the only vocabulary of glyphs sid has. Every entry resolves to a
//! bundled SVG, verified by a test that loads the actual asset bytes, so a typo or an
//! upstream icon removal is a build failure rather than an invisible glyph. Icons
//! inherit the ambient text colour and size, so they follow the theme for free.
//!
//! **No emoji, ever** — enforced for the whole workspace by `tests/hygiene.rs`. Lucide
//! is monochrome line art, which is the house rule already.
//!
//! # The bundle is now all of Lucide
//!
//! `gpui-component-assets` 0.5.1 shipped **86** SVGs, a Lucide subset, and seven glyphs
//! sid's screens wanted were simply not among them (`play`, `download`, `trash`,
//! `pencil`, `container`, `boxes`, `network`). They were tracked in an `UNBUNDLED`
//! ratchet, pinned by a test that would fail the day a bundle bump shipped one.
//!
//! The `gpui-kit-assets` 0.6.1 bump shipped **all** of them — the bundle is 1830 SVGs,
//! the whole Lucide set — so the ratchet fired and has been retired, and all seven have
//! been drawn for real: [`Icon::Trash`] is a bin, [`Icon::Rename`] is a pencil, and the
//! Database tab's Run/Export controls ([`Icon::Run`], [`Icon::Export`]) plus Network's
//! Docker/Kubernetes/Interfaces sub-views ([`Icon::Docker`], [`Icon::Kubernetes`],
//! [`Icon::Interfaces`]) all draw the glyph they actually wanted. That also means
//! [`Icon::name`] now resolves through `gpui_kit_assets::IconName` — the complete
//! catalog — rather than `gpui_component`'s old 86-icon compatibility subset, so a
//! future entry just needs a name from the full 1830.
//!
//! The rule this file follows: a substitution is allowed only when the stand-in reads
//! as the *same act* at 14px (`redo` for [`Icon::Refresh`] — a curved arrow is a re-run,
//! and it is still a substitution: the 0.6.1 bundle does ship a literal `refresh-cw` now,
//! but swapping a 19-call-site glyph is a design decision of its own, not made here).
//! Where nothing in the bundle does, there is no registry entry, and the call site uses
//! a word instead of a glyph.

use gpui::{App, IntoElement, RenderOnce, SharedString, Window};
use gpui_component::{Sizable as _, Size};
use gpui_kit_assets::IconName;

/// A named icon from the bundled Lucide set.
///
/// Names are sid's, not Lucide's: they say what the glyph *means* here, so a call site
/// reads as intent and a substitution (see [`Icon::Refresh`]) is invisible to it.
#[derive(IntoElement, Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum Icon {
    /// Re-run a fetch. The bundle has no `refresh-cw`; Lucide's `redo` (a curved
    /// arrow) is the closest monochrome stand-in and reads correctly at 14px.
    Refresh,
    /// Filter / find affordance — the leading glyph of a search field.
    Search,
    /// A stored dataset — the Database tab's mark.
    Database,
    /// Create.
    Add,
    /// Remove one of something (not a delete).
    Remove,
    /// Dismiss: close a modal, clear a field.
    Close,
    /// Destroy: delete a saved item.
    ///
    /// Draws Lucide's `trash` — a real bin. Until the 0.6.1 bundle this drew `circle-x`:
    /// the 86-icon `gpui-component-assets` 0.5.1 set had no `trash`/`trash-2`, and the
    /// entry that *looked* like the obvious candidate — `delete` — is Lucide's
    /// **backspace key** (a pointed-left rectangle with an X in it), which reads as
    /// "erase a character", not "destroy this". See
    /// [`tests::destroy_is_not_drawn_as_a_keyboard_key`], still pinned against that one.
    Trash,
    /// Copy to clipboard.
    Copy,
    /// Rename in place.
    ///
    /// Draws Lucide's `pencil` — a real pencil, shipped for the first time in the 0.6.1
    /// bundle. It had two prior stand-ins: `case-sensitive` (the find toolbar's "Aa"
    /// toggle, which next to a host alias read as a typography control) and then
    /// `replace` (a rounded square with an arrow curving into a second one — closer, but
    /// still "put a different one of these here" rather than "edit this").
    Rename,
    /// Confirmed / selected.
    Check,
    /// Cautionary state.
    Warning,
    /// Informational note.
    Info,
    /// Failure.
    Error,
    /// Success.
    Ok,
    /// Disclosure, collapsed.
    ChevronRight,
    /// Disclosure, expanded.
    ChevronDown,
    /// Navigate back.
    ChevronLeft,
    /// Navigate up.
    ChevronUp,
    /// A sortable-but-unsorted column, or a dropdown affordance.
    ChevronsUpDown,
    /// Move this item **up** an ordered list — promote a route, a column, a rule.
    ///
    /// A full arrow rather than a chevron on purpose: the chevrons are disclosure and
    /// sort marks, and a reorder control that borrowed one would read as "expand".
    ArrowUp,
    /// Move this item **down** an ordered list — demote.
    ArrowDown,
    /// Sorted ascending.
    SortAscending,
    /// Sorted descending.
    SortDescending,
    /// A directory, closed.
    Folder,
    /// A directory, open.
    FolderOpen,
    /// A file.
    File,
    /// A shell / terminal session.
    Terminal,
    /// A network address or a remote host.
    Globe,
    /// Settings.
    Settings,
    /// A user / account.
    User,
    /// Overflow menu (horizontal).
    Ellipsis,
    /// Overflow menu (vertical) — the row-level "more actions" affordance.
    More,
    /// Opens something outside sid.
    ExternalLink,
    /// Reveal a masked value.
    Eye,
    /// Hide a value.
    EyeOff,
    /// A hamburger / panel toggle.
    Menu,
    /// Pinned / favourite.
    Star,
    /// Un-pin / un-favourite — the paired "off" glyph, so a toggle's two states are two
    /// icons rather than one icon and a colour.
    StarOff,
    /// In-flight work.
    Spinner,
    /// Expand a pane to fill.
    Maximize,
    /// Restore a pane's size.
    Minimize,
    /// An overview screen.
    Dashboard,
    /// Theme / appearance.
    Palette,
    /// Notifications.
    Bell,
    /// A masked secret's stand-in character.
    Asterisk,
    /// A date.
    Calendar,
    /// Execute a query — the Database tab's primary action.
    Run,
    /// Save results out of sid — the Database tab's export control.
    Export,
    /// A Docker container.
    Docker,
    /// A Kubernetes cluster / pod set.
    Kubernetes,
    /// Network interfaces / adapters.
    Interfaces,
    /// A relationships/ER diagram — the Database tab's schema pop-out.
    ///
    /// Draws Lucide's `workflow` (two linked boxes) rather than `git-fork` (three
    /// linked circles) or `network` — the latter is already [`Icon::Interfaces`], and a
    /// second meaning on the same glyph is confusable at 14px next to a real one.
    /// `workflow`'s two boxes-and-a-line read as "entities, related", which is the
    /// diagram button's actual job; `git-fork` reads as version-control branching.
    Diagram,
}

impl Icon {
    /// Every registered icon — the test sweep, and a future gallery's source list.
    pub const ALL: &'static [Icon] = &[
        Icon::Refresh,
        Icon::Search,
        Icon::Database,
        Icon::Add,
        Icon::Remove,
        Icon::Close,
        Icon::Trash,
        Icon::Copy,
        Icon::Rename,
        Icon::Check,
        Icon::Warning,
        Icon::Info,
        Icon::Error,
        Icon::Ok,
        Icon::ChevronRight,
        Icon::ChevronDown,
        Icon::ChevronLeft,
        Icon::ChevronUp,
        Icon::ChevronsUpDown,
        Icon::ArrowUp,
        Icon::ArrowDown,
        Icon::SortAscending,
        Icon::SortDescending,
        Icon::Folder,
        Icon::FolderOpen,
        Icon::File,
        Icon::Terminal,
        Icon::Globe,
        Icon::Settings,
        Icon::User,
        Icon::Ellipsis,
        Icon::More,
        Icon::ExternalLink,
        Icon::Eye,
        Icon::EyeOff,
        Icon::Menu,
        Icon::Star,
        Icon::StarOff,
        Icon::Spinner,
        Icon::Maximize,
        Icon::Minimize,
        Icon::Dashboard,
        Icon::Palette,
        Icon::Bell,
        Icon::Asterisk,
        Icon::Calendar,
        Icon::Run,
        Icon::Export,
        Icon::Docker,
        Icon::Kubernetes,
        Icon::Interfaces,
        Icon::Diagram,
    ];

    /// The bundled asset this icon draws. Going through the library's own `IconName`
    /// means a rename upstream is a compile error here, not a blank square.
    fn name(self) -> IconName {
        match self {
            Icon::Refresh => IconName::Redo,
            Icon::Search => IconName::Search,
            Icon::Database => IconName::Database,
            Icon::Add => IconName::Plus,
            Icon::Remove => IconName::Minus,
            Icon::Close => IconName::Close,
            Icon::Trash => IconName::Trash,
            Icon::Copy => IconName::Copy,
            Icon::Rename => IconName::Pencil,
            Icon::Check => IconName::Check,
            Icon::Warning => IconName::TriangleAlert,
            Icon::Info => IconName::Info,
            Icon::Error => IconName::CircleX,
            Icon::Ok => IconName::CircleCheck,
            Icon::ChevronRight => IconName::ChevronRight,
            Icon::ChevronDown => IconName::ChevronDown,
            Icon::ChevronLeft => IconName::ChevronLeft,
            Icon::ChevronUp => IconName::ChevronUp,
            Icon::ChevronsUpDown => IconName::ChevronsUpDown,
            Icon::ArrowUp => IconName::ArrowUp,
            Icon::ArrowDown => IconName::ArrowDown,
            Icon::SortAscending => IconName::SortAscending,
            Icon::SortDescending => IconName::SortDescending,
            Icon::Folder => IconName::Folder,
            Icon::FolderOpen => IconName::FolderOpen,
            Icon::File => IconName::File,
            Icon::Terminal => IconName::SquareTerminal,
            Icon::Globe => IconName::Globe,
            Icon::Settings => IconName::Settings,
            Icon::User => IconName::User,
            Icon::Ellipsis => IconName::Ellipsis,
            Icon::More => IconName::EllipsisVertical,
            Icon::ExternalLink => IconName::ExternalLink,
            Icon::Eye => IconName::Eye,
            Icon::EyeOff => IconName::EyeOff,
            Icon::Menu => IconName::Menu,
            Icon::Star => IconName::Star,
            Icon::StarOff => IconName::StarOff,
            Icon::Spinner => IconName::LoaderCircle,
            Icon::Maximize => IconName::Maximize,
            Icon::Minimize => IconName::Minimize,
            Icon::Dashboard => IconName::LayoutDashboard,
            Icon::Palette => IconName::Palette,
            Icon::Bell => IconName::Bell,
            Icon::Asterisk => IconName::Asterisk,
            Icon::Calendar => IconName::Calendar,
            Icon::Run => IconName::Play,
            Icon::Export => IconName::Download,
            Icon::Docker => IconName::Container,
            Icon::Kubernetes => IconName::Boxes,
            Icon::Interfaces => IconName::Network,
            Icon::Diagram => IconName::Workflow,
        }
    }

    /// The embedded asset path, e.g. `icons/search.svg`.
    pub fn path(self) -> SharedString {
        self.name().path()
    }

    /// The icon as a styleable element — chain `.text_color(..)` / `.size_*()` on the
    /// result. Plain `Icon` also renders directly as a child, inheriting the ambient
    /// text colour and size.
    pub fn el(self) -> gpui_component::Icon {
        gpui_component::Icon::new(self.name())
    }

    /// The icon at 14px — the size that sits level with `text_xs` and `text_sm`, which
    /// is nearly every inline use: an error line, a row marker, a caption.
    ///
    /// It exists so a call site does not have to import `gpui_component::Sizable` to say
    /// "small". Tab modules are supposed to stop naming the library (CLAUDE.md rule 1),
    /// and a sizing trait is exactly the kind of import that quietly puts it back.
    pub fn small(self) -> gpui_component::Icon {
        self.el().with_size(Size::Small)
    }
}

impl RenderOnce for Icon {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        self.el()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::AssetSource as _;
    use std::collections::HashSet;

    #[test]
    fn every_named_icon_exists_in_the_bundle() {
        // `AllAssets`, not `Assets`: the latter is `gpui_kit_assets`'s 86-icon
        // compatibility subset, which is missing exactly the glyphs this registry now
        // relies on (`trash`, `pencil`, `play`, ...). `main.rs`'s `with_assets(..)` has
        // to register the same source this test loads bytes from, or a name that
        // resolves here still renders as nothing at runtime.
        let assets = gpui_kit_assets::AllAssets;
        for &icon in Icon::ALL {
            let path = icon.path();
            let bytes = assets
                .load(&path)
                .unwrap_or_else(|e| panic!("{icon:?} ({path}): {e}"))
                .unwrap_or_else(|| panic!("{icon:?}: no asset at {path}"));
            assert!(!bytes.is_empty(), "{icon:?}: empty asset at {path}");
            assert!(
                path.ends_with(".svg"),
                "{icon:?}: {path} is not an SVG — monochrome line art only"
            );
        }
    }

    #[test]
    fn all_lists_every_variant_exactly_once() {
        // `ALL` is hand-maintained and has to stay in step with the enum. This asserts
        // on the *variants*, not on their resolved paths: the path-based version was a
        // proxy that would also forbid two names deliberately sharing one asset — not
        // currently the case, but a legitimate future state (two meanings, one glyph),
        // so this test does not rule it out.
        let variants: HashSet<_> = Icon::ALL.iter().copied().collect();
        assert_eq!(
            variants.len(),
            Icon::ALL.len(),
            "duplicate entry in Icon::ALL"
        );
    }

    #[test]
    fn rename_draws_a_real_pencil() {
        // `case-sensitive` is the find toolbar's "Aa" button, and `replace` was the
        // second stand-in before the 0.6.1 bundle shipped an actual pencil. Pinned so a
        // future edit cannot drift back to either.
        assert_ne!(Icon::Rename.path(), IconName::CaseSensitive.path());
        assert_ne!(Icon::Rename.path(), IconName::Replace.path());
        assert_eq!(Icon::Rename.path(), IconName::Pencil.path());
    }

    #[test]
    fn reorder_uses_arrows_and_never_a_chevron() {
        // Promote/demote must not borrow the disclosure or sort marks: three different
        // meanings sharing one picture in a table header is how a control stops being
        // readable at 14px.
        assert_eq!(Icon::ArrowUp.path(), IconName::ArrowUp.path());
        assert_eq!(Icon::ArrowDown.path(), IconName::ArrowDown.path());
        for chevron in [
            Icon::ChevronUp,
            Icon::ChevronDown,
            Icon::ChevronsUpDown,
            Icon::SortAscending,
            Icon::SortDescending,
        ] {
            assert_ne!(Icon::ArrowUp.path(), chevron.path());
            assert_ne!(Icon::ArrowDown.path(), chevron.path());
        }
    }

    #[test]
    fn destroy_is_not_drawn_as_a_keyboard_key() {
        // Lucide's `delete` is the backspace key, not a bin — it shipped as the `Trash`
        // glyph until the System-tab migration and read as "erase a character" on every
        // destructive control in the app. Pinned so a future bundle bump cannot quietly
        // restore it.
        assert_ne!(Icon::Trash.path(), IconName::Delete.path());
        assert!(
            Icon::ALL
                .iter()
                .all(|i| i.path() != IconName::Delete.path()),
            "the backspace glyph is back in the registry"
        );
    }
}
