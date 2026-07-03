//! `sid-containers` — CLI-shelling implementations of
//! `sid_core::containers::{ContainerProvider, KubeProvider}`.
//!
//! [`DockerCliProvider`] shells out to `docker` and [`KubectlCliProvider`] to
//! `kubectl`, both via `tokio::process::Command` for every call (never
//! `std::process`) — same reasoning as `sid-svcctl`'s module doc: every call here
//! only ever runs inside a `ssh_runtime().spawn(async move { .. })` block, never
//! inline in `render`, and `tokio::process` lets the spawned task yield to the
//! runtime while the subprocess does its I/O instead of parking a worker thread on
//! `wait(2)`. `docker`/`kubectl` are named ONLY in this crate — `sid_core` and
//! `crates/sid` never see them, matching the adapter rule.
//!
//! Both providers are stateless (no cached handle, unlike `sid-sysinfo`'s
//! `SysinfoProvider`) — there is nothing to serialize access to, so callers hold them
//! as plain `Arc<dyn ContainerProvider>` / `Arc<dyn KubeProvider>` with no mutex.
//!
//! Read-only: this crate lists containers/contexts/pods only. Management
//! (start/stop/exec/logs) is out of scope for this pass.

mod docker;
mod kube;

pub use docker::DockerCliProvider;
pub use kube::KubectlCliProvider;
