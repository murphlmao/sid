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

// Pure parsing/classification compile and test everywhere; only the `systemctl`
// invocations are Linux-only. Off Linux the parsers are unused, hence the allow.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod classify;
#[cfg(target_os = "linux")]
mod client;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
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

#[cfg(target_os = "linux")]
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

/// The error every call on a platform without systemd returns. The Network ->
/// Services view renders provider errors verbatim, so this is the whole UX: one
/// honest line instead of a failed spawn of a binary that does not exist.
pub fn unsupported_message(os: &str) -> String {
    format!("service management is not supported on {os} (systemd only)")
}

/// A [`ServiceProvider`] that reports the platform has no supported service manager.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedServiceProvider;

#[async_trait]
impl ServiceProvider for UnsupportedServiceProvider {
    async fn list_services(&self, _scope: SvcScope) -> Result<Vec<ServiceInfo>, SvcError> {
        Err(SvcError::Other(unsupported_message(std::env::consts::OS)))
    }

    async fn control(
        &self,
        _name: &str,
        _action: SvcAction,
        _scope: SvcScope,
    ) -> Result<(), SvcError> {
        Err(SvcError::Other(unsupported_message(std::env::consts::OS)))
    }
}

/// Off Linux `SvcctlProvider` is the unsupported provider: the Network tab names this
/// type concretely, and keeping that name stable keeps the tab free of `cfg`.
#[cfg(not(target_os = "linux"))]
#[async_trait]
impl ServiceProvider for SvcctlProvider {
    async fn list_services(&self, scope: SvcScope) -> Result<Vec<ServiceInfo>, SvcError> {
        UnsupportedServiceProvider.list_services(scope).await
    }

    async fn control(
        &self,
        name: &str,
        action: SvcAction,
        scope: SvcScope,
    ) -> Result<(), SvcError> {
        UnsupportedServiceProvider
            .control(name, action, scope)
            .await
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

    #[test]
    fn unsupported_message_names_the_platform() {
        let m = unsupported_message("macos");
        assert!(m.contains("macos") && m.contains("systemd"));
    }

    #[test]
    fn unsupported_provider_errors_on_both_calls() {
        use std::future::Future;
        use std::pin::pin;
        use std::task::{Context, Poll, Waker};
        fn block_on<F: Future>(f: F) -> F::Output {
            let mut f = pin!(f);
            let mut cx = Context::from_waker(Waker::noop());
            loop {
                if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                    return v;
                }
            }
        }
        let p = UnsupportedServiceProvider;
        assert!(matches!(
            block_on(p.list_services(SvcScope::System)),
            Err(SvcError::Other(_))
        ));
        assert!(matches!(
            block_on(p.control("x.service", SvcAction::Start, SvcScope::System)),
            Err(SvcError::Other(_))
        ));
    }
}
