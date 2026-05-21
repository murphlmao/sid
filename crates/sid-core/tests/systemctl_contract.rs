//! Verifies SystemctlClient is dyn-compatible and a MockClient covers every method.

use sid_core::adapters::systemctl::{
    JournalEntry, SystemUnit, SystemctlClient, SystemctlError, UnitBus, UnitFilter, UnitState,
};

struct MockClient;

impl SystemctlClient for MockClient {
    fn list_units(&self, _f: UnitFilter) -> Result<Vec<SystemUnit>, SystemctlError> {
        Ok(vec![])
    }
    fn status(&self, _bus: UnitBus, _unit: &str) -> Result<SystemUnit, SystemctlError> {
        Ok(SystemUnit {
            name: "x.service".into(),
            bus: UnitBus::User,
            state: UnitState::Inactive,
            sub_state: "dead".into(),
            description: "x".into(),
            load_state: "loaded".into(),
        })
    }
    fn start(&self, _bus: UnitBus, _unit: &str) -> Result<(), SystemctlError> {
        Ok(())
    }
    fn stop(&self, _bus: UnitBus, _unit: &str) -> Result<(), SystemctlError> {
        Ok(())
    }
    fn restart(&self, _bus: UnitBus, _unit: &str) -> Result<(), SystemctlError> {
        Ok(())
    }
    fn journal_tail(
        &self,
        _bus: UnitBus,
        _unit: &str,
        _lines: usize,
    ) -> Result<Vec<JournalEntry>, SystemctlError> {
        Ok(vec![])
    }
}

#[test]
fn client_is_dyn_compatible() {
    let c: Box<dyn SystemctlClient> = Box::new(MockClient);
    assert!(c.list_units(UnitFilter::default()).unwrap().is_empty());
}

#[test]
fn client_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Box<dyn SystemctlClient>>();
}

#[test]
fn unit_state_variants_exist() {
    let _ = UnitState::Active;
    let _ = UnitState::Inactive;
    let _ = UnitState::Failed;
    let _ = UnitState::Activating;
    let _ = UnitState::Deactivating;
    let _ = UnitState::Unknown;
}

#[test]
fn unit_bus_variants_exist() {
    let _ = UnitBus::User;
    let _ = UnitBus::System;
}

#[test]
fn unit_filter_default_is_empty() {
    let f = UnitFilter::default();
    assert!(f.name_substring.is_none());
    assert!(f.state.is_none());
    assert_eq!(f.bus, UnitBus::User);
    assert!(!f.bus_both);
}

#[test]
fn errors_render_strings() {
    assert!(format!("{}", SystemctlError::SystemctlMissing).contains("systemctl"));
    assert!(format!("{}", SystemctlError::UnitNotFound("x".into())).contains("x"));
    assert!(format!("{}", SystemctlError::SudoRequired).contains("root"));
}
