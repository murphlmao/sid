//! The keyboard-driven system (`docs/superpowers/plans/2026-07-02-keyboard-system.md`):
//! the [`Action`] enum, the default [`Binding`] registry, and the one necessary
//! terminal-focus exception.
//!
//! Inside a **focused terminal**, `Ctrl+<letter>` are shell control codes (`Ctrl+C`
//! SIGINT, `Ctrl+R` reverse-search, `Ctrl+W` kill-word, ...) — the terminal must get
//! first dibs on them or the shell is broken. So sid's own letter accelerators (`Ctrl+K`
//! palette, `Ctrl+T`/`Ctrl+W` session new/close) fall back to their `Ctrl+Shift+<letter>`
//! form in that context only; everywhere else, plain `Ctrl+<letter>` works. Non-letter
//! accelerators (`Ctrl+1..5`, `Ctrl+Tab`/`Ctrl+Shift+Tab`) never collide with readline,
//! so they're global in both contexts.
//!
//! Everything in this module is pure and gpui-light (only [`gpui::Keystroke`]) — the
//! lookup, conflict detection, and the terminal-focus fallback rule are unit-tested
//! without a window. `app.rs`'s root key handler is the only caller: it computes
//! [`FocusContext`] (is the active SSH session's terminal focused?) and calls [`resolve`]
//! on every keystroke that bubbles to the window root.

use gpui::Keystroke;

/// Every keyboard-reachable app-level action (v1 — the plan's seed set). More will be
/// added as later tabs/slices grow their own bindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Open (or, if already open, close) the fuzzy command palette.
    CommandPalette,
    /// Switch to primary tab `1..=5` (SSH, Database, Network, Workspaces, System, in
    /// that order — see `app::Tab::ALL`). Any other value is simply never bound.
    PrimaryTab(u8),
    /// Cycle forward through the primary tabs — always, even while the SSH tab is
    /// active. (Session tabs have their own dedicated cycle below; letting `Ctrl+Tab`
    /// switch meanings on landing in the SSH shell trapped the cycle there.)
    CycleTabForward,
    /// Cycle backward — the mirror of [`Self::CycleTabForward`].
    CycleTabBack,
    /// SSH shell: next session tab (Home is its own stop). No-op outside the SSH tab.
    CycleSessionForward,
    /// SSH shell: previous session tab — the mirror of [`Self::CycleSessionForward`].
    CycleSessionBack,
    /// SSH shell: open a new session (goes Home to pick one).
    NewSession,
    /// SSH shell: close the active session tab.
    CloseSession,
    /// Open Settings. No dedicated screen exists yet — v1 stands in with `Tab::System`;
    /// Settings → Keymap itself (rebinding UI) is deferred, per the plan.
    Settings,
    /// Toggle the keyboard cheat-sheet overlay.
    CheatSheet,
    /// Focus the active tab's find/filter box. Currently wired to the Network tab's
    /// shared filter `TextInput` only (`app::dispatch_action`); a no-op everywhere else
    /// until later tabs grow their own filter input.
    FocusFilter,
}

impl Action {
    /// A short, human label for the command palette / cheat sheet. Deliberately
    /// self-contained (no dependency on `app`'s private `Tab` type) — see this module's
    /// doc comment on why `keymap` stays decoupled from `app`.
    pub fn label(self) -> &'static str {
        match self {
            Action::CommandPalette => "Command Palette",
            Action::PrimaryTab(1) => "Go to SSH / SFTP",
            Action::PrimaryTab(2) => "Go to Database",
            Action::PrimaryTab(3) => "Go to Network",
            Action::PrimaryTab(4) => "Go to Workspaces",
            Action::PrimaryTab(5) => "Go to System",
            Action::PrimaryTab(_) => "Go to tab",
            Action::CycleTabForward => "Next Tab",
            Action::CycleTabBack => "Previous Tab",
            Action::CycleSessionForward => "Next SSH Session",
            Action::CycleSessionBack => "Previous SSH Session",
            Action::NewSession => "New SSH Session",
            Action::CloseSession => "Close SSH Session",
            Action::Settings => "Settings",
            Action::CheatSheet => "Keyboard Shortcuts",
            Action::FocusFilter => "Find / Filter",
        }
    }
}

/// The full v1 action set, in the order the command palette lists them.
pub const ALL_ACTIONS: &[Action] = &[
    Action::CommandPalette,
    Action::PrimaryTab(1),
    Action::PrimaryTab(2),
    Action::PrimaryTab(3),
    Action::PrimaryTab(4),
    Action::PrimaryTab(5),
    Action::CycleTabForward,
    Action::CycleTabBack,
    Action::CycleSessionForward,
    Action::CycleSessionBack,
    Action::NewSession,
    Action::CloseSession,
    Action::Settings,
    Action::CheatSheet,
    Action::FocusFilter,
];

/// Whether the keyboard focus is currently inside a live SSH terminal pane — the one
/// axis the terminal-focus exception is gated on. `app.rs` computes this by comparing
/// `window.focused(cx)` against the active session's
/// `SshSession::terminal_focus_handle()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusContext {
    Terminal,
    Normal,
}

/// Which [`FocusContext`] a [`Binding`] fires in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingContext {
    /// Active regardless of focus — reserved for accelerators that never collide with a
    /// shell control code (non-letter chords: digits, Tab, punctuation).
    Global,
    /// Active only outside a focused terminal — a plain `Ctrl+<letter>`.
    NormalOnly,
    /// Active only inside a focused terminal — the `Ctrl+Shift+<letter>` fallback.
    TerminalOnly,
}

/// A key chord: a base key plus modifiers.
///
/// `shift` is `Some(bool)` when the shift state must match exactly — true for
/// letters/digits/Tab, whose resolved [`Keystroke::key`] stays the same either way, so
/// shift is the only signal that tells `Ctrl+K` apart from `Ctrl+Shift+K`. It's `None` to
/// ignore shift entirely for symbol keys like `?`: gpui's xkb glue already resolves the
/// *shifted* character into `key` itself (`Keysym::question` -> `"?"`) — requiring
/// `shift: Some(false)` there would make the binding untypeable on any layout where the
/// symbol needs a physical Shift (true of `?` on a standard US layout).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub key: &'static str,
    pub ctrl: bool,
    pub shift: Option<bool>,
}

impl Chord {
    fn matches(&self, keystroke: &Keystroke) -> bool {
        let m = &keystroke.modifiers;
        if m.alt || m.platform {
            return false;
        }
        if m.control != self.ctrl {
            return false;
        }
        if let Some(want_shift) = self.shift
            && m.shift != want_shift
        {
            return false;
        }
        keystroke.key.eq_ignore_ascii_case(self.key)
    }

    /// A human-readable label (`"Ctrl+K"`, `"Ctrl+Shift+Tab"`) for the palette/cheat
    /// sheet.
    pub fn label(&self) -> String {
        let mut s = String::new();
        if self.ctrl {
            s.push_str("Ctrl+");
        }
        if self.shift == Some(true) {
            s.push_str("Shift+");
        }
        s.push_str(&display_key(self.key));
        s
    }
}

/// Capitalize a raw key string for display (`"tab"` -> `"Tab"`, `"k"` -> `"K"`, `","` ->
/// `","`).
fn display_key(key: &str) -> String {
    if key.chars().count() == 1 {
        return key.to_uppercase();
    }
    let mut chars = key.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// One entry in the binding registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub chord: Chord,
    pub context: BindingContext,
    pub action: Action,
}

fn chord(key: &'static str, ctrl: bool, shift: Option<bool>) -> Chord {
    Chord { key, ctrl, shift }
}

fn binding(chord: Chord, context: BindingContext, action: Action) -> Binding {
    Binding {
        chord,
        context,
        action,
    }
}

/// The default binding registry (v1, per the plan's "Bindings" table). Later, Settings →
/// Keymap can build a user-overridden `Vec<Binding>` of the same shape; nothing
/// downstream (`resolve`/`find_conflicts`) cares where the list came from.
pub fn default_bindings() -> Vec<Binding> {
    use BindingContext::{Global, NormalOnly, TerminalOnly};

    let mut bindings = vec![
        // Command palette: the letter accelerator + its terminal-focus fallback.
        binding(
            chord("k", true, Some(false)),
            NormalOnly,
            Action::CommandPalette,
        ),
        binding(
            chord("k", true, Some(true)),
            TerminalOnly,
            Action::CommandPalette,
        ),
        // Cycle: non-letter, so it's global in both contexts (the plan's rule).
        binding(
            chord("tab", true, Some(false)),
            Global,
            Action::CycleTabForward,
        ),
        binding(chord("tab", true, Some(true)), Global, Action::CycleTabBack),
        // Session cycling gets its own non-letter chords so `Ctrl+Tab` never changes
        // meaning mid-cycle (the "trapped in the SSH tab" bug). PgDn/PgUp don't collide
        // with readline; TUIs that want them lose out — acceptable, same trade as
        // Ctrl+1..5.
        binding(
            chord("pagedown", true, Some(false)),
            Global,
            Action::CycleSessionForward,
        ),
        binding(
            chord("pageup", true, Some(false)),
            Global,
            Action::CycleSessionBack,
        ),
        // SSH shell session management.
        binding(
            chord("t", true, Some(false)),
            NormalOnly,
            Action::NewSession,
        ),
        binding(
            chord("t", true, Some(true)),
            TerminalOnly,
            Action::NewSession,
        ),
        binding(
            chord("w", true, Some(false)),
            NormalOnly,
            Action::CloseSession,
        ),
        binding(
            chord("w", true, Some(true)),
            TerminalOnly,
            Action::CloseSession,
        ),
        // Settings: non-letter (a comma isn't a readline control code either) -> global.
        binding(chord(",", true, Some(false)), Global, Action::Settings),
        // Cheat sheet: bare `?`, no Ctrl at all. `app.rs`'s root handler adds the one
        // extra guard this (Keystroke, FocusContext) lookup alone can't express: never
        // fire while some other widget (a text field, most importantly) holds keyboard
        // focus, so a literal `?` typed anywhere is never stolen.
        binding(chord("?", false, None), NormalOnly, Action::CheatSheet),
        // Find/filter: no terminal-focus fallback is bound (the tabs that currently wire
        // this — Network — have no terminal), so plain `NormalOnly` is enough; inside a
        // focused terminal both chords simply resolve to `None` and pass through as their
        // usual shell control codes (`Ctrl+F` forward-char, `Ctrl+/` undo, in readline's
        // emacs mode).
        binding(
            chord("f", true, Some(false)),
            NormalOnly,
            Action::FocusFilter,
        ),
        binding(chord("/", true, None), NormalOnly, Action::FocusFilter),
    ];

    for (n, digit) in [(1u8, "1"), (2, "2"), (3, "3"), (4, "4"), (5, "5")] {
        bindings.push(binding(
            chord(digit, true, Some(false)),
            Global,
            Action::PrimaryTab(n),
        ));
    }

    // Self-check, stripped in release builds: a shipped registry with an internal
    // conflict would be a silent, hard-to-notice bug (whichever binding happens to come
    // first in the `Vec` would just silently shadow the other). This is the same
    // property `find_conflicts`'s own tests hold it to, just also checked against
    // reality on every debug-build startup.
    debug_assert!(
        find_conflicts(&bindings).is_empty(),
        "default_bindings() must be internally conflict-free"
    );
    bindings
}

/// Resolve one keystroke, in the given focus context, against `bindings`. `None` means
/// "not ours" — the caller must let the keystroke propagate untouched (the terminal's
/// own passthrough, a form field's text entry, ...).
pub fn resolve(keystroke: &Keystroke, focus: FocusContext, bindings: &[Binding]) -> Option<Action> {
    bindings
        .iter()
        .find(|b| b.chord.matches(keystroke) && context_active(b.context, focus))
        .map(|b| b.action)
}

fn context_active(context: BindingContext, focus: FocusContext) -> bool {
    matches!(
        (context, focus),
        (BindingContext::Global, _)
            | (BindingContext::NormalOnly, FocusContext::Normal)
            | (BindingContext::TerminalOnly, FocusContext::Terminal)
    )
}

/// The first non-terminal-only binding's label for `action` — what the palette/cheat
/// sheet display next to an action (the `Ctrl+Shift+<letter>` terminal fallback is an
/// implementation detail, not what most users should see as "the" shortcut).
pub fn primary_shortcut(action: Action, bindings: &[Binding]) -> Option<String> {
    bindings
        .iter()
        .find(|b| b.action == action && b.context != BindingContext::TerminalOnly)
        .map(|b| b.chord.label())
}

// ---- conflict detection (pure, unit-tested) --------------------------------------

/// Whether two [`BindingContext`]s can both be "live" for the same physical keystroke —
/// i.e. whether two bindings sharing a chord in these contexts would race. `NormalOnly`
/// and `TerminalOnly` never overlap by construction (that's the whole point of the
/// terminal-focus fallback), so two bindings split exactly that way are not a conflict.
fn contexts_overlap(a: BindingContext, b: BindingContext) -> bool {
    use BindingContext::{Global, NormalOnly, TerminalOnly};
    matches!(
        (a, b),
        (Global, _) | (_, Global) | (NormalOnly, NormalOnly) | (TerminalOnly, TerminalOnly)
    )
}

/// Whether two chords could match the same physical keystroke. Chords that ignore shift
/// (`shift: None`) are treated as potentially colliding with anything sharing their
/// key/ctrl, since they don't rule any shift state out.
fn chords_collide(a: &Chord, b: &Chord) -> bool {
    if a.ctrl != b.ctrl || !a.key.eq_ignore_ascii_case(b.key) {
        return false;
    }
    match (a.shift, b.shift) {
        (Some(x), Some(y)) => x == y,
        _ => true,
    }
}

/// Every pair of bindings that could both fire for the same keystroke in some reachable
/// focus context — a non-empty result means the registry is ambiguous and needs fixing.
/// Used both by this module's own tests on [`default_bindings`] and, later, whenever
/// Settings → Keymap lets a user add an override.
pub fn find_conflicts(bindings: &[Binding]) -> Vec<(Binding, Binding)> {
    let mut conflicts = Vec::new();
    for i in 0..bindings.len() {
        for j in (i + 1)..bindings.len() {
            let (a, b) = (bindings[i], bindings[j]);
            if chords_collide(&a.chord, &b.chord) && contexts_overlap(a.context, b.context) {
                conflicts.push((a, b));
            }
        }
    }
    conflicts
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Modifiers;

    fn key(k: &str) -> Keystroke {
        Keystroke {
            modifiers: Modifiers::default(),
            key: k.to_string(),
            key_char: None,
        }
    }

    fn ctrl(k: &str) -> Keystroke {
        Keystroke {
            modifiers: Modifiers {
                control: true,
                ..Default::default()
            },
            key: k.to_string(),
            key_char: None,
        }
    }

    fn ctrl_shift(k: &str) -> Keystroke {
        Keystroke {
            modifiers: Modifiers {
                control: true,
                shift: true,
                ..Default::default()
            },
            key: k.to_string(),
            key_char: None,
        }
    }

    // ---- default registry sanity -------------------------------------------------

    #[test]
    fn default_bindings_have_no_conflicts() {
        let conflicts = find_conflicts(&default_bindings());
        assert!(
            conflicts.is_empty(),
            "the shipped registry must be unambiguous, found: {conflicts:?}"
        );
    }

    // ---- plain lookup ---------------------------------------------------------

    #[test]
    fn ctrl_1_through_5_switch_primary_tabs_in_both_contexts() {
        let bindings = default_bindings();
        for (digit, n) in [("1", 1), ("2", 2), ("3", 3), ("4", 4), ("5", 5)] {
            assert_eq!(
                resolve(&ctrl(digit), FocusContext::Normal, &bindings),
                Some(Action::PrimaryTab(n))
            );
            assert_eq!(
                resolve(&ctrl(digit), FocusContext::Terminal, &bindings),
                Some(Action::PrimaryTab(n)),
                "non-letter accelerators must be global even inside a focused terminal"
            );
        }
    }

    #[test]
    fn ctrl_tab_cycles_in_both_contexts() {
        let bindings = default_bindings();
        assert_eq!(
            resolve(&ctrl("tab"), FocusContext::Normal, &bindings),
            Some(Action::CycleTabForward)
        );
        assert_eq!(
            resolve(&ctrl("tab"), FocusContext::Terminal, &bindings),
            Some(Action::CycleTabForward)
        );
        assert_eq!(
            resolve(&ctrl_shift("tab"), FocusContext::Normal, &bindings),
            Some(Action::CycleTabBack)
        );
        assert_eq!(
            resolve(&ctrl_shift("tab"), FocusContext::Terminal, &bindings),
            Some(Action::CycleTabBack)
        );
    }

    #[test]
    fn ctrl_page_up_down_cycle_sessions_in_both_contexts() {
        let bindings = default_bindings();
        for focus in [FocusContext::Normal, FocusContext::Terminal] {
            assert_eq!(
                resolve(&ctrl("pagedown"), focus, &bindings),
                Some(Action::CycleSessionForward)
            );
            assert_eq!(
                resolve(&ctrl("pageup"), focus, &bindings),
                Some(Action::CycleSessionBack)
            );
        }
    }

    #[test]
    fn ctrl_comma_opens_settings_in_both_contexts() {
        let bindings = default_bindings();
        assert_eq!(
            resolve(&ctrl(","), FocusContext::Normal, &bindings),
            Some(Action::Settings)
        );
        assert_eq!(
            resolve(&ctrl(","), FocusContext::Terminal, &bindings),
            Some(Action::Settings)
        );
    }

    // ---- the terminal-focus fallback rule (the load-bearing one) ---------------

    #[test]
    fn plain_ctrl_letter_fires_the_action_outside_a_terminal() {
        let bindings = default_bindings();
        assert_eq!(
            resolve(&ctrl("k"), FocusContext::Normal, &bindings),
            Some(Action::CommandPalette)
        );
        assert_eq!(
            resolve(&ctrl("t"), FocusContext::Normal, &bindings),
            Some(Action::NewSession)
        );
        assert_eq!(
            resolve(&ctrl("w"), FocusContext::Normal, &bindings),
            Some(Action::CloseSession)
        );
    }

    #[test]
    fn plain_ctrl_letter_passes_through_inside_a_focused_terminal() {
        let bindings = default_bindings();
        // `None` here is the whole point: the caller must NOT stop propagation, so the
        // keystroke reaches the PTY as a shell control code (Ctrl+K kill-line, Ctrl+T
        // swap-chars, Ctrl+W kill-word, Ctrl+C SIGINT — the last of which isn't even in
        // this registry at all, and so is *always* None regardless of context).
        assert_eq!(resolve(&ctrl("k"), FocusContext::Terminal, &bindings), None);
        assert_eq!(resolve(&ctrl("t"), FocusContext::Terminal, &bindings), None);
        assert_eq!(resolve(&ctrl("w"), FocusContext::Terminal, &bindings), None);
        assert_eq!(resolve(&ctrl("c"), FocusContext::Terminal, &bindings), None);
        assert_eq!(resolve(&ctrl("c"), FocusContext::Normal, &bindings), None);
    }

    #[test]
    fn ctrl_shift_letter_fires_the_action_only_inside_a_focused_terminal() {
        let bindings = default_bindings();
        assert_eq!(
            resolve(&ctrl_shift("k"), FocusContext::Terminal, &bindings),
            Some(Action::CommandPalette)
        );
        assert_eq!(
            resolve(&ctrl_shift("t"), FocusContext::Terminal, &bindings),
            Some(Action::NewSession)
        );
        assert_eq!(
            resolve(&ctrl_shift("w"), FocusContext::Terminal, &bindings),
            Some(Action::CloseSession)
        );
        // Outside a terminal, plain Ctrl+<letter> is already the action — the
        // Ctrl+Shift+<letter> fallback isn't bound to anything there.
        assert_eq!(
            resolve(&ctrl_shift("k"), FocusContext::Normal, &bindings),
            None
        );
    }

    #[test]
    fn ctrl_f_and_ctrl_slash_focus_filter_outside_a_terminal() {
        let bindings = default_bindings();
        assert_eq!(
            resolve(&ctrl("f"), FocusContext::Normal, &bindings),
            Some(Action::FocusFilter)
        );
        assert_eq!(
            resolve(&ctrl("/"), FocusContext::Normal, &bindings),
            Some(Action::FocusFilter)
        );
    }

    #[test]
    fn ctrl_f_and_ctrl_slash_pass_through_inside_a_focused_terminal() {
        let bindings = default_bindings();
        // No `TerminalOnly` fallback is bound for either chord — unlike the
        // command-palette/session letter accelerators, the tabs that currently wire
        // `FocusFilter` (Network) have no terminal, so there's nothing to fall back to.
        // `None` here means the keystroke reaches the PTY untouched, same as any other
        // unbound-in-terminal shell control code.
        assert_eq!(resolve(&ctrl("f"), FocusContext::Terminal, &bindings), None);
        assert_eq!(resolve(&ctrl("/"), FocusContext::Terminal, &bindings), None);
    }

    #[test]
    fn cheat_sheet_bare_question_mark_only_in_normal_context() {
        let bindings = default_bindings();
        assert_eq!(
            resolve(&key("?"), FocusContext::Normal, &bindings),
            Some(Action::CheatSheet)
        );
        assert_eq!(resolve(&key("?"), FocusContext::Terminal, &bindings), None);
    }

    #[test]
    fn unbound_chord_resolves_to_none() {
        let bindings = default_bindings();
        assert_eq!(resolve(&key("a"), FocusContext::Normal, &bindings), None);
        assert_eq!(resolve(&ctrl("z"), FocusContext::Normal, &bindings), None);
    }

    #[test]
    fn alt_or_platform_modifier_never_matches_a_ctrl_chord() {
        let bindings = default_bindings();
        let mut k = ctrl("k");
        k.modifiers.alt = true;
        assert_eq!(resolve(&k, FocusContext::Normal, &bindings), None);

        let mut k = ctrl("k");
        k.modifiers.platform = true;
        assert_eq!(resolve(&k, FocusContext::Normal, &bindings), None);
    }

    // ---- conflict detection itself ---------------------------------------------

    #[test]
    fn find_conflicts_flags_two_global_bindings_on_the_same_chord() {
        let bindings = vec![
            binding(
                chord("1", true, Some(false)),
                BindingContext::Global,
                Action::Settings,
            ),
            binding(
                chord("1", true, Some(false)),
                BindingContext::Global,
                Action::CheatSheet,
            ),
        ];
        assert_eq!(find_conflicts(&bindings).len(), 1);
    }

    #[test]
    fn find_conflicts_allows_normal_and_terminal_only_split_on_the_same_letter() {
        // This is exactly the shape the shipped registry relies on for every letter
        // accelerator — must never be flagged.
        let bindings = vec![
            binding(
                chord("k", true, Some(false)),
                BindingContext::NormalOnly,
                Action::CommandPalette,
            ),
            binding(
                chord("k", true, Some(true)),
                BindingContext::TerminalOnly,
                Action::CommandPalette,
            ),
        ];
        assert!(find_conflicts(&bindings).is_empty());
    }

    #[test]
    fn find_conflicts_flags_global_overlapping_normal_only() {
        let bindings = vec![
            binding(
                chord("1", true, Some(false)),
                BindingContext::Global,
                Action::Settings,
            ),
            binding(
                chord("1", true, Some(false)),
                BindingContext::NormalOnly,
                Action::CheatSheet,
            ),
        ];
        assert_eq!(find_conflicts(&bindings).len(), 1);
    }

    // ---- display labels ---------------------------------------------------------

    #[test]
    fn chord_label_formats_ctrl_and_ctrl_shift() {
        assert_eq!(chord("k", true, Some(false)).label(), "Ctrl+K");
        assert_eq!(chord("k", true, Some(true)).label(), "Ctrl+Shift+K");
        assert_eq!(chord("tab", true, Some(false)).label(), "Ctrl+Tab");
        assert_eq!(chord(",", true, Some(false)).label(), "Ctrl+,");
    }

    #[test]
    fn primary_shortcut_prefers_the_non_terminal_only_binding() {
        let bindings = default_bindings();
        assert_eq!(
            primary_shortcut(Action::CommandPalette, &bindings).as_deref(),
            Some("Ctrl+K")
        );
        assert_eq!(
            primary_shortcut(Action::PrimaryTab(3), &bindings).as_deref(),
            Some("Ctrl+3")
        );
        // `FocusFilter` has two `NormalOnly` bindings (`Ctrl+F`, `Ctrl+/`) and no
        // `TerminalOnly` one at all — the first non-`TerminalOnly` binding registered
        // wins, which is `Ctrl+F` (registration order in `default_bindings`).
        assert_eq!(
            primary_shortcut(Action::FocusFilter, &bindings).as_deref(),
            Some("Ctrl+F")
        );
    }
}
