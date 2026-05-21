use sid_core::adapters::sys::SysProvider;
use sid_sysinfo::SysinfoProvider;

#[test]
fn new_constructs_without_panicking() {
    let _ = SysinfoProvider::new();
}

#[test]
fn provider_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SysinfoProvider>();
}

#[test]
fn boxes_into_dyn_provider() {
    let p: Box<dyn SysProvider> = Box::new(SysinfoProvider::new());
    drop(p);
}

#[test]
fn many_news_in_sequence_does_not_leak() {
    for _ in 0..50 {
        let _ = SysinfoProvider::new();
    }
}
