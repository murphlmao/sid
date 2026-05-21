//! Proptest harness — verify parsers never panic on adversarial input.
//! CLAUDE.md mandates this for any parser-shaped function.

use proptest::prelude::*;
use sid_core::adapters::systemctl::UnitBus;
use sid_system::parse::{parse_journal, parse_list_units, parse_status, parse_unit_state};

proptest! {
    #![proptest_config(ProptestConfig { cases: 4096, ..ProptestConfig::default() })]

    #[test]
    fn list_units_parser_never_panics_on_arbitrary_str(s in ".*") {
        let _ = parse_list_units(&s, UnitBus::User);
    }

    #[test]
    fn list_units_parser_never_panics_on_arbitrary_bytes(b in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let s = String::from_utf8_lossy(&b);
        let _ = parse_list_units(&s, UnitBus::User);
    }

    #[test]
    fn unit_state_is_total(s in ".*") {
        let _ = parse_unit_state(&s);
    }

    #[test]
    fn status_parser_never_panics_on_arbitrary_str(s in ".*", name in "[a-z0-9.-]{1,40}") {
        let _ = parse_status(&s, &name, UnitBus::User);
    }

    #[test]
    fn status_parser_never_panics_on_arbitrary_bytes(
        b in proptest::collection::vec(any::<u8>(), 0..2048),
        name in "[a-z0-9.-]{1,40}",
    ) {
        let s = String::from_utf8_lossy(&b);
        let _ = parse_status(&s, &name, UnitBus::User);
    }

    #[test]
    fn journal_parser_never_panics_on_arbitrary_str(s in ".*") {
        let _ = parse_journal(&s);
    }

    #[test]
    fn journal_parser_never_panics_on_arbitrary_bytes(
        b in proptest::collection::vec(any::<u8>(), 0..4096),
    ) {
        let s = String::from_utf8_lossy(&b);
        let _ = parse_journal(&s);
    }
}
