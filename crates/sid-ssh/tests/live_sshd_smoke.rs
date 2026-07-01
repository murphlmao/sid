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
    let mut shell = client
        .open_shell("xterm-256color", 24, 80)
        .await
        .expect("open shell");
    shell
        .write(b"printf '\\033[31mRED\\033[0m\\n'\n")
        .await
        .expect("shell write");
    // Give the remote a moment to echo + run.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    // Drive the screen through the trait object the GPUI view (Plan 3C) will hold.
    let mut screen: Box<dyn TerminalScreen> = Box::new(Vt100Screen::new(24, 80));
    let bytes = shell.try_read().await.expect("shell read");
    assert!(!bytes.is_empty(), "shell should have produced output");
    screen.feed(&bytes);
    let cells = screen.cells();
    let non_empty = cells.iter().flatten().any(|c| !c.text.is_empty());
    assert!(non_empty, "styled cell grid should have visible content");
    shell.close().await.expect("shell close");

    // 4. sftp list("/tmp") returns entries.
    let mut sftp = client.open_sftp().await.expect("open sftp");
    let entries = sftp.list("/tmp").await.expect("sftp list /tmp");
    assert!(!entries.is_empty(), "/tmp should list at least one entry");
    sftp.close().await.expect("sftp close");

    client.disconnect().await.expect("disconnect");
}
