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
    /// Switch to primary tab `1..=6` (SSH, Database, Network, Workspaces, System,
    /// Settings, in that order — see `app::Tab::ALL`). Any other value is simply
    /// never bound.
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
    /// Open Settings (`Tab::Settings` — round-E §C). A second way in besides
    /// `PrimaryTab(6)`/`Ctrl+6`; Settings → Keymap itself (rebinding UI) stays
    /// deferred, per the plan.
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
            Action::PrimaryTab(6) => "Go to Settings",
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

    /// The stable, persisted identity of this action — the key a user keybinding
    /// override is stored under (`sid_store::KeyBinding::action`).
    ///
    /// This string is **on disk**: renaming one silently drops that action's override
    /// (the row no longer parses, so [`parse_override`] skips it and the default wins).
    /// Add variants freely; never rename an existing id. `Action::label` is deliberately
    /// *not* reused for this — labels are prose and get reworded.
    pub fn id(self) -> &'static str {
        match self {
            Action::CommandPalette => "command_palette",
            Action::PrimaryTab(1) => "primary_tab_1",
            Action::PrimaryTab(2) => "primary_tab_2",
            Action::PrimaryTab(3) => "primary_tab_3",
            Action::PrimaryTab(4) => "primary_tab_4",
            Action::PrimaryTab(5) => "primary_tab_5",
            Action::PrimaryTab(6) => "primary_tab_6",
            // Unreachable through `ALL_ACTIONS`; an unbindable tab index gets no
            // persistable id rather than one that would collide with a real tab.
            Action::PrimaryTab(_) => "primary_tab_unbound",
            Action::CycleTabForward => "cycle_tab_forward",
            Action::CycleTabBack => "cycle_tab_back",
            Action::CycleSessionForward => "cycle_session_forward",
            Action::CycleSessionBack => "cycle_session_back",
            Action::NewSession => "new_session",
            Action::CloseSession => "close_session",
            Action::Settings => "settings",
            Action::CheatSheet => "cheat_sheet",
            Action::FocusFilter => "focus_filter",
        }
    }

    /// The inverse of [`Action::id`], over [`ALL_ACTIONS`]. `None` for an id this build
    /// doesn't know — a stored override for an action that was removed (or written by a
    /// newer sid) is skipped, never a decode error.
    pub fn from_id(id: &str) -> Option<Action> {
        ALL_ACTIONS.iter().copied().find(|a| a.id() == id)
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
    Action::PrimaryTab(6),
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

    for (n, digit) in [(1u8, "1"), (2, "2"), (3, "3"), (4, "4"), (5, "5"), (6, "6")] {
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

// ---- rebinding (Settings -> Keymap; pure, unit-tested) ---------------------------
//
// The rebinding editor's entire policy lives here, decided by pure functions over
// `(registry, action, chord)`; `ui::settings_tab` only renders the verdicts and writes
// the accepted ones to the store. Three rules carry the weight:
//
// 1. **A conflict is refused, never resolved.** A chord another action already owns is
//    rejected with that action's name — sid will not silently unbind something to make
//    room. Every action therefore always keeps at least one binding, which is what makes
//    "the user cannot strand themselves" a structural fact rather than a special case.
// 2. **The terminal keeps plain `Ctrl+<letter>`.** Not by blocklist: [`override_context`]
//    *derives* `NormalOnly` for every letter chord and mints its `Ctrl+Shift+<letter>`
//    in-terminal twin, exactly as the shipped defaults do. No override can be expressed
//    that claims a shell control code inside a focused terminal.
// 3. **Only [`REBINDABLE_KEYS`] exist.** The allowlist is the validator *and* the source
//    of the `&'static str` in a runtime-built [`Chord`], so neither a captured keystroke
//    nor a hand-edited store can produce a chord this app can't render or match.

/// Every key a user override may bind.
///
/// What's absent matters as much as what's present: `escape`, `enter`, `backspace`,
/// `delete`, the arrows and `space` belong to whichever widget has focus, and a registry
/// entry on one of them would shadow it app-wide with nothing to notice it by.
pub const REBINDABLE_KEYS: &[&str] = &[
    "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s",
    "t", "u", "v", "w", "x", "y", "z", //
    "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", //
    "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12", //
    "tab", "pageup", "pagedown", "home", "end", //
    ",", ".", "/", ";", "'", "[", "]", "\\", "-", "=", "`", "?",
];

/// Intern `key` into the [`REBINDABLE_KEYS`] allowlist, case-insensitively. `None` means
/// "not a key sid will bind" — the single validation gate every runtime-built [`Chord`]
/// passes through.
pub fn intern_key(key: &str) -> Option<&'static str> {
    REBINDABLE_KEYS
        .iter()
        .copied()
        .find(|k| k.eq_ignore_ascii_case(key))
}

/// Whether `key` is a single ASCII letter — the only key class that collides with a
/// shell control code, and so the only one that needs a terminal-focus fallback.
fn is_letter_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if c.is_ascii_alphabetic())
}

/// Whether `key` is a single non-alphanumeric character (`,`, `?`, `/`) — the class
/// whose *shifted* form gpui resolves into `key` itself, so demanding an exact shift
/// state would make the chord untypeable (see [`Chord::shift`]).
fn is_symbol_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if !c.is_ascii_alphanumeric())
}

/// The shift discipline for `key`: exact for letters/digits/named keys, ignored for
/// symbols.
fn shift_for(key: &str, shift_held: bool) -> Option<bool> {
    if is_symbol_key(key) {
        None
    } else {
        Some(shift_held)
    }
}

/// Turn a captured keystroke into a bindable [`Chord`], or say why it isn't one. The
/// `Err` is the message the editor shows verbatim.
pub fn chord_from_keystroke(k: &Keystroke) -> Result<Chord, &'static str> {
    let m = &k.modifiers;
    if m.alt || m.platform {
        return Err("shortcuts use Ctrl, optionally with Shift");
    }
    if !m.control {
        return Err("a shortcut must include Ctrl");
    }
    let key = intern_key(&k.key).ok_or("that key can't be used in a shortcut")?;
    Ok(Chord {
        key,
        ctrl: true,
        shift: shift_for(key, m.shift),
    })
}

/// Chords the rest of the app already owns, and why. A rebind may never take one.
///
/// The clipboard/editing set (`c`/`v`/`x`/`a`/`z`/`d`) is needed by every `TextInput`,
/// the SQL editor and the terminal's own copy/paste; `n`/`p` are claimed by the command
/// palette's navigation in `app::handle_root_key_down` *ahead of* this registry, so a
/// binding on one would silently not fire whenever the palette is open. Reserving the
/// letter covers its shifted form too — `Ctrl+Shift+C` is the terminal's copy.
const RESERVED_LETTERS: &[(&str, &str)] = &[
    ("c", "Ctrl+C is copy (and SIGINT in a shell)"),
    ("v", "Ctrl+V is paste"),
    ("x", "Ctrl+X is cut"),
    ("a", "Ctrl+A is select-all"),
    ("z", "Ctrl+Z is undo"),
    ("d", "Ctrl+D is end-of-input"),
    ("n", "Ctrl+N moves down the command palette"),
    ("p", "Ctrl+P moves up the command palette"),
];

/// Why `chord` is reserved, if it is. Keys the app can't bind at all (`escape`, `enter`,
/// the arrows) never reach this — they're not in [`REBINDABLE_KEYS`], so they come back
/// as [`RebindOutcome::Invalid`] instead.
fn reserved_reason(chord: &Chord) -> Option<&'static str> {
    if !chord.ctrl {
        return None;
    }
    RESERVED_LETTERS
        .iter()
        .find(|(k, _)| chord.key.eq_ignore_ascii_case(k))
        .map(|&(_, why)| why)
}

/// Which [`BindingContext`] a user override on `chord` gets.
///
/// The load-bearing rule, and the reason no blocklist is needed: a plain `Ctrl+<letter>`
/// is a shell control code, so an override on a letter is `NormalOnly` — the focused
/// terminal still gets the keystroke. Everything else (digits, Tab, the page keys,
/// symbols, function keys, and any shifted chord) never collides with readline and is
/// `Global`, exactly as the shipped defaults are.
pub fn override_context(chord: &Chord) -> BindingContext {
    if is_letter_key(chord.key) && chord.shift != Some(true) {
        BindingContext::NormalOnly
    } else {
        BindingContext::Global
    }
}

/// The full set of bindings a user override becomes: the chord itself, plus — for a
/// letter — the `Ctrl+Shift+<letter>` twin that reaches the action from inside a focused
/// terminal. Same shape the defaults hand-write for `Ctrl+K`/`Ctrl+T`/`Ctrl+W`.
pub fn expand_override(action: Action, chord: Chord) -> Vec<Binding> {
    let context = override_context(&chord);
    let mut out = vec![binding(chord, context, action)];
    if context == BindingContext::NormalOnly {
        out.push(binding(
            Chord {
                shift: Some(true),
                ..chord
            },
            BindingContext::TerminalOnly,
            action,
        ));
    }
    out
}

/// The result of asking to bind `chord` to `action`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RebindOutcome {
    /// The rebind is legal; these bindings replace **every** binding for that action.
    Applied { bindings: Vec<Binding> },
    /// Another action already owns that chord — nothing is changed, and the editor names
    /// `with` so the user can reset that action first. sid never steals a chord, because
    /// stealing means silently unbinding something the user can't see from here.
    Conflict { with: Action },
    /// The chord belongs to the rest of the app (clipboard, palette navigation, the
    /// shell) and can never be taken. `reason` is shown verbatim.
    Reserved { reason: &'static str },
    /// The chord isn't a bindable chord at all (no Ctrl, or a key outside
    /// [`REBINDABLE_KEYS`]). `reason` is shown verbatim.
    Invalid { reason: &'static str },
}

/// The first binding in `registry` that would race `candidate`, ignoring `candidate`'s
/// own action (rebinding an action onto a chord it already holds is a no-op, not a
/// conflict). Same predicate pair [`find_conflicts`] uses, asked about one candidate.
fn colliding_action(registry: &[Binding], candidate: &Binding) -> Option<Action> {
    registry
        .iter()
        .find(|b| {
            b.action != candidate.action
                && chords_collide(&b.chord, &candidate.chord)
                && contexts_overlap(b.context, candidate.context)
        })
        .map(|b| b.action)
}

/// Decide whether `action` may be bound to `chord`, given the currently effective
/// `registry`. Pure: it changes nothing and allocates only the expansion it hands back.
pub fn resolve_rebind(registry: &[Binding], action: Action, chord: Chord) -> RebindOutcome {
    if !chord.ctrl {
        return RebindOutcome::Invalid {
            reason: "a shortcut must include Ctrl",
        };
    }
    let Some(key) = intern_key(chord.key) else {
        return RebindOutcome::Invalid {
            reason: "that key can't be used in a shortcut",
        };
    };
    let chord = Chord { key, ..chord };
    if let Some(reason) = reserved_reason(&chord) {
        return RebindOutcome::Reserved { reason };
    }
    let bindings = expand_override(action, chord);
    for candidate in &bindings {
        if let Some(with) = colliding_action(registry, candidate) {
            return RebindOutcome::Conflict { with };
        }
    }
    RebindOutcome::Applied { bindings }
}

/// A user override that has been validated: the action id was known to this build and
/// the key interned. Produced by [`parse_override`] from a stored row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyOverride {
    pub action: Action,
    pub chord: Chord,
}

/// Parse one stored override row (`sid_store::KeyBinding`'s fields) into a
/// [`KeyOverride`]. `None` for anything this build can't trust — an unknown action id, a
/// key outside the allowlist, a chord without Ctrl — in which case that action simply
/// keeps its default. A store written by a newer sid degrades; it never fails to open.
pub fn parse_override(action_id: &str, key: &str, ctrl: bool, shift: bool) -> Option<KeyOverride> {
    if !ctrl {
        return None;
    }
    let action = Action::from_id(action_id)?;
    let key = intern_key(key)?;
    Some(KeyOverride {
        action,
        chord: Chord {
            key,
            ctrl: true,
            shift: shift_for(key, shift),
        },
    })
}

/// The one function that answers "what is bound right now": the defaults, with each
/// overridden action's defaults *replaced* (not shadowed) by its override's expansion.
///
/// Total by construction. An override that would make the registry ambiguous — not
/// reachable through the editor, which refuses conflicts, but reachable by hand-editing
/// the store — is dropped and that action keeps its default rather than the app becoming
/// ambiguous or panicking. Overrides are applied in action-id order so which one loses
/// is deterministic rather than dependent on redb's iteration.
pub fn effective_bindings(overrides: &[KeyOverride]) -> Vec<Binding> {
    if overrides.is_empty() {
        return default_bindings();
    }
    let mut overrides = overrides.to_vec();
    overrides.sort_by_key(|o| o.action.id());
    overrides.dedup_by_key(|o| o.action.id());

    let defaults = default_bindings();
    let mut out: Vec<Binding> = defaults
        .iter()
        .copied()
        .filter(|b| !overrides.iter().any(|o| o.action == b.action))
        .collect();

    let mut rejected = Vec::new();
    for o in &overrides {
        if !push_if_free(&mut out, expand_override(o.action, o.chord)) {
            rejected.push(o.action);
        }
    }
    for action in rejected {
        let restored = defaults
            .iter()
            .copied()
            .filter(|b| b.action == action)
            .collect();
        push_if_free(&mut out, restored);
    }
    out
}

/// Append `candidates` to `out` only if none of them would race something already there.
/// All-or-nothing: a half-applied override (the primary in, the terminal twin dropped)
/// would be a binding that works outside a terminal and vanishes inside one.
fn push_if_free(out: &mut Vec<Binding>, candidates: Vec<Binding>) -> bool {
    if candidates
        .iter()
        .any(|c| colliding_action(out, c).is_some())
    {
        return false;
    }
    out.extend(candidates);
    true
}

#[cfg(test)]
mod rebinding_tests {
    //! Settings -> Keymap's decision logic. Everything here is pure: no store, no
    //! window — the rebind editor's whole policy (validate, reserve, conflict, expand,
    //! compose) is decided by these functions and only rendered by `ui::settings_tab`.

    use super::*;
    use gpui::Modifiers;

    fn ks(key: &str, ctrl: bool, shift: bool) -> Keystroke {
        Keystroke {
            modifiers: Modifiers {
                control: ctrl,
                shift,
                ..Default::default()
            },
            key: key.to_string(),
            key_char: None,
        }
    }

    fn ctrl_chord(key: &'static str) -> Chord {
        Chord {
            key,
            ctrl: true,
            shift: Some(false),
        }
    }

    // ---- action ids are the persisted key ---------------------------------------

    #[test]
    fn every_action_id_round_trips() {
        for &action in ALL_ACTIONS {
            let id = action.id();
            assert!(!id.is_empty(), "{action:?} has no id");
            assert_eq!(
                Action::from_id(id),
                Some(action),
                "{id} must map back to {action:?}"
            );
        }
    }

    #[test]
    fn action_ids_are_unique() {
        let mut ids: Vec<&str> = ALL_ACTIONS.iter().map(|a| a.id()).collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), total, "two actions share a persisted id");
    }

    #[test]
    fn an_unknown_action_id_is_not_an_error() {
        assert_eq!(Action::from_id("from_a_newer_sid"), None);
    }

    // ---- key interning is the validation gate -----------------------------------

    #[test]
    fn intern_key_accepts_the_rebindable_set() {
        assert!(!REBINDABLE_KEYS.is_empty(), "nothing would be rebindable");
        for &key in REBINDABLE_KEYS {
            assert_eq!(intern_key(key), Some(key), "{key} must intern");
        }
        // Letters, digits, function keys, the page keys and the symbol row are the
        // shape the editor offers.
        for key in ["k", "9", "f5", "tab", "pagedown", ","] {
            assert!(intern_key(key).is_some(), "{key} should be rebindable");
        }
    }

    #[test]
    fn intern_key_is_case_insensitive() {
        assert_eq!(intern_key("K"), Some("k"));
        assert_eq!(intern_key("PageDown"), Some("pagedown"));
    }

    #[test]
    fn intern_key_rejects_keys_the_app_needs() {
        // Not a matter of taste: every one of these is a widget's own key (text entry,
        // list navigation, modal dismissal). A registry entry on one would shadow it
        // app-wide with no way to notice.
        for key in [
            "escape",
            "enter",
            "backspace",
            "delete",
            "up",
            "down",
            "left",
            "right",
            "space",
            "",
            "ctrl",
            "shift",
            "nonsense",
        ] {
            assert_eq!(intern_key(key), None, "{key:?} must not be rebindable");
        }
    }

    // ---- capture: keystroke -> chord ---------------------------------------------

    #[test]
    fn capture_requires_ctrl() {
        // A bare key is text. The bare-`?` cheat-sheet default is grandfathered as a
        // *default*; a user override always carries Ctrl so it can never eat typing.
        assert!(chord_from_keystroke(&ks("k", false, false)).is_err());
        assert!(chord_from_keystroke(&ks("k", false, true)).is_err());
    }

    #[test]
    fn capture_rejects_alt_and_platform_modifiers() {
        let mut k = ks("k", true, false);
        k.modifiers.alt = true;
        assert!(chord_from_keystroke(&k).is_err());
        let mut k = ks("k", true, false);
        k.modifiers.platform = true;
        assert!(chord_from_keystroke(&k).is_err());
    }

    #[test]
    fn capture_interns_the_key_and_keeps_shift_exact_for_letters() {
        assert_eq!(
            chord_from_keystroke(&ks("G", true, false)),
            Ok(Chord {
                key: "g",
                ctrl: true,
                shift: Some(false)
            })
        );
        assert_eq!(
            chord_from_keystroke(&ks("g", true, true)),
            Ok(Chord {
                key: "g",
                ctrl: true,
                shift: Some(true)
            })
        );
    }

    #[test]
    fn capture_treats_a_symbol_key_as_shift_agnostic() {
        // gpui's xkb glue resolves the *shifted* character into `key` itself, so
        // demanding an exact shift state on a symbol makes the binding untypeable on
        // layouts where the symbol needs a physical Shift (see `Chord::shift`).
        assert_eq!(
            chord_from_keystroke(&ks("?", true, true)),
            Ok(Chord {
                key: "?",
                ctrl: true,
                shift: None
            })
        );
    }

    #[test]
    fn capture_rejects_a_key_that_is_not_rebindable() {
        assert!(chord_from_keystroke(&ks("escape", true, false)).is_err());
        assert!(chord_from_keystroke(&ks("backspace", true, false)).is_err());
    }

    // ---- the terminal-passthrough invariant, made structural --------------------

    #[test]
    fn a_plain_ctrl_letter_override_is_normal_only() {
        // The load-bearing one: inside a focused terminal `Ctrl+<letter>` is a shell
        // control code. An override on a letter must therefore be NormalOnly — never
        // Global — or the user has just broken their own shell.
        assert_eq!(
            override_context(&ctrl_chord("g")),
            BindingContext::NormalOnly
        );
    }

    #[test]
    fn a_non_letter_override_is_global() {
        for key in ["9", "tab", "pagedown", ",", "f5"] {
            assert_eq!(
                override_context(&ctrl_chord(key)),
                BindingContext::Global,
                "{key} never collides with a shell control code"
            );
        }
    }

    #[test]
    fn expand_override_gives_a_letter_its_terminal_fallback() {
        let bindings = expand_override(Action::Settings, ctrl_chord("g"));
        assert_eq!(bindings.len(), 2, "primary + terminal fallback");
        assert_eq!(bindings[0].context, BindingContext::NormalOnly);
        assert_eq!(bindings[0].chord.label(), "Ctrl+G");
        assert_eq!(bindings[1].context, BindingContext::TerminalOnly);
        assert_eq!(bindings[1].chord.label(), "Ctrl+Shift+G");
        assert!(bindings.iter().all(|b| b.action == Action::Settings));
    }

    #[test]
    fn expand_override_gives_a_non_letter_no_fallback() {
        let bindings = expand_override(Action::Settings, ctrl_chord("9"));
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].context, BindingContext::Global);
    }

    // ---- resolve_rebind: the four outcomes ---------------------------------------

    #[test]
    fn a_free_chord_applies() {
        let registry = default_bindings();
        let outcome = resolve_rebind(&registry, Action::Settings, ctrl_chord("g"));
        assert_eq!(
            outcome,
            RebindOutcome::Applied {
                bindings: expand_override(Action::Settings, ctrl_chord("g"))
            }
        );
    }

    #[test]
    fn a_chord_owned_by_another_action_conflicts_and_names_it() {
        // The whole conflict policy: a chord already spoken for is REFUSED, and the
        // refusal names the owner so the user can go reset that one first. Nothing is
        // silently stolen and nothing is silently unbound.
        let registry = default_bindings();
        assert_eq!(
            resolve_rebind(&registry, Action::Settings, ctrl_chord("k")),
            RebindOutcome::Conflict {
                with: Action::CommandPalette
            }
        );
        assert_eq!(
            resolve_rebind(&registry, Action::CheatSheet, ctrl_chord("3")),
            RebindOutcome::Conflict {
                with: Action::PrimaryTab(3)
            }
        );
    }

    #[test]
    fn a_conflict_leaves_the_registry_untouched() {
        let registry = default_bindings();
        let before = registry.clone();
        let _ = resolve_rebind(&registry, Action::Settings, ctrl_chord("k"));
        assert_eq!(registry, before);
    }

    #[test]
    fn the_terminal_fallback_of_another_action_also_conflicts() {
        // `Ctrl+Shift+K` is CommandPalette's in-terminal fallback. Handing it to another
        // action would make the palette unreachable from a focused terminal.
        let registry = default_bindings();
        assert_eq!(
            resolve_rebind(
                &registry,
                Action::Settings,
                Chord {
                    key: "k",
                    ctrl: true,
                    shift: Some(true)
                }
            ),
            RebindOutcome::Conflict {
                with: Action::CommandPalette
            }
        );
    }

    #[test]
    fn rebinding_an_action_onto_its_own_chord_is_not_a_conflict() {
        let registry = default_bindings();
        assert!(matches!(
            resolve_rebind(&registry, Action::CommandPalette, ctrl_chord("k")),
            RebindOutcome::Applied { .. }
        ));
    }

    #[test]
    fn reserved_chords_are_refused_with_a_reason() {
        let registry = default_bindings();
        for key in ["c", "v", "x", "a", "z", "d", "n", "p"] {
            let outcome = resolve_rebind(&registry, Action::Settings, ctrl_chord(key));
            match outcome {
                RebindOutcome::Reserved { reason } => {
                    assert!(!reason.is_empty(), "Ctrl+{key} needs a stated reason")
                }
                other => panic!("Ctrl+{key} must be reserved, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_reserved_letter_is_reserved_in_its_shifted_form_too() {
        // Ctrl+Shift+C is the terminal's copy; reserving only the unshifted form would
        // leave the shifted one takeable.
        let registry = default_bindings();
        assert!(matches!(
            resolve_rebind(
                &registry,
                Action::Settings,
                Chord {
                    key: "c",
                    ctrl: true,
                    shift: Some(true)
                }
            ),
            RebindOutcome::Reserved { .. }
        ));
    }

    #[test]
    fn no_default_binding_sits_on_a_reserved_chord() {
        // The reservation table and the shipped defaults must agree: a default on a
        // reserved chord would be a binding the editor can describe but never restore.
        let registry = default_bindings();
        for b in &registry {
            if !b.chord.ctrl {
                continue;
            }
            let outcome = resolve_rebind(&registry, b.action, b.chord);
            assert!(
                !matches!(outcome, RebindOutcome::Reserved { .. }),
                "default {:?} sits on a reserved chord",
                b.chord
            );
        }
    }

    #[test]
    fn a_chord_without_ctrl_is_invalid() {
        let registry = default_bindings();
        assert!(matches!(
            resolve_rebind(
                &registry,
                Action::Settings,
                Chord {
                    key: "g",
                    ctrl: false,
                    shift: Some(false)
                }
            ),
            RebindOutcome::Invalid { .. }
        ));
    }

    #[test]
    fn an_unbindable_key_is_invalid() {
        let registry = default_bindings();
        assert!(matches!(
            resolve_rebind(&registry, Action::Settings, ctrl_chord("enter")),
            RebindOutcome::Invalid { .. }
        ));
    }

    // ---- effective bindings: default union override ------------------------------

    #[test]
    fn with_no_overrides_the_effective_registry_is_the_defaults() {
        assert_eq!(effective_bindings(&[]), default_bindings());
    }

    #[test]
    fn an_override_replaces_every_default_binding_for_its_action() {
        let over = parse_override("command_palette", "g", true, false).expect("parses");
        let bindings = effective_bindings(&[over]);
        // The new chord resolves...
        assert_eq!(
            resolve(&ks("g", true, false), FocusContext::Normal, &bindings),
            Some(Action::CommandPalette)
        );
        // ...its derived in-terminal fallback resolves...
        assert_eq!(
            resolve(&ks("g", true, true), FocusContext::Terminal, &bindings),
            Some(Action::CommandPalette)
        );
        // ...and the old chord is gone in BOTH contexts (no leftover default).
        assert_eq!(
            resolve(&ks("k", true, false), FocusContext::Normal, &bindings),
            None
        );
        assert_eq!(
            resolve(&ks("k", true, true), FocusContext::Terminal, &bindings),
            None
        );
        // Every other action is untouched.
        assert_eq!(
            resolve(&ks("3", true, false), FocusContext::Normal, &bindings),
            Some(Action::PrimaryTab(3))
        );
    }

    #[test]
    fn parse_override_rejects_what_it_cannot_trust() {
        assert!(parse_override("no_such_action", "g", true, false).is_none());
        assert!(parse_override("settings", "nonsense", true, false).is_none());
        assert!(
            parse_override("settings", "g", false, false).is_none(),
            "a stored chord without Ctrl was never reachable through the editor"
        );
    }

    #[test]
    fn parse_override_derives_shift_from_the_key_class() {
        let letter = parse_override("settings", "g", true, true).expect("parses");
        assert_eq!(letter.chord.shift, Some(true));
        let symbol = parse_override("settings", "?", true, true).expect("parses");
        assert_eq!(symbol.chord.shift, None, "symbols ignore shift");
    }

    #[test]
    fn the_effective_registry_is_conflict_free_even_when_two_overrides_collide() {
        // Not reachable through the editor (the second rebind would be refused), but a
        // hand-edited or half-written store must not make the app ambiguous — and must
        // never panic. The loser keeps its default, so no action ends up unbound.
        let a = parse_override("settings", "g", true, false).expect("parses");
        let b = parse_override("cheat_sheet", "g", true, false).expect("parses");
        let bindings = effective_bindings(&[a, b]);
        assert!(
            find_conflicts(&bindings).is_empty(),
            "effective registry must stay unambiguous: {:?}",
            find_conflicts(&bindings)
        );
        for &action in ALL_ACTIONS {
            assert!(
                bindings.iter().any(|x| x.action == action),
                "{action:?} lost every binding"
            );
        }
    }

    // ---- the two safety properties, over the whole rebindable space --------------

    #[test]
    fn no_single_rebind_can_strand_an_action_or_break_the_registry() {
        let defaults = default_bindings();
        for &action in ALL_ACTIONS {
            for &key in REBINDABLE_KEYS {
                let chord = Chord {
                    key,
                    ctrl: true,
                    shift: Some(false),
                };
                let RebindOutcome::Applied { .. } = resolve_rebind(&defaults, action, chord) else {
                    continue; // refused outcomes change nothing at all
                };
                let over = KeyOverride { action, chord };
                let bindings = effective_bindings(&[over]);
                assert!(
                    find_conflicts(&bindings).is_empty(),
                    "{action:?} -> {key} made the registry ambiguous"
                );
                for &other in ALL_ACTIONS {
                    assert!(
                        bindings.iter().any(|b| b.action == other),
                        "{action:?} -> {key} left {other:?} unbound"
                    );
                }
                // Settings stays reachable by keyboard, always (the brief's brick).
                assert!(
                    bindings.iter().any(|b| b.action == Action::Settings)
                        && bindings.iter().any(|b| b.action == Action::PrimaryTab(6)),
                    "{action:?} -> {key} stranded Settings"
                );
            }
        }
    }

    #[test]
    fn no_rebind_can_take_a_plain_ctrl_letter_away_from_a_focused_terminal() {
        // The invariant the whole app rests on: inside a terminal, plain Ctrl+<letter>
        // must reach the PTY. Checked for EVERY applicable override, not just the
        // defaults — the expansion rule, not a blocklist, is what guarantees it.
        let defaults = default_bindings();
        let letters = "abcdefghijklmnopqrstuvwxyz";
        for &action in ALL_ACTIONS {
            for &key in REBINDABLE_KEYS {
                let chord = Chord {
                    key,
                    ctrl: true,
                    shift: Some(false),
                };
                let RebindOutcome::Applied { .. } = resolve_rebind(&defaults, action, chord) else {
                    continue;
                };
                let bindings = effective_bindings(&[KeyOverride { action, chord }]);
                for letter in letters.chars() {
                    let stroke = ks(&letter.to_string(), true, false);
                    assert_eq!(
                        resolve(&stroke, FocusContext::Terminal, &bindings),
                        None,
                        "{action:?} -> {key} stole Ctrl+{letter} from the terminal"
                    );
                }
            }
        }
    }
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
    fn ctrl_1_through_6_switch_primary_tabs_in_both_contexts() {
        let bindings = default_bindings();
        for (digit, n) in [("1", 1), ("2", 2), ("3", 3), ("4", 4), ("5", 5), ("6", 6)] {
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
