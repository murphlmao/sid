//! `tokio::process`-backed `docker` invocations.
//!
//! Request/response only — JSON-line parsing lives in [`crate::docker::parse`].

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

/// Run `docker ps -a --format '{{json .}}'` and return raw stdout (one JSON object per
/// line, one line per container — see `parse::parse_ps_lines`).
pub(crate) async fn list_containers_raw() -> Result<String, ContainerError> {
    let out = Command::new("docker")
        .args(["ps", "-a", "--format", "{{json .}}"])
        .output()
        .await
        .map_err(map_spawn_err)?;
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
