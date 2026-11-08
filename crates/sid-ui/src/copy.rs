//! Copy voice — the one text transform sid's components apply to the words they are given.
//!
//! v1 copy was lowercase everywhere (`connect`, `add a connection`, `no connection
//! selected`) with UPPERCASE section headers, and the two together read terse and
//! machine-like. Design system v2 speaks in **sentence case**. Rather than ask ~120 call
//! sites to retype their strings, the components that own a *UI-vocabulary* slot — a
//! button's label, a card's title, an empty state's headline, a modal's or toast's title,
//! a menu item — pass it through [`sentence_case`]. The voice is then a property of the
//! component, the same way its size is.
//!
//! Data is never re-cased: a host alias, a path, a theme name or a table cell goes into a
//! slot that does not call this, so `prod-db` stays `prod-db`.

/// Capitalise the first letter and leave everything else exactly as written.
///
/// Deliberately minimal. It does not lowercase the rest (`SSH keys` stays `SSH keys`,
/// `Add SFTP bookmark` keeps its acronym), and it skips leading punctuation and digits so
/// `"+ add"` and `"2 hosts"` are left alone rather than mangled — only a leading
/// lowercase *letter* changes.
pub fn sentence_case(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) if first.is_lowercase() => first.to_uppercase().chain(chars).collect(),
        _ => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lowercase_label_gains_a_capital_and_nothing_else_moves() {
        assert_eq!(sentence_case("add connection"), "Add connection");
        assert_eq!(sentence_case("unlock with sudo"), "Unlock with sudo");
        assert_eq!(sentence_case("browse…"), "Browse…");
        assert_eq!(sentence_case("hidden (3)"), "Hidden (3)");
    }

    #[test]
    fn existing_capitals_and_acronyms_survive() {
        assert_eq!(sentence_case("Connect"), "Connect");
        assert_eq!(sentence_case("SSH keys"), "SSH keys");
        assert_eq!(sentence_case("open in SFTP"), "Open in SFTP");
    }

    #[test]
    fn a_leading_non_letter_is_left_alone() {
        assert_eq!(sentence_case(""), "");
        assert_eq!(sentence_case("+ add"), "+ add");
        assert_eq!(sentence_case("2 hosts"), "2 hosts");
        assert_eq!(sentence_case("\"foo\" not found"), "\"foo\" not found");
    }

    #[test]
    fn a_non_ascii_first_letter_is_capitalised_correctly() {
        assert_eq!(sentence_case("élan"), "Élan");
    }
}
