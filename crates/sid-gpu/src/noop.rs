//! The pre-flight for platforms with nothing to probe.

use crate::Report;
use sid_core::gpu::{Diagnosis, GpuPreflight, PreflightOk, RenderPath};

/// Reports "not applicable on this platform" and always lets startup proceed.
///
/// The Linux probe exists because blade's Vulkan device discovery panics when no
/// usable ICD is installed. macOS and Windows renderers have no such loader-level
/// failure mode to pre-empt, so there is no probe, no stamp and no crash marker.
#[derive(Debug, Clone, Default)]
pub struct NoopGpuPreflight;

impl NoopGpuPreflight {
    /// Same constructor shape as the Linux pre-flight so the frontend is `cfg`-free.
    pub fn from_env(_app_version: &str) -> Self {
        Self
    }

    /// No crash marker is kept on this platform.
    pub fn mark_launch_attempt(&self) {}

    /// No crash marker is kept on this platform.
    pub fn clear_launch_attempt(&self) {}

    /// `sid --gpu-report`: states that the probe does not apply here.
    pub fn report(&self) -> Report {
        Report {
            text: format!(
                "GPU pre-flight: not applicable on this platform ({}).\n\
                 The Vulkan driver probe is Linux-only; the renderer initialises directly.\n",
                std::env::consts::OS
            ),
            renderable: true,
        }
    }
}

impl GpuPreflight for NoopGpuPreflight {
    fn ensure_renderable(&self) -> Result<PreflightOk, Diagnosis> {
        Ok(PreflightOk {
            path: RenderPath::Hardware,
            env_pins: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_renderable_with_no_pins() {
        let ok = NoopGpuPreflight.ensure_renderable().unwrap();
        assert!(ok.env_pins.is_empty());
        assert!(matches!(ok.path, RenderPath::Hardware));
    }

    #[test]
    fn report_says_not_applicable() {
        let r = NoopGpuPreflight::from_env("0").report();
        assert!(r.renderable);
        assert!(r.text.contains("not applicable"));
    }
}
