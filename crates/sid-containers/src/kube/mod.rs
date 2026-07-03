//! `KubectlCliProvider` — `kubectl` backed `KubeProvider` impl.

mod client;
mod parse;

use async_trait::async_trait;
use sid_core::containers::{KubeContext, KubeError, KubePod, KubeProvider};

/// CLI-shelling implementation of [`KubeProvider`]. Stateless — every call spawns its
/// own `kubectl` subprocess; there is no cached handle to lock.
#[derive(Debug, Default, Clone, Copy)]
pub struct KubectlCliProvider;

impl KubectlCliProvider {
    /// Construct a new provider. `crates/sid` is the one crate allowed to name this
    /// constructor concretely (mirrors `DockerCliProvider::new()`) — everything after
    /// construction goes back out through `sid_core::containers::KubeProvider`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sid_containers::KubectlCliProvider;
    /// let _p = KubectlCliProvider::new();
    /// ```
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl KubeProvider for KubectlCliProvider {
    async fn list_contexts(&self) -> Result<Vec<KubeContext>, KubeError> {
        let raw = client::get_contexts_raw().await?;
        let names = parse::parse_context_names(&raw);
        if names.is_empty() {
            return Ok(Vec::new());
        }
        // Best-effort: failing to resolve which context is "current" (e.g. none set)
        // must not fail the whole listing — it just means no row gets the `current`
        // marker.
        let current = client::current_context_raw().await.unwrap_or(None);
        Ok(parse::build_contexts(names, current.as_deref()))
    }

    async fn list_pods(&self, context: Option<&str>) -> Result<Vec<KubePod>, KubeError> {
        let raw = client::get_pods_raw(context).await?;
        parse::parse_pods_json(&raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Object-safety: the Network tab holds this behind `Arc<dyn KubeProvider>` — no
    // `Mutex` needed since `KubectlCliProvider` carries no mutable state. Compile-only.
    #[allow(dead_code)]
    fn assert_object_safe(_p: &dyn KubeProvider) {}

    #[test]
    fn boxed_trait_object_constructs() {
        let provider: std::sync::Arc<dyn KubeProvider> =
            std::sync::Arc::new(KubectlCliProvider::new());
        assert_object_safe(&*provider);
    }
}
