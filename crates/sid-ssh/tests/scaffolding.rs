use std::path::Path;

use sid_ssh::{RusshClientFactory, read_ssh_config};

#[test]
fn factory_constructs() {
    let _f1 = RusshClientFactory::new();
    let _f2: RusshClientFactory = Default::default();
}

#[test]
fn config_reader_returns_empty_on_missing_file() {
    let v = read_ssh_config(Path::new("/nonexistent-ssh-config-file")).unwrap();
    assert!(v.is_empty());
}
