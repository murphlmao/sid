//! `SystemctlCmdClient` — CLI-shelling implementation of [`SystemctlClient`].
//!
//! The client resolves `systemctl` + `journalctl` via [`which`] at construction
//! time, then shells out for each operation. Stdout is parsed by the pure
//! parsers in [`crate::parse`].

use std::path::PathBuf;
use std::process::Command;

use sid_core::adapters::systemctl::{
    JournalEntry, SystemUnit, SystemctlClient, SystemctlError, UnitBus, UnitFilter,
};

use crate::parse::{parse_journal, parse_list_units, parse_status};

/// CLI-shelling implementation of [`SystemctlClient`].
///
/// Construct via [`SystemctlCmdClient::new`]; the constructor verifies that
/// both `systemctl` and `journalctl` are reachable on PATH.
///
/// # Examples
///
/// ```no_run
/// use sid_system::SystemctlCmdClient;
/// // On a systemd host, this succeeds; on others it returns
/// // `SystemctlError::SystemctlMissing` / `JournalctlMissing`.
/// let _ = SystemctlCmdClient::new();
/// ```
#[derive(Debug)]
pub struct SystemctlCmdClient {
    systemctl_path: PathBuf,
    journalctl_path: PathBuf,
}

impl SystemctlCmdClient {
    /// Resolve `systemctl` and `journalctl` via [`which`]. Errors if either is missing.
    pub fn new() -> Result<Self, SystemctlError> {
        let systemctl_path =
            which::which("systemctl").map_err(|_| SystemctlError::SystemctlMissing)?;
        let journalctl_path =
            which::which("journalctl").map_err(|_| SystemctlError::JournalctlMissing)?;
        Ok(Self {
            systemctl_path,
            journalctl_path,
        })
    }

    fn run_list(&self, bus: UnitBus) -> Result<String, SystemctlError> {
        let bus_flag = bus_flag(bus);
        let out = Command::new(&self.systemctl_path)
            .args([
                bus_flag,
                "--no-pager",
                "--no-ask-password",
                "--plain",
                "--no-legend",
                "list-units",
                "--type=service",
                "--all",
            ])
            .output()
            .map_err(|e| SystemctlError::Io(format!("spawn systemctl: {e}")))?;
        if !out.status.success() {
            return Err(SystemctlError::NonZeroExit(
                String::from_utf8_lossy(&out.stderr).to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn run_action(&self, bus: UnitBus, unit: &str, action: &str) -> Result<(), SystemctlError> {
        let bus_flag = bus_flag(bus);
        // `--no-ask-password` makes systemctl/polkit return the auth-required
        // error immediately instead of starting an interactive password agent.
        // Critical for two reasons:
        //   1. Tests against a real system bus don't block on a sudo prompt.
        //   2. The TUI owns the terminal; a polkit ttyagent prompt would
        //      collide with our raw-mode input. We surface SudoRequired as
        //      a toast and let the user re-run via their DE's polkit agent.
        let out = Command::new(&self.systemctl_path)
            .args([bus_flag, "--no-pager", "--no-ask-password", action, unit])
            .output()
            .map_err(|e| SystemctlError::Io(format!("spawn systemctl: {e}")))?;
        if out.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("Failed to enable bus")
            || stderr.contains("Authentication is required")
            || stderr.contains("Interactive authentication required")
            || stderr.contains("Access denied")
            || stderr.contains("Operation not permitted")
        {
            return Err(SystemctlError::SudoRequired);
        }
        if stderr.contains("could not be found") {
            return Err(SystemctlError::UnitNotFound(unit.to_string()));
        }
        Err(SystemctlError::NonZeroExit(stderr.into_owned()))
    }

    /// Spawn a `journalctl -f -u <unit>` follower. Returns a tokio mpsc
    /// receiver of parsed [`JournalEntry`] rows + the worker task handle.
    /// Dropping the handle kills the child via tokio's `kill_on_drop`.
    ///
    /// This is **not** part of the [`SystemctlClient`] trait — streaming has
    /// a different shape (cancellable + async). The widget calls this directly
    /// via `Arc<SystemctlCmdClient>`.
    pub async fn journal_follow(
        self: std::sync::Arc<Self>,
        bus: UnitBus,
        unit: &str,
    ) -> Result<
        (
            tokio::sync::mpsc::Receiver<JournalEntry>,
            tokio::task::JoinHandle<()>,
        ),
        SystemctlError,
    > {
        use tokio::io::{AsyncBufReadExt, BufReader};
        use tokio::process::Command as TokioCommand;

        let bus_flag = bus_flag(bus);
        let mut child = TokioCommand::new(&self.journalctl_path)
            .args([
                bus_flag,
                "--no-pager",
                "--output=short-iso",
                "-f",
                "-u",
                unit,
            ])
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| SystemctlError::Io(format!("spawn journalctl -f: {e}")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SystemctlError::Io("no stdout pipe".into()))?;
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        let handle = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if let Ok(mut entries) = parse_journal(&line) {
                    if let Some(e) = entries.pop() {
                        if tx.send(e).await.is_err() {
                            break;
                        }
                    }
                }
            }
            let _ = child.kill().await;
        });
        Ok((rx, handle))
    }
}

fn bus_flag(bus: UnitBus) -> &'static str {
    match bus {
        UnitBus::User => "--user",
        UnitBus::System => "--system",
    }
}

impl SystemctlClient for SystemctlCmdClient {
    fn list_units(&self, filter: UnitFilter) -> Result<Vec<SystemUnit>, SystemctlError> {
        let buses: Vec<UnitBus> = if filter.bus_both {
            vec![UnitBus::User, UnitBus::System]
        } else {
            vec![filter.bus]
        };
        let mut out = Vec::new();
        for bus in buses {
            let raw = self.run_list(bus)?;
            out.extend(parse_list_units(&raw, bus)?);
        }
        if let Some(needle) = filter.name_substring.as_deref() {
            out.retain(|u| u.name.contains(needle));
        }
        if let Some(want) = filter.state {
            out.retain(|u| u.state == want);
        }
        Ok(out)
    }

    fn status(&self, bus: UnitBus, unit: &str) -> Result<SystemUnit, SystemctlError> {
        let bus_flag = bus_flag(bus);
        // See note on `run_action` for why `--no-ask-password` is binding.
        let out = Command::new(&self.systemctl_path)
            .args([bus_flag, "--no-pager", "--no-ask-password", "status", unit])
            .output()
            .map_err(|e| SystemctlError::Io(format!("spawn systemctl: {e}")))?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("could not be found") {
            return Err(SystemctlError::UnitNotFound(unit.to_string()));
        }
        parse_status(&stdout, unit, bus)
    }

    fn start(&self, bus: UnitBus, unit: &str) -> Result<(), SystemctlError> {
        self.run_action(bus, unit, "start")
    }

    fn stop(&self, bus: UnitBus, unit: &str) -> Result<(), SystemctlError> {
        self.run_action(bus, unit, "stop")
    }

    fn restart(&self, bus: UnitBus, unit: &str) -> Result<(), SystemctlError> {
        self.run_action(bus, unit, "restart")
    }

    fn journal_tail(
        &self,
        bus: UnitBus,
        unit: &str,
        lines: usize,
    ) -> Result<Vec<JournalEntry>, SystemctlError> {
        let bus_flag = bus_flag(bus);
        let lines_str = lines.to_string();
        let out = Command::new(&self.journalctl_path)
            .args([
                bus_flag,
                "--no-pager",
                "--output=short-iso",
                "-n",
                &lines_str,
                "-u",
                unit,
            ])
            .output()
            .map_err(|e| SystemctlError::Io(format!("spawn journalctl: {e}")))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if stderr.contains("No entries") {
                return Ok(Vec::new());
            }
            return Err(SystemctlError::NonZeroExit(stderr.into_owned()));
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        parse_journal(&stdout)
    }
}
