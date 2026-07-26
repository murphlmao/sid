//! GPU pre-flight seam: "can this machine bring up a rendering context, and
//! under what environment?" — asked by the frontend *before* it constructs any
//! renderer, because the renderer's own init failure is an unrecoverable panic.
//!
//! Concrete probing lives in `sid-gpu` (Linux today; other platforms are empty
//! slots that answer [`RenderPath::Hardware`] unconditionally). Per the adapter
//! rule, no renderer, windowing, or GPU-API name appears in this module — the
//! seam speaks only in outcomes (which path, which env pins, what's broken).

/// Which rendering tier the machine will actually get.
///
/// # Examples
///
/// ```
/// use sid_core::gpu::RenderPath;
/// let soft = RenderPath::Software { reason: "no GPU device — using llvmpipe".into() };
/// assert_ne!(RenderPath::Hardware, soft);
/// match &soft {
///     RenderPath::Software { reason } => assert!(reason.contains("llvmpipe")),
///     RenderPath::Hardware => unreachable!("constructed as Software"),
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderPath {
    /// A real GPU device initializes.
    Hardware,
    /// Only a software rasterizer initializes — usable but CPU-rendered.
    /// `reason` is the human-readable why; the frontend surfaces it as a
    /// persistent badge so the degradation is never silent.
    Software { reason: String },
}

/// A successful pre-flight: the path plus environment pins the frontend must
/// export before constructing the renderer (e.g. a driver-manifest pin that
/// routes around a broken driver). Pins are plain `KEY=VALUE` pairs; an empty
/// vec means the vanilla environment already works.
///
/// # Examples
///
/// ```
/// use sid_core::gpu::{PreflightOk, RenderPath};
/// let vanilla = PreflightOk { path: RenderPath::Hardware, env_pins: vec![] };
/// assert!(vanilla.env_pins.is_empty());
///
/// let pinned = PreflightOk {
///     path: RenderPath::Hardware,
///     env_pins: vec![("PIN_KEY".into(), "/path/to/driver/manifest.json".into())],
/// };
/// assert_eq!(pinned.env_pins[0].0, "PIN_KEY");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightOk {
    pub path: RenderPath,
    pub env_pins: Vec<(String, String)>,
}

/// Why no rendering path exists at all, classified coarsely enough for the
/// frontend to phrase a remedy without knowing platform details.
///
/// # Examples
///
/// ```
/// use sid_core::gpu::FailureCause;
/// assert_ne!(FailureCause::NoDriverInstalled, FailureCause::NoDeviceAccess);
/// // `Copy`: one classified cause can feed several phrasing helpers without cloning.
/// let cause = FailureCause::DriverMismatchRebootNeeded;
/// let (summary_input, remedy_input) = (cause, cause);
/// assert_eq!(summary_input, remedy_input);
/// assert_ne!(cause, FailureCause::Unknown);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureCause {
    /// Kernel/userspace driver versions disagree — typically a driver update
    /// that needs a reboot to take effect.
    DriverMismatchRebootNeeded,
    /// No usable GPU driver is installed for this session's hardware.
    NoDriverInstalled,
    /// Devices exist but this session cannot open them (group permissions,
    /// a VM without 3D acceleration, a headless seat).
    NoDeviceAccess,
    /// Probing failed in a way the classifier couldn't attribute.
    Unknown,
}

/// A terminal pre-flight failure: what's wrong and what to do about it.
///
/// # Examples
///
/// ```
/// use sid_core::gpu::{Diagnosis, FailureCause};
/// let d = Diagnosis {
///     cause: FailureCause::DriverMismatchRebootNeeded,
///     summary: "GPU driver was updated; a reboot is required".into(),
///     remedy: "Reboot, then start sid again.".into(),
///     detail: "probe exit: 101\nloaded module: amdgpu\n".into(),
/// };
/// assert_eq!(d.cause, FailureCause::DriverMismatchRebootNeeded);
/// // `summary`/`remedy` are the one-liners a terminal or notification shows;
/// // `detail` is the multi-line evidence, never required reading.
/// assert!(!d.summary.contains('\n'));
/// assert!(d.detail.lines().count() > 1);
/// ```
#[derive(Debug, Clone)]
pub struct Diagnosis {
    pub cause: FailureCause,
    /// One line: what is wrong ("GPU driver was updated; a reboot is required").
    pub summary: String,
    /// One or two lines: exactly what the user should do next.
    pub remedy: String,
    /// Multi-line supporting evidence (probe log tail, devices seen, ...) for
    /// terminal output and bug reports; never required reading.
    pub detail: String,
}

/// The launch-time question. Implementations self-heal where possible (retrying
/// alternate drivers, falling back to software rendering) and only return `Err`
/// when no rendering path exists on this machine as-is.
///
/// # Examples
///
/// A platform with nothing to probe answers [`RenderPath::Hardware`] flat, pinning no
/// environment — the whole shape of a not-yet-implemented platform slot:
///
/// ```
/// use sid_core::gpu::{Diagnosis, GpuPreflight, PreflightOk, RenderPath};
///
/// struct AssumeRenderable;
/// impl GpuPreflight for AssumeRenderable {
///     fn ensure_renderable(&self) -> Result<PreflightOk, Diagnosis> {
///         Ok(PreflightOk { path: RenderPath::Hardware, env_pins: vec![] })
///     }
/// }
///
/// let ok = AssumeRenderable.ensure_renderable().expect("empty slot never fails");
/// assert_eq!(ok.path, RenderPath::Hardware);
/// assert!(ok.env_pins.is_empty());
/// ```
pub trait GpuPreflight {
    fn ensure_renderable(&self) -> Result<PreflightOk, Diagnosis>;
}
