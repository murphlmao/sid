//! The failure classifier: when every ladder rung fails, turn the evidence
//! (probe stderr, manifest scan, kernel modules, `nvidia-smi`) into a
//! `Diagnosis` a human can act on — the difference between an opaque panic and
//! "your driver updated; reboot". Arms are ordered most-specific first; every
//! check reads through `sys_root` / injected commands so tests never touch the
//! real machine.

use sid_core::gpu::{Diagnosis, FailureCause};
use std::path::Path;
use std::time::Duration;

use crate::LinuxGpuPreflight;
use crate::icd::IcdManifest;
use crate::probe;

/// What the ladder observed, reduced to the shapes the classifier reads.
///
/// Bundled rather than passed as loose arguments because every arm below is a
/// statement *about this evidence*, and a bare `bool` at the call sites would say
/// nothing about which shape it describes.
pub(crate) struct LadderEvidence<'a> {
    /// Rung 0's captured stderr — the vanilla probe's own words, where every
    /// recognizable error signature appears. Empty when the capture was lost
    /// (`ProbeOutcome::no_capture`) or when the probe never ran.
    pub rung0_stderr: &'a str,
    /// Rung 0 ran with its output discarded, because nowhere was writable.
    ///
    /// Carried so the evidence bundle can say *why* it has no probe stderr. The
    /// two states are indistinguishable in `rung0_stderr` alone (both are empty),
    /// and printing a bare `(empty)` for the second one hides the actual fault —
    /// a report reader then blames a silent probe rather than an unwritable state
    /// dir and temp dir.
    pub rung0_no_capture: bool,
    /// Every rung that ran was killed at its timeout: nothing ever answered.
    pub all_timed_out: bool,
}

pub(crate) fn diagnose(
    pf: &LinuxGpuPreflight,
    icds: &[IcdManifest],
    modules: &[(String, String)],
    evidence: &LadderEvidence<'_>,
) -> Diagnosis {
    let rung0_stderr = evidence.rung0_stderr;
    // Gathered once, up front: every diagnosis carries the hardware inventory in
    // its evidence, and arm 8 compares it against the driver inventory. Reading
    // /sys is a handful of tiny files — no subprocess, no thread, no ordering
    // constraint against the arms below.
    let gpus = gpus_present(&pf.sys_root);
    let detail = build_detail(
        icds,
        modules,
        &gpus,
        rung0_stderr,
        evidence.rung0_no_capture,
    );

    // Arm 1: the Vulkan loader itself never loaded — blade reports it before any
    // driver is consulted ("Missing Vulkan entry points" / a Loading(...) error).
    if rung0_stderr.contains("Missing Vulkan entry points") || rung0_stderr.contains("Loading(") {
        return Diagnosis {
            cause: FailureCause::NoDriverInstalled,
            summary: "the Vulkan loader is not installed".into(),
            remedy: distro_hint(&pf.sys_root),
            detail,
        };
    }

    // Arm 2: NVIDIA kernel/userspace mismatch — the classic post-update state
    // where every GPU call fails until a reboot loads the matching module.
    // nvidia-smi is the cheapest oracle: on mismatch it exits nonzero printing
    // "Failed to initialize NVML: Driver/library version mismatch".
    if pf.sys_root.join("sys/module/nvidia/version").exists() {
        // Captured through the probe's own capture discipline, which buys two
        // things: a per-call unique name (a shared one lets a concurrent sid
        // truncate this capture mid-write and hand us ITS output instead — same
        // hazard as the probe capture, see `ladder::run` — and a pid-only name is
        // not unique, because every process in a fresh PID namespace is pid 1),
        // and the temp-dir fallback, without which a read-only state dir silenced
        // this oracle and sent a post-update NVIDIA box to "file a bug" instead of
        // "reboot". See `probe::run_captured` for the remaining floor: when
        // nothing at all is writable the output is gone and so is this arm.
        let (ok, output) = probe::run_captured(
            &pf.nvidia_smi_cmd,
            &pf.state_dir,
            "nvidia-smi",
            Duration::from_secs(3),
        );
        if !ok && output.to_lowercase().contains("mismatch") {
            return Diagnosis {
                cause: FailureCause::DriverMismatchRebootNeeded,
                summary: "NVIDIA driver was updated but the running kernel module is older".into(),
                remedy: "Reboot to load the matching kernel module.".into(),
                detail,
            };
        }
    }

    // Arm 3: nothing ever answered — every rung that ran was killed at its
    // timeout. This is not a configuration shape at all: a driver call that never
    // returns means a wedged kernel driver, or a compositor that stopped serving
    // its socket (observed live: a mute Wayland socket hangs every rung
    // identically). No stderr signature can say so, because the child is killed
    // mid-init and its evidence is truncated by definition — which is exactly why
    // this shape used to fall through to "unrecognized reason" after tens of
    // seconds of silence.
    //
    // Classified NoDeviceAccess: devices may well exist, but this session cannot
    // complete an open on any of them, and the remedy is about the session rather
    // than about installing anything.
    if evidence.all_timed_out {
        return Diagnosis {
            cause: FailureCause::NoDeviceAccess,
            summary: "every GPU probe hung and had to be killed — nothing responded".into(),
            remedy: "Check that your graphical session is healthy (compositor still running, \
                     WAYLAND_DISPLAY/DISPLAY pointing at it). If it is, the GPU driver is \
                     most likely wedged — reboot to clear it."
                .into(),
            detail,
        };
    }

    // Arm 4: a loader with nothing to load — no driver manifests anywhere.
    if icds.is_empty() {
        return Diagnosis {
            cause: FailureCause::NoDriverInstalled,
            summary: "no Vulkan driver is installed (the loader found no driver manifests)".into(),
            remedy: distro_hint(&pf.sys_root),
            detail,
        };
    }

    // Arm 5: the loader came up but offers no presentation support at all, so
    // blade fails at instance creation before it ever inspects a device
    // (`Instance extension "VK_KHR_surface" is not supported`, blade's
    // vulkan/init.rs — which then reports the generic NoSupportedDeviceFound).
    // Manifests do exist on disk (arm 4 passed) and the ladder already tried
    // pinning each one, so the usual cause is the loader being told to ignore
    // them: a driver-filtering environment variable, or a loader/driver package
    // pair whose versions were split by a partial upgrade.
    if rung0_stderr.contains("Instance extension") && rung0_stderr.contains("is not supported") {
        return Diagnosis {
            cause: FailureCause::NoDriverInstalled,
            summary: "the Vulkan loader reports no usable driver (no presentation support)".into(),
            remedy: "Unset any driver-filtering environment variables \
                     (VK_LOADER_DRIVERS_DISABLE, VK_LOADER_DRIVERS_SELECT, VK_DRIVER_FILES, \
                     VK_ICD_FILENAMES) and retry. If none are set, reinstall your Vulkan \
                     driver and loader packages together so their versions match."
                .into(),
            detail,
        };
    }

    // Arm 6: devices were found and every one was rejected for lacking a Vulkan
    // feature the renderer requires — blade logs a "Rejected for ..." line per
    // device (API version, a required device extension, inline uniform blocks,
    // timeline semaphores, dynamic rendering). That's hardware or a driver too
    // old for the renderer, not a misconfiguration, so the remedy differs from
    // every arm above: update, or accept software rendering.
    if rung0_stderr.contains("Rejected for") {
        return Diagnosis {
            cause: FailureCause::NoDriverInstalled,
            summary: "this GPU or driver lacks Vulkan features sid's renderer requires".into(),
            remedy: format!(
                "Update your GPU driver and retry. If the hardware itself is too old, install \
                 a software rasterizer instead — sid then falls back to it automatically. \
                 Packages: {}",
                distro_hint(&pf.sys_root)
            ),
            detail,
        };
    }

    // Arm 7: drivers exist but no render node is visible to this session —
    // a VM without 3D acceleration, or device-permission trouble.
    let dri = pf.sys_root.join("dev/dri");
    let has_render_node = std::fs::read_dir(&dri)
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with("renderD"))
        })
        .unwrap_or(false);
    if !has_render_node {
        return Diagnosis {
            cause: FailureCause::NoDeviceAccess,
            summary: "no GPU render node is visible to this session".into(),
            remedy: "In a VM, enable 3D acceleration; on bare metal check that your user can \
                     access /dev/dri (video/render group)."
                .into(),
            detail,
        };
    }

    // Arm 8: a driver is installed, but for hardware this machine does not have.
    // The loader loads it, it enumerates zero devices, and instance creation dies
    // with ERROR_INITIALIZATION_FAILED (issue #1: an Intel Iris Xe laptop whose
    // only manifest was `radeon_icd.json` from vulkan-radeon; the loader's own
    // debug output said "Failed to detect any valid GPUs in the current config").
    //
    // Ordered HERE, second to last, for two reasons. It must come after every arm
    // above because each of those reads a *more specific* signal — a named stderr
    // signature, an oracle's verdict, an absent render node — and any of them
    // being true makes the vendor comparison beside the point: with no /dev/dri at
    // all (arm 7) the drivers' vendors are not what is stopping this session. And
    // it must come before the fallthrough because it is the residual shape the
    // fallthrough was swallowing: hardware present, drivers present, and no
    // correspondence between them, which carries no stderr signature at all (the
    // reported panic is blade info lines and then a bare initialization failure).
    //
    // Software rasterizers are excluded from the comparison by construction: they
    // serve every vendor, and had one been installed the ladder would already have
    // won on it rather than reaching the classifier.
    let hardware_icds: Vec<&IcdManifest> = icds.iter().filter(|i| !i.software).collect();
    if !gpus.is_empty() && !hardware_icds.is_empty() {
        // Deliberately conservative on BOTH sides: the message names an exact
        // package, so it is only earned when every GPU and every hardware driver
        // is one we recognize. An unmapped manifest could be the very driver that
        // serves this GPU, and an unmapped vendor id (virtio-gpu in a VM) has no
        // package to recommend — in either case a confident "install X" is worse
        // than the generic message, so the arm stays silent and falls through.
        let gpu_vendor_ids = unique(gpus.iter().map(|g| g.vendor_id));
        let served: Option<Vec<u16>> = hardware_icds
            .iter()
            .map(|icd| icd_vendor_id(&icd.path))
            .collect();
        let all_gpus_known = gpu_vendor_ids.iter().all(|id| vendor_name(*id).is_some());
        if let (true, Some(served)) = (all_gpus_known, served) {
            let served = unique(served.into_iter());
            let serves_something_present = served.iter().any(|id| gpu_vendor_ids.contains(id));
            if !serves_something_present {
                let names = |ids: &[u16]| {
                    joined_names(
                        &ids.iter()
                            .filter_map(|id| vendor_name(*id))
                            .collect::<Vec<_>>(),
                    )
                };
                return Diagnosis {
                    cause: FailureCause::NoDriverInstalled,
                    summary: format!(
                        "a Vulkan driver is installed, but only for {} GPUs — this machine has \
                         {} graphics",
                        names(&served),
                        names(&gpu_vendor_ids)
                    ),
                    remedy: wrong_vendor_remedy(&pf.sys_root, &gpu_vendor_ids),
                    detail,
                };
            }
        }
    }

    // Fallthrough: real evidence, unrecognized shape — hand the human the tools.
    Diagnosis {
        cause: FailureCause::Unknown,
        summary: "GPU initialization failed for an unrecognized reason".into(),
        remedy: "Run `sid --gpu-report` and file the output.".into(),
        detail,
    }
}

/// Every diagnosis carries the same evidence bundle: what drivers were visible,
/// what kernel modules were loaded, and what the probe actually said.
///
/// `no_capture` distinguishes "the probe said nothing" from "we could not keep
/// what it said": with no writable directory anywhere, the tail is empty for a
/// reason that has nothing to do with the GPU, and a bare `(empty)` would send
/// the reader hunting for a silent probe instead.
fn build_detail(
    icds: &[IcdManifest],
    modules: &[(String, String)],
    gpus: &[GpuEntry],
    rung0_stderr: &str,
    no_capture: bool,
) -> String {
    let mut d = String::from("driver manifests found:\n");
    if icds.is_empty() {
        d.push_str("  (none)\n");
    }
    for icd in icds {
        let class = if icd.software { "software" } else { "hardware" };
        d.push_str(&format!("  {} [{}]\n", icd.path.display(), class));
    }
    d.push_str("kernel GPU modules:\n");
    if modules.is_empty() {
        d.push_str("  (none with a version file)\n");
    }
    for (name, version) in modules {
        d.push_str(&format!("  {name} {version}\n"));
    }
    d.push_str("GPUs present (/sys/class/drm):\n");
    if gpus.is_empty() {
        d.push_str("  (none detected)\n");
    }
    for gpu in gpus {
        d.push_str(&format!("  {}\n", gpu.describe()));
    }
    d.push_str("probe stderr (tail):\n");
    let lines: Vec<&str> = rung0_stderr.lines().collect();
    let tail_start = lines.len().saturating_sub(30);
    if lines.is_empty() {
        if no_capture {
            d.push_str(
                "  probe output could not be captured (no writable directory — state dir and \
                 temp dir both failed)\n",
            );
        } else {
            d.push_str("  (empty)\n");
        }
    }
    for line in &lines[tail_start..] {
        d.push_str(&format!("  {line}\n"));
    }
    d
}

/// One display device as the kernel exposes it under `/sys/class/drm`.
///
/// `device_id` is optional and `vendor_id` is not: a card whose vendor cannot be
/// read tells us nothing and is dropped, but a readable vendor with an
/// unreadable device id is still a GPU whose vendor we must weigh in arm 8 —
/// requiring both would let one missing file silence the diagnosis.
struct GpuEntry {
    /// The `cardN` directory name, e.g. `card1`.
    card: String,
    vendor_id: u16,
    device_id: Option<u16>,
}

impl GpuEntry {
    /// One evidence line: `card1: vendor 0x8086 (Intel), device 0x46a6`.
    fn describe(&self) -> String {
        let vendor = vendor_name(self.vendor_id).unwrap_or("unrecognized vendor");
        let device = match self.device_id {
            Some(id) => format!("0x{id:04x}"),
            None => "unknown".to_string(),
        };
        format!(
            "{}: vendor 0x{:04x} ({vendor}), device {device}",
            self.card, self.vendor_id
        )
    }
}

/// PCI vendor ids of the GPU makers sid can both recognize and name a driver
/// package for. Anything else is left unattributed on purpose — see arm 8.
const PCI_VENDOR_AMD: u16 = 0x1002;
const PCI_VENDOR_INTEL: u16 = 0x8086;
const PCI_VENDOR_NVIDIA: u16 = 0x10de;

/// Human name for a PCI vendor id, or `None` for anything outside the three
/// vendors above (virtio-gpu, VMware, QXL, an ARM SoC block, ...). `None` is a
/// load-bearing answer: it disables the vendor-mismatch claim rather than
/// letting it guess.
fn vendor_name(id: u16) -> Option<&'static str> {
    match id {
        PCI_VENDOR_AMD => Some("AMD"),
        PCI_VENDOR_INTEL => Some("Intel"),
        PCI_VENDOR_NVIDIA => Some("NVIDIA"),
        _ => None,
    }
}

/// The GPUs this machine actually has, read through `sys_root` so the classifier
/// is testable against a fake tree (`lspci` would be a subprocess and a
/// dependency for data the kernel already publishes as files).
///
/// `/sys/class/drm` holds three kinds of entry and only the first is a GPU:
/// `cardN` (the device), `cardN-eDP-1` (a connector *on* that device), and
/// `renderD128` (that same device's render node). Counting the latter two would
/// report one laptop iGPU as three GPUs, so the filter is "starts with `card`
/// and contains no `-`". Sorted by card name for a report that reads the same
/// way twice — `read_dir` order is arbitrary.
fn gpus_present(sys_root: &Path) -> Vec<GpuEntry> {
    let Ok(entries) = std::fs::read_dir(sys_root.join("sys/class/drm")) else {
        return Vec::new();
    };
    let mut out: Vec<GpuEntry> = entries
        .flatten()
        .filter_map(|entry| {
            let card = entry.file_name().to_string_lossy().into_owned();
            if !card.starts_with("card") || card.contains('-') {
                return None;
            }
            let device_dir = entry.path().join("device");
            Some(GpuEntry {
                card,
                vendor_id: read_sysfs_hex(&device_dir.join("vendor"))?,
                device_id: read_sysfs_hex(&device_dir.join("device")),
            })
        })
        .collect();
    out.sort_by(|a, b| a.card.cmp(&b.card));
    out
}

/// A sysfs `0x`-prefixed hex id (`vendor`, `device`). Anything unreadable or
/// unparseable is `None` — never a default, which would invent hardware.
fn read_sysfs_hex(path: &Path) -> Option<u16> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim();
    u16::from_str_radix(text.strip_prefix("0x").unwrap_or(text), 16).ok()
}

/// Which vendor's silicon a driver manifest serves, by needle. Both halves of
/// the reported failure hang off this table, so each entry names a driver that
/// actually ships: RADV (`radeon_icd.json` → `libvulkan_radeon.so`) and AMDVLK
/// (`amd_icd64.json` → `libamdvlk64.so`) for AMD; ANV and the older HasVK
/// (`intel_icd.json`, `intel_hasvk_icd.json`) for Intel; the proprietary driver
/// (`nvidia_icd.json` → `libGLX_nvidia.so`) and Mesa's NVK
/// (`nouveau_icd.json` → `libvulkan_nouveau.so`) for NVIDIA — NVK omitted would
/// tell an NVK-only machine it has no NVIDIA driver at all.
const ICD_VENDOR_NEEDLES: &[(&str, u16)] = &[
    ("radeon", PCI_VENDOR_AMD),
    ("amdvlk", PCI_VENDOR_AMD),
    ("amd_icd", PCI_VENDOR_AMD),
    ("amdgpu", PCI_VENDOR_AMD),
    ("intel", PCI_VENDOR_INTEL),
    ("nvidia", PCI_VENDOR_NVIDIA),
    ("nouveau", PCI_VENDOR_NVIDIA),
];
/// The vendor a manifest's driver serves, or `None` when it is not in the table.
///
/// Two signals, in precedence order: the file name (the packager's own label)
/// and then the manifest body, whose `library_path` names the driver library —
/// the same authority `icd::is_software` uses, and the same plain-substring scan
/// of a tiny JSON rather than a parser dependency. `None` disables arm 8
/// entirely: an unrecognized driver could be exactly the one serving this GPU.
fn icd_vendor_id(manifest: &Path) -> Option<u16> {
    let name = manifest
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();
    if let Some((_, id)) = ICD_VENDOR_NEEDLES.iter().find(|(n, _)| name.contains(n)) {
        return Some(*id);
    }
    let body = std::fs::read_to_string(manifest)
        .unwrap_or_default()
        .to_lowercase();
    ICD_VENDOR_NEEDLES
        .iter()
        .find(|(n, _)| body.contains(n))
        .map(|(_, id)| *id)
}

/// Values in first-seen order, deduplicated — the summary must say "Intel", not
/// "Intel and Intel", on a machine that lists one GPU twice.
fn unique(ids: impl Iterator<Item = u16>) -> Vec<u16> {
    let mut out: Vec<u16> = Vec::new();
    for id in ids {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// `["Intel"]` → "Intel"; `["Intel", "AMD"]` → "Intel and AMD"; three or more
/// take commas. Feeds a `Diagnosis::summary`, which is a single line by contract.
fn joined_names(names: &[&str]) -> String {
    let mut unique: Vec<&str> = Vec::new();
    for name in names {
        if !unique.contains(name) {
            unique.push(name);
        }
    }
    match unique.split_last() {
        None => String::new(),
        Some((last, [])) => (*last).to_string(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

/// The remedy for arm 8: the package that serves the GPUs actually present, for
/// the detected distro, plus the software rasterizer as the optional lower rung.
///
/// Only reached with every id in `gpu_vendor_ids` recognized (arm 8's
/// conservatism), so the per-vendor lookup is total.
fn wrong_vendor_remedy(sys_root: &Path, gpu_vendor_ids: &[u16]) -> String {
    let tokens = distro_tokens(sys_root);
    let has = |t: &str| tokens.iter().any(|x| x == t);
    // (installer command, package per vendor, software-rasterizer package)
    let (installer, package, software): (Option<&str>, fn(u16) -> &'static str, &str) =
        if has("arch") {
            (
                Some("sudo pacman -S"),
                |id| match id {
                    PCI_VENDOR_AMD => "vulkan-radeon",
                    PCI_VENDOR_NVIDIA => "nvidia-utils",
                    _ => "vulkan-intel",
                },
                "vulkan-swrast",
            )
        } else if has("debian") || has("ubuntu") {
            (
                Some("sudo apt install"),
                |id| match id {
                    PCI_VENDOR_NVIDIA => "nvidia-driver",
                    _ => "mesa-vulkan-drivers",
                },
                "mesa-vulkan-drivers",
            )
        } else if has("fedora") || has("rhel") || has("centos") {
            (
                Some("sudo dnf install"),
                |id| match id {
                    PCI_VENDOR_NVIDIA => "akmod-nvidia",
                    _ => "mesa-vulkan-drivers",
                },
                "mesa-vulkan-drivers",
            )
        } else {
            (
                None,
                |id| match id {
                    PCI_VENDOR_NVIDIA => "the NVIDIA driver package",
                    _ => "mesa-vulkan-drivers",
                },
                "mesa-vulkan-drivers",
            )
        };

    let packages: Vec<&str> = {
        let mut out: Vec<&str> = Vec::new();
        for id in gpu_vendor_ids {
            let p = package(*id);
            if !out.contains(&p) {
                out.push(p);
            }
        }
        out
    };
    let install = match installer {
        Some(cmd) => format!("{cmd} {}", packages.join(" ")),
        None => format!("install {}", packages.join(" and ")),
    };
    // On Mesa-packaged distros the driver and the rasterizer are the same
    // package, and telling someone to install it "instead" is nonsense.
    if packages.contains(&software) {
        format!(
            "Install the Vulkan driver for the GPU you actually have: {install}. (That package \
             also ships the lavapipe software rasterizer, which sid falls back to on its own if \
             the hardware driver still cannot start.)"
        )
    } else {
        format!(
            "Install the Vulkan driver for the GPU you actually have: {install}. If you would \
             rather run without a GPU driver, install the software rasterizer {software} instead \
             — sid falls back to it automatically."
        )
    }
}

/// `ID` + `ID_LIKE` tokens from `/etc/os-release`, lowercased. Shared by both
/// remedy builders so a distro is recognized identically wherever it matters.
fn distro_tokens(sys_root: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(sys_root.join("etc/os-release")).unwrap_or_default();
    let mut tokens: Vec<String> = Vec::new();
    for line in text.lines() {
        for key in ["ID=", "ID_LIKE="] {
            if let Some(value) = line.strip_prefix(key) {
                let value = value.trim().trim_matches('"');
                tokens.extend(value.split_whitespace().map(|t| t.to_lowercase()));
            }
        }
    }
    tokens
}

/// Distro-aware install hint from `/etc/os-release` (`ID` + `ID_LIKE` tokens).
fn distro_hint(sys_root: &Path) -> String {
    let tokens = distro_tokens(sys_root);
    let has = |t: &str| tokens.iter().any(|x| x == t);
    if has("arch") {
        "sudo pacman -S vulkan-radeon  # or vulkan-intel / nvidia-utils for your GPU; \
         add vulkan-swrast for a software fallback"
            .into()
    } else if has("debian") || has("ubuntu") {
        "sudo apt install mesa-vulkan-drivers".into()
    } else if has("fedora") || has("rhel") || has("centos") {
        "sudo dnf install mesa-vulkan-drivers".into()
    } else {
        "Install your GPU's Vulkan driver package (Mesa for AMD/Intel, nvidia-utils for NVIDIA)."
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icd;

    /// A preflight over a fake root; `nvidia_smi_cmd` defaults to a command that
    /// can't match arm 2 (exits 0, no output).
    fn pf(sys_root: &Path, state: &Path) -> LinuxGpuPreflight {
        LinuxGpuPreflight {
            state_dir: state.to_path_buf(),
            probe_cmd: vec!["/bin/true".into()],
            timeout: Duration::from_secs(1),
            ladder_budget: crate::ladder::LADDER_BUDGET,
            sys_root: sys_root.to_path_buf(),
            icd_dirs: Some(vec![]),
            nvidia_smi_cmd: vec!["/bin/true".into()],
            app_version: "test".into(),
            env_snapshot: Vec::new(),
            instance_id: "test-instance".into(),
        }
    }

    fn fake_root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    /// The ordinary evidence shape: rungs ran and failed with their output kept,
    /// so only `stderr` decides.
    fn ev(rung0_stderr: &str) -> LadderEvidence<'_> {
        LadderEvidence {
            rung0_stderr,
            rung0_no_capture: false,
            all_timed_out: false,
        }
    }

    /// A fake `nvidia-smi` in the mismatched state: nonzero exit + the phrase.
    fn mismatching_smi() -> Vec<String> {
        vec![
            "/bin/sh".into(),
            "-c".into(),
            "echo 'Failed to initialize NVML: Driver/library version mismatch' >&2; exit 12".into(),
        ]
    }

    #[test]
    fn missing_loader_classifies_as_no_driver_with_distro_hint() {
        let root = fake_root();
        std::fs::create_dir_all(root.path().join("etc")).unwrap();
        std::fs::write(root.path().join("etc/os-release"), "ID=arch\n").unwrap();
        let state = fake_root();
        let d = diagnose(
            &pf(root.path(), state.path()),
            &[],
            &[],
            &ev("Missing Vulkan entry points: LibraryLoadFailure(...)"),
        );
        assert_eq!(d.cause, FailureCause::NoDriverInstalled);
        assert!(d.summary.contains("loader"));
        assert!(
            d.remedy.contains("pacman"),
            "arch hint expected: {}",
            d.remedy
        );
    }

    #[test]
    fn nvidia_mismatch_wins_when_smi_reports_it() {
        let root = fake_root();
        std::fs::create_dir_all(root.path().join("sys/module/nvidia")).unwrap();
        std::fs::write(root.path().join("sys/module/nvidia/version"), "570.86.16\n").unwrap();
        let state = fake_root();
        let mut p = pf(root.path(), state.path());
        p.nvidia_smi_cmd = mismatching_smi();
        let d = diagnose(
            &p,
            &[],
            &[("nvidia".into(), "570.86.16".into())],
            &ev("some stderr"),
        );
        assert_eq!(d.cause, FailureCause::DriverMismatchRebootNeeded);
        assert!(d.remedy.to_lowercase().contains("reboot"));
    }

    /// The oracle here is nvidia-smi's OUTPUT, not its exit status, so an
    /// unwritable state dir used to lose this arm outright: `File::create` failed,
    /// `run_captured` returned nothing, and a post-driver-update NVIDIA box was
    /// told to file a bug instead of to reboot. The temp-dir fallback fixes it.
    /// (Above the fallback there is still a floor — nowhere writable at all loses
    /// the arm; see `probe::run_captured`.)
    #[test]
    fn nvidia_mismatch_survives_an_unwritable_state_dir() {
        let root = fake_root();
        std::fs::create_dir_all(root.path().join("sys/module/nvidia")).unwrap();
        std::fs::write(root.path().join("sys/module/nvidia/version"), "570.86.16\n").unwrap();
        let icd_dir = fake_root();
        std::fs::write(icd_dir.path().join("nvidia_icd.json"), "{}").unwrap();
        let icds = icd::scan(
            Path::new("/nonexistent"),
            Some(&[icd_dir.path().to_path_buf()]),
        );
        let mut p = pf(root.path(), Path::new("/dev/null/nope"));
        p.nvidia_smi_cmd = mismatching_smi();
        let d = diagnose(
            &p,
            &icds,
            &[("nvidia".into(), "570.86.16".into())],
            &ev("some stderr"),
        );
        assert_eq!(
            d.cause,
            FailureCause::DriverMismatchRebootNeeded,
            "an unwritable state dir must not silence the mismatch oracle: {}",
            d.summary
        );
    }

    /// Finding 4: with nowhere writable, rung 0's stderr is empty for a reason
    /// that has nothing to do with the GPU — the detail must say which.
    #[test]
    fn a_lost_capture_is_explained_instead_of_printed_as_empty() {
        let (root, _icd_dir, icds) = healthy_hardware_world();
        let state = fake_root();
        let lost = diagnose(
            &pf(root.path(), state.path()),
            &icds,
            &[],
            &LadderEvidence {
                rung0_stderr: "",
                rung0_no_capture: true,
                all_timed_out: false,
            },
        );
        assert!(
            lost.detail.contains("could not be captured")
                && lost.detail.contains("no writable directory"),
            "the detail must say why there is no evidence: {}",
            lost.detail
        );
        assert!(
            !lost.detail.contains("(empty)"),
            "a bare (empty) hides the real fault: {}",
            lost.detail
        );
        // And a probe that ran, kept its output, and simply said nothing still
        // reads as empty — the two states must stay distinguishable.
        let silent = diagnose(&pf(root.path(), state.path()), &icds, &[], &ev(""));
        assert!(silent.detail.contains("(empty)"), "{}", silent.detail);
    }

    #[test]
    fn nvidia_present_but_healthy_smi_falls_through() {
        let root = fake_root();
        std::fs::create_dir_all(root.path().join("sys/module/nvidia")).unwrap();
        std::fs::write(root.path().join("sys/module/nvidia/version"), "570.86.16\n").unwrap();
        // No manifests → should land on arm 3, not the mismatch arm.
        let state = fake_root();
        std::fs::create_dir_all(root.path().join("etc")).unwrap();
        std::fs::write(
            root.path().join("etc/os-release"),
            "ID=ubuntu\nID_LIKE=debian\n",
        )
        .unwrap();
        let d = diagnose(&pf(root.path(), state.path()), &[], &[], &ev("stderr"));
        assert_eq!(d.cause, FailureCause::NoDriverInstalled);
        assert!(
            d.remedy.contains("apt"),
            "debian-family hint expected: {}",
            d.remedy
        );
    }

    #[test]
    fn no_render_node_classifies_as_device_access() {
        let root = fake_root();
        // One (hardware) manifest exists, so arm 3 passes; /dev/dri is absent.
        let icd_dir = fake_root();
        std::fs::write(icd_dir.path().join("radeon_icd.json"), "{}").unwrap();
        let icds = icd::scan(
            Path::new("/nonexistent"),
            Some(&[icd_dir.path().to_path_buf()]),
        );
        let state = fake_root();
        let d = diagnose(&pf(root.path(), state.path()), &icds, &[], &ev("stderr"));
        assert_eq!(d.cause, FailureCause::NoDeviceAccess);
    }

    #[test]
    fn unknown_fallthrough_keeps_the_evidence() {
        let root = fake_root();
        let icd_dir = fake_root();
        std::fs::write(icd_dir.path().join("radeon_icd.json"), "{}").unwrap();
        let icds = icd::scan(
            Path::new("/nonexistent"),
            Some(&[icd_dir.path().to_path_buf()]),
        );
        // A render node exists → all specific arms pass → Unknown.
        std::fs::create_dir_all(root.path().join("dev/dri")).unwrap();
        std::fs::write(root.path().join("dev/dri/renderD128"), "").unwrap();
        let state = fake_root();
        let stderr = "ERROR blade_graphics::vulkan::init] enumerate_physical_devices: ERROR_INITIALIZATION_FAILED";
        let d = diagnose(&pf(root.path(), state.path()), &icds, &[], &ev(stderr));
        assert_eq!(d.cause, FailureCause::Unknown);
        assert!(d.remedy.contains("--gpu-report"));
        assert!(d.detail.contains("radeon_icd.json"));
        assert!(d.detail.contains("ERROR_INITIALIZATION_FAILED"));
    }

    /// Fixture for the two stderr-signature arms: manifests present and a render
    /// node present, so only the stderr shape can decide the outcome.
    fn healthy_hardware_world() -> (tempfile::TempDir, tempfile::TempDir, Vec<IcdManifest>) {
        let root = fake_root();
        std::fs::create_dir_all(root.path().join("dev/dri")).unwrap();
        std::fs::write(root.path().join("dev/dri/renderD128"), "").unwrap();
        std::fs::create_dir_all(root.path().join("etc")).unwrap();
        std::fs::write(root.path().join("etc/os-release"), "ID=arch\n").unwrap();
        let icd_dir = fake_root();
        std::fs::write(icd_dir.path().join("intel_icd.json"), "{}").unwrap();
        let icds = icd::scan(
            Path::new("/nonexistent"),
            Some(&[icd_dir.path().to_path_buf()]),
        );
        (root, icd_dir, icds)
    }

    /// Observed live: `VK_LOADER_DRIVERS_DISABLE='*'` makes blade fail at instance
    /// creation, which it reports as the generic NoSupportedDeviceFound. Without
    /// this arm the user gets "unrecognized reason" for a one-variable fix.
    #[test]
    fn no_presentation_support_blames_driver_filtering() {
        let (root, _icd_dir, icds) = healthy_hardware_world();
        let state = fake_root();
        let stderr = "[ERROR blade_graphics::hal::init] Instance extension \
                      \"VK_KHR_surface\" is not supported\n\
                      Unable to init GPU context: NoSupportedDeviceFound";
        let d = diagnose(&pf(root.path(), state.path()), &icds, &[], &ev(stderr));
        assert_eq!(d.cause, FailureCause::NoDriverInstalled);
        assert!(d.summary.contains("no usable driver"), "{}", d.summary);
        assert!(
            d.remedy.contains("VK_LOADER_DRIVERS_DISABLE") && d.remedy.contains("VK_DRIVER_FILES"),
            "the remedy must name the variables to unset: {}",
            d.remedy
        );
    }

    /// Every device enumerated but rejected for a missing feature — an old GPU or
    /// old driver, whose remedy (update, or accept software rendering) differs
    /// from the driver-filtering arm above.
    #[test]
    fn rejected_devices_blame_an_old_gpu_or_driver() {
        let (root, _icd_dir, icds) = healthy_hardware_world();
        let state = fake_root();
        let stderr = "[WARN blade_graphics::hal::init] Rejected for device extension \
                      \"VK_KHR_dynamic_rendering\" not supported. Please update the driver!";
        let d = diagnose(&pf(root.path(), state.path()), &icds, &[], &ev(stderr));
        assert_eq!(d.cause, FailureCause::NoDriverInstalled);
        assert!(d.summary.contains("lacks Vulkan features"), "{}", d.summary);
        assert!(d.remedy.contains("software rasterizer"), "{}", d.remedy);
        // Carries the distro package hint for the update it just recommended.
        assert!(d.remedy.contains("pacman"), "{}", d.remedy);
    }

    /// The stderr arms must not swallow the generic case: an initialization
    /// failure with neither signature still reaches the fallthrough.
    #[test]
    fn stderr_arms_do_not_capture_unrelated_failures() {
        let (root, _icd_dir, icds) = healthy_hardware_world();
        let state = fake_root();
        let d = diagnose(
            &pf(root.path(), state.path()),
            &icds,
            &[],
            &ev("create_device: ERROR_INITIALIZATION_FAILED"),
        );
        assert_eq!(d.cause, FailureCause::Unknown);
    }

    #[test]
    fn unknown_distro_gets_the_generic_hint() {
        let root = fake_root();
        let state = fake_root();
        let d = diagnose(&pf(root.path(), state.path()), &[], &[], &ev("x"));
        assert_eq!(d.cause, FailureCause::NoDriverInstalled);
        assert!(d.remedy.contains("Vulkan driver package"));
    }

    /// Write a GPU into a fake root the way the kernel exposes one: a `cardN`
    /// entry under `/sys/class/drm` whose `device/` holds the PCI ids. Connector
    /// entries (`card1-eDP-1`) and the render node live in the same directory on
    /// a real machine, so the fixture writes them too — they must not be counted
    /// as extra GPUs.
    fn add_gpu(root: &Path, card: &str, vendor_id: &str, device_id: &str) {
        let dev = root.join("sys/class/drm").join(card).join("device");
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::write(dev.join("vendor"), format!("{vendor_id}\n")).unwrap();
        std::fs::write(dev.join("device"), format!("{device_id}\n")).unwrap();
        // The noise a real /sys/class/drm carries alongside each card.
        let drm = root.join("sys/class/drm");
        std::fs::create_dir_all(drm.join(format!("{card}-eDP-1"))).unwrap();
        std::fs::create_dir_all(drm.join("renderD128").join("device")).unwrap();
    }

    /// Driver manifests with the content shape the real files have, so the
    /// `library_path` half of the vendor classification is exercised rather than
    /// only the file name.
    fn write_icds(dir: &Path, manifests: &[(&str, &str)]) -> Vec<IcdManifest> {
        for (name, library) in manifests {
            std::fs::write(
                dir.join(name),
                format!(
                    "{{\n    \"ICD\": {{\n        \"api_version\": \"1.4.348\",\n        \
                     \"library_path\": \"{library}\"\n    }},\n    \
                     \"file_format_version\": \"1.0.1\"\n}}\n"
                ),
            )
            .unwrap();
        }
        icd::scan(Path::new("/nonexistent"), Some(&[dir.to_path_buf()]))
    }

    /// A root with a render node and an Arch os-release: arms 1-7 all pass, so
    /// only the hardware/driver correspondence can decide.
    fn arch_root_with_render_node() -> tempfile::TempDir {
        let root = fake_root();
        std::fs::create_dir_all(root.path().join("dev/dri")).unwrap();
        std::fs::write(root.path().join("dev/dri/renderD128"), "").unwrap();
        std::fs::create_dir_all(root.path().join("etc")).unwrap();
        std::fs::write(root.path().join("etc/os-release"), "ID=arch\n").unwrap();
        root
    }

    /// What this machine's stderr actually looks like: blade's info chatter and
    /// then a bare initialization failure — no signature any other arm can read.
    const REPORTED_STDERR: &str = "[INFO blade_graphics::hal::init] Adapter inspection\n\
         thread 'main' panicked at gpui/src/platform/linux/wayland/client.rs:451:14:\n\
         Unable to init GPU context: Platform(Init(ERROR_INITIALIZATION_FAILED))";

    /// Issue #1, exactly as reported: an Intel Iris Xe laptop whose only Vulkan
    /// driver is `vulkan-radeon` (RADV, AMD-only). The loader loads RADV, RADV
    /// enumerates zero AMD devices, and instance creation dies — a shape with no
    /// stderr signature at all, so before this arm the user was told to file a
    /// bug for what is one `pacman -S` away.
    #[test]
    fn a_driver_for_the_wrong_vendor_names_both_sides() {
        let root = arch_root_with_render_node();
        add_gpu(root.path(), "card1", "0x8086", "0x46a6");
        let icd_dir = fake_root();
        let icds = write_icds(
            icd_dir.path(),
            &[("radeon_icd.x86_64.json", "libvulkan_radeon.so")],
        );
        let state = fake_root();
        let d = diagnose(
            &pf(root.path(), state.path()),
            &icds,
            &[],
            &ev(REPORTED_STDERR),
        );
        assert_eq!(d.cause, FailureCause::NoDriverInstalled);
        assert!(
            d.summary.contains("AMD") && d.summary.contains("Intel"),
            "the summary must name the driver's vendor AND the machine's: {}",
            d.summary
        );
        assert!(
            d.remedy.contains("vulkan-intel"),
            "the remedy must name the package that actually fixes it: {}",
            d.remedy
        );
        assert!(
            d.remedy.contains("vulkan-swrast"),
            "the software-fallback rung must be offered too: {}",
            d.remedy
        );
        assert!(
            d.detail.contains("0x8086") && d.detail.contains("Intel"),
            "the evidence must show the hardware that was seen: {}",
            d.detail
        );
    }

    /// A working PRIME laptop: AMD iGPU + NVIDIA dGPU with only RADV installed
    /// renders fine on the AMD side. One match is enough — claiming a mismatch
    /// here would be a confident lie about a healthy configuration.
    #[test]
    fn a_driver_matching_any_present_gpu_never_fires() {
        let root = arch_root_with_render_node();
        add_gpu(root.path(), "card0", "0x1002", "0x164e");
        add_gpu(root.path(), "card1", "0x10de", "0x25a2");
        let icd_dir = fake_root();
        let icds = write_icds(
            icd_dir.path(),
            &[("radeon_icd.x86_64.json", "libvulkan_radeon.so")],
        );
        let state = fake_root();
        let d = diagnose(
            &pf(root.path(), state.path()),
            &icds,
            &[],
            &ev(REPORTED_STDERR),
        );
        assert_eq!(d.cause, FailureCause::Unknown, "{}", d.summary);
        assert!(!d.summary.contains("only for"), "{}", d.summary);
    }

    /// Conservative by construction: the claim names a specific package, so it
    /// is only made when BOTH sides are recognized. An unmapped GPU vendor
    /// (virtio-gpu in a VM) or an unmapped manifest silences it — the generic
    /// message costs far less than a confident "install X" that is wrong.
    #[test]
    fn unknown_vendors_on_either_side_stay_unknown() {
        let state = fake_root();

        // Unknown hardware, known driver.
        let vm = arch_root_with_render_node();
        add_gpu(vm.path(), "card0", "0x1af4", "0x1050");
        let vm_icds_dir = fake_root();
        let vm_icds = write_icds(
            vm_icds_dir.path(),
            &[("radeon_icd.x86_64.json", "libvulkan_radeon.so")],
        );
        let d = diagnose(
            &pf(vm.path(), state.path()),
            &vm_icds,
            &[],
            &ev(REPORTED_STDERR),
        );
        assert_eq!(d.cause, FailureCause::Unknown, "{}", d.summary);

        // Known hardware, unmapped driver.
        let box_ = arch_root_with_render_node();
        add_gpu(box_.path(), "card1", "0x8086", "0x46a6");
        let odd_dir = fake_root();
        let odd_icds = write_icds(odd_dir.path(), &[("powervr_icd.json", "libPVRVK.so")]);
        let d = diagnose(
            &pf(box_.path(), state.path()),
            &odd_icds,
            &[],
            &ev(REPORTED_STDERR),
        );
        assert_eq!(d.cause, FailureCause::Unknown, "{}", d.summary);
    }

    /// No GPU visible at all is not evidence of a vendor mismatch — and the
    /// evidence must say so rather than leaving the section blank.
    #[test]
    fn no_detectable_gpu_never_claims_a_vendor_mismatch() {
        let root = arch_root_with_render_node();
        let icd_dir = fake_root();
        let icds = write_icds(
            icd_dir.path(),
            &[("radeon_icd.x86_64.json", "libvulkan_radeon.so")],
        );
        let state = fake_root();
        let d = diagnose(
            &pf(root.path(), state.path()),
            &icds,
            &[],
            &ev(REPORTED_STDERR),
        );
        assert_eq!(d.cause, FailureCause::Unknown, "{}", d.summary);
        assert!(d.detail.contains("(none detected)"), "{}", d.detail);
    }

    /// A lavapipe-only machine has no hardware driver to be wrong about — the
    /// arm needs a hardware manifest on the other side of the comparison, so
    /// this shape keeps whatever diagnosis it had before.
    #[test]
    fn a_software_only_driver_set_is_not_a_vendor_mismatch() {
        let root = arch_root_with_render_node();
        add_gpu(root.path(), "card1", "0x8086", "0x46a6");
        let icd_dir = fake_root();
        let icds = write_icds(
            icd_dir.path(),
            &[("lvp_icd.x86_64.json", "libvulkan_lvp.so")],
        );
        assert!(icds[0].software, "fixture must be a software rasterizer");
        let state = fake_root();
        let d = diagnose(
            &pf(root.path(), state.path()),
            &icds,
            &[],
            &ev(REPORTED_STDERR),
        );
        assert_eq!(d.cause, FailureCause::Unknown, "{}", d.summary);
        assert!(!d.summary.contains("only for"), "{}", d.summary);
    }

    /// A lavapipe manifest sitting beside the wrong-vendor hardware driver is a
    /// common Mesa install, and it must not cost the user the diagnosis: a
    /// software rasterizer is not attributable to any vendor, so counting it as
    /// a hardware driver would trip the "unrecognized manifest" conservatism and
    /// silence an arm that has everything it needs to fire.
    #[test]
    fn a_software_rasterizer_alongside_it_does_not_suppress_the_diagnosis() {
        let root = arch_root_with_render_node();
        add_gpu(root.path(), "card1", "0x8086", "0x46a6");
        let icd_dir = fake_root();
        let icds = write_icds(
            icd_dir.path(),
            &[
                ("radeon_icd.x86_64.json", "libvulkan_radeon.so"),
                ("lvp_icd.x86_64.json", "libvulkan_lvp.so"),
            ],
        );
        let state = fake_root();
        let d = diagnose(
            &pf(root.path(), state.path()),
            &icds,
            &[],
            &ev(REPORTED_STDERR),
        );
        assert_eq!(d.cause, FailureCause::NoDriverInstalled, "{}", d.summary);
        assert!(
            d.summary.contains("only for AMD") && d.summary.contains("Intel graphics"),
            "{}",
            d.summary
        );
    }

    /// The single line that would have made issue #1 self-diagnosing: what GPU
    /// is in the machine. It rides on EVERY diagnosis, not just the new arm —
    /// on Intel the "kernel GPU modules" section is empty (i915 exposes no
    /// `/sys/module/i915/version`), so without this the report named no hardware
    /// whatsoever.
    #[test]
    fn the_detail_lists_the_gpus_present() {
        let root = arch_root_with_render_node();
        add_gpu(root.path(), "card1", "0x8086", "0x46a6");
        let icd_dir = fake_root();
        // A matching driver, so the diagnosis is the generic one.
        let icds = write_icds(
            icd_dir.path(),
            &[("intel_icd.x86_64.json", "libvulkan_intel.so")],
        );
        let state = fake_root();
        let d = diagnose(
            &pf(root.path(), state.path()),
            &icds,
            &[],
            &ev(REPORTED_STDERR),
        );
        assert_eq!(d.cause, FailureCause::Unknown, "{}", d.summary);
        assert!(
            d.detail
                .contains("card1: vendor 0x8086 (Intel), device 0x46a6"),
            "the evidence must identify the hardware: {}",
            d.detail
        );
        // Connector entries and the render node share the directory with the
        // card and must not be mistaken for further GPUs.
        assert_eq!(
            d.detail.matches("vendor 0x").count(),
            1,
            "one GPU, one line: {}",
            d.detail
        );
    }

    /// Observed live: a compositor socket that accepts but never answers hangs
    /// every rung identically (40.17s of silence with two manifests). The stderr
    /// is truncated mid-init, so before this arm existed the user was told
    /// "unrecognized reason" — for a session-level fault with an obvious remedy.
    #[test]
    fn every_rung_timing_out_blames_the_session_not_the_configuration() {
        let (root, _icd_dir, icds) = healthy_hardware_world();
        let state = fake_root();
        let d = diagnose(
            &pf(root.path(), state.path()),
            &icds,
            &[],
            &LadderEvidence {
                // What a killed probe leaves behind: a start, no verdict.
                rung0_stderr: "[INFO blade_graphics::hal::init] Adapter inspection",
                rung0_no_capture: false,
                all_timed_out: true,
            },
        );
        assert_eq!(d.cause, FailureCause::NoDeviceAccess);
        assert!(d.summary.contains("hung"), "{}", d.summary);
        assert!(
            d.remedy.contains("compositor") && d.remedy.to_lowercase().contains("reboot"),
            "the remedy must point at the session and the wedged driver: {}",
            d.remedy
        );
        // The converse — rungs that ran and failed must NOT reach this arm — is
        // `stderr_arms_do_not_capture_unrelated_failures` above, which asserts
        // Unknown for the same world with `all_timed_out: false`.
    }
}
