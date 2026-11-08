//! Vulkan driver-manifest (ICD) discovery and classification.
//!
//! The loader picks drivers from small JSON manifests; `VK_DRIVER_FILES` pins it
//! to exactly one — which is how the remediation ladder routes around a broken
//! driver. Manifests are classified hardware vs software (lavapipe/SwiftShader)
//! so the ladder can prefer real GPUs and keep software strictly last.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// One discovered driver manifest. `len`/`mtime_unix` feed the environment
/// fingerprint (a driver update touches these even when the path set is stable).
#[derive(Debug, Clone)]
pub(crate) struct IcdManifest {
    pub path: PathBuf,
    pub software: bool,
    pub len: u64,
    pub mtime_unix: u64,
}

impl IcdManifest {
    /// Short display name (file name) for ladder labels and reports.
    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

/// Substrings identifying software rasterizers in manifest names or their
/// `library_path` (Mesa ships lavapipe as `lvp_icd.*.json` → `libvulkan_lvp.so`).
const SOFTWARE_NEEDLES: &[&str] = &["lvp", "llvmpipe", "lavapipe", "swrast", "swiftshader"];

/// Discover manifests: hardware first, software last, stable path order within
/// each class (the ladder tries them in exactly this order). `override_dirs`
/// replaces the standard loader search path (tests, report introspection).
pub(crate) fn scan(sys_root: &Path, override_dirs: Option<&[PathBuf]>) -> Vec<IcdManifest> {
    let dirs: Vec<PathBuf> = match override_dirs {
        Some(d) => d.to_vec(),
        None => default_dirs(sys_root),
    };
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            // The same dir can appear twice in the search path (XDG_DATA_DIRS
            // defaults overlap /usr/share) — first sighting wins.
            if !seen.insert(path.clone()) {
                continue;
            }
            let meta = entry.metadata().ok();
            let len = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let mtime_unix = meta
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            out.push(IcdManifest {
                software: is_software(&path),
                path,
                len,
                mtime_unix,
            });
        }
    }
    // bool sorts false < true — hardware first, software last, by design.
    out.sort_by(|a, b| (a.software, &a.path).cmp(&(b.software, &b.path)));
    out
}

/// A manifest is software if its file name or content names a software
/// rasterizer. Content check is a plain substring scan of the tiny JSON (its
/// `library_path` is the authoritative field; a full JSON dependency isn't worth
/// it) — an unreadable manifest is assumed hardware, keeping it in the ladder's
/// preferred tier rather than silently demoting a real GPU.
fn is_software(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();
    if SOFTWARE_NEEDLES.iter().any(|n| name.contains(n)) {
        return true;
    }
    let content = std::fs::read_to_string(path)
        .unwrap_or_default()
        .to_lowercase();
    SOFTWARE_NEEDLES.iter().any(|n| content.contains(n))
}

/// The Vulkan loader's manifest search path, in the loader's own precedence
/// order: the XDG *config* tiers first, then the system dirs, then the XDG *data*
/// tiers. System dirs resolve under `sys_root` (so tests and `--gpu-report` can
/// point at a fake root); the user-level XDG dirs stay absolute, exactly as the
/// loader resolves them.
///
/// The config tiers (`$XDG_CONFIG_HOME/vulkan/icd.d`, `$XDG_CONFIG_DIRS/...`,
/// defaulting to `~/.config` and `/etc/xdg`) are real and searched FIRST —
/// verified against the live loader with `VK_LOADER_DEBUG=driver vulkaninfo`,
/// which lists them ahead of the /etc and share/data tiers. A driver installed
/// there would otherwise be invisible to both the fingerprint and the ladder.
fn default_dirs(sys_root: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // Tier 1: user config, then system config dirs.
    if let Some(config_home) = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        dirs.push(config_home.join("vulkan/icd.d"));
    } else if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        dirs.push(home.join(".config/vulkan/icd.d"));
    }
    match std::env::var_os("XDG_CONFIG_DIRS") {
        Some(config_dirs) => {
            for dir in std::env::split_paths(&config_dirs) {
                dirs.push(dir.join("vulkan/icd.d"));
            }
        }
        // The XDG default when unset — a system dir, so root-relative.
        None => dirs.push(sys_root.join("etc/xdg/vulkan/icd.d")),
    }

    // Tier 2: the fixed system dirs.
    dirs.extend([
        sys_root.join("usr/local/etc/vulkan/icd.d"),
        sys_root.join("usr/local/share/vulkan/icd.d"),
        sys_root.join("etc/vulkan/icd.d"),
        sys_root.join("usr/share/vulkan/icd.d"),
    ]);

    // Tier 3: user data, then system data dirs.
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        dirs.push(data_home.join("vulkan/icd.d"));
    } else if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        dirs.push(home.join(".local/share/vulkan/icd.d"));
    }
    if let Some(data_dirs) = std::env::var_os("XDG_DATA_DIRS") {
        for dir in std::env::split_paths(&data_dirs) {
            dirs.push(dir.join("vulkan/icd.d"));
        }
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, content: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn classifies_by_file_name() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "lvp_icd.x86_64.json",
            r#"{"ICD":{"library_path":"x"}}"#,
        );
        write(
            dir.path(),
            "radeon_icd.x86_64.json",
            r#"{"ICD":{"library_path":"libvulkan_radeon.so"}}"#,
        );
        let icds = scan(Path::new("/nonexistent"), Some(&[dir.path().to_path_buf()]));
        assert_eq!(icds.len(), 2);
        // Hardware (radeon) sorts before software (lvp) despite alphabetical order.
        assert!(!icds[0].software);
        assert!(icds[0].file_name().contains("radeon"));
        assert!(icds[1].software);
    }

    #[test]
    fn classifies_by_library_path_content() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "innocuous_name.json",
            r#"{"ICD":{"library_path":"/usr/lib/libvulkan_lvp.so","api_version":"1.3.289"}}"#,
        );
        let icds = scan(Path::new("/nonexistent"), Some(&[dir.path().to_path_buf()]));
        assert!(icds[0].software);
    }

    #[test]
    fn non_json_files_are_ignored_and_software_sorts_last() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "README.txt", "not a manifest");
        write(dir.path(), "a_lavapipe.json", "{}");
        write(
            dir.path(),
            "z_nvidia.json",
            r#"{"ICD":{"library_path":"libGLX_nvidia.so"}}"#,
        );
        let icds = scan(Path::new("/nonexistent"), Some(&[dir.path().to_path_buf()]));
        assert_eq!(icds.len(), 2);
        assert!(icds[0].file_name().contains("nvidia"));
        assert!(
            icds.last().unwrap().software,
            "software must be strictly last"
        );
    }

    #[test]
    fn missing_dirs_scan_to_empty() {
        let icds = scan(
            Path::new("/nonexistent"),
            Some(&[PathBuf::from("/also/nonexistent")]),
        );
        assert!(icds.is_empty());
    }

    /// `default_dirs` reads the ambient environment, so these assertions are
    /// structural (position/shape) — nothing is ever set, keeping the test
    /// hermetic and thread-safe.
    #[test]
    fn default_dirs_searches_the_xdg_config_tiers_before_the_system_tiers() {
        let root = Path::new("/fake-root");
        let dirs = default_dirs(root);

        // Every entry is a driver-manifest dir.
        assert!(
            dirs.iter().all(|d| d.ends_with("vulkan/icd.d")),
            "unexpected entry shape: {dirs:?}"
        );

        let first_system = dirs
            .iter()
            .position(|d| d == &root.join("usr/local/etc/vulkan/icd.d"))
            .expect("the fixed system tier is always present");
        assert!(
            first_system > 0,
            "at least one XDG *config* tier must precede the system dirs: {dirs:?}"
        );

        // The system tier keeps its documented internal order...
        let expected_system = [
            root.join("usr/local/etc/vulkan/icd.d"),
            root.join("usr/local/share/vulkan/icd.d"),
            root.join("etc/vulkan/icd.d"),
            root.join("usr/share/vulkan/icd.d"),
        ];
        assert_eq!(&dirs[first_system..first_system + 4], &expected_system);

        // ...and the config-tier entries all sit before it, config-shaped.
        for dir in &dirs[..first_system] {
            let s = dir.to_string_lossy();
            assert!(
                s.contains("config") || s.contains("/etc/xdg") || !s.starts_with("/fake-root"),
                "config tier entry looks wrong: {s}"
            );
        }

        // `XDG_CONFIG_DIRS` unset → the loader's `/etc/xdg` default, root-relative
        // and ahead of `/etc`.
        if std::env::var_os("XDG_CONFIG_DIRS").is_none() {
            let xdg = dirs
                .iter()
                .position(|d| d == &root.join("etc/xdg/vulkan/icd.d"))
                .expect("the /etc/xdg default must be searched");
            assert!(xdg < first_system);
        }
        // `XDG_CONFIG_HOME` set → its dir leads the list (highest precedence).
        if let Some(home) = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
        {
            assert_eq!(dirs.first(), Some(&home.join("vulkan/icd.d")));
        }
    }

    #[test]
    fn software_sorts_last_even_from_a_higher_precedence_dir() {
        // A lavapipe manifest in an earlier search dir must still land behind a
        // hardware one found later — the ladder's ordering invariant survives the
        // new config tiers.
        let early = tempfile::tempdir().unwrap();
        let late = tempfile::tempdir().unwrap();
        write(early.path(), "lvp_icd.x86_64.json", "{}");
        write(
            late.path(),
            "nvidia_icd.json",
            r#"{"ICD":{"library_path":"libGLX_nvidia.so"}}"#,
        );
        let icds = scan(
            Path::new("/nonexistent"),
            Some(&[early.path().to_path_buf(), late.path().to_path_buf()]),
        );
        assert_eq!(icds.len(), 2);
        assert!(icds[0].file_name().contains("nvidia"));
        assert!(icds[1].software);
    }

    #[test]
    fn repeated_dirs_are_deduplicated() {
        // XDG defaults overlap the fixed tiers (and now the config tiers too), so
        // the same manifest can be reached twice — it must be reported once.
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "radeon_icd.json", "{}");
        let d = dir.path().to_path_buf();
        let icds = scan(Path::new("/nonexistent"), Some(&[d.clone(), d.clone(), d]));
        assert_eq!(icds.len(), 1, "one sighting per manifest path");
    }
}
