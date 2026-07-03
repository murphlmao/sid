//! Docker + Kubernetes **read-only** provider traits + supporting domain types.
//! Implementations live in `sid-containers`.
//!
//! Mirrors `svc.rs`'s shape: domain types ahead of the traits, dedicated
//! `thiserror` errors per provider, no `serde` (the Network tab is live/ephemeral —
//! nothing here is ever persisted). Both traits are `async` for the same reason
//! `ServiceProvider` is: the concrete impls (`sid_containers::DockerCliProvider` /
//! `KubectlCliProvider`) shell out via `tokio::process::Command` on every call and are
//! stateless, so callers hold them as plain `Arc<dyn ContainerProvider>` / `Arc<dyn
//! KubeProvider>` with no mutex.
//!
//! Management (start/stop/exec/logs) is explicitly out of scope for this pass —
//! read-only listing only.
//!
//! ## Graceful absence
//!
//! Docker and kubectl are both optional local tooling — sid must never treat "not
//! installed" as a hard error. Each error enum below carries a [`ContainerError::NotInstalled`]
//! / [`KubeError::NotInstalled`] variant for exactly that: the binary is missing, the
//! Docker socket/daemon isn't reachable, or no Kubernetes cluster is reachable from the
//! configured context. Concrete impls map subprocess spawn failures (`ENOENT`) and the
//! relevant daemon/cluster-unreachable stderr text into this variant; the UI renders it
//! as a dim notice instead of a red error line — see `crates/sid/src/ui/network_tab.rs`.

use async_trait::async_trait;

/// One container row (from `docker ps -a`).
///
/// # Examples
///
/// ```
/// use sid_core::containers::ContainerInfo;
/// let c = ContainerInfo {
///     id: "abc123".into(),
///     name: "dev-eggsightv2-1".into(),
///     image: "postgres:16".into(),
///     state: "running".into(),
///     status: "Up 3 hours".into(),
///     ports: vec!["0.0.0.0:5432->5432/tcp".into()],
/// };
/// assert_eq!(c.name, "dev-eggsightv2-1");
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct ContainerInfo {
    /// Container ID (`docker ps`'s `.ID`, typically a short 12-char hex).
    pub id: String,
    /// Container name (`.Names`, first name if the daemon reports several).
    pub name: String,
    /// Image reference (`.Image`).
    pub image: String,
    /// Coarse lifecycle state (`.State`, e.g. `"running"`, `"exited"`, `"paused"`).
    pub state: String,
    /// Human-readable status (`.Status`, e.g. `"Up 3 hours"`, `"Exited (0) 2 days ago"`).
    pub status: String,
    /// Published/exposed port mappings (`.Ports`, split on `,`), e.g.
    /// `"0.0.0.0:5432->5432/tcp"`. Empty when the container publishes no ports.
    pub ports: Vec<String>,
}

/// One `kubectl config get-contexts` entry.
///
/// # Examples
///
/// ```
/// use sid_core::containers::KubeContext;
/// let c = KubeContext { name: "minikube".into(), current: true };
/// assert!(c.current);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct KubeContext {
    /// Context name, as `kubectl config get-contexts -o name` prints it.
    pub name: String,
    /// Whether this is the kubeconfig's `current-context`.
    pub current: bool,
}

/// One pod row (from `kubectl get pods -A -o json`).
///
/// # Examples
///
/// ```
/// use sid_core::containers::KubePod;
/// let p = KubePod {
///     namespace: "default".into(),
///     name: "web-7f6b9c-abcde".into(),
///     ready: "1/1".into(),
///     phase: "Running".into(),
///     restarts: 0,
///     node: "minikube".into(),
/// };
/// assert_eq!(p.ready, "1/1");
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct KubePod {
    /// Namespace the pod lives in.
    pub namespace: String,
    /// Pod name.
    pub name: String,
    /// `ready/total` container count, e.g. `"1/1"` — matches `kubectl get pods`'
    /// READY column, computed from `status.containerStatuses[].ready`.
    pub ready: String,
    /// Pod phase (`status.phase`): `"Pending"`, `"Running"`, `"Succeeded"`,
    /// `"Failed"`, `"Unknown"`, or empty if unset.
    pub phase: String,
    /// Total restart count summed across the pod's containers.
    pub restarts: u32,
    /// Node the pod is scheduled on (`spec.nodeName`); empty if unscheduled.
    pub node: String,
}

/// Domain-shaped Docker probe error.
///
/// # Examples
///
/// ```
/// use sid_core::containers::ContainerError;
/// let e = ContainerError::NotInstalled;
/// assert!(format!("{e}").contains("not"));
/// ```
#[derive(Debug, thiserror::Error)]
pub enum ContainerError {
    /// The `docker` binary is missing, or `docker ps` failed because the daemon/socket
    /// isn't reachable. The UI degrades to a dim notice rather than an error banner.
    #[error("docker not installed or not running")]
    NotInstalled,
    /// Anything else: unexpected subprocess failure, unparseable output, ...
    #[error("container probe error: {0}")]
    Other(String),
}

/// Domain-shaped Kubernetes probe error.
///
/// # Examples
///
/// ```
/// use sid_core::containers::KubeError;
/// let e = KubeError::NotInstalled;
/// assert!(format!("{e}").contains("kubectl"));
/// ```
#[derive(Debug, thiserror::Error)]
pub enum KubeError {
    /// The `kubectl` binary is missing, no kubeconfig/context is set, or no cluster is
    /// reachable from the current context. All three collapse to the same variant
    /// because the UI's graceful-absence notice ("kubectl not installed — no cluster")
    /// covers both cases identically — see `crates/sid/src/ui/network_tab.rs`.
    #[error("kubectl not installed — no cluster")]
    NotInstalled,
    /// Anything else: unexpected subprocess failure, unparseable output, ...
    #[error("kube probe error: {0}")]
    Other(String),
}

/// Docker container listing. Implementations live in `sid-containers`
/// (`DockerCliProvider`).
///
/// # Object safety
///
/// `#[async_trait]` boxes the returned future, so `Arc<dyn ContainerProvider>` works
/// despite the `async fn` below.
#[async_trait]
pub trait ContainerProvider: Send + Sync {
    /// List all containers (running and stopped) — maps to `docker ps -a`.
    async fn list_containers(&self) -> Result<Vec<ContainerInfo>, ContainerError>;
}

/// Kubernetes context + pod listing. Implementations live in `sid-containers`
/// (`KubectlCliProvider`).
///
/// # Object safety
///
/// `#[async_trait]` boxes the returned futures, so `Arc<dyn KubeProvider>` works
/// despite the `async fn`s below.
#[async_trait]
pub trait KubeProvider: Send + Sync {
    /// List configured kubeconfig contexts, with [`KubeContext::current`] marking the
    /// active one.
    async fn list_contexts(&self) -> Result<Vec<KubeContext>, KubeError>;

    /// List pods across all namespaces. `context` selects a specific kubeconfig
    /// context (`kubectl --context <name>`); `None` uses kubectl's own
    /// `current-context`.
    async fn list_pods(&self, context: Option<&str>) -> Result<Vec<KubePod>, KubeError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Object-safety: the Network tab holds these behind `Arc<dyn ContainerProvider>` /
    // `Arc<dyn KubeProvider>` — a compile-only check that a non-dispatchable method
    // never sneaks in.
    #[allow(dead_code)]
    fn assert_container_provider_object_safe(_p: &dyn ContainerProvider) {}
    #[allow(dead_code)]
    fn assert_kube_provider_object_safe(_p: &dyn KubeProvider) {}

    #[test]
    fn boxed_container_provider_trait_object_constructs() {
        fn takes_provider(_: Box<dyn ContainerProvider>) {}
        let _ = takes_provider;
    }

    #[test]
    fn boxed_kube_provider_trait_object_constructs() {
        fn takes_provider(_: Box<dyn KubeProvider>) {}
        let _ = takes_provider;
    }

    #[test]
    fn container_not_installed_message_is_stable() {
        assert_eq!(
            ContainerError::NotInstalled.to_string(),
            "docker not installed or not running"
        );
    }

    #[test]
    fn container_other_message_contains_detail() {
        let e = ContainerError::Other("spawn docker: boom".into());
        assert!(e.to_string().contains("boom"));
    }

    #[test]
    fn kube_not_installed_message_is_stable() {
        assert_eq!(
            KubeError::NotInstalled.to_string(),
            "kubectl not installed — no cluster"
        );
    }

    #[test]
    fn kube_other_message_contains_detail() {
        let e = KubeError::Other("parse pods json: boom".into());
        assert!(e.to_string().contains("boom"));
    }
}
