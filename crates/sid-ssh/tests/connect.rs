//! Tests for connect/disconnect that do not require a real SSH server.
//! Tests against a real server are gated behind `#[ignore = "needs SSH"]`.

use std::time::Duration;

use sid_core::adapters::ssh::{SshAuth, SshClient, SshError, SshHostSpec};
use sid_ssh::RusshClientFactory;

#[tokio::test]
async fn fresh_client_is_not_connected() {
    let client = RusshClientFactory::new().new_client();
    assert!(!client.is_connected());
}

#[tokio::test]
async fn double_disconnect_is_idempotent() {
    let mut client = RusshClientFactory::new().new_client();
    client.disconnect().await.unwrap();
    client.disconnect().await.unwrap();
    assert!(!client.is_connected());
}

#[tokio::test]
async fn exec_without_connect_returns_not_connected() {
    let mut client = RusshClientFactory::new().new_client();
    let err = client.exec("anything").await.unwrap_err();
    assert!(matches!(err, SshError::NotConnected));
}

#[tokio::test]
async fn open_shell_without_connect_returns_not_connected() {
    let mut client = RusshClientFactory::new().new_client();
    match client.open_shell("xterm", 24, 80).await {
        Err(SshError::NotConnected) => {}
        Err(e) => panic!("expected NotConnected, got {e}"),
        Ok(_) => panic!("expected NotConnected, got Ok"),
    }
}

#[tokio::test]
async fn open_sftp_without_connect_returns_not_connected() {
    let mut client = RusshClientFactory::new().new_client();
    match client.open_sftp().await {
        Err(SshError::NotConnected) => {}
        Err(e) => panic!("expected NotConnected, got {e}"),
        Ok(_) => panic!("expected NotConnected, got Ok"),
    }
}

#[tokio::test]
async fn connect_fails_on_unreachable_port() {
    let mut client = RusshClientFactory::new().new_client();
    let res = tokio::time::timeout(
        Duration::from_secs(3),
        client.connect(
            &SshHostSpec {
                host: "127.0.0.1".into(),
                port: 1,
                user: "x".into(),
            },
            &SshAuth::None,
        ),
    )
    .await;
    // Either timed out or returned a ConnectFailed/Other.
    match res {
        Err(_elapsed) => {}
        Ok(Err(SshError::ConnectFailed(_))) => {}
        Ok(Err(SshError::Other(_))) => {}
        other => panic!("unexpected result: {other:?}"),
    }
    assert!(!client.is_connected());
}

#[tokio::test]
async fn connect_with_bad_key_path_returns_auth_failed_or_connect_failed() {
    // No server is reachable here, so we'll hit ConnectFailed before AuthFailed.
    let mut client = RusshClientFactory::new().new_client();
    let res = tokio::time::timeout(
        Duration::from_millis(500),
        client.connect(
            &SshHostSpec {
                host: "127.0.0.1".into(),
                port: 1,
                user: "test".into(),
            },
            &SshAuth::Key {
                path: "/nonexistent/key".into(),
                passphrase: None,
            },
        ),
    )
    .await;
    match res {
        Err(_) => {}
        Ok(Err(SshError::ConnectFailed(_))) => {}
        Ok(Err(SshError::AuthFailed(_))) => {}
        Ok(Err(SshError::Other(_))) => {}
        other => panic!("unexpected: {other:?}"),
    }
}
