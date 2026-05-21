//! `~/.ssh/config` reader. Hand-rolled minimal parser — supports the keywords
//! sid actually uses: `Host`, `HostName`, `User`, `Port`, `IdentityFile`,
//! `ProxyJump`. Everything else is ignored. Globs in `Host` patterns are kept
//! verbatim (the SSH tab does not expand them).

use std::path::Path;

/// A single parsed Host block from an OpenSSH config.
///
/// # Examples
///
/// ```
/// use sid_ssh::SshConfigEntry;
/// let mut e = SshConfigEntry::default();
/// e.host = "example".into();
/// assert_eq!(e.host, "example");
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SshConfigEntry {
    pub host: String,
    pub hostname: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
    pub proxy_jump: Option<String>,
}

/// Read an OpenSSH config file. Returns `Ok(vec![])` if the file is missing.
/// Tolerant: unknown keywords are skipped; malformed lines are skipped.
///
/// # Examples
///
/// ```no_run
/// use sid_ssh::read_ssh_config;
/// use std::path::Path;
/// let entries = read_ssh_config(Path::new("/home/x/.ssh/config")).unwrap();
/// println!("{} hosts", entries.len());
/// ```
pub fn read_ssh_config(path: &Path) -> std::io::Result<Vec<SshConfigEntry>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path)?;
    let mut out = Vec::new();
    let mut current: Option<SshConfigEntry> = None;
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let key = parts.next().unwrap_or("");
        let val = parts.next().unwrap_or("").trim();
        if val.is_empty() {
            continue;
        }
        if key.eq_ignore_ascii_case("Host") {
            if let Some(e) = current.take() {
                out.push(e);
            }
            current = Some(SshConfigEntry {
                host: val.to_string(),
                ..Default::default()
            });
            continue;
        }
        let Some(entry) = current.as_mut() else {
            continue;
        };
        if key.eq_ignore_ascii_case("HostName") {
            entry.hostname = Some(val.to_string());
        } else if key.eq_ignore_ascii_case("User") {
            entry.user = Some(val.to_string());
        } else if key.eq_ignore_ascii_case("Port") {
            entry.port = val.parse().ok();
        } else if key.eq_ignore_ascii_case("IdentityFile") {
            entry.identity_file = Some(val.to_string());
        } else if key.eq_ignore_ascii_case("ProxyJump") {
            entry.proxy_jump = Some(val.to_string());
        }
    }
    if let Some(e) = current {
        out.push(e);
    }
    Ok(out)
}
