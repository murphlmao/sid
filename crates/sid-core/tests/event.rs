use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use sid_core::event::{Event, KeyChord};

#[test]
fn from_crossterm_key_extracts_chord() {
    let crossterm_ev = crossterm::event::Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    let ev = Event::from_crossterm(crossterm_ev);
    match ev {
        Event::Key(chord) => {
            assert_eq!(chord, KeyChord::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        }
        other => panic!("expected Key, got {other:?}"),
    }
}

#[test]
fn tick_event_constructs() {
    let _ = Event::Tick;
}
