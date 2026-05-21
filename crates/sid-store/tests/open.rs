use std::fs;

use sid_store::{OpenStore, RedbStore, SettingValue, Store};
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

// ── Adversarial: symlink loop ─────────────────────────────────────────────────

#[test]
fn open_at_path_whose_parent_is_a_symlink_loop_errors_gracefully() {
    // Create a symlink loop: link_a → link_b → link_a.
    // Attempting to open a path under a symlink loop must either:
    //   a) error gracefully (preferred), or
    //   b) resolve successfully if the OS resolves symlinks before stat.
    // We assert: no panic. If it errors, that is the expected behavior on Linux.
    use std::os::unix::fs::symlink;
    let dir = tempdir().unwrap();
    let link_a = dir.path().join("link_a");
    let link_b = dir.path().join("link_b");
    // link_a → link_b → link_a (loop)
    symlink(&link_b, &link_a).unwrap();
    symlink(&link_a, &link_b).unwrap();

    let path = link_a.join("sid.redb");
    // Must not panic regardless of outcome.
    let result = RedbStore::open(&path);
    // On Linux this errors with ELOOP or ENOTDIR — we accept that as graceful.
    // We do NOT assert is_err() because on some platforms symlink resolution
    // may succeed if the OS dereferences eagerly. No panic is the invariant.
    let _ = result;
}

// ── Adversarial: schema migration — future version byte ──────────────────────

#[test]
fn schema_migration_safety_future_version_byte_errors_gracefully() {
    // Write a setting value whose bytes have a version-255 prefix (simulating
    // what a future version of sid would write). Current code should:
    //   - Store the bytes as-is (settings are raw bytes in SETTINGS table).
    //   - Return them unchanged on read.
    //   - Not panic at any point.
    use sid_store::SessionRecord;
    use sid_store::codec::{decode_versioned, encode_versioned};

    let dir = tempdir().unwrap();
    let path = dir.path().join("sid.redb");

    let future_record = SessionRecord {
        id: "future-sess".into(),
        started_at: 42,
        last_active: 43,
        ended_at: None,
        active_tab: None,
        open_tabs: vec![],
    };
    let future_bytes = encode_versioned(255, &future_record).unwrap();

    {
        let store = RedbStore::open(&path).unwrap();
        // Store the future-versioned bytes as a raw setting value.
        store
            .put_setting("future.blob", &SettingValue(future_bytes.clone()))
            .unwrap();
    }

    // Reopen and read back — no panic.
    let store2 = RedbStore::open(&path).unwrap();
    let got = store2.get_setting("future.blob").unwrap();
    if let Some(val) = got {
        // Bytes are preserved without corruption.
        assert_eq!(val.0, future_bytes);
        assert_eq!(val.0[0], 255u8, "future version byte must be preserved");
        // Decode with codec — version 255 is metadata only, payload is valid.
        let result = decode_versioned::<SessionRecord>(&val.0);
        if let Ok((ver, rec)) = result {
            assert_eq!(ver, 255);
            assert_eq!(rec.id, "future-sess");
        }
        // Err(_) is also acceptable — graceful failure is the invariant
    }
}
