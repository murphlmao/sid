use sid_system::parse::parse_journal;

const SAMPLE: &str = "\
2026-05-21T08:30:11+0000 myhost nginx[12345]: starting up
2026-05-21T08:30:12+0000 myhost nginx[12345]: ready to accept connections
2026-05-21T08:30:13+0000 myhost systemd[1]: Started nginx.service.
";

#[test]
fn parses_three_journal_lines() {
    let entries = parse_journal(SAMPLE).unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].hostname, "myhost");
    assert_eq!(entries[0].source, "nginx[12345]");
    assert_eq!(entries[0].message, "starting up");
    assert!(entries[0].timestamp_secs > 0);
    assert_eq!(entries[2].source, "systemd[1]");
}

#[test]
fn empty_input_returns_empty_list() {
    let entries = parse_journal("").unwrap();
    assert!(entries.is_empty());
}

#[test]
fn malformed_line_is_skipped_not_errored() {
    let s = "this is not a journal line\n2026-05-21T08:30:11+0000 host src: ok\n";
    let entries = parse_journal(s).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].message, "ok");
}

#[test]
fn crlf_line_endings_are_tolerated() {
    let s = "2026-05-21T08:30:11+0000 host src: ok\r\n";
    let entries = parse_journal(s).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].message, "ok");
}

#[test]
fn journal_parser_output_snapshot() {
    let entries = parse_journal(SAMPLE).unwrap();
    insta::assert_debug_snapshot!(entries);
}
