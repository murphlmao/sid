use std::fs;

use sid_store::{OpenStore, RedbStore};
use tempfile::tempdir;

// ── Happy-path tests (plan minimums) ─────────────────────────────────────────

#[test]
fn open_creates_db_file_and_tables() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("sid.redb");
    let _store = RedbStore::open(&path).unwrap();
    assert!(path.exists());
}

#[test]
fn reopen_existing_db_does_not_error() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("sid.redb");
    {
        let _ = RedbStore::open(&path).unwrap();
    }
    // Second open of the same path must succeed.
    let _ = RedbStore::open(&path).unwrap();
}

// ── Adversarial tests ─────────────────────────────────────────────────────────

#[test]
fn open_at_nonexistent_parent_dir_errors_gracefully() {
    let dir = tempdir().unwrap();
    // Path whose parent directory does not exist.
    let path = dir.path().join("does_not_exist").join("sid.redb");
    let result = RedbStore::open(&path);
    // Should return an error, not panic.
    assert!(result.is_err(), "opening at nonexistent parent must error");
}

#[test]
fn open_with_readonly_file_errors_gracefully() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("readonly.redb");
    // First create the file normally.
    {
        let _ = RedbStore::open(&path).unwrap();
    }
    // Make the file read-only.
    let mut perms = fs::metadata(&path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o444); // r--r--r--
    }
    fs::set_permissions(&path, perms).unwrap();

    let result = RedbStore::open(&path);
    // Should error gracefully — not panic.
    assert!(result.is_err(), "opening read-only DB file must error");
}

#[test]
fn open_is_idempotent_multiple_reopens() {
    // Simulates process restart: open, drop, reopen multiple times.
    let dir = tempdir().unwrap();
    let path = dir.path().join("sid.redb");
    for _ in 0..5 {
        let _ = RedbStore::open(&path).unwrap();
    }
    assert!(path.exists());
}

#[test]
fn db_file_size_is_nonzero_after_open() {
    // Opening and creating tables should write something to disk.
    let dir = tempdir().unwrap();
    let path = dir.path().join("sid.redb");
    let _ = RedbStore::open(&path).unwrap();
    let size = fs::metadata(&path).unwrap().len();
    assert!(size > 0, "DB file must have non-zero size after open");
}
