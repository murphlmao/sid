//! `docker ps -a --format '{{json .}}'` → [`ContainerInfo`] mapping.
//!
//! Unlike `sid-svcctl`'s `systemctl --output=json` (one JSON *array*), `docker ps
//! --format '{{json .}}'` emits one JSON *object per line* — there is no enclosing
//! array. Each line is parsed independently.

use serde::Deserialize;
use sid_core::containers::{ContainerError, ContainerInfo};

/// One row exactly as `docker ps --format '{{json .}}'` emits it. Field names match
/// Docker's Go-template keys verbatim (capitalized).
#[derive(Deserialize)]
struct RawContainer {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "Names")]
    names: String,
    #[serde(rename = "Image")]
    image: String,
    #[serde(rename = "State")]
    state: String,
    #[serde(rename = "Status")]
    status: String,
    #[serde(rename = "Ports", default)]
    ports: String,
}

/// Parse `docker ps -a --format '{{json .}}'` stdout into [`ContainerInfo`] rows.
/// Never panics; malformed JSON on any one line is a [`ContainerError::Other`] for the
/// whole batch, not a panic and not a partial result — callers treat it the same as
/// any other probe failure. Empty input (no containers at all — a valid, common state,
/// unlike `systemctl`'s JSON-array output which is never truly empty stdout) yields an
/// empty `Vec`, not an error.
pub(crate) fn parse_ps_lines(raw: &str) -> Result<Vec<ContainerInfo>, ContainerError> {
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|line| {
            let row: RawContainer = serde_json::from_str(line)
                .map_err(|e| ContainerError::Other(format!("parse docker ps json line: {e}")))?;
            Ok(ContainerInfo {
                id: row.id,
                // Docker can report multiple comma-joined names for a container with
                // legacy `--link` aliases; the table shows the first (primary) one.
                name: row.names.split(',').next().unwrap_or_default().to_string(),
                image: row.image,
                state: row.state,
                status: row.status,
                ports: split_ports(&row.ports),
            })
        })
        .collect()
}

/// Split docker's comma-joined `Ports` string (e.g.
/// `"0.0.0.0:5432->5432/tcp, :::5432->5432/tcp"`) into individual mapping strings.
/// Empty input (a container with no published ports) yields an empty `Vec`.
fn split_ports(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_realistic_row() {
        let json = r#"{"ID":"a1b2c3d4e5f6","Names":"dev-eggsightv2-1","Image":"postgres:16","State":"running","Status":"Up 3 hours","Ports":"0.0.0.0:5432->5432/tcp, :::5432->5432/tcp"}"#;
        let rows = parse_ps_lines(json).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "a1b2c3d4e5f6");
        assert_eq!(rows[0].name, "dev-eggsightv2-1");
        assert_eq!(rows[0].image, "postgres:16");
        assert_eq!(rows[0].state, "running");
        assert_eq!(rows[0].status, "Up 3 hours");
        assert_eq!(
            rows[0].ports,
            vec!["0.0.0.0:5432->5432/tcp", ":::5432->5432/tcp"]
        );
    }

    #[test]
    fn parses_multiple_lines() {
        let raw = concat!(
            r#"{"ID":"1","Names":"a","Image":"img-a","State":"running","Status":"Up 1 hour","Ports":""}"#,
            "\n",
            r#"{"ID":"2","Names":"b","Image":"img-b","State":"exited","Status":"Exited (0) 2 days ago","Ports":""}"#,
        );
        let rows = parse_ps_lines(raw).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "a");
        assert_eq!(rows[1].name, "b");
        assert_eq!(rows[1].state, "exited");
    }

    #[test]
    fn no_published_ports_is_an_empty_vec() {
        let json =
            r#"{"ID":"1","Names":"a","Image":"img","State":"running","Status":"Up","Ports":""}"#;
        let rows = parse_ps_lines(json).unwrap();
        assert!(rows[0].ports.is_empty());
    }

    #[test]
    fn multiple_names_takes_the_first() {
        let json = r#"{"ID":"1","Names":"primary,alias1,alias2","Image":"img","State":"running","Status":"Up","Ports":""}"#;
        let rows = parse_ps_lines(json).unwrap();
        assert_eq!(rows[0].name, "primary");
    }

    #[test]
    fn missing_ports_field_defaults_to_empty() {
        let json = r#"{"ID":"1","Names":"a","Image":"img","State":"running","Status":"Up"}"#;
        let rows = parse_ps_lines(json).unwrap();
        assert!(rows[0].ports.is_empty());
    }

    #[test]
    fn empty_input_is_an_empty_vec_not_an_error() {
        // No containers at all (fresh docker install, or everything pruned) is a
        // valid, common state — distinct from `sid-svcctl`'s JSON-array parsing, where
        // an empty stdout capture is invalid input.
        assert!(parse_ps_lines("").unwrap().is_empty());
    }

    #[test]
    fn whitespace_only_input_is_an_empty_vec() {
        assert!(parse_ps_lines("\n  \n\t\n").unwrap().is_empty());
    }

    #[test]
    fn malformed_json_line_is_other_error_not_panic() {
        let e = parse_ps_lines("not json").unwrap_err();
        assert!(matches!(e, ContainerError::Other(_)));
    }

    #[test]
    fn one_malformed_line_fails_the_whole_batch() {
        let raw = concat!(
            r#"{"ID":"1","Names":"a","Image":"img","State":"running","Status":"Up","Ports":""}"#,
            "\nnot json",
        );
        let e = parse_ps_lines(raw).unwrap_err();
        assert!(matches!(e, ContainerError::Other(_)));
    }

    #[test]
    fn row_missing_required_field_is_an_error() {
        let json = r#"{"Names":"a","Image":"img","State":"running","Status":"Up"}"#;
        let e = parse_ps_lines(json).unwrap_err();
        assert!(matches!(e, ContainerError::Other(_)));
    }
}
