//! The command palette (`Ctrl+K` / `Ctrl+Shift+K` in a focused terminal): a fuzzy
//! filter over every [`keymap::Action`], every saved connection in the active scope, and
//! every open SSH session tab. `Enter` runs the selected row; `Esc` closes; `↑`/`↓` or
//! `Ctrl+N`/`Ctrl+P` move the selection. All three of those are handled by `app.rs`'s
//! root key dispatcher (it already owns the keystroke) — this module owns the palette's
//! *state* ([`PaletteState`]), *matching* ([`fuzzy_score`], pure/unit-tested), and
//! *rendering*.
//!
//! [`PaletteState`] is a plain struct on `AppState` (`app::AppState::palette:
//! Option<PaletteState>`, `None` = closed) — the same "sibling state + a second `impl
//! AppState` block" shape as `ui::db_tab::DbTabState`/`ui::ssh_home::HomeTabState`, not a
//! separate `Entity`/event-emitter like `HostForm`. That fits better here: the palette
//! has no multi-field validation or submit lifecycle to model as events, just "open,
//! type, move, confirm" — plain methods on `AppState` are simpler and match the
//! `ssh_home`/`db_tab` convention already used for this kind of per-feature view state.

use gpui::{
    ClickEvent, Context, Entity, IntoElement, SharedString, Window, anchored, deferred, div, point,
    prelude::*, px, rgb, rgba,
};

use crate::app::AppState;
use crate::keymap::{self, Action};
use crate::ui::TextInput;

// Dark-theme palette, aligned with `app.rs`/`host_form.rs`. Kept local so `ui` stays
// self-contained (same convention as every other view module here).
const PANEL_BG: u32 = 0x1d1d20;
const BORDER: u32 = 0x2c2c30;
const FG: u32 = 0xdcdce0;
const FG_DIM: u32 = 0x8a8a90;
const ACTIVE_BG: u32 = 0x33343a;
const ACTIVE_FG: u32 = 0xffffff;
const BRAND: u32 = 0x5a9ad0;

/// How many matches the palette shows at once — plenty for the v1 candidate set
/// (a dozen actions, plus however many hosts/sessions are around) without the list
/// growing unbounded if a query matches everything.
const MAX_RESULTS: usize = 30;

/// What confirming a given row does. `Connection`/`Session` carry an index into
/// `AppState::hosts`/`AppState::ssh_sessions` rather than owned data — both lists are
/// already loaded and cheap to re-index at confirm time, and this keeps [`PaletteEntry`]
/// (rebuilt on every keystroke) from cloning a whole `Host` per candidate per frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaletteTarget {
    Action(Action),
    /// Index into the *active scope's* composed `AppState::hosts` — the same list the
    /// SSH tab's connection manager and tree already show, so "saved connections" here
    /// means "saved connections visible from here right now", not a fresh global scan.
    Connection(usize),
    /// Index into `AppState::ssh_sessions`.
    Session(usize),
}

/// One filtered/scored row, rebuilt fresh from `AppState`'s live data on every
/// keystroke (the candidate set — actions + hosts + sessions — is small enough that
/// this costs nothing worth caching).
struct PaletteEntry {
    target: PaletteTarget,
    label: SharedString,
    subtitle: Option<SharedString>,
    shortcut: Option<SharedString>,
    score: i32,
}

/// The palette's open state: `None` on `AppState::palette` means closed. Holds the
/// query `TextInput` (so `app.rs`/this module can read `.content()` live, same as
/// `ui::ssh_home`'s quick-connect box) and the current selection index, clamped against
/// the live filtered list wherever it's read (see [`AppState::palette_entries`]) rather
/// than being kept in lockstep with every keystroke.
pub(crate) struct PaletteState {
    query: Entity<TextInput>,
    selection: usize,
}

impl AppState {
    // ---- open/close (called from `dispatch_action`) --------------------------------

    /// `Ctrl+K`: open the palette, or close it if it's already open. Refuses to open
    /// over another modal (host/db-connection form, secret unlock) — those already own
    /// the keyboard, and stacking overlays is more confusing than doing nothing.
    pub(crate) fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            self.close_palette(cx);
        } else if !self.blocking_modal_open() {
            let query = cx.new(|cx| TextInput::new(cx, "Type a command, connection, or tab…"));
            query.read(cx).focus(window);
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

    /// Move the selection by `delta` (`+1`/`-1` from `↓`/`↑`/`Ctrl+N`/`Ctrl+P`),
    /// wrapping. A no-op while closed or with nothing matching the current query.
    pub(crate) fn palette_move_selection(&mut self, delta: i32, cx: &mut Context<Self>) {
        let Some(query) = self
            .palette
            .as_ref()
            .map(|p| p.query.read(cx).content().to_string())
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
        let Some(target) = self.palette_selected_target(cx) else {
            return;
        };
        self.close_palette(cx);
        match target {
            PaletteTarget::Action(action) => self.dispatch_action(action, window, cx),
            PaletteTarget::Connection(ix) => {
                if let Some(a) = self.hosts.get(ix).cloned() {
                    let source = Some((a.item.alias.clone(), a.origin.clone()));
                    self.connect_host(a.item, source, window, cx);
                }
            }
            PaletteTarget::Session(ix) => self.activate_session(ix, window, cx),
        }
    }

    fn palette_selected_target(&self, cx: &Context<Self>) -> Option<PaletteTarget> {
        let palette = self.palette.as_ref()?;
        let query = palette.query.read(cx).content().to_string();
        let entries = self.palette_entries(&query);
        entries
            .get(palette.selection.min(entries.len().saturating_sub(1)))
            .map(|e| e.target)
    }

    // ---- candidate list + fuzzy filter (rebuilt live, not cached) ------------------

    /// Every action/connection/session that matches `query`, scored and sorted best
    /// first. Pure given `&self` + `query` — no gpui `cx` needed, so this (and
    /// [`fuzzy_score`] underneath it) is straightforward to unit-test without a window.
    fn palette_entries(&self, query: &str) -> Vec<PaletteEntry> {
        let bindings = keymap::default_bindings();
        let mut entries = Vec::new();

        for &action in keymap::ALL_ACTIONS {
            let label = action.label();
            if let Some(score) = fuzzy_score(query, label) {
                entries.push(PaletteEntry {
                    target: PaletteTarget::Action(action),
                    label: label.into(),
                    subtitle: None,
                    shortcut: keymap::primary_shortcut(action, &bindings).map(Into::into),
                    score,
                });
            }
        }

        for (ix, a) in self.hosts.iter().enumerate() {
            if let Some(score) = fuzzy_score(query, &a.item.alias) {
                entries.push(PaletteEntry {
                    target: PaletteTarget::Connection(ix),
                    label: a.item.alias.clone().into(),
                    subtitle: Some(format!("connect — {}@{}", a.item.user, a.item.host).into()),
                    shortcut: None,
                    score,
                });
            }
        }

        for (ix, tab) in self.ssh_sessions.iter().enumerate() {
            if let Some(score) = fuzzy_score(query, &tab.label) {
                entries.push(PaletteEntry {
                    target: PaletteTarget::Session(ix),
                    label: tab.label.clone(),
                    subtitle: Some("open session tab".into()),
                    shortcut: None,
                    score,
                });
            }
        }

        // Stable sort: ties keep the actions-then-connections-then-sessions order the
        // candidates were pushed in above.
        entries.sort_by_key(|e| std::cmp::Reverse(e.score));
        entries.truncate(MAX_RESULTS);
        entries
    }

    // ---- rendering -------------------------------------------------------------

    /// `None` while closed. Mirrors `app.rs`'s host-form overlay: a viewport-sized,
    /// occluding `deferred`/`anchored` backdrop with the panel centered on top.
    pub(crate) fn palette_overlay(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let palette = self.palette.as_ref()?;
        let query_text = palette.query.read(cx).content().to_string();
        let entries = self.palette_entries(&query_text);
        let selection = palette.selection.min(entries.len().saturating_sub(1));
        let query_input = palette.query.clone();
        let viewport = window.viewport_size();

        let rows: Vec<_> = entries
            .iter()
            .enumerate()
            .map(|(ix, entry)| self.palette_row(ix, entry, ix == selection, cx))
            .collect();
        let empty_notice = entries.is_empty().then(|| {
            div()
                .px_3()
                .py_4()
                .text_sm()
                .text_color(rgb(FG_DIM))
                .child("no matches")
        });

        Some(
            deferred(
                anchored().position(point(px(0.), px(0.))).child(
                    div()
                        .id("palette-backdrop")
                        .occlude()
                        .flex()
                        .items_start()
                        .justify_center()
                        .pt(px(120.))
                        .w(viewport.width)
                        .h(viewport.height)
                        .bg(rgba(0x000000a8))
                        .child(
                            div()
                                .id("palette-panel")
                                .w(px(560.))
                                .max_h(px(420.))
                                .flex()
                                .flex_col()
                                .bg(rgb(PANEL_BG))
                                .border_1()
                                .border_color(rgb(BORDER))
                                .rounded_md()
                                .child(
                                    div()
                                        .px_2()
                                        .py_2()
                                        .border_b_1()
                                        .border_color(rgb(BORDER))
                                        .child(query_input),
                                )
                                .child(
                                    div()
                                        .id("palette-results")
                                        .flex()
                                        .flex_col()
                                        .overflow_y_scroll()
                                        .py_1()
                                        .children(rows)
                                        .children(empty_notice),
                                ),
                        ),
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
        div()
            .id(("palette-row", ix))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .cursor_pointer()
            .bg(rgb(if selected { ACTIVE_BG } else { PANEL_BG }))
            .text_color(rgb(if selected { ACTIVE_FG } else { FG }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .child(div().text_sm().child(entry.label.clone()))
                    .children(
                        entry
                            .subtitle
                            .clone()
                            .map(|s| div().text_xs().text_color(rgb(FG_DIM)).child(s)),
                    ),
            )
            .children(
                entry
                    .shortcut
                    .clone()
                    .map(|s| div().text_xs().text_color(rgb(BRAND)).child(s)),
            )
            .on_click(cx.listener(move |this, _ev: &ClickEvent, window, cx| {
                if let Some(palette) = &mut this.palette {
                    palette.selection = ix;
                }
                this.palette_confirm(window, cx);
            }))
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
