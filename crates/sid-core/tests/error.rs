use sid_core::SidError;

#[test]
fn error_display_includes_message() {
    let e = SidError::Other("boom".into());
    let msg = format!("{e}");
    assert!(msg.contains("boom"));
}
