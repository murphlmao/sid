use std::sync::mpsc;

use sid_core::{
    context::WidgetCtx,
    event::Event,
    widget::{EventOutcome, RenderTarget},
};
use sid_widgets::stub::ComingSoonBody;

struct FakeTarget;
impl RenderTarget for FakeTarget {
    fn width(&self) -> u16 {
        80
    }
    fn height(&self) -> u16 {
        24
    }
}

#[test]
fn body_returns_title_and_subtitle() {
    let b = ComingSoonBody::new("Workspaces", "git operations across registered repos");
    assert_eq!(b.title(), "Workspaces");
    assert!(b.subtitle().contains("git operations"));
}

#[test]
fn body_subtitle_full_string() {
    let b = ComingSoonBody::new(
        "SSH",
        "SSH host list + embedded terminal + SFTP — coming in Plan 3",
    );
    assert_eq!(
        b.subtitle(),
        "SSH host list + embedded terminal + SFTP — coming in Plan 3"
    );
}

#[test]
fn handle_event_bubbles() {
    let (tx, _rx) = mpsc::channel();
    let mut ctx = WidgetCtx::new(tx);
    let mut b = ComingSoonBody::new("Database", "query runner");
    assert_eq!(b.handle_event(&Event::Tick, &mut ctx), EventOutcome::Bubble);
}

#[test]
fn render_is_noop() {
    let b = ComingSoonBody::new("Network", "ports");
    let mut target = FakeTarget;
    b.render(&mut target); // must not panic
}

// adversarial
#[test]
fn empty_strings() {
    let b = ComingSoonBody::new("", "");
    assert_eq!(b.title(), "");
    assert_eq!(b.subtitle(), "");
}

#[test]
fn very_long_strings() {
    let long = "z".repeat(100_000);
    let b = ComingSoonBody::new(long.clone(), long.clone());
    assert_eq!(b.title().len(), 100_000);
    assert_eq!(b.subtitle().len(), 100_000);
}

#[test]
fn unicode_strings() {
    let title = "Рабочие пространства 🪐";
    let subtitle = "日本語 · 한국어 · العربية";
    let b = ComingSoonBody::new(title, subtitle);
    assert_eq!(b.title(), title);
    assert_eq!(b.subtitle(), subtitle);
}
