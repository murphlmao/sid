//! `tokio::process`-backed `kubectl` invocations.
//!
//! Request/response only — text/JSON parsing lives in [`crate::kube::parse`].

use std::process::Output;
use std::time::Duration;

use sid_core::containers::KubeError;
use tokio::process::Command;

use crate::proc::{RunError, run_within};

/// How long one `kubectl` call may take. An unreachable cluster otherwise hangs on the
/// TCP connect; past this the probe is a retryable failure, never an endless spinner.
const CALL_TIMEOUT: Duration = Duration::from_secs(15);

/// Passed to every cluster-facing call so kubectl itself gives up first, with its own
/// (classified) message; [`CALL_TIMEOUT`] is the backstop.
const REQUEST_TIMEOUT_ARG: &str = "--request-timeout=10s";

/// Run `cmd` under `limit`, mapping failures to [`KubeError`].
async fn output_within(cmd: &mut Command, limit: Duration) -> Result<Output, KubeError> {
    match run_within(cmd, limit).await {
        Ok(out) => Ok(out),
        Err(RunError::Spawn(e)) => Err(map_spawn_err(e)),
        Err(RunError::TimedOut(limit)) => Err(KubeError::Other(format!(
            "kubectl didn't answer within {limit:?}; the cluster may be unreachable"
        ))),
    }
}

/// Map a subprocess spawn failure. `ENOENT` (binary not on `PATH`) is the clearest
/// "kubectl isn't installed" signal available — everything else is a genuine
/// unexpected failure.
fn map_spawn_err(e: std::io::Error) -> KubeError {
    if e.kind() == std::io::ErrorKind::NotFound {
        KubeError::NotInstalled
    } else {
        KubeError::Other(format!("spawn kubectl: {e}"))
    }
}

/// Classify `kubectl`'s stderr for a non-zero exit. Substring-matching only — never
/// panics on adversarial input. "No cluster reachable" and "no kubeconfig at all" both
/// fold to [`KubeError::NotInstalled`] alongside the missing-binary case — from the
/// Network tab's perspective all three are the same "nothing to show here" state (see
/// `sid_core::containers::KubeError`'s doc comment).
fn classify_stderr(stderr: &str) -> KubeError {
    let trimmed = stderr.trim();
    const NOT_INSTALLED_MARKERS: &[&str] = &[
        "executable file not found",
        "command not found",
        "no configuration has been provided",
        "no context is set",
        "current-context is not set",
        "couldn't get current server API group list",
        "connection refused",
        "Unable to connect to the server",
        "the server could not find the requested resource",
        "no such host",
    ];
    if NOT_INSTALLED_MARKERS
        .iter()
        .any(|marker| stderr.contains(marker))
    {
        return KubeError::NotInstalled;
    }
    KubeError::Other(trimmed.to_string())
}

/// Run `kubectl config get-contexts -o name` and return raw stdout (one context name
/// per line — see `parse::parse_context_names`).
pub(crate) async fn get_contexts_raw() -> Result<String, KubeError> {
    let out = output_within(
        Command::new("kubectl").args(["config", "get-contexts", "-o", "name"]),
        CALL_TIMEOUT,
    )
    .await?;
    if !out.status.success() {
        return Err(classify_stderr(&String::from_utf8_lossy(&out.stderr)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run `kubectl config current-context`. `Ok(None)` means kubectl ran fine but no
/// current context is set (a valid state, distinct from "kubectl absent" — contexts
/// may still be listed, just none marked current).
pub(crate) async fn current_context_raw() -> Result<Option<String>, KubeError> {
    let out = output_within(
        Command::new("kubectl").args(["config", "current-context"]),
        CALL_TIMEOUT,
    )
    .await?;
    if out.status.success() {
        let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
        return Ok(if name.is_empty() { None } else { Some(name) });
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.contains("current-context is not set") {
        return Ok(None);
    }
    Err(classify_stderr(&stderr))
}

/// Run `kubectl get pods -A -o json` (optionally scoped to `--context <context>`) and
/// return raw stdout (see `parse::parse_pods_json`).
pub(crate) async fn get_pods_raw(context: Option<&str>) -> Result<String, KubeError> {
    let mut cmd = Command::new("kubectl");
    cmd.args(["get", "pods", "-A", "-o", "json", REQUEST_TIMEOUT_ARG]);
    if let Some(ctx) = context {
        cmd.args(["--context", ctx]);
    }
    let out = output_within(&mut cmd, CALL_TIMEOUT).await?;
    if !out.status.success() {
        return Err(classify_stderr(&String::from_utf8_lossy(&out.stderr)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_call_that_outlives_its_limit_is_a_retryable_error() {
        let res = output_within(Command::new("sleep").arg("5"), Duration::from_millis(50)).await;
        match res {
            Err(KubeError::Other(msg)) => assert!(msg.contains("didn't answer"), "{msg}"),
            other => panic!("expected a timeout error, got {other:?}"),
        }
    }

    #[test]
    fn connection_refused_is_not_installed() {
        let e = classify_stderr(
            "E0101 kubectl get pods: Get \"https://127.0.0.1:6443/api\": dial tcp \
             127.0.0.1:6443: connect: connection refused",
        );
        assert!(matches!(e, KubeError::NotInstalled));
    }

    #[test]
    fn no_configuration_provided_is_not_installed() {
        let e = classify_stderr(
            "error: no configuration has been provided, try setting KUBERNETES_MASTER \
             environment variable",
        );
        assert!(matches!(e, KubeError::NotInstalled));
    }

    #[test]
    fn current_context_not_set_is_not_installed() {
        let e = classify_stderr("error: current-context is not set");
        assert!(matches!(e, KubeError::NotInstalled));
    }

    #[test]
    fn unrelated_stderr_is_other() {
        let e = classify_stderr("error: some genuinely unexpected kubectl failure");
        match e {
            KubeError::Other(msg) => {
                assert_eq!(msg, "error: some genuinely unexpected kubectl failure")
            }
            other => panic!("expected Other, got {other:?}"),
        }
    }
}
