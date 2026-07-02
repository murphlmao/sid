//! Platform-specific default-route lookup.
//!
//! Returns the name of the network interface that holds the default route
//! (where outbound packets without a more-specific destination go), or
//! `None` if no default route is set.

use sid_core::sys::SysError;

/// Linux: parse `/proc/net/route` for a row with `Destination == 00000000`
/// (all-zeros — the default route) and return the value in the first column
/// (the interface name).
///
/// Format (tab-separated, one header line):
///
/// ```text
/// Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT
/// wlan0\t00000000\t0102A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0
/// ```
#[cfg(target_os = "linux")]
pub fn read_default_route_iface() -> Result<Option<String>, SysError> {
    let bytes = match std::fs::read_to_string("/proc/net/route") {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(SysError::Other(format!("/proc/net/route: {e}"))),
    };
    Ok(parse_proc_net_route(&bytes))
}

/// macOS: shell out to `route -n get default` and parse the `interface:`
/// line.
#[cfg(target_os = "macos")]
pub fn read_default_route_iface() -> Result<Option<String>, SysError> {
    use std::process::Command;
    let out = Command::new("route")
        .args(["-n", "get", "default"])
        .output()
        .map_err(|e| SysError::Other(format!("spawn route: {e}")))?;
    if !out.status.success() {
        // Non-zero often just means "no default route" — return Ok(None).
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("interface:") {
            let name = rest.trim().to_string();
            if name.is_empty() {
                return Ok(None);
            }
            return Ok(Some(name));
        }
    }
    Ok(None)
}

/// Other platforms: not supported, return `Ok(None)`.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn read_default_route_iface() -> Result<Option<String>, SysError> {
    Ok(None)
}

/// Linux: parse the body of `/proc/net/route` directly. Pulled out for
/// testability. Skips the header line, then finds the first row whose
/// second column is `00000000` (the all-zeros destination — the default
/// route).
///
/// # Examples
///
/// ```
/// use sid_sysinfo::default_route::parse_proc_net_route;
/// // Single default route → return its iface name.
/// let body = "Iface\tDestination\tGateway\nwlan0\t00000000\t0102A8C0\n";
/// assert_eq!(parse_proc_net_route(body), Some("wlan0".to_string()));
///
/// // No default route → None.
/// assert_eq!(parse_proc_net_route("Iface\tDestination\tGateway\n"), None);
/// ```
pub fn parse_proc_net_route(body: &str) -> Option<String> {
    for line in body.lines().skip(1) {
        let mut cols = line.split('\t').filter(|s| !s.is_empty());
        let iface = cols.next()?;
        let dest = cols.next()?;
        if dest == "00000000" {
            return Some(iface.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_default_route_row() {
        let body =
            "Iface\tDestination\tGateway\nwlan0\t00000000\t0102A8C0\neth0\t0000A8C0\t00000000\n";
        assert_eq!(parse_proc_net_route(body), Some("wlan0".to_string()));
    }

    #[test]
    fn no_default_route_row_returns_none() {
        let body = "Iface\tDestination\tGateway\neth0\t0000A8C0\t00000000\n";
        assert_eq!(parse_proc_net_route(body), None);
    }

    #[test]
    fn empty_body_returns_none() {
        assert_eq!(parse_proc_net_route(""), None);
        assert_eq!(parse_proc_net_route("Iface\tDestination\tGateway\n"), None);
    }
}
