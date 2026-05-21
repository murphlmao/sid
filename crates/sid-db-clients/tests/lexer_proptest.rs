//! Property tests for the SQL lexer. Per CLAUDE.md, parser-shaped code is a
//! `cargo fuzz` target; until fuzzing is wired into CI, this proptest serves
//! the same purpose: assert that the lexer never panics and never hangs on
//! arbitrary inputs.

use proptest::prelude::*;
use sid_db_clients::lexer::tokenize;

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1024,
        ..ProptestConfig::default()
    })]

    /// Arbitrary UTF-8 strings up to 4 KiB never panic the lexer.
    #[test]
    fn prop_tokenize_never_panics_on_utf8(s in ".{0,4096}") {
        let _ = tokenize(&s);
    }

    /// Arbitrary byte sequences (lossy-decoded) never panic.
    #[test]
    fn prop_tokenize_never_panics_on_lossy_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let s = String::from_utf8_lossy(&bytes);
        let _ = tokenize(&s);
    }

    /// Concatenation invariant: token texts joined equal the input.
    #[test]
    fn prop_token_texts_concat_equals_input(s in ".{0,2048}") {
        let toks = tokenize(&s);
        let recon: String = toks.iter().map(|t| t.text.as_ref()).collect();
        prop_assert_eq!(recon, s);
    }

    /// Token offsets are monotonically non-decreasing and within bounds.
    #[test]
    fn prop_token_offsets_monotone(s in ".{0,2048}") {
        let toks = tokenize(&s);
        let mut last = 0;
        for t in &toks {
            prop_assert!(t.offset >= last, "offset {} < last {}", t.offset, last);
            prop_assert!(t.offset <= s.len());
            last = t.offset;
        }
    }
}
