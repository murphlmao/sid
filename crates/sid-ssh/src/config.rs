//! `~/.ssh/config` reader. Filled in by Task 11.

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

/// Read `~/.ssh/config` (or the file at `path`). Returns `Ok(vec![])` if
/// missing. Task 11 fills in the parser; this stub exists for type wiring.
///
/// # Examples
///
/// ```
/// use sid_ssh::read_ssh_config;
/// use std::path::Path;
/// let entries = read_ssh_config(Path::new("/nonexistent-file-xyz")).unwrap();
/// assert!(entries.is_empty());
/// ```
pub fn read_ssh_config(_path: &Path) -> std::io::Result<Vec<SshConfigEntry>> {
    Ok(Vec::new())
}
