//! Live-sshd smoke test (Plan 3B, B5). `#[ignore]`d — compiles in CI but is
//! run manually against a real sshd on this machine.
//!
//! Prereqs on the test machine:
//!   - an sshd listening on localhost:22 that accepts *your* key,
//!   - a running ssh-agent with that key loaded (`ssh-add -l` non-empty;
//!     `SSH_AUTH_SOCK` exported),
//!   - your host key already trusted, or a writable temp app known_hosts (this
//!     test uses a temp file, so first contact TOFU-learns and succeeds).
//!
//! Run with:
//!   cargo test -p sid-ssh --test live_sshd_smoke -- --ignored --nocapture

use std::path::PathBuf;

use sid_core::ssh::{SshAuth, SshClient, SshHostSpec};
use sid_core::term::TerminalScreen;
use sid_ssh::RusshClientFactory;
use sid_term::Vt100Screen;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live sshd + ssh-agent on localhost; run manually (B5 gate)"]
async fn live_sshd_agent_exec_shell_sftp() {
    // TOFU into a throwaway file so we never touch the real ~/.ssh/known_hosts.
    let tmp = tempfile::tempdir().unwrap();
    let app_known_hosts: PathBuf = tmp.path().join("known_hosts");

    let factory = RusshClientFactory::new(app_known_hosts);
    let mut client = factory.new_client();

    let user = std::env::var("USER").unwrap_or_else(|_| "root".to_string());
    let host = SshHostSpec::new("localhost", user);

    // 1. Connect via ssh-agent.
    client
        .connect(&host, &SshAuth::Agent)
        .await
        .expect("agent-auth connect to localhost");
    assert!(client.is_connected());

    // 2. exec("echo ok") → exit 0, stdout contains "ok".
    let result = client.exec("echo ok").await.expect("exec echo ok");
    assert_eq!(result.exit_code, 0, "echo should exit 0");
    assert!(
        String::from_utf8_lossy(&result.stdout).contains("ok"),
        "stdout should contain 'ok'"
    );

    // 3. open_shell → feed output through Vt100Screen → styled cells non-empty.
    // `open_shell` returns the read/write halves separately (Bug 1 fix: a shared
    // mutex across both meant a write awaiting flow-control window could starve the
    // read loop) — write via the writer, read via the reader.
    let (mut shell_reader, mut shell_writer) = client
        .open_shell("xterm-256color", 24, 80)
        .await
        .expect("open shell");
    shell_writer
        .write(b"printf '\\033[31mRED\\033[0m\\n'\n")
        .await
        .expect("shell write");
    // Give the remote a moment to echo + run.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    // Drive the screen through the trait object the GPUI view (Plan 3C) will hold.
    let mut screen: Box<dyn TerminalScreen> = Box::new(Vt100Screen::new(24, 80));
    let bytes = shell_reader.try_read().await.expect("shell read");
    assert!(!bytes.is_empty(), "shell should have produced output");
    screen.feed(&bytes);
    let cells = screen.cells();
    let non_empty = cells.iter().flatten().any(|c| !c.text.is_empty());
    assert!(non_empty, "styled cell grid should have visible content");
    shell_writer.close().await.expect("shell close");

    // 4. sftp list("/tmp") returns entries.
    let mut sftp = client.open_sftp().await.expect("open sftp");
    let entries = sftp.list("/tmp").await.expect("sftp list /tmp");
    assert!(!entries.is_empty(), "/tmp should list at least one entry");
    sftp.close().await.expect("sftp close");

    client.disconnect().await.expect("disconnect");
}

/// Docker-sshd integration test (Plan: sid integration/automation harness,
/// 2026-07-02). Unlike [`live_sshd_agent_exec_shell_sftp`] above (which needs
/// *your* real ssh-agent + a trusted localhost sshd), this points at the
/// throwaway `sshd` service in `docker/docker-compose.test.yml` — a fixed
/// user/key baked into `docker/ssh/Dockerfile` — using key-file auth
/// (`SshAuth::Key`), so it's runnable in CI with no agent involved.
///
/// Exercises exec + an SFTP round-trip: list the baked fixture directory,
/// `put` a file, `get` it back, and assert the bytes match (closing the gap
/// the smoke test above only half-covers — `list` alone doesn't prove
/// upload/download correctness).
///
/// Run manually:
///   docker compose -f docker/docker-compose.test.yml up -d sshd
///   cargo test -p sid-ssh --test live_sshd_smoke docker_sshd -- --ignored --nocapture
///   docker compose -f docker/docker-compose.test.yml down -v
///
/// Or via `scripts/test-ssh.sh`, which sets up the env vars below itself.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires docker/docker-compose.test.yml's sshd service; run via scripts/test-ssh.sh"]
async fn docker_sshd_key_auth_exec_and_sftp_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let app_known_hosts: PathBuf = tmp.path().join("known_hosts");

    let host = std::env::var("SID_TEST_SSH_HOST").unwrap_or_else(|_| "localhost".to_string());
    let port: u16 = std::env::var("SID_TEST_SSH_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(2222);
    let user = std::env::var("SID_TEST_SSH_USER").unwrap_or_else(|_| "sid_test".to_string());
    let key_path: PathBuf = std::env::var("SID_TEST_SSH_KEY")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docker/ssh/test_id_ed25519")
        });

    let factory = RusshClientFactory::new(app_known_hosts);
    let mut client = factory.new_client();

    let mut spec = SshHostSpec::new(host, user);
    spec.port = port;
    let auth = SshAuth::Key {
        path: key_path,
        passphrase: None,
    };

    // 1. Connect via the baked-in disposable test key (TOFU-learns the
    // container's host key into the throwaway known_hosts above).
    client
        .connect(&spec, &auth)
        .await
        .expect("key-auth connect to dockerized sshd");
    assert!(client.is_connected());

    // 2. exec — same shape as the agent-based smoke test.
    let result = client.exec("echo ok").await.expect("exec echo ok");
    assert_eq!(result.exit_code, 0, "echo should exit 0");
    assert!(String::from_utf8_lossy(&result.stdout).contains("ok"));

    // 3. SFTP round-trip: list the fixture dir baked into the image
    // (docker/ssh/Dockerfile's `sftp-fixture/hello.txt`), then prove
    // put+get actually moves bytes, not just names.
    let mut sftp = client.open_sftp().await.expect("open sftp");
    let entries = sftp
        .list("sftp-fixture")
        .await
        .expect("sftp list sftp-fixture");
    assert!(
        entries.iter().any(|e| e.name == "hello.txt"),
        "expected the baked-in fixture file in the listing, got: {entries:?}"
    );

    // Overwrite case: `put` targets a file the image already contains
    // (`sftp-fixture/writable.txt`) — kept as a regression pin for the "existing
    // file" path even though `put` no longer needs `CREATE` to succeed here.
    let payload = b"sid-ssh sftp round-trip integration test payload".to_vec();
    sftp.put("sftp-fixture/writable.txt", &payload)
        .await
        .expect("sftp put (overwrite of a pre-existing remote file)");
    let downloaded = sftp
        .get("sftp-fixture/writable.txt")
        .await
        .expect("sftp get");
    assert_eq!(
        downloaded, payload,
        "downloaded bytes must match what was uploaded"
    );

    // Bug 2 regression: `sid_ssh::RusshSftp::put` (crates/sid-ssh/src/sftp.rs) used
    // to wrap russh-sftp's `SftpSession::write`, which opens with `OpenFlags::WRITE`
    // only (no `CREATE`) — it could overwrite an existing remote file but never
    // create a new one. `put` now opens via `SftpSession::create` (`CREATE |
    // TRUNCATE | WRITE`), so a put to a path that has never existed must succeed
    // and round-trip, not just the pre-existing-file case above.
    let new_path = "sftp-fixture/created-by-put.txt";
    let new_payload = b"sid-ssh sftp put-creates-a-new-file regression payload".to_vec();
    sftp.put(new_path, &new_payload)
        .await
        .expect("sftp put to a brand-new remote path must create it (Bug 2 fix)");
    let downloaded_new = sftp
        .get(new_path)
        .await
        .expect("sftp get of the newly created file");
    assert_eq!(
        downloaded_new, new_payload,
        "downloaded bytes must match what was uploaded to the new path"
    );
    // Clean up so a `--keep`'d container stays idempotent across repeat runs.
    sftp.remove_file(new_path)
        .await
        .expect("cleanup: remove the newly created file");

    sftp.close().await.expect("sftp close");

    client.disconnect().await.expect("disconnect");
}
