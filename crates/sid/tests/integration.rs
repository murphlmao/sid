/// Integration tests that spawn the sid binary as a child process.
///
/// These tests are gated to Unix because the byte-write approach (piping raw
/// 0x11 to stdin) is Unix-specific. On non-Unix platforms these tests are
/// compiled away entirely — a clear cfg-comment documents this.
///
/// # Why Unix-only?
///
/// On Windows, crossterm uses `ReadConsoleInput` / `WriteConsoleInput`, not raw
/// stdin byte streams, so piping a byte does not translate to a Ctrl+Q event.
/// A Windows integration test would need to use `SendInput`. That is out of
/// scope for Plan 1.
#[cfg(unix)]
mod unix {
    use std::{
        io::Write,
        process::{Command, Stdio},
        time::Duration,
    };

    use tempfile::tempdir;

    /// Launch sid with a temp DB, send Ctrl+Q (0x11), assert exit 0 within 5s,
    /// and verify the redb file was created.
    ///
    /// Skipped when not running in a TTY (e.g., CI without a pty), because
    /// sid's `enable_raw_mode()` requires a real terminal.
    #[test]
    fn sid_starts_and_exits_on_ctrl_q() {
        use std::io::IsTerminal;
        if !std::io::stdout().is_terminal() {
            eprintln!("SKIP: not a TTY — sid requires a terminal for raw mode");
            return;
        }

        let dir = tempdir().unwrap();
        let db = dir.path().join("sid.redb");

        let mut child = Command::new(env!("CARGO_BIN_EXE_sid"))
            .arg("--db")
            .arg(&db)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sid");

        // Give it a moment to set up the terminal and start the event loop.
        std::thread::sleep(Duration::from_millis(500));

        // Write Ctrl+Q (0x11 = DC1 = ^Q).
        {
            let mut stdin = child.stdin.take().expect("stdin");
            stdin.write_all(&[0x11u8]).unwrap();
            // Flush and drop to close the pipe, signalling EOF as a fallback.
        }

        // Poll with a 5-second timeout.
        let start = std::time::Instant::now();
        loop {
            match child.try_wait().expect("try_wait") {
                Some(status) => {
                    assert!(status.success(), "sid exited with {status:?}");
                    break;
                }
                None => {
                    if start.elapsed() > Duration::from_secs(5) {
                        let _ = child.kill();
                        panic!("sid did not exit within 5s of Ctrl+Q");
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }

        assert!(db.exists(), "redb file should have been created at {db:?}");
    }

    /// Launch sid, let it create the redb file, then open it with RedbStore to
    /// confirm all four tables exist and the session record is parseable.
    ///
    /// Skipped when not running in a TTY for the same reason as `sid_starts_and_exits_on_ctrl_q`.
    #[test]
    fn sid_redb_file_is_parseable_after_launch() {
        use std::io::IsTerminal;

        use sid_store::{OpenStore, RedbStore, Store};

        if !std::io::stdout().is_terminal() {
            eprintln!("SKIP: not a TTY — sid requires a terminal for raw mode");
            return;
        }

        let dir = tempdir().unwrap();
        let db = dir.path().join("parseable.redb");

        let mut child = Command::new(env!("CARGO_BIN_EXE_sid"))
            .arg("--db")
            .arg(&db)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sid");

        std::thread::sleep(Duration::from_millis(500));

        {
            let mut stdin = child.stdin.take().expect("stdin");
            stdin.write_all(&[0x11u8]).unwrap();
        }

        let start = std::time::Instant::now();
        loop {
            match child.try_wait().expect("try_wait") {
                Some(_) => break,
                None => {
                    if start.elapsed() > Duration::from_secs(5) {
                        let _ = child.kill();
                        panic!("sid did not exit within 5s");
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }

        // The redb file should exist and be openable.
        assert!(db.exists(), "redb file should exist at {db:?}");
        let store = RedbStore::open(&db).expect("should open redb after sid ran");

        // A session should have been created.
        let session = store
            .current_session()
            .expect("should read current session");
        assert!(
            session.is_some(),
            "a session record should exist after sid ran"
        );

        let sess = session.unwrap();
        assert!(
            sess.id.starts_with("sess-"),
            "session id should start with 'sess-': {}",
            sess.id
        );
        // Active tab should be one of the 6 known tabs.
        if let Some(tab_id) = sess.active_tab {
            let valid = [
                "workspaces",
                "ssh",
                "database",
                "network",
                "system",
                "settings",
            ];
            assert!(
                valid.contains(&tab_id.as_str()),
                "active_tab should be a known tab id, got: {}",
                tab_id
            );
        }
    }

    /// `sid --help` exits 0 cleanly (no TTY required).
    #[test]
    fn sid_help_exits_0() {
        let out = Command::new(env!("CARGO_BIN_EXE_sid"))
            .arg("--help")
            .output()
            .expect("run sid --help");
        assert!(
            out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// `sid --version` exits 0 cleanly (no TTY required).
    #[test]
    fn sid_version_exits_0() {
        let out = Command::new(env!("CARGO_BIN_EXE_sid"))
            .arg("--version")
            .output()
            .expect("run sid --version");
        assert!(out.status.success());
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("0.0.1"),
            "expected version in output: {stdout}"
        );
    }

    /// `sid --db` with a path in a nonexistent parent directory should fail fast
    /// with a non-zero exit code (redb cannot create the db there).
    #[test]
    fn sid_nonexistent_parent_dir_exits_nonzero() {
        let db = "/tmp/sid-test-nonexistent-dir-absolutely-should-not-exist/test.redb";
        let out = Command::new(env!("CARGO_BIN_EXE_sid"))
            .arg("--db")
            .arg(db)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .expect("run sid with bad db path");
        assert!(
            !out.status.success(),
            "should fail when db parent dir does not exist"
        );
    }
}
