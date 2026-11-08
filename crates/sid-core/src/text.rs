//! Pure text-safety predicates shared by anything that shows untrusted strings.
//!
//! `sid-ui` cannot depend on this crate (it may not know the domain), so
//! `sid-ui::toast` carries a copy of [`is_deceptive`]; keep the two in step.

/// Characters that change how text is *displayed* without being visible: bidi
/// embeddings/overrides/isolates and marks (U+202A-202E, U+2066-2069, U+200E/F, U+061C),
/// zero-width space / joiners (U+200B-200D), word joiner and BOM (U+2060, U+FEFF).
/// A notice built from repo-committed names could use them to reorder or hide text.
pub fn is_deceptive(c: char) -> bool {
    matches!(
        c,
        '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{200B}'..='\u{200F}'
            | '\u{061C}'
            | '\u{2060}'
            | '\u{FEFF}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_bidi_and_zero_width() {
        for c in [
            '\u{202E}', '\u{2066}', '\u{2069}', '\u{200E}', '\u{200F}', '\u{200D}', '\u{FEFF}',
        ] {
            assert!(is_deceptive(c), "{c:?}");
        }
        assert!(!is_deceptive('a') && !is_deceptive('é') && !is_deceptive('日'));
    }
}
