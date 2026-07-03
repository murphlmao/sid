//! `DockerCliProvider` — `docker ps -a` backed `ContainerProvider` impl.

mod client;
mod parse;

use async_trait::async_trait;
use sid_core::containers::{ContainerError, ContainerInfo, ContainerProvider};

/// CLI-shelling implementation of [`ContainerProvider`]. Stateless — every call spawns
/// its own `docker` subprocess; there is no cached handle to lock.
#[derive(Debug, Default, Clone, Copy)]
pub struct DockerCliProvider;

impl DockerCliProvider {
    /// Construct a new provider. `crates/sid` is the one crate allowed to name this
    /// constructor concretely (mirrors `SvcctlProvider::new()` /
    /// `SysinfoProvider::new()`) — everything after construction goes back out through
    /// `sid_core::containers::ContainerProvider`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_containers::DockerCliProvider;
    /// let _p = DockerCliProvider::new();
    /// ```
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ContainerProvider for DockerCliProvider {
    async fn list_containers(&self) -> Result<Vec<ContainerInfo>, ContainerError> {
        let raw = client::list_containers_raw().await?;
        parse::parse_ps_lines(&raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Object-safety: the Network tab holds this behind `Arc<dyn ContainerProvider>` —
    // no `Mutex` needed since `DockerCliProvider` carries no mutable state.
    // Compile-only.
    #[allow(dead_code)]
    fn assert_object_safe(_p: &dyn ContainerProvider) {}

    #[test]
    fn boxed_trait_object_constructs() {
        let provider: std::sync::Arc<dyn ContainerProvider> =
            std::sync::Arc::new(DockerCliProvider::new());
        assert_object_safe(&*provider);
    }
}
