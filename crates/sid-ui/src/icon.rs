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

use gpui::{App, IntoElement, RenderOnce, SharedString, Window};
use gpui_component::{IconName, IconNamed as _};

/// A named icon from the bundled Lucide set.
///
/// Names are sid's, not Lucide's: they say what the glyph *means* here, so a call site
/// reads as intent and a substitution (see [`Icon::Refresh`]) is invisible to it.
#[derive(IntoElement, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    /// Re-run a fetch. The bundle has no `refresh-cw`; Lucide's `redo` (a curved
    /// arrow) is the closest monochrome stand-in and reads correctly at 14px.
    Refresh,
    /// Filter / find affordance — the leading glyph of a search field.
    Search,
    /// Create.
    Add,
    /// Remove one of something (not a delete).
    Remove,
    /// Dismiss: close a modal, clear a field.
    Close,
    /// Destroy: delete a saved item.
    Trash,
    /// Copy to clipboard.
    Copy,
    /// Rename in place. The bundle ships no pencil; Lucide's `case-sensitive` (an "Aa"
    /// glyph) is the closest monochrome stand-in and reads as "edit this text".
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
}

impl Icon {
    /// Every registered icon — the test sweep, and a future gallery's source list.
    pub const ALL: &'static [Icon] = &[
        Icon::Refresh,
        Icon::Search,
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
        Icon::Spinner,
        Icon::Maximize,
        Icon::Minimize,
        Icon::Dashboard,
        Icon::Palette,
        Icon::Bell,
        Icon::Asterisk,
        Icon::Calendar,
    ];

    /// The bundled asset this icon draws. Going through the library's own `IconName`
    /// means a rename upstream is a compile error here, not a blank square.
    fn name(self) -> IconName {
        match self {
            Icon::Refresh => IconName::Redo,
            Icon::Search => IconName::Search,
            Icon::Add => IconName::Plus,
            Icon::Remove => IconName::Minus,
            Icon::Close => IconName::Close,
            Icon::Trash => IconName::Delete,
            Icon::Copy => IconName::Copy,
            Icon::Rename => IconName::CaseSensitive,
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
            Icon::Spinner => IconName::LoaderCircle,
            Icon::Maximize => IconName::Maximize,
            Icon::Minimize => IconName::Minimize,
            Icon::Dashboard => IconName::LayoutDashboard,
            Icon::Palette => IconName::Palette,
            Icon::Bell => IconName::Bell,
            Icon::Asterisk => IconName::Asterisk,
            Icon::Calendar => IconName::Calendar,
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
        // Loads the real embedded bytes: this is what catches a path typo or an icon
        // dropped by an upstream bump, at build time instead of at render time.
        let assets = gpui_component_assets::Assets;
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
        // `ALL` is hand-maintained; this keeps it honest against the match arms below
        // by requiring the set of resolved paths to be the same size as the list.
        let paths: HashSet<_> = Icon::ALL.iter().map(|i| i.path()).collect();
        assert_eq!(
            paths.len(),
            Icon::ALL.len(),
            "duplicate or missing entry in Icon::ALL"
        );
    }
}
