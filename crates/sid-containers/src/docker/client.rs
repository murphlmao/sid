//! `tokio::process`-backed `docker` invocations.
//!
//! Request/response only — JSON-line parsing lives in [`crate::docker::parse`].

use std::process::Output;
use std::time::Duration;

use sid_core::containers::ContainerError;
use tokio::process::Command;

/// Map a subprocess spawn failure. `ENOENT` (binary not on `PATH`) is the clearest
/// "docker isn't installed" signal available — everything else is a genuine
/// unexpected failure.
fn map_spawn_err(e: std::io::Error) -> ContainerError {
    if e.kind() == std::io::ErrorKind::NotFound {
        ContainerError::NotInstalled
    } else {
        ContainerError::Other(format!("spawn docker: {e}"))
    }
}

/// `docker ps -a`'s stderr when the daemon/socket isn't reachable. Substring-matching
/// only — never panics on adversarial input. Folds to [`ContainerError::NotInstalled`]
/// (rather than [`ContainerError::Other`]) so the UI shows the same graceful "docker
/// not running" notice whether the binary is missing or just not started.
fn is_daemon_unavailable(stderr: &str) -> bool {
    stderr.contains("Cannot connect to the Docker daemon")
        || stderr.contains("Is the docker daemon running")
        || stderr.contains("docker daemon is not running")
        || stderr.contains("permission denied while trying to connect")
}

/// `docker ps -a` as JSON lines. `--size=false` is load-bearing: `{{json .}}` touches
/// every field, `.Size` included, and the CLI answers that by asking the daemon to walk
/// every container's writable layer (`size=true`) — 7-8 s for 21 containers on a warm
/// daemon, far longer on a socket-activated cold one, against ~10 ms without it. The
/// table never shows a size.
const PS_ARGS: [&str; 5] = ["ps", "-a", "--size=false", "--format", "{{json .}}"];

/// How long one `docker` call may take. A socket-activated daemon cold-starts in ~2 s;
/// past this the probe is a retryable failure, never a view that spins forever.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

/// Run `cmd`, giving up (and killing the child) after `limit`.
async fn output_within(cmd: &mut Command, limit: Duration) -> Result<Output, ContainerError> {
    match crate::proc::run_within(cmd, limit).await {
        Ok(out) => Ok(out),
        Err(crate::proc::RunError::Spawn(e)) => Err(map_spawn_err(e)),
        Err(crate::proc::RunError::TimedOut(limit)) => Err(ContainerError::Other(format!(
            "docker didn't answer within {limit:?}; the daemon may still be starting"
        ))),
    }
}

/// Run `docker ps -a --format '{{json .}}'` and return raw stdout (one JSON object per
/// line, one line per container — see `parse::parse_ps_lines`).
pub(crate) async fn list_containers_raw() -> Result<String, ContainerError> {
    let out = output_within(Command::new("docker").args(PS_ARGS), CALL_TIMEOUT).await?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if is_daemon_unavailable(&stderr) {
            return Err(ContainerError::NotInstalled);
        }
        return Err(ContainerError::Other(stderr.trim().to_string()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_never_asks_the_daemon_to_compute_container_sizes() {
        // `{{json .}}` touches every field, `.Size` included, and the CLI then silently
        // requests `size=true`: 7-8 s for 21 containers on a warm daemon (10 ms without).
        assert!(PS_ARGS.contains(&"--size=false"), "{PS_ARGS:?}");
    }

    #[tokio::test]
    async fn a_call_that_outlives_its_limit_is_a_retryable_error() {
        let res = output_within(Command::new("sleep").arg("5"), Duration::from_millis(50)).await;
        match res {
            Err(ContainerError::Other(msg)) => assert!(msg.contains("didn't answer"), "{msg}"),
            other => panic!("expected a timeout error, got {other:?}"),
        }
    }

    #[test]
    fn cannot_connect_to_daemon_is_not_installed() {
        assert!(is_daemon_unavailable(
            "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. \
             Is the docker daemon running?"
        ));
    }

    #[test]
    fn permission_denied_socket_is_not_installed() {
        assert!(is_daemon_unavailable(
            "Got permission denied while trying to connect to the Docker daemon socket"
        ));
    }

    #[test]
    fn unrelated_stderr_is_not_flagged_as_daemon_unavailable() {
        assert!(!is_daemon_unavailable("some other docker failure"));
    }
}
