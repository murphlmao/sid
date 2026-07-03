//! `sid-svcctl` — CLI-shelling implementation of `sid_core::svc::ServiceProvider`.
//!
//! [`SvcctlProvider`] shells out to `systemctl` via `tokio::process::Command` for
//! every call — see [`client`]'s module doc for why that (rather than
//! `std::process::Command`) is the right tool here. It is deliberately stateless
//! (unlike `sid-sysinfo`'s `SysinfoProvider`, which caches a `sysinfo::System`
//! handle): there is nothing to serialize access to, so it needs no
//! `Arc<Mutex<_>>` wrapper at the call site — `Arc<dyn ServiceProvider>` is enough.
//!
//! Salvaged from the archived `sid-poc`'s `sid-system` crate
//! (`~/vcs/sid-poc/crates/sid-system/src/{client,parse}.rs`): the stderr → error
//! classification (`Access denied` / `Interactive authentication required` / ... →
//! a permission error; `could not be found` → not-found) carries over near-verbatim
//! in [`classify`]. The list-units parsing does not: the POC parsed
//! `--plain --no-legend` whitespace columns; this rebuild parses `--output=json`
//! instead (see [`parse`]'s module doc), which is new code, not a port.

mod classify;
mod client;
mod parse;

use async_trait::async_trait;
use sid_core::svc::{ServiceInfo, ServiceProvider, SvcAction, SvcError, SvcScope};

/// CLI-shelling implementation of [`ServiceProvider`]. Stateless — every call spawns
/// its own `systemctl` subprocess; there is no cached handle to lock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SvcctlProvider;

impl SvcctlProvider {
    /// Construct a new provider. `crates/sid` is the one crate allowed to name this
    /// constructor concretely (mirrors `SysinfoProvider::new()` /
    /// `RusshClientFactory::new()`) — everything after construction goes back out
    /// through `sid_core::svc::ServiceProvider`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_svcctl::SvcctlProvider;
    /// let _p = SvcctlProvider::new();
    /// ```
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ServiceProvider for SvcctlProvider {
    async fn list_services(&self, scope: SvcScope) -> Result<Vec<ServiceInfo>, SvcError> {
        let raw = client::list_units_json(scope).await?;
        parse::parse_list_units(&raw)
    }

    async fn control(
        &self,
        name: &str,
        action: SvcAction,
        scope: SvcScope,
    ) -> Result<(), SvcError> {
        client::run_action(scope, action, name).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Object-safety: the Network tab holds this behind `Arc<dyn ServiceProvider>` —
    // no `Mutex` needed since `SvcctlProvider` carries no mutable state. Compile-only.
    #[allow(dead_code)]
    fn assert_object_safe(_p: &dyn ServiceProvider) {}

    #[test]
    fn boxed_trait_object_constructs() {
        let provider: std::sync::Arc<dyn ServiceProvider> =
            std::sync::Arc::new(SvcctlProvider::new());
        assert_object_safe(&*provider);
    }
}
