//! The pre-flight stamp: the cached probe verdict, keyed by the environment
//! fingerprint. A plain TOML file under the XDG state dir — deliberately NOT
//! the redb store: the probe subprocess must never contend for the store's
//! single-writer lock with the parent that spawned it, and launch-cache state
//! is machine-local, not user data ("nothing lost" doesn't apply).

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Bumped when the stamp's meaning changes; any other version is ignored (a
/// stale stamp is merely a skipped optimization, never an error).
pub(crate) const STAMP_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Pin {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Stamp {
    pub version: u32,
    pub fingerprint: String,
    /// Environment pins that made init succeed (empty: vanilla worked).
    pub pins: Vec<Pin>,
    /// Whether the cached verdict is the software-rendering path.
    pub software: bool,
    pub software_reason: Option<String>,
    pub saved_at_unix: u64,
}

/// Load a stamp; anything imperfect (missing, unreadable, unparseable, wrong
/// version) is `None` — the caller just probes again.
pub(crate) fn load(path: &Path) -> Option<Stamp> {
    let text = std::fs::read_to_string(path).ok()?;
    let stamp: Stamp = toml::from_str(&text).ok()?;
    (stamp.version == STAMP_VERSION).then_some(stamp)
}

/// Save via write-temp-then-rename so a crash mid-write can't leave a torn
/// stamp (a torn read would just re-probe, but why leave the hazard).
///
/// Two savers racing means two identical worlds, so last-writer-wins on the final
/// (atomically renamed) stamp is always a correct outcome — provided they never
/// share the *temp*, which would rename a spliced file. See [`tmp_path`].
pub(crate) fn save(path: &Path, stamp: &Stamp) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let text = toml::to_string(stamp).map_err(std::io::Error::other)?;
    save_via_temp(&tmp_path(path), path, text.as_bytes())
}

/// Create `tmp` (never clobbering), fill it, then atomically commit it onto
/// `final_path` — unlinking `tmp` if anything after its creation fails.
///
/// The temp used to leak whenever the rename failed — most plausibly with
/// something already squatting at the stamp path (a directory: EISDIR) — which is
/// a permanent condition, so the leak was one file per launch, forever, in the
/// user's state dir.
///
/// The cleanup lives strictly DOWNSTREAM of a successful `create_new`, and that
/// placement is the invariant: "we only ever unlink a temp we made ourselves". A
/// cleanup above it would, on an `AlreadyExists` create failure — two
/// `unique_token()`s colliding across PID namespaces inside the same nanosecond —
/// delete the *winner's* in-flight temp and sabotage its rename. Vanishingly rare,
/// but the guard is structural and costs nothing.
///
/// Split out from [`save`] (rather than inlined) so that collision is testable:
/// with `tmp` chosen by the caller it needs no race to reproduce.
fn save_via_temp(tmp: &Path, final_path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    // `create_new`, not `create`: with a unique name a collision is already
    // impossible, and this makes "we never truncate — or unlink — a file we did
    // not make" structural rather than a property of the naming scheme.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(tmp)?;
    let result = file
        .write_all(bytes)
        .and_then(|()| std::fs::rename(tmp, final_path));
    if result.is_err() {
        // Ours (the `create_new` above succeeded), so this is safe to remove.
        let _ = std::fs::remove_file(tmp);
    }
    result
}

/// The scratch name `save` writes before renaming — unique per call, per process,
/// and across PID namespaces.
///
/// The pid alone is NOT unique: every process in a fresh PID namespace
/// (containers, `unshare`, bubblewrap/Flatpak) is pid 1, so two sandboxed sids
/// saving concurrently used the same temp name and could rename a spliced stamp —
/// a *corrupt cached verdict*, which is worse than no cache. Shares the probe
/// captures' uniqueness primitive deliberately: one hazard, one answer.
fn tmp_path(path: &Path) -> std::path::PathBuf {
    path.with_extension(format!("toml.{}.tmp", crate::probe::unique_token()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Stamp {
        Stamp {
            version: STAMP_VERSION,
            fingerprint: "abc123".into(),
            pins: vec![Pin {
                key: "VK_DRIVER_FILES".into(),
                value: "/usr/share/vulkan/icd.d/lvp_icd.x86_64.json".into(),
            }],
            software: true,
            software_reason: Some("hardware GPU init failed".into()),
            saved_at_unix: 1_750_000_000,
        }
    }

    #[test]
    fn round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gpu-preflight.toml");
        save(&path, &sample()).unwrap();
        let loaded = load(&path).expect("stamp loads back");
        assert_eq!(loaded.fingerprint, "abc123");
        assert_eq!(loaded.pins.len(), 1);
        assert!(loaded.software);
        assert_eq!(
            loaded.software_reason.as_deref(),
            Some("hardware GPU init failed")
        );
    }

    #[test]
    fn corrupt_and_missing_files_load_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gpu-preflight.toml");
        assert!(load(&path).is_none(), "missing file");
        std::fs::write(&path, "not = [valid toml").unwrap();
        assert!(load(&path).is_none(), "corrupt file");
    }

    /// Every `*.tmp` scratch file left in `dir`.
    fn leftover_temps(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect()
    }

    #[test]
    fn save_uses_a_namespace_safe_temp_and_leaves_none_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gpu-preflight.toml");

        // A process-shared `*.toml.tmp` would let two concurrent savers write the
        // same scratch file and rename a spliced result — and the pid alone is
        // process-shared inside a PID namespace, where everyone is pid 1.
        let a = tmp_path(&path);
        let b = tmp_path(&path);
        let name = a.file_name().unwrap().to_string_lossy().into_owned();
        assert_ne!(name, "gpu-preflight.toml.tmp");
        assert!(
            name.contains(&std::process::id().to_string()),
            "temp name must still carry the pid: {name}"
        );
        assert_ne!(a, b, "and must not repeat, pid namespace or not");

        save(&path, &sample()).unwrap();
        assert!(load(&path).is_some(), "the rename still lands the stamp");
        assert!(
            leftover_temps(dir.path()).is_empty(),
            "the temp is renamed away, not left behind"
        );
    }

    #[test]
    fn a_failed_rename_does_not_leak_the_temp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gpu-preflight.toml");
        // A directory squatting at the stamp path: the write succeeds, the rename
        // cannot (EISDIR). This is permanent, so a leaked temp per launch would
        // grow without bound.
        std::fs::create_dir(&path).unwrap();

        assert!(save(&path, &sample()).is_err(), "the rename cannot succeed");
        assert!(
            leftover_temps(dir.path()).is_empty(),
            "a failed save must clean up after itself: {:?}",
            leftover_temps(dir.path())
        );
    }

    #[test]
    fn a_temp_collision_never_unlinks_the_other_savers_file() {
        // Two `unique_token()`s can only collide across PID namespaces inside the
        // same nanosecond, so the case is exercised by CHOOSING the temp name
        // rather than by racing. The loser must refuse and walk away: the file it
        // found is the winner's in-flight temp, and removing it would sabotage the
        // winner's rename (a save that silently loses its stamp).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gpu-preflight.toml");
        let foreign = dir.path().join("gpu-preflight.toml.winner.tmp");
        std::fs::write(&foreign, b"the winner's in-flight bytes").unwrap();

        let err = save_via_temp(&foreign, &path, b"ours").expect_err("create_new must refuse");
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(
            std::fs::read(&foreign).unwrap(),
            b"the winner's in-flight bytes",
            "a temp we did not create must never be truncated or unlinked"
        );
        assert!(!path.exists(), "and nothing of ours was committed");
    }

    #[test]
    fn unknown_version_loads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gpu-preflight.toml");
        let mut s = sample();
        s.version = STAMP_VERSION + 1;
        save(&path, &s).unwrap();
        assert!(load(&path).is_none());
    }
}
