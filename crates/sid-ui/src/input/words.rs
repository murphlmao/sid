//! Word boundaries — where ctrl-backspace stops.
//!
//! # Why this is here
//!
//! sid's old hand-rolled field ([`crates/sid/src/ui/text_input.rs`]) bound `WordLeft`
//! and `WordRight` and **no word-delete at all**, so ctrl-backspace in any sid field
//! deleted a single character — the complaint filed as "support for std ctrl
//! operations". The replacement in [`super`] fixes that by construction, because
//! `gpui_component::InputState` already binds the whole family on Linux:
//!
//! | Chord | What it does |
//! |---|---|
//! | `ctrl-backspace` | delete to the previous word start |
//! | `ctrl-delete` | delete to the next word end |
//! | `ctrl-left` / `ctrl-right` | move by word |
//! | `ctrl-shift-left` / `ctrl-shift-right` | extend the selection by word |
//!
//! This module is the *rule those chords follow*, written down where it can be tested.
//! The library's implementation lives behind `pub(super)` methods on `InputState` and
//! needs a live `Window` to exercise, so the behaviour sid promises would otherwise be
//! untestable and undocumented — which is how the old widget shipped a word-motion
//! binding set with a hole in it. Both sides segment with the same crate and the same
//! UAX#29 rule, so this is a statement of the contract rather than a second opinion,
//! and it is the rule any *other* sid surface that moves by word (a breadcrumb, a
//! filter box, the terminal's own `ctrl-w`) should use so the app has one answer.
//!
//! # The rule
//!
//! Segment the text into UAX#29 word bounds, then:
//!
//! - **backwards** ([`prev_word_start`]): skip the whitespace immediately behind the
//!   caret, then jump to the *start* of the word behind that. So in `foo    |bar` the
//!   whole of `foo    ` goes, not just the spaces — the behaviour GTK and Firefox have
//!   and the one the library implements.
//! - **forwards** ([`next_word_end`]): skip the whitespace ahead of the caret, then jump
//!   to the *end* of the word after it.
//!
//! Segments, not `char`s, is the load-bearing part: `é` written as `e` + U+0301 is one
//! cluster, and a ZWJ emoji sequence (U+1F469 U+200D U+1F4BB is one picture made of two
//! scalars and a joiner) is another. A rule written over `char` boundaries lands
//! *inside* both. Every offset returned here is a `char` boundary and never splits a
//! grapheme.
//!
//! Two consequences of UAX#29 worth knowing before reading the tests, because both are
//! surprising and neither is a bug:
//!
//! - **A dot between letters does not break** (WB6/WB7), so `db.internal` is one word
//!   and one chord takes the whole hostname. A `/` or a `:` is not covered by that rule,
//!   so a path or a DSN walks one component at a time.
//! - **CJK has no spaces to key off**, so each Han ideograph is its own word and a chord
//!   moves one character. Kana runs stay together.

use unicode_segmentation::UnicodeSegmentation as _;

/// The byte offset ctrl-backspace (or ctrl-shift-left) moves to, from `caret`.
///
/// Returns `0` at the start of the text and for a caret in leading whitespace — there is
/// nothing further back to delete, and returning something else would be a silent
/// no-op the caller has to detect. `caret` is clamped into `text` and snapped down to a
/// `char` boundary, so a caller that has lost track of the two cannot panic this.
pub fn prev_word_start(text: &str, caret: usize) -> usize {
    let caret = clamp_boundary(text, caret);
    text[..caret]
        .split_word_bound_indices()
        .rfind(|(_, segment)| !is_blank(segment))
        .map_or(0, |(at, _)| at)
}

/// The byte offset ctrl-delete (or ctrl-shift-right) moves to, from `caret`.
///
/// Returns `text.len()` at the end of the text and for a caret in trailing whitespace.
pub fn next_word_end(text: &str, caret: usize) -> usize {
    let caret = clamp_boundary(text, caret);
    text[caret..]
        .split_word_bound_indices()
        .find(|(_, segment)| !is_blank(segment))
        .map_or(text.len(), |(at, segment)| caret + at + segment.len())
}

/// Whether a segment is only whitespace — the segments both directions skip over.
///
/// UAX#29 emits a run of spaces as **one** segment, so "skip the whitespace" is a single
/// step rather than a loop, and `foo\t \n bar` behaves like `foo bar`.
fn is_blank(segment: &str) -> bool {
    segment.chars().all(char::is_whitespace)
}

/// `caret`, clamped to `text` and snapped **down** to the nearest `char` boundary.
///
/// Snapping down rather than panicking matters because the offsets in play come from a
/// live editor: a caret that arrives mid-cluster (a stale index after an IME commit, a
/// caller measuring in UTF-16) should cost a few bytes of motion, not a crash.
fn clamp_boundary(text: &str, caret: usize) -> usize {
    let mut caret = caret.min(text.len());
    while caret > 0 && !text.is_char_boundary(caret) {
        caret -= 1;
    }
    caret
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every offset this module returns has to be a `char` boundary, or the caller
    /// slices a `String` at it and panics. Asserted on every case, not just the
    /// interesting ones.
    fn boundary(text: &str, at: usize) -> usize {
        assert!(
            text.is_char_boundary(at),
            "{at} is inside a character of {text:?}"
        );
        at
    }

    #[test]
    fn a_word_at_a_time_backwards() {
        let text = "hello world again";
        assert_eq!(boundary(text, prev_word_start(text, 17)), 12);
        assert_eq!(boundary(text, prev_word_start(text, 12)), 6);
        assert_eq!(boundary(text, prev_word_start(text, 6)), 0);
    }

    #[test]
    fn a_word_at_a_time_forwards() {
        let text = "hello world again";
        assert_eq!(boundary(text, next_word_end(text, 0)), 5);
        assert_eq!(boundary(text, next_word_end(text, 5)), 11);
        assert_eq!(boundary(text, next_word_end(text, 11)), 17);
    }

    #[test]
    fn the_ends_of_the_text_are_fixed_points() {
        // A chord that does nothing must *land* somewhere sane: at 0 going left and at
        // len going right. Anything else and the caller either slices backwards or
        // cannot tell "nothing to do" from "something went wrong".
        let text = "hello";
        assert_eq!(prev_word_start(text, 0), 0);
        assert_eq!(next_word_end(text, 5), 5);
        assert_eq!(prev_word_start("", 0), 0);
        assert_eq!(next_word_end("", 0), 0);
    }

    #[test]
    fn a_run_of_whitespace_goes_with_the_word_behind_it() {
        // THE case the rule has to state explicitly, because editors disagree: at
        // `foo    |bar`, does ctrl-backspace eat the spaces or the spaces *and* `foo`?
        // sid answers "and foo", which is GTK/Firefox behaviour and is what the
        // library's `previous_start_of_word` does — so this test is also the check
        // that the two have not drifted.
        let text = "foo    bar";
        assert_eq!(prev_word_start(text, 7), 0);
        // Tabs and newlines are the same whitespace as far as the rule is concerned.
        let mixed = "foo \t\n bar";
        assert_eq!(prev_word_start(mixed, 7), 0);
        // Forwards, symmetrically: the gap and the word after it go together.
        assert_eq!(next_word_end(text, 3), 10);
    }

    #[test]
    fn trailing_whitespace_does_not_strand_the_caret() {
        let text = "foo bar   ";
        assert_eq!(prev_word_start(text, 10), 4, "the spaces and `bar`");
        assert_eq!(next_word_end(text, 7), 10, "nothing but spaces ahead");
        assert_eq!(prev_word_start("   ", 3), 0, "nothing but spaces behind");
    }

    #[test]
    fn a_separator_is_its_own_word_but_an_infix_dot_is_not() {
        // The surprising half of UAX#29, and the reason this is pinned rather than
        // assumed: WB6/WB7 keep a full stop *between letters* inside the word, so
        // `db.internal` is ONE segment and one chord takes the whole hostname. A path
        // separator is not covered by that rule, so `/usr/lib/systemd` is six segments
        // and ctrl-backspace walks it one component at a time.
        let host = "foo.bar";
        assert_eq!(
            boundary(host, prev_word_start(host, host.len())),
            0,
            "a dotted host is one word"
        );
        let path = "/usr/lib/systemd";
        assert_eq!(boundary(path, prev_word_start(path, path.len())), 9);
        assert_eq!(boundary(path, prev_word_start(path, 9)), 8, "the slash");
        assert_eq!(boundary(path, prev_word_start(path, 8)), 5);
    }

    #[test]
    fn a_connection_string_walks_in_the_pieces_a_typo_lives_in() {
        // The practical test of the rule: fixing the port in a DSN should not cost the
        // whole line. Each chord stops at a piece a user might have got wrong.
        let dsn = "postgres://db.internal:5432/app";
        let mut stops = Vec::new();
        let mut at = dsn.len();
        while at > 0 {
            at = boundary(dsn, prev_word_start(dsn, at));
            stops.push(at);
        }
        assert_eq!(stops, vec![28, 27, 23, 22, 11, 10, 9, 8, 0]);
        assert_eq!(&dsn[23..27], "5432", "the port is reachable in one chord");
    }

    #[test]
    fn an_accented_cluster_is_never_split() {
        // `é` as `e` + U+0301 (combining acute) is two scalars and one grapheme. A rule
        // written over `char`s lands between them and the caller slices a broken string.
        let text = "cafe\u{301} wo\u{308}rld";
        let back = boundary(text, prev_word_start(text, text.len()));
        assert_eq!(&text[back..], "wo\u{308}rld");
        let fwd = boundary(text, next_word_end(text, 0));
        assert_eq!(&text[..fwd], "cafe\u{301}");
    }

    #[test]
    fn a_zwj_emoji_sequence_is_one_unit() {
        // U+1F469 U+200D U+1F4BB — "woman technologist", three scalars joined by a ZWJ.
        // Any offset inside it is a valid `char` boundary, so the boundary assertion
        // alone would not catch a split: the test has to name the whole cluster.
        let emoji = "\u{1F469}\u{200D}\u{1F4BB}";
        let text = format!("hi {emoji}");
        let back = boundary(&text, prev_word_start(&text, text.len()));
        assert_eq!(&text[back..], emoji, "the joiner was cut");
        let fwd = boundary(&text, next_word_end(&text, 0));
        assert_eq!(&text[..fwd], "hi");
    }

    #[test]
    fn cjk_moves_one_ideograph_at_a_time() {
        // UAX#29 gives each ideograph its own word bound (there are no spaces to key
        // off), so a chord moves one character. Stated as a test rather than left to be
        // discovered, because it is the one place the rule is not "a whole word".
        let text = "日本語";
        assert_eq!(boundary(text, prev_word_start(text, 9)), 6);
        assert_eq!(boundary(text, next_word_end(text, 0)), 3);
    }

    #[test]
    fn an_out_of_range_or_mid_character_caret_cannot_panic() {
        // These offsets come from a live editor across an IME commit and a UTF-16
        // conversion; a stale one must cost a few bytes of motion, not a crash.
        let text = "caf\u{e9} bar";
        assert_eq!(
            prev_word_start(text, 9_999),
            prev_word_start(text, text.len())
        );
        assert_eq!(next_word_end(text, 9_999), text.len());
        // Byte 4 is inside the two-byte `é`: snapped down to 3, which is inside `café`,
        // so the word start is still 0.
        assert_eq!(boundary(text, prev_word_start(text, 4)), 0);
        assert_eq!(boundary(text, next_word_end(text, 4)), 5);
    }

    #[test]
    fn walking_the_whole_string_terminates_and_covers_it() {
        // A boundary rule that can return its own input loops forever in a caller that
        // repeats the chord. Both directions must make progress from every position.
        for text in [
            "hello world",
            "foo    bar",
            "/usr/lib/systemd",
            "postgres://db.internal:5432/app",
            "日本語 テスト",
            "caf\u{e9}  \u{1F469}\u{200D}\u{1F4BB}",
            "   ",
            "",
        ] {
            let mut at = text.len();
            let mut steps = 0;
            while at > 0 {
                let next = prev_word_start(text, at);
                assert!(next < at, "{text:?}: stuck at {at}");
                at = boundary(text, next);
                steps += 1;
                assert!(steps < 100, "{text:?}: no progress");
            }
            let mut at = 0;
            let mut steps = 0;
            while at < text.len() {
                let next = next_word_end(text, at);
                assert!(next > at, "{text:?}: stuck at {at}");
                at = boundary(text, next);
                steps += 1;
                assert!(steps < 100, "{text:?}: no progress");
            }
        }
    }
}
