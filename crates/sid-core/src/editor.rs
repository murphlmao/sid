//! Editor selection for the System-tab config-editing flow.
//!
//! Lives in `sid-core` as a plain, behaviour-light data type so widgets
//! (the Settings tab choice toggle), the store (persisted under
//! `settings_keys::CONFIG_EDITOR`), and the binary (which actually resolves
//! the editor binary and shells out) all reference the same shape without
//! pulling in any external crate.
//!
//! The string forms here are the canonical persisted representation: the
//! Settings choice toggle offers exactly `nano`/`vim`/`vi`/`terminal`, the
//! store writes the lowercase string, and [`EditorChoice::from_setting`]
//! round-trips it back. Unknown / legacy / empty values degrade to the
//! [`EditorChoice::default`] (`Nano`) rather than erroring, so a corrupted or
//! hand-edited setting never blocks config editing.

use serde::{Deserialize, Serialize};

/// Which editor the System tab uses when opening a config file.
///
/// `Nano`, `Vim`, and `Vi` are *inline* editors: the binary suspends the TUI
/// and runs them in-place against the current terminal. `Terminal` instead
/// spawns the user's terminal emulator with their configured editor, leaving
/// sid running underneath.
///
/// Persisted as the lowercase variant name (`"nano"` / `"vim"` / `"vi"` /
/// `"terminal"`) via [`EditorChoice::as_setting`]; decoded — case-insensitively
/// and lenient about unknown values — via [`EditorChoice::from_setting`].
///
/// # Examples
///
/// ```
/// use sid_core::editor::EditorChoice;
///
/// assert_eq!(EditorChoice::default(), EditorChoice::Nano);
/// assert_eq!(EditorChoice::Vim.as_setting(), "vim");
/// assert_eq!(EditorChoice::from_setting("VIM"), EditorChoice::Vim);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EditorChoice {
    /// The `nano` editor, run inline. The default — present on virtually every
    /// system and forgiving for casual edits.
    #[default]
    Nano,
    /// The `vim` editor, run inline.
    Vim,
    /// The `vi` editor, run inline. The POSIX fallback when `vim` is absent.
    Vi,
    /// Spawn the user's terminal emulator with their configured editor, leaving
    /// sid running in the background.
    Terminal,
}

impl EditorChoice {
    /// Decode a persisted setting string into an [`EditorChoice`].
    ///
    /// Matching is case-insensitive (the input is lowercased first). Any value
    /// that is not one of the four canonical strings — including the empty
    /// string and legacy / hand-edited garbage — maps to the default
    /// ([`EditorChoice::Nano`]) rather than failing. This keeps a corrupted
    /// setting from blocking config editing entirely.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::editor::EditorChoice;
    ///
    /// assert_eq!(EditorChoice::from_setting("nano"), EditorChoice::Nano);
    /// assert_eq!(EditorChoice::from_setting("Vim"), EditorChoice::Vim);
    /// assert_eq!(EditorChoice::from_setting("vi"), EditorChoice::Vi);
    /// assert_eq!(EditorChoice::from_setting("terminal"), EditorChoice::Terminal);
    /// // Unknown / empty -> default.
    /// assert_eq!(EditorChoice::from_setting(""), EditorChoice::Nano);
    /// assert_eq!(EditorChoice::from_setting("emacs"), EditorChoice::Nano);
    /// ```
    pub fn from_setting(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "nano" => Self::Nano,
            "vim" => Self::Vim,
            "vi" => Self::Vi,
            "terminal" => Self::Terminal,
            _ => Self::default(),
        }
    }

    /// The canonical lowercase string this choice persists as.
    ///
    /// This is the exact value written to `settings_keys::CONFIG_EDITOR` and
    /// the exact option string the Settings choice toggle offers, so
    /// `from_setting(x.as_setting()) == x` holds for every variant.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::editor::EditorChoice;
    ///
    /// assert_eq!(EditorChoice::Nano.as_setting(), "nano");
    /// assert_eq!(EditorChoice::Vim.as_setting(), "vim");
    /// assert_eq!(EditorChoice::Vi.as_setting(), "vi");
    /// assert_eq!(EditorChoice::Terminal.as_setting(), "terminal");
    /// ```
    pub fn as_setting(&self) -> &'static str {
        match self {
            Self::Nano => "nano",
            Self::Vim => "vim",
            Self::Vi => "vi",
            Self::Terminal => "terminal",
        }
    }

    /// Whether this choice spawns a separate terminal emulator rather than
    /// running inline.
    ///
    /// The binary branches on this: `true` means "spawn a terminal", `false`
    /// means "suspend the TUI and run [`inline_binary`](Self::inline_binary)
    /// in-place".
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::editor::EditorChoice;
    ///
    /// assert!(EditorChoice::Terminal.is_terminal());
    /// assert!(!EditorChoice::Nano.is_terminal());
    /// assert!(!EditorChoice::Vim.is_terminal());
    /// ```
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Terminal)
    }

    /// The binary name to run inline for the inline editors, or `None` for
    /// [`EditorChoice::Terminal`].
    ///
    /// `Some("nano" | "vim" | "vi")` is the program the binary spawns after
    /// suspending the TUI. `Terminal` returns `None` because its editor binary
    /// is resolved separately (via the terminal-command setting / `$EDITOR`),
    /// not from this enum.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_core::editor::EditorChoice;
    ///
    /// assert_eq!(EditorChoice::Nano.inline_binary(), Some("nano"));
    /// assert_eq!(EditorChoice::Vim.inline_binary(), Some("vim"));
    /// assert_eq!(EditorChoice::Vi.inline_binary(), Some("vi"));
    /// assert_eq!(EditorChoice::Terminal.inline_binary(), None);
    /// ```
    pub fn inline_binary(&self) -> Option<&'static str> {
        match self {
            Self::Nano => Some("nano"),
            Self::Vim => Some("vim"),
            Self::Vi => Some("vi"),
            Self::Terminal => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [EditorChoice; 4] = [
        EditorChoice::Nano,
        EditorChoice::Vim,
        EditorChoice::Vi,
        EditorChoice::Terminal,
    ];

    #[test]
    fn default_is_nano() {
        assert_eq!(EditorChoice::default(), EditorChoice::Nano);
    }

    #[test]
    fn as_setting_then_from_setting_round_trips_all_variants() {
        for choice in ALL {
            assert_eq!(
                EditorChoice::from_setting(choice.as_setting()),
                choice,
                "round-trip failed for {choice:?}"
            );
        }
    }

    #[test]
    fn as_setting_values_are_canonical_lowercase() {
        assert_eq!(EditorChoice::Nano.as_setting(), "nano");
        assert_eq!(EditorChoice::Vim.as_setting(), "vim");
        assert_eq!(EditorChoice::Vi.as_setting(), "vi");
        assert_eq!(EditorChoice::Terminal.as_setting(), "terminal");
    }

    #[test]
    fn from_setting_known_lowercase_values() {
        assert_eq!(EditorChoice::from_setting("nano"), EditorChoice::Nano);
        assert_eq!(EditorChoice::from_setting("vim"), EditorChoice::Vim);
        assert_eq!(EditorChoice::from_setting("vi"), EditorChoice::Vi);
        assert_eq!(
            EditorChoice::from_setting("terminal"),
            EditorChoice::Terminal
        );
    }

    #[test]
    fn from_setting_is_case_insensitive() {
        assert_eq!(EditorChoice::from_setting("NANO"), EditorChoice::Nano);
        assert_eq!(EditorChoice::from_setting("Vim"), EditorChoice::Vim);
        assert_eq!(EditorChoice::from_setting("Vi"), EditorChoice::Vi);
        assert_eq!(
            EditorChoice::from_setting("TeRmInAl"),
            EditorChoice::Terminal
        );
    }

    #[test]
    fn from_setting_unknown_and_empty_default_to_nano() {
        assert_eq!(EditorChoice::from_setting(""), EditorChoice::Nano);
        assert_eq!(EditorChoice::from_setting("   "), EditorChoice::Nano);
        assert_eq!(EditorChoice::from_setting("emacs"), EditorChoice::Nano);
        assert_eq!(EditorChoice::from_setting("notepad"), EditorChoice::Nano);
        // A near-miss that must not partial-match.
        assert_eq!(EditorChoice::from_setting("vims"), EditorChoice::Nano);
    }

    #[test]
    fn is_terminal_only_for_terminal() {
        assert!(EditorChoice::Terminal.is_terminal());
        assert!(!EditorChoice::Nano.is_terminal());
        assert!(!EditorChoice::Vim.is_terminal());
        assert!(!EditorChoice::Vi.is_terminal());
    }

    #[test]
    fn inline_binary_maps_inline_editors_and_none_for_terminal() {
        assert_eq!(EditorChoice::Nano.inline_binary(), Some("nano"));
        assert_eq!(EditorChoice::Vim.inline_binary(), Some("vim"));
        assert_eq!(EditorChoice::Vi.inline_binary(), Some("vi"));
        assert_eq!(EditorChoice::Terminal.inline_binary(), None);
    }

    #[test]
    fn inline_binary_is_some_iff_not_terminal() {
        for choice in ALL {
            assert_eq!(
                choice.inline_binary().is_some(),
                !choice.is_terminal(),
                "inline_binary/is_terminal disagree for {choice:?}"
            );
        }
    }

    #[test]
    fn serde_round_trips_as_variant_name() {
        for choice in ALL {
            let json = serde_json::to_string(&choice).expect("serialize");
            let back: EditorChoice = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, choice);
        }
        // Wire form is the PascalCase variant name (no rename_all), matching
        // the sibling animation enums.
        assert_eq!(
            serde_json::to_string(&EditorChoice::Nano).unwrap(),
            "\"Nano\""
        );
        assert_eq!(
            serde_json::to_string(&EditorChoice::Terminal).unwrap(),
            "\"Terminal\""
        );
    }
}
