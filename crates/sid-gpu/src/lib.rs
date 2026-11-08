//! GPU pre-flight adapters (`sid_core::gpu::GpuPreflight` impls).
//!
//! The probe-in-a-subprocess ladder (Vulkan driver manifests, kernel-module
//! fingerprints, `VK_DRIVER_FILES` pinning) is a Linux concern, so it lives in the
//! `linux` module and is compiled on Linux only. Everywhere else the frontend gets
//! [`NoopGpuPreflight`]: macOS (Metal) and Windows (DX11/DX12) renderers do not go
//! through the Vulkan loader, so there is nothing to probe or pin, and the preflight
//! says so honestly instead of pretending to have checked.
//!
//! [`PlatformGpuPreflight`] names the right one for the target; the frontend uses
//! that alias and never a `cfg` of its own.

#[cfg(target_os = "linux")]
mod linux;
mod noop;

#[cfg(target_os = "linux")]
pub use linux::{LinuxGpuPreflight, send_failure_notification};
pub use noop::NoopGpuPreflight;

/// The pre-flight for the platform this binary was built for.
#[cfg(target_os = "linux")]
pub type PlatformGpuPreflight = LinuxGpuPreflight;
/// The pre-flight for the platform this binary was built for.
#[cfg(not(target_os = "linux"))]
pub type PlatformGpuPreflight = NoopGpuPreflight;

/// What `sid --gpu-report` produced: the human-readable text, plus whether any
/// rung could actually render. The flag exists so the CLI can exit nonzero on a
/// machine that cannot start sid — a scriptable health check ("does sid run
/// here?") rather than a report that always claims success.
pub struct Report {
    pub text: String,
    pub renderable: bool,
}

/// Best-effort desktop notification for a terminal pre-flight failure. Only the
/// Linux pre-flight can fail, so elsewhere this is a no-op kept for call-site symmetry.
#[cfg(not(target_os = "linux"))]
pub fn send_failure_notification(_diagnosis: &sid_core::gpu::Diagnosis) {}
