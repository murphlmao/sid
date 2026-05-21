use sid_pty::Vt100Screen;

#[test]
fn new_screen_is_blank() {
    let s = Vt100Screen::new(3, 10);
    let lines = s.lines();
    assert_eq!(lines.len(), 3);
    for l in &lines {
        assert!(l.trim().is_empty());
    }
}

#[test]
fn feed_plain_text_appears_in_lines() {
    let mut s = Vt100Screen::new(3, 10);
    s.feed(b"hello");
    let lines = s.lines();
    assert!(lines[0].contains("hello"));
}

#[test]
fn cursor_position_reports_correctly_after_feed() {
    let mut s = Vt100Screen::new(3, 10);
    s.feed(b"abc");
    let (row, col) = s.cursor_position();
    assert_eq!(row, 0);
    assert_eq!(col, 3);
}

#[test]
fn resize_changes_dimensions() {
    let mut s = Vt100Screen::new(3, 10);
    s.resize(5, 20);
    assert_eq!(s.size(), (5, 20));
    assert_eq!(s.lines().len(), 5);
}

#[test]
fn ansi_escape_codes_do_not_appear_in_rendered_lines() {
    let mut s = Vt100Screen::new(3, 20);
    s.feed(b"\x1b[31mhi\x1b[0m");
    let lines = s.lines();
    assert!(lines[0].contains("hi"));
    assert!(!lines[0].contains("\x1b"));
    assert!(!lines[0].contains("[31m"));
}

#[test]
fn malformed_ansi_does_not_panic() {
    let mut s = Vt100Screen::new(3, 10);
    s.feed(b"\x1b\x1b\x1b[[[33");
    let _ = s.lines();
}

#[test]
fn very_wide_unicode_renders() {
    let mut s = Vt100Screen::new(3, 20);
    s.feed("abcdef".as_bytes());
    let l = s.lines();
    assert!(!l[0].trim().is_empty());
}

use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_feed_arbitrary_bytes_never_panics(b in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let mut s = Vt100Screen::new(24, 80);
        s.feed(&b);
        let _ = s.lines();
        let _ = s.cursor_position();
    }
}
