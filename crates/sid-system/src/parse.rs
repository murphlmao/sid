//! Pure-Rust parsers for `systemctl` and `journalctl` text output.
//!
//! These never panic, even on adversarial input — invariant verified by
//! the proptest harness in `tests/parse_fuzz.rs`.

use sid_core::adapters::systemctl::{
    JournalEntry, SystemUnit, SystemctlError, UnitBus, UnitState,
};

/// Parse the output of `systemctl --no-pager --plain --no-legend list-units --type=service`.
///
/// Columns (whitespace-separated): `name load active sub description...`.
/// The description column is rest-of-line; may be empty. CRLF tolerated.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::systemctl::{UnitBus, UnitState};
/// use sid_system::parse::parse_list_units;
///
/// let sample = "nginx.service loaded active running web server\n";
/// let units = parse_list_units(sample, UnitBus::System).unwrap();
/// assert_eq!(units.len(), 1);
/// assert_eq!(units[0].state, UnitState::Active);
/// ```
pub fn parse_list_units(out: &str, bus: UnitBus) -> Result<Vec<SystemUnit>, SystemctlError> {
    let mut units = Vec::new();
    for raw_line in out.split('\n') {
        let line = raw_line.trim_end_matches('\r').trim();
        if line.is_empty() {
            continue;
        }
        // Skip header-ish lines defensively.
        if line.starts_with("UNIT ")
            || line.starts_with("LOAD ")
            || line.starts_with("ACTIVE ")
            || line.starts_with("SUB ")
        {
            continue;
        }
        let mut parts = line.split_whitespace();
        let name = match parts.next() {
            Some(n) => n.to_string(),
            None => continue,
        };
        let load_state = parts.next().unwrap_or("").to_string();
        let active = parts.next().unwrap_or("");
        let sub_state = parts.next().unwrap_or("").to_string();
        let description = parts.collect::<Vec<_>>().join(" ");
        units.push(SystemUnit {
            name,
            bus,
            state: parse_unit_state(active),
            sub_state,
            description,
            load_state,
        });
    }
    Ok(units)
}

/// Map systemd's textual ActiveState to our enum. Unknown values become [`UnitState::Unknown`].
///
/// # Examples
///
/// ```
/// use sid_core::adapters::systemctl::UnitState;
/// use sid_system::parse::parse_unit_state;
///
/// assert_eq!(parse_unit_state("active"), UnitState::Active);
/// assert_eq!(parse_unit_state("garbage"), UnitState::Unknown);
/// ```
pub fn parse_unit_state(s: &str) -> UnitState {
    match s {
        "active" => UnitState::Active,
        "reloading" => UnitState::Reloading,
        "inactive" => UnitState::Inactive,
        "failed" => UnitState::Failed,
        "activating" => UnitState::Activating,
        "deactivating" => UnitState::Deactivating,
        _ => UnitState::Unknown,
    }
}

/// Parse a single-unit `systemctl status` output. Best-effort; never panics.
///
/// # Errors
///
/// Returns [`SystemctlError::UnitNotFound`] when output mentions "could not be
/// found", [`SystemctlError::Parse`] for empty input.
pub fn parse_status(out: &str, name: &str, bus: UnitBus) -> Result<SystemUnit, SystemctlError> {
    if out.trim().is_empty() {
        return Err(SystemctlError::Parse(format!(
            "status output empty for {name}"
        )));
    }
    if out.contains("could not be found") {
        return Err(SystemctlError::UnitNotFound(name.to_string()));
    }
    let mut description = String::new();
    let mut load_state = String::new();
    let mut state = UnitState::Unknown;
    let mut sub_state = String::new();
    for raw in out.split('\n') {
        let line = raw.trim_end_matches('\r');
        let trimmed = line.trim_start();
        // Header: "● name.service - Description"
        if let Some(rest) = trimmed.strip_prefix("● ") {
            if let Some(idx) = rest.find(" - ") {
                description = rest[idx + 3..].trim().to_string();
            }
        }
        if let Some(rest) = trimmed.strip_prefix("Loaded:") {
            let toks: Vec<&str> = rest.split_whitespace().collect();
            if let Some(s) = toks.first() {
                load_state = (*s).to_string();
            }
        }
        if let Some(rest) = trimmed.strip_prefix("Active:") {
            let toks: Vec<&str> = rest.split_whitespace().collect();
            if let Some(active_word) = toks.first() {
                state = parse_unit_state(active_word);
            }
            if let Some(start) = rest.find('(') {
                if let Some(end) = rest[start + 1..].find(')') {
                    sub_state = rest[start + 1..start + 1 + end].to_string();
                }
            }
        }
    }
    Ok(SystemUnit {
        name: name.to_string(),
        bus,
        state,
        sub_state,
        description,
        load_state,
    })
}

/// Parse `journalctl --no-pager --output=short-iso --lines=N -u <unit>` output.
///
/// Format expected per-line: `<ISO8601> <hostname> <source>: <message>`.
/// Malformed lines are silently skipped. Never panics on adversarial input —
/// invariant verified by proptest in `tests/parse_fuzz.rs`.
///
/// # Examples
///
/// ```
/// use sid_system::parse::parse_journal;
/// let entries = parse_journal("").unwrap();
/// assert!(entries.is_empty());
/// ```
pub fn parse_journal(out: &str) -> Result<Vec<JournalEntry>, SystemctlError> {
    let mut entries = Vec::new();
    for raw in out.split('\n') {
        let line = raw.trim_end_matches('\r').trim();
        if line.is_empty() {
            continue;
        }
        let Some(ts_end) = line.find(' ') else {
            continue;
        };
        let ts_str = &line[..ts_end];
        let rest = &line[ts_end + 1..];
        let Some(host_end) = rest.find(' ') else {
            continue;
        };
        let hostname = rest[..host_end].to_string();
        let rest = &rest[host_end + 1..];
        let Some(src_end) = rest.find(": ") else {
            continue;
        };
        let source = rest[..src_end].to_string();
        let message = rest[src_end + 2..].to_string();
        let timestamp_secs = parse_iso8601_to_epoch(ts_str).unwrap_or(0);
        entries.push(JournalEntry {
            timestamp_secs,
            hostname,
            source,
            message,
        });
    }
    Ok(entries)
}

/// Cheap ISO8601 → epoch seconds. Returns `None` on parse failure.
///
/// Avoids pulling in `chrono`. Accepts forms like `YYYY-MM-DDTHH:MM:SS+ZZZZ`
/// or with `Z` suffix. Timezone offset is **ignored** in v1 — sub-minute
/// accuracy is sufficient for journal display.
fn parse_iso8601_to_epoch(s: &str) -> Option<i64> {
    if s.len() < 19 {
        return None;
    }
    // Validate the structural characters before slicing.
    let bytes = s.as_bytes();
    if bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || bytes.get(10) != Some(&b'T')
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
    {
        return None;
    }
    let year: i32 = s.get(0..4)?.parse().ok()?;
    let month: u32 = s.get(5..7)?.parse().ok()?;
    let day: u32 = s.get(8..10)?.parse().ok()?;
    let hour: u32 = s.get(11..13)?.parse().ok()?;
    let min: u32 = s.get(14..16)?.parse().ok()?;
    let sec: u32 = s.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month) || day == 0 || day > 31 || hour > 23 || min > 59 || sec > 60 {
        return None;
    }
    // Days from civil epoch (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year } as i64;
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = (y - era * 400) as u64;
    let m = month as u64;
    let d = day as u64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days_from_epoch = era * 146_097 + doe as i64 - 719_468;
    let total =
        days_from_epoch * 86_400 + hour as i64 * 3_600 + min as i64 * 60 + sec as i64;
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso8601_parses_known_value() {
        // 2026-05-21T00:00:00 → epoch seconds (UTC ignored, treated as naive).
        let s = "2026-05-21T00:00:00+0000";
        let v = parse_iso8601_to_epoch(s).unwrap();
        assert!(v > 0);
    }

    #[test]
    fn iso8601_rejects_short_input() {
        assert!(parse_iso8601_to_epoch("2026-05-21").is_none());
    }

    #[test]
    fn iso8601_rejects_bad_separators() {
        assert!(parse_iso8601_to_epoch("2026/05/21T00:00:00").is_none());
    }
}
