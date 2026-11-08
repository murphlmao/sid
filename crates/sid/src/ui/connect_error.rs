//! Friendly copy for a failed SSH connect. Pure string mapping, no GPUI: the failed pane
//! shows [`friendly`]'s sentence and keeps the raw error as a "Details" line.

/// Map the raw error text of a failed dial to one plain sentence naming `host:port` and
/// what to check. `None` means "no better wording" (auth failures, host-key refusals and
/// anything unrecognised already read well, or are better left verbatim).
pub(crate) fn friendly(raw: &str, host: &str, port: u16) -> Option<String> {
    let lower = raw.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| lower.contains(n));
    // Anything that got past the transport is the server talking: leave its words alone.
    if has(&["authentication failed", "host key", "not trusted"]) {
        return None;
    }
    if has(&["connection refused", "os error 111"]) {
        return Some(format!(
            "Nothing is listening on {host}:{port}. Is the SSH server running, and is the port right?"
        ));
    }
    if has(&["timed out", "timeout"]) {
        let after = lower
            .split("timed out after ")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .map(str::to_string)
            .unwrap_or_else(|| "10s".to_string());
        return Some(format!(
            "No answer from {host}:{port} within {after}. Check the address and your network."
        ));
    }
    if has(&[
        "lookup address",
        "name or service not known",
        "name resolution",
        "no address associated",
        "nodename nor servname",
        "dns",
    ]) {
        return Some(format!(
            "Couldn't find {host}. Check the spelling, or whether the name resolves on this network."
        ));
    }
    if has(&[
        "no route to host",
        "network is unreachable",
        "os error 113",
        "os error 101",
    ]) {
        return Some(format!(
            "Can't reach {host}:{port} from this network. Check your connection or VPN."
        ));
    }
    // A live session whose link went away underneath it (server stopped, network
    // dropped): the SFTP/exec layers report this as a closed session or channel.
    if has(&[
        "session closed",
        "channel closed",
        "connection reset",
        "broken pipe",
        "os error 104",
        "os error 32",
        "unexpected eof",
    ]) {
        return Some(format!(
            "The connection to {host}:{port} dropped. Reconnect the session and try again."
        ));
    }
    None
}

/// The failed pane's title: the alias plus where it tried to connect.
pub(crate) fn title(alias: &str, host: &str, port: u16) -> String {
    format!("Couldn't connect to {alias} ({host}:{port})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refused() {
        let s = friendly(
            "connect failed: Connection refused (os error 111)",
            "h",
            2222,
        )
        .unwrap();
        assert_eq!(
            s,
            "Nothing is listening on h:2222. Is the SSH server running, and is the port right?"
        );
    }

    #[test]
    fn timeout_keeps_duration() {
        let s = friendly("operation timed out after 10s", "h", 22).unwrap();
        assert_eq!(
            s,
            "No answer from h:22 within 10s. Check the address and your network."
        );
    }

    #[test]
    fn dns() {
        let s = friendly(
            "connect failed: failed to lookup address information: Name or service not known",
            "nope.invalid",
            22,
        )
        .unwrap();
        assert!(s.starts_with("Couldn't find nope.invalid."));
    }

    #[test]
    fn unreachable() {
        assert!(friendly("connect failed: No route to host (os error 113)", "h", 22).is_some());
    }

    #[test]
    fn dropped_session() {
        let s = friendly("ssh operation failed: sftp: session closed", "h", 22).unwrap();
        assert!(s.starts_with("The connection to h:22 dropped."));
    }

    #[test]
    fn auth_and_unknown_pass_through() {
        assert!(friendly("authentication failed: bad password", "h", 22).is_none());
        assert!(friendly("host key not trusted - something", "h", 22).is_none());
        assert!(friendly("something odd", "h", 22).is_none());
    }

    #[test]
    fn title_has_port() {
        assert_eq!(
            title("web", "10.0.0.1", 22),
            "Couldn't connect to web (10.0.0.1:22)"
        );
    }
}
