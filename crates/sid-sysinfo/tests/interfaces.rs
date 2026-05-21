use sid_core::adapters::sys::SysProvider;
use sid_sysinfo::SysinfoProvider;

#[test]
fn list_interfaces_includes_loopback() {
    let mut p = SysinfoProvider::new();
    let ifs = p.list_interfaces().unwrap();
    assert!(
        ifs.iter()
            .any(|i| i.name == "lo" || i.name == "lo0" || i.name.starts_with("lo")),
        "expected loopback interface in {:?}",
        ifs.iter().map(|i| &i.name).collect::<Vec<_>>()
    );
}

#[test]
fn interfaces_have_unique_names() {
    let mut p = SysinfoProvider::new();
    let ifs = p.list_interfaces().unwrap();
    let mut names: Vec<_> = ifs.iter().map(|i| i.name.clone()).collect();
    names.sort();
    let total = names.len();
    names.dedup();
    assert_eq!(names.len(), total, "interface names must be unique");
}

#[test]
fn rx_tx_monotonic_over_two_polls() {
    let mut p = SysinfoProvider::new();
    let a = p.list_interfaces().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    let b = p.list_interfaces().unwrap();
    for ai in &a {
        if let Some(bi) = b.iter().find(|x| x.name == ai.name) {
            assert!(
                bi.rx_bytes >= ai.rx_bytes,
                "rx went backwards on {}",
                ai.name
            );
            assert!(
                bi.tx_bytes >= ai.tx_bytes,
                "tx went backwards on {}",
                ai.name
            );
        }
    }
}

#[test]
fn no_interfaces_does_not_panic() {
    let mut p = SysinfoProvider::new();
    for _ in 0..10 {
        let _ = p.list_interfaces().unwrap();
    }
}

#[test]
fn output_sorted_by_name() {
    let mut p = SysinfoProvider::new();
    let ifs = p.list_interfaces().unwrap();
    for w in ifs.windows(2) {
        assert!(w[0].name <= w[1].name, "not sorted: {ifs:?}");
    }
}
