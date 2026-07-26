//! The GPU-environment fingerprint: a cheap hash of everything that could
//! change the probe's verdict — driver manifests (path/size/mtime), kernel GPU
//! module versions, the loader/display environment, and sid's own version. A
//! matching stamp means "same world as last time" and the probe is skipped
//! entirely; any driver install/update, kernel driver change, loader-env change,
//! or sid upgrade re-triggers exactly one probe.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;

use crate::icd::IcdManifest;

/// Kernel modules whose version files participate in the fingerprint. Not all
/// drivers expose `/sys/module/<name>/version` (i915 doesn't) — absent files are
/// simply skipped; the ICD manifest mtimes carry the update signal in that case.
pub(crate) const KERNEL_MODULES: &[&str] =
    &["nvidia", "nvidia_drm", "amdgpu", "i915", "xe", "nouveau"];

/// The loaded GPU kernel modules' versions, as (name, version) pairs.
pub(crate) fn kernel_modules(sys_root: &Path) -> Vec<(String, String)> {
    KERNEL_MODULES
        .iter()
        .filter_map(|name| {
            let path = sys_root.join("sys/module").join(name).join("version");
            std::fs::read_to_string(&path)
                .ok()
                .map(|v| (name.to_string(), v.trim().to_string()))
        })
        .collect()
}

/// Environment variables whose VALUE changes the probe's verdict, so a verdict
/// cached under one launch environment is never reused under another. The probe
/// child inherits the parent's environment, so without this a launch with e.g.
/// `DRI_PRIME=1` would cache a verdict that a later plain launch also matches —
/// wrong classification, stale pins.
///
/// Rationale, in groups: the first six steer the Vulkan **loader** (which ICD
/// manifests and layers it reads at all); `ZED_DEVICE_ID` is the renderer's own
/// PCI device-id override, which decides which enumerated device is accepted; the
/// Mesa/PRIME group re-points the driver or the physical device selection; the XDG
/// tiers change *where* manifests are discovered (see `icd::default_dirs`).
const ENV_VALUE_KEYS: &[&str] = &[
    "VK_DRIVER_FILES",
    "VK_ICD_FILENAMES",
    "VK_LOADER_DRIVERS_SELECT",
    "VK_LOADER_DRIVERS_DISABLE",
    "VK_LOADER_LAYERS_ENABLE",
    "VK_INSTANCE_LAYERS",
    "ZED_DEVICE_ID",
    "MESA_VK_DEVICE_SELECT",
    "MESA_LOADER_DRIVER_OVERRIDE",
    "DRI_PRIME",
    "__NV_PRIME_RENDER_OFFLOAD",
    "__GLX_VENDOR_LIBRARY_NAME",
    "XDG_DATA_HOME",
    "XDG_DATA_DIRS",
    "XDG_CONFIG_HOME",
    "XDG_CONFIG_DIRS",
];

/// Display-server variables recorded by PRESENCE only. Which of these is set
/// decides the windowing client and surface extension the renderer uses, but the
/// socket name changes per session — hashing the value would thrash the cache on
/// every login for no verdict change.
const ENV_PRESENCE_KEYS: &[&str] = &["WAYLAND_DISPLAY", "DISPLAY"];

/// Placeholder recorded for a present-but-unhashed [`ENV_PRESENCE_KEYS`] variable.
const PRESENT: &str = "<set>";

/// Snapshot the fingerprint-relevant environment. Injected into
/// `LinuxGpuPreflight` rather than read inside [`fingerprint`] so tests stay
/// hermetic — this crate never mutates the environment (`set_var` is unsafe in
/// edition 2024 and the whole module is threadless by discipline).
///
/// Absent variables are omitted entirely (so "unset" and "set to empty" differ),
/// and the result is sorted for determinism.
pub(crate) fn env_snapshot() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for key in ENV_VALUE_KEYS {
        if let Some(value) = std::env::var_os(key) {
            out.push((key.to_string(), value.to_string_lossy().into_owned()));
        }
    }
    for key in ENV_PRESENCE_KEYS {
        if std::env::var_os(key).is_some() {
            out.push((key.to_string(), PRESENT.to_string()));
        }
    }
    out.sort();
    out
}

/// Hash the environment into a stable hex token. `icds` arrive in `icd::scan`'s
/// deterministic order and `env` in [`env_snapshot`]'s sorted order, so the same
/// world always hashes the same.
pub(crate) fn fingerprint(
    icds: &[IcdManifest],
    modules: &[(String, String)],
    env: &[(String, String)],
    app_version: &str,
) -> String {
    let mut h = DefaultHasher::new();
    for icd in icds {
        icd.path.hash(&mut h);
        icd.len.hash(&mut h);
        icd.mtime_unix.hash(&mut h);
    }
    for (name, version) in modules {
        name.hash(&mut h);
        version.hash(&mut h);
    }
    for (key, value) in env {
        key.hash(&mut h);
        value.hash(&mut h);
    }
    app_version.hash(&mut h);
    format!("{:016x}", h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icd;

    fn scan_of(dir: &Path) -> Vec<IcdManifest> {
        icd::scan(Path::new("/nonexistent"), Some(&[dir.to_path_buf()]))
    }

    #[test]
    fn stable_across_rescans_of_the_same_world() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("radeon_icd.json"), "{}").unwrap();
        let a = fingerprint(&scan_of(dir.path()), &[], &[], "0.0.1");
        let b = fingerprint(&scan_of(dir.path()), &[], &[], "0.0.1");
        assert_eq!(a, b);
    }

    #[test]
    fn changes_when_a_manifest_appears_or_changes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("radeon_icd.json"), "{}").unwrap();
        let before = fingerprint(&scan_of(dir.path()), &[], &[], "0.0.1");

        // A new driver install (new manifest) must change the fingerprint...
        std::fs::write(dir.path().join("intel_icd.json"), "{}").unwrap();
        let with_new = fingerprint(&scan_of(dir.path()), &[], &[], "0.0.1");
        assert_ne!(before, with_new);

        // ...and so must a content change to an existing one (size differs).
        std::fs::write(dir.path().join("intel_icd.json"), r#"{"updated": true}"#).unwrap();
        let with_changed = fingerprint(&scan_of(dir.path()), &[], &[], "0.0.1");
        assert_ne!(with_new, with_changed);
    }

    #[test]
    fn changes_with_kernel_module_version_and_app_version() {
        let icds: Vec<IcdManifest> = Vec::new();
        let m1 = vec![("nvidia".to_string(), "570.86.16".to_string())];
        let m2 = vec![("nvidia".to_string(), "575.51.02".to_string())];
        assert_ne!(
            fingerprint(&icds, &m1, &[], "0.0.1"),
            fingerprint(&icds, &m2, &[], "0.0.1")
        );
        assert_ne!(
            fingerprint(&icds, &m1, &[], "0.0.1"),
            fingerprint(&icds, &m1, &[], "0.0.2")
        );
    }

    #[test]
    fn changes_with_the_loader_environment_snapshot() {
        let icds: Vec<IcdManifest> = Vec::new();
        let plain: Vec<(String, String)> = Vec::new();
        let prime = vec![("DRI_PRIME".to_string(), "1".to_string())];
        let pinned = vec![(
            "VK_DRIVER_FILES".to_string(),
            "/usr/share/vulkan/icd.d/lvp_icd.x86_64.json".to_string(),
        )];

        // A launch with loader env set must not reuse a plain launch's verdict.
        assert_ne!(
            fingerprint(&icds, &[], &plain, "0.0.1"),
            fingerprint(&icds, &[], &prime, "0.0.1")
        );
        assert_ne!(
            fingerprint(&icds, &[], &prime, "0.0.1"),
            fingerprint(&icds, &[], &pinned, "0.0.1")
        );
        // A differing VALUE under the same key counts too.
        let prime2 = vec![("DRI_PRIME".to_string(), "0".to_string())];
        assert_ne!(
            fingerprint(&icds, &[], &prime, "0.0.1"),
            fingerprint(&icds, &[], &prime2, "0.0.1")
        );
        // An identical snapshot is stable.
        assert_eq!(
            fingerprint(&icds, &[], &prime, "0.0.1"),
            fingerprint(&icds, &[], &prime.clone(), "0.0.1")
        );
    }

    #[test]
    fn display_vars_are_recorded_by_presence_not_value() {
        // The snapshot builder is env-reading, so assert the semantics on the
        // shape it produces: a display var never contributes its socket name, so
        // two sessions with different socket names hash identically.
        let session_a = vec![("WAYLAND_DISPLAY".to_string(), PRESENT.to_string())];
        let session_b = vec![("WAYLAND_DISPLAY".to_string(), PRESENT.to_string())];
        assert_eq!(
            fingerprint(&[], &[], &session_a, "0.0.1"),
            fingerprint(&[], &[], &session_b, "0.0.1")
        );
        // But present-vs-absent (Wayland vs a headless/X11 launch) does differ.
        assert_ne!(
            fingerprint(&[], &[], &session_a, "0.0.1"),
            fingerprint(&[], &[], &[], "0.0.1")
        );

        // And the live snapshot never leaks a socket name for those keys.
        for (key, value) in env_snapshot() {
            if ENV_PRESENCE_KEYS.contains(&key.as_str()) {
                assert_eq!(value, PRESENT, "{key} must be presence-only");
            }
        }
    }

    #[test]
    fn env_snapshot_is_sorted_and_omits_absent_vars() {
        let snap = env_snapshot();
        let mut sorted = snap.clone();
        sorted.sort();
        assert_eq!(snap, sorted, "the snapshot must be deterministic");
        // Only keys we actually track may appear, and each at most once.
        let mut keys: Vec<&str> = snap.iter().map(|(k, _)| k.as_str()).collect();
        for key in &keys {
            assert!(
                ENV_VALUE_KEYS.contains(key) || ENV_PRESENCE_KEYS.contains(key),
                "unexpected key {key}"
            );
        }
        keys.dedup();
        assert_eq!(keys.len(), snap.len(), "no duplicate keys");
    }

    #[test]
    fn kernel_modules_reads_only_existing_version_files() {
        let root = tempfile::tempdir().unwrap();
        let nvidia = root.path().join("sys/module/nvidia");
        std::fs::create_dir_all(&nvidia).unwrap();
        std::fs::write(nvidia.join("version"), "570.86.16\n").unwrap();
        // i915 present but (realistically) without a version file.
        std::fs::create_dir_all(root.path().join("sys/module/i915")).unwrap();

        let mods = kernel_modules(root.path());
        assert_eq!(mods, vec![("nvidia".to_string(), "570.86.16".to_string())]);

        // A root with no /sys at all yields an empty, not-erroring list.
        let empty = tempfile::tempdir().unwrap();
        assert!(kernel_modules(empty.path()).is_empty());
    }
}
