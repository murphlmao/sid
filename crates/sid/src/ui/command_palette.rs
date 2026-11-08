//! The command palette (`Ctrl+K` / `Ctrl+Shift+K` in a focused terminal): the hub. A
//! fuzzy filter over every [`keymap::Action`] (with its live binding), "Connect to <host>"
//! for every saved SSH host in the active scope, "Open <workspace>" for every workspace,
//! the Settings sections, and every open SSH session tab. Recently used entries float to
//! the top (all of them, when the query is empty). `Enter` runs the selected row; `Esc`
//! closes; `↑`/`↓` or `Ctrl+N`/`Ctrl+P` move the selection. All of those are handled by
//! `app.rs`'s root key dispatcher (it already owns the keystroke) — this module owns the
//! palette's *state* ([`PaletteState`]), *matching* ([`fuzzy_score`], pure/unit-tested),
//! and *rendering*.
//!
//! [`PaletteState`] is a plain struct on `AppState` (`app::AppState::palette:
//! Option<PaletteState>`, `None` = closed) — the same "sibling state + a second `impl
//! AppState` block" shape as `ui::db_tab::DbTabState`/`ui::ssh_home::HomeTabState`, not a
//! separate `Entity`/event-emitter like `HostForm`.

use gpui::{
    ClickEvent, ClipboardItem, Context, Entity, IntoElement, SharedString, Window, anchored,
    deferred, div, point, prelude::*, px, rgba,
};

use crate::app::{AppState, Tab};
use crate::keymap::{self, Action};
use sid_ui::{
    Elevation, EmptyState, Enter, Icon, InputState, Kbd, Radius, Rounded as _, Row, StyledExt as _,
    TextInput, Typography as _, bridge, motion, scaled, theme,
};

/// How many matches the palette shows at once.
const MAX_RESULTS: usize = 30;

/// How many recently run entries are remembered (this run only).
pub(crate) const MAX_RECENT: usize = 8;

/// What confirming a given row does. Index variants point into `AppState` lists that are
/// already loaded and cheap to re-index at confirm time, which keeps [`PaletteEntry`]
/// (rebuilt on every keystroke) from cloning whole records per candidate per frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaletteTarget {
    Action(Action),
    /// Index into the *active scope's* composed `AppState::hosts`: connect to it.
    Connection(usize),
    /// Index into the same list: copy `user@host:port` to the clipboard.
    CopyAddress(usize),
    /// Index into `AppState::ssh_sessions`.
    Session(usize),
    /// Index into `AppState::scopes`: focus that workspace.
    Workspace(usize),
    /// Index into the Settings sections: jump to Settings on that section.
    SettingsSection(usize),
}

/// One filtered/scored row.
struct PaletteEntry {
    target: PaletteTarget,
    /// Stable identity for the "recent" list (survives the candidate list changing).
    key: String,
    icon: Icon,
    label: SharedString,
    subtitle: Option<SharedString>,
    shortcut: Option<SharedString>,
    score: i32,
}

/// The palette's open state: `None` on `AppState::palette` means closed.
pub(crate) struct PaletteState {
    query: Entity<InputState>,
    selection: usize,
}

/// Score added to a recently used entry. With an empty query every entry scores 0, so
/// this alone orders them most-recent-first; with a query it is a nudge that breaks ties
/// between comparable matches, not enough to beat a better text match.
fn recency_bonus(recent: &[String], key: &str, query_is_empty: bool) -> i32 {
    match recent.iter().position(|k| k == key) {
        Some(pos) if query_is_empty => 1_000 - pos as i32,
        Some(pos) => 40 - pos as i32,
        None => 0,
    }
}

/// Move `key` to the front of `recent`, capped at [`MAX_RECENT`].
pub(crate) fn remember_recent(recent: &mut Vec<String>, key: String) {
    recent.retain(|k| k != &key);
    recent.insert(0, key);
    recent.truncate(MAX_RECENT);
}

impl AppState {
    // ---- open/close (called from `dispatch_action`) --------------------------------

    /// `Ctrl+K`: open the palette, or close it if it's already open. Refuses to open
    /// over another modal (host/db-connection form, secret unlock).
    pub(crate) fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            self.close_palette(cx);
        } else if !self.blocking_modal_open() {
            let query = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Search commands, hosts, workspaces and settings…")
            });
            query.update(cx, |state, cx| state.focus(window, cx));
            self.palette = Some(PaletteState {
                query,
                selection: 0,
            });
            cx.notify();
        }
    }

    pub(crate) fn close_palette(&mut self, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            cx.notify();
        }
    }

    // ---- selection + confirm (called from `app.rs`'s root key handler) -------------

    /// Move the selection by `delta`, wrapping. A no-op while closed or with nothing
    /// matching the current query.
    pub(crate) fn palette_move_selection(&mut self, delta: i32, cx: &mut Context<Self>) {
        let Some(query) = self
            .palette
            .as_ref()
            .map(|p| p.query.read(cx).value().to_string())
        else {
            return;
        };
        let len = self.palette_entries(&query).len();
        if len == 0 {
            return;
        }
        if let Some(palette) = &mut self.palette {
            let current = palette.selection.min(len - 1) as i32;
            palette.selection = (current + delta).rem_euclid(len as i32) as usize;
        }
        cx.notify();
    }

    /// `Enter`: run whatever the selected row does, then close. A no-op if nothing is
    /// selected (e.g. the query matches nothing).
    pub(crate) fn palette_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((target, key)) = self.palette_selected(cx) else {
            return;
        };
        self.close_palette(cx);
        remember_recent(&mut self.recent_commands, key);
        match target {
            PaletteTarget::Action(action) => self.dispatch_action(action, window, cx),
            PaletteTarget::Connection(ix) => {
                if let Some(a) = self.hosts.get(ix).cloned() {
                    let source = Some((a.item.alias.clone(), a.origin.clone()));
                    self.connect_host(a.item, source, window, cx);
                }
            }
            PaletteTarget::CopyAddress(ix) => {
                if let Some(a) = self.hosts.get(ix) {
                    let h = &a.item;
                    let addr = if h.user.is_empty() {
                        format!("{}:{}", h.host, h.port)
                    } else {
                        format!("{}@{}:{}", h.user, h.host, h.port)
                    };
                    cx.write_to_clipboard(ClipboardItem::new_string(addr));
                }
            }
            PaletteTarget::Session(ix) => self.activate_session(ix, window, cx),
            PaletteTarget::Workspace(ix) => {
                if let Some(choice) = self.scopes.get(ix) {
                    let scope = choice.scope.clone();
                    self.set_scope(scope);
                    cx.notify();
                }
            }
            PaletteTarget::SettingsSection(ix) => {
                self.show_settings_section(ix);
                self.switch_to_tab(Tab::Settings, window, cx);
            }
        }
    }

    fn palette_selected(&self, cx: &Context<Self>) -> Option<(PaletteTarget, String)> {
        let palette = self.palette.as_ref()?;
        let query = palette.query.read(cx).value().to_string();
        let entries = self.palette_entries(&query);
        entries
            .get(palette.selection.min(entries.len().saturating_sub(1)))
            .map(|e| (e.target, e.key.clone()))
    }

    // ---- candidate list + fuzzy filter (rebuilt live, not cached) ------------------

    /// Every candidate that matches `query`, scored and sorted best first.
    fn palette_entries(&self, query: &str) -> Vec<PaletteEntry> {
        // The *effective* registry, not the defaults: the shortcut column has to show
        // what the user actually rebound.
        let bindings = self.effective_bindings();
        let empty = query.trim().is_empty();
        let recent = &self.recent_commands;
        let mut entries = Vec::new();
        let mut push = |mut e: PaletteEntry, score: i32| {
            e.score = score + recency_bonus(recent, &e.key, empty);
            entries.push(e);
        };

        for &action in keymap::ALL_ACTIONS {
            let label = action.label();
            if let Some(score) = fuzzy_score(query, label) {
                push(
                    PaletteEntry {
                        target: PaletteTarget::Action(action),
                        key: format!("a:{}", action.id()),
                        icon: action_icon(action),
                        label: label.into(),
                        subtitle: None,
                        shortcut: keymap::primary_shortcut(action, &bindings).map(Into::into),
                        score: 0,
                    },
                    score,
                );
            }
        }

        for (ix, name) in crate::ui::settings_tab::section_labels().iter().enumerate() {
            let label = format!("Settings: {name}");
            if let Some(score) = fuzzy_score(query, &label) {
                push(
                    PaletteEntry {
                        target: PaletteTarget::SettingsSection(ix),
                        key: format!("s:{name}"),
                        icon: Icon::Settings,
                        label: label.into(),
                        subtitle: None,
                        shortcut: None,
                        score: 0,
                    },
                    score,
                );
            }
        }

        for (ix, a) in self.hosts.iter().enumerate() {
            let label = format!("Connect to {}", a.item.alias);
            if let Some(score) = fuzzy_score(query, &label) {
                push(
                    PaletteEntry {
                        target: PaletteTarget::Connection(ix),
                        key: format!("h:{}", a.item.alias),
                        icon: Icon::Terminal,
                        label: label.into(),
                        subtitle: Some(format!("{}@{}", a.item.user, a.item.host).into()),
                        shortcut: None,
                        score: 0,
                    },
                    score,
                );
            }
            // The clipboard affordance. Only offered once the user is typing, so a bare
            // palette is not doubled in length by a copy twin of every host.
            let copy_label = format!("Copy address of {}", a.item.alias);
            if !empty && let Some(score) = fuzzy_score(query, &copy_label) {
                push(
                    PaletteEntry {
                        target: PaletteTarget::CopyAddress(ix),
                        key: format!("c:{}", a.item.alias),
                        icon: Icon::Copy,
                        label: copy_label.into(),
                        subtitle: Some(format!("{}@{}", a.item.user, a.item.host).into()),
                        shortcut: None,
                        score: 0,
                    },
                    score - 50,
                );
            }
        }

        for (ix, choice) in self.scopes.iter().enumerate() {
            if matches!(choice.scope, sid_store::Scope::Global) {
                continue;
            }
            let label = format!("Open {}", choice.label);
            if let Some(score) = fuzzy_score(query, &label) {
                push(
                    PaletteEntry {
                        target: PaletteTarget::Workspace(ix),
                        key: format!("w:{}", choice.label),
                        icon: Icon::Folder,
                        label: label.into(),
                        subtitle: Some("workspace".into()),
                        shortcut: None,
                        score: 0,
                    },
                    score,
                );
            }
        }

        for (ix, tab) in self.ssh_sessions.iter().enumerate() {
            if let Some(score) = fuzzy_score(query, &tab.label) {
                push(
                    PaletteEntry {
                        target: PaletteTarget::Session(ix),
                        key: format!("t:{}", tab.label),
                        icon: Icon::Terminal,
                        label: tab.label.clone(),
                        subtitle: Some("open session".into()),
                        shortcut: None,
                        score: 0,
                    },
                    score,
                );
            }
        }

        // Stable sort: ties keep the push order above.
        entries.sort_by_key(|e| std::cmp::Reverse(e.score));
        entries.truncate(MAX_RESULTS);
        entries
    }

    // ---- rendering -------------------------------------------------------------

    /// `None` while closed. A viewport-sized, occluding `deferred`/`anchored` backdrop
    /// with the panel centred near the top. Not a `Modal` (it is not a form), so it
    /// carries its own v2 chrome: floating elevation, large radius, rise-in.
    pub(crate) fn palette_overlay(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let t = theme::active(cx).clone();
        let palette = self.palette.as_ref()?;
        let query_text = palette.query.read(cx).value().to_string();
        let entries = self.palette_entries(&query_text);
        let selection = palette.selection.min(entries.len().saturating_sub(1));
        let query_input = palette.query.clone();
        let viewport = window.viewport_size();
        let no_matches = entries.is_empty();

        let rows: Vec<_> = entries
            .iter()
            .enumerate()
            .map(|(ix, entry)| self.palette_row(ix, entry, ix == selection, cx))
            .collect();

        let panel = div()
            .id("palette-panel")
            .w(scaled(580.))
            .max_h(scaled(460.))
            .flex()
            .flex_col()
            .elevation(Elevation::Floating, &t)
            .corners(Radius::Lg)
            .overflow_hidden()
            .child(
                div()
                    .p_2()
                    .border_b_1()
                    .border_color(gpui::rgb(t.border))
                    .child(TextInput::new(&query_input)),
            )
            .child(
                div()
                    .id("palette-results")
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .p_1()
                    .children(rows)
                    .when(no_matches, |this| {
                        this.child(
                            EmptyState::new("No matches")
                                .guidance("Try a host name, a tab, or a setting.")
                                .icon(Icon::Search)
                                .compact(),
                        )
                    }),
            );
        let panel = motion::enter("palette-enter", Enter::Rise, motion::FAST, cx, panel);

        Some(
            deferred(
                anchored().position(point(px(0.), px(0.))).child(
                    div()
                        .id("palette-backdrop")
                        .occlude()
                        .flex()
                        .items_start()
                        .justify_center()
                        .pt(scaled(120.))
                        .w(viewport.width)
                        .h(viewport.height)
                        .bg(rgba(bridge::scrim(&t)))
                        .child(panel),
                ),
            )
            .with_priority(2),
        )
    }

    fn palette_row(
        &self,
        ix: usize,
        entry: &PaletteEntry,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme::active(cx).clone();
        let ink = if selected { t.fg_strong } else { t.fg };
        let me = cx.entity();
        // The label/subtitle column carries whatever a saved host is called, inside a
        // fixed-width panel: `min_w(0)` + the clamps restore the shrink gpui's
        // text-min-content rule takes away.
        let text = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.))
            .child(
                div()
                    .text_body(&t)
                    .text_color(gpui::rgb(ink))
                    .clamp_one_line()
                    .child(entry.label.clone()),
            )
            .children(
                entry
                    .subtitle
                    .clone()
                    .map(|s| div().text_meta(&t).clamp_one_line().child(s)),
            );
        let mut row = Row::new(("palette-row", ix))
            .compact()
            .selected(selected)
            .leading(entry.icon.small())
            .child(text)
            .on_click(move |_ev: &ClickEvent, window, app| {
                me.update(app, |this, cx| {
                    if let Some(palette) = &mut this.palette {
                        palette.selection = ix;
                    }
                    this.palette_confirm(window, cx);
                });
            });
        if let Some(s) = entry.shortcut.clone() {
            row = row.meta(div().flex_none().child(Kbd::new(s)));
        }
        row
    }
}

/// The glyph each action wears in the palette.
fn action_icon(action: Action) -> Icon {
    match action {
        Action::CommandPalette => Icon::Search,
        Action::PrimaryTab(1) => Icon::Terminal,
        Action::PrimaryTab(2) => Icon::Database,
        Action::PrimaryTab(3) => Icon::Interfaces,
        Action::PrimaryTab(4) => Icon::Folder,
        Action::PrimaryTab(5) => Icon::Dashboard,
        Action::PrimaryTab(_) | Action::Settings => Icon::Settings,
        Action::CycleTabForward | Action::CycleSessionForward => Icon::ChevronRight,
        Action::CycleTabBack | Action::CycleSessionBack => Icon::ChevronLeft,
        Action::NewSession => Icon::Add,
        Action::CloseSession => Icon::Close,
        Action::CheatSheet => Icon::Info,
        Action::FocusFilter => Icon::Search,
        Action::ZoomIn | Action::ZoomOut | Action::ZoomReset => Icon::Maximize,
    }
}

// ---- fuzzy matching (pure, unit-tested) -----------------------------------------

/// Case-insensitive fuzzy match of `query` against `candidate`. `None` means no match;
/// otherwise a higher score is a better match. An empty (or all-whitespace) query
/// matches everything with the same baseline score, so opening the palette with nothing
/// typed yet just browses the full candidate list. Ranking, best first: exact match >
/// prefix match > substring match > in-order subsequence (denser spans score higher).
pub(crate) fn fuzzy_score(query: &str, candidate: &str) -> Option<i32> {
    let query = query.trim();
    if query.is_empty() {
        return Some(0);
    }
    let q = query.to_lowercase();
    let c = candidate.to_lowercase();

    if c == q {
        return Some(1_000);
    }
    if c.starts_with(&q) {
        return Some(500 - c.len() as i32);
    }
    if let Some(pos) = c.find(&q) {
        return Some(250 - pos as i32);
    }

    // In-order subsequence fallback: every query char must appear in `c`, in order
    // (not necessarily contiguous) — classic fuzzy-finder behavior ("cnn" matches
    // "Connection"). `found_all` only flips once `wanted` is actually exhausted *by a
    // confirmed match* — advancing to the next wanted char is not itself a match, so a
    // query's last character must still be found in `c` after it, not just assumed
    // found because there was nothing left to advance to.
    let mut wanted = q.chars();
    let mut want = wanted.next()?;
    let mut last_ix: i32 = -1;
    let mut found_all = false;
    for (ix, ch) in c.chars().enumerate() {
        if ch == want {
            last_ix = ix as i32;
            match wanted.next() {
                Some(next) => want = next,
                None => {
                    found_all = true;
                    break;
                }
            }
        }
    }
    if found_all { Some(100 - last_ix) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_entries_rank_first_on_an_empty_query_most_recent_leading() {
        let mut recent = Vec::new();
        remember_recent(&mut recent, "a:settings".into());
        remember_recent(&mut recent, "h:vps".into());
        assert_eq!(recent, vec!["h:vps".to_string(), "a:settings".to_string()]);
        assert!(recency_bonus(&recent, "h:vps", true) > recency_bonus(&recent, "a:settings", true));
        assert_eq!(recency_bonus(&recent, "w:other", true), 0);
    }

    #[test]
    fn a_recent_nudge_never_beats_a_better_text_match() {
        let recent = vec!["a:x".to_string()];
        let nudged =
            fuzzy_score("set", "Offset thing").unwrap() + recency_bonus(&recent, "a:x", false);
        let exact_prefix = fuzzy_score("set", "Settings").unwrap();
        assert!(exact_prefix > nudged);
    }

    #[test]
    fn recents_are_capped_and_deduplicated() {
        let mut recent = Vec::new();
        for i in 0..20 {
            remember_recent(&mut recent, format!("k{}", i % 12));
        }
        assert!(recent.len() <= MAX_RECENT);
        let mut sorted = recent.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), recent.len());
    }

    #[test]
    fn empty_query_matches_everything_with_baseline_score() {
        assert_eq!(fuzzy_score("", "anything"), Some(0));
        assert_eq!(fuzzy_score("   ", "anything"), Some(0));
    }

    #[test]
    fn exact_match_scores_highest() {
        let exact = fuzzy_score("database", "Database").unwrap();
        let prefix = fuzzy_score("data", "Database").unwrap();
        let substring = fuzzy_score("abas", "Database").unwrap();
        let subsequence = fuzzy_score("dtb", "Database").unwrap();
        assert!(exact > prefix);
        assert!(prefix > substring);
        assert!(substring > subsequence);
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert_eq!(
            fuzzy_score("DATABASE", "database"),
            fuzzy_score("database", "database")
        );
    }

    #[test]
    fn subsequence_must_stay_in_order() {
        // "ssh" as a subsequence of "hss" would require going backwards -> no match.
        assert_eq!(fuzzy_score("ssh", "hss"), None);
        assert!(fuzzy_score("ssh", "s-s-h").is_some());
    }

    #[test]
    fn no_match_returns_none() {
        assert_eq!(fuzzy_score("zzz", "Database"), None);
    }

    #[test]
    fn denser_subsequence_spans_score_higher() {
        // "cnn" is a tight subsequence of "Connection" (early letters); a scattered
        // match across a longer string should score no better.
        let tight = fuzzy_score("cnn", "connection").unwrap();
        let scattered = fuzzy_score("cnn", "c................n.........n").unwrap();
        assert!(tight > scattered);
    }
}
