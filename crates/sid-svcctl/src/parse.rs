//! `systemctl list-units --output=json` → [`ServiceInfo`] mapping.
//!
//! The archived `sid-poc`'s `sid-system::parse::parse_list_units` (see
//! `~/vcs/sid-poc/crates/sid-system/src/parse.rs`) split `--plain --no-legend`
//! whitespace-delimited columns by hand. `systemctl` has supported `--output=json`
//! since systemd 230 and this machine's live `systemctl --user list-units
//! --type=service --all --output=json` was used to confirm the row shape during
//! development (`{"unit":..,"load":..,"active":..,"sub":..,"description":..}`) — JSON
//! removes the whitespace-splitting heuristics (and their edge cases around
//! multi-word descriptions) entirely, so this is written fresh rather than ported.

use serde::Deserialize;
use sid_core::svc::{ServiceInfo, SvcActiveState, SvcError};

/// One row exactly as systemd's JSON writer emits it.
#[derive(Deserialize)]
struct RawUnit {
    unit: String,
    active: String,
    sub: String,
    #[serde(default)]
    description: String,
}

/// Parse `systemctl list-units --output=json` stdout into [`ServiceInfo`] rows.
/// Never panics; malformed JSON is a [`SvcError::Other`], not a panic — callers treat
/// it the same as any other probe failure.
pub(crate) fn parse_list_units(raw: &str) -> Result<Vec<ServiceInfo>, SvcError> {
    let rows: Vec<RawUnit> = serde_json::from_str(raw)
        .map_err(|e| SvcError::Other(format!("parse list-units json: {e}")))?;
    Ok(rows
        .into_iter()
        .map(|r| ServiceInfo {
            name: r.unit,
            description: r.description,
            active: parse_active_state(&r.active),
            sub_state: r.sub,
        })
        .collect())
}

/// Map systemd's textual `ActiveState` to [`SvcActiveState`]. Unknown values fold to
/// `Other` rather than erroring — the table still renders the row, just with the dim
/// "other" badge instead of failing the whole list.
pub(crate) fn parse_active_state(s: &str) -> SvcActiveState {
    match s {
        "active" => SvcActiveState::Active,
        "inactive" => SvcActiveState::Inactive,
        "failed" => SvcActiveState::Failed,
        _ => SvcActiveState::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_realistic_row() {
        let json = r#"[{"unit":"nginx.service","load":"loaded","active":"active","sub":"running","description":"A high performance web server"}]"#;
        let rows = parse_list_units(json).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "nginx.service");
        assert_eq!(rows[0].active, SvcActiveState::Active);
        assert_eq!(rows[0].sub_state, "running");
        assert_eq!(rows[0].description, "A high performance web server");
    }

    #[test]
    fn parses_multiple_rows_with_mixed_states() {
        let json = r#"[
            {"unit":"a.service","load":"loaded","active":"active","sub":"running","description":"a"},
            {"unit":"b.service","load":"loaded","active":"failed","sub":"failed","description":"b"},
            {"unit":"c.service","load":"loaded","active":"inactive","sub":"dead","description":"c"}
        ]"#;
        let rows = parse_list_units(json).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].active, SvcActiveState::Failed);
        assert_eq!(rows[2].active, SvcActiveState::Inactive);
    }

    #[test]
    fn missing_description_defaults_to_empty() {
        let json = r#"[{"unit":"a.service","load":"loaded","active":"active","sub":"running"}]"#;
        let rows = parse_list_units(json).unwrap();
        assert_eq!(rows[0].description, "");
    }

    #[test]
    fn empty_array_is_empty_vec() {
        assert!(parse_list_units("[]").unwrap().is_empty());
    }

    #[test]
    fn malformed_json_is_other_error_not_panic() {
        let e = parse_list_units("not json").unwrap_err();
        assert!(matches!(e, SvcError::Other(_)));
    }

    #[test]
    fn active_state_maps_known_and_unknown_values() {
        assert_eq!(parse_active_state("active"), SvcActiveState::Active);
        assert_eq!(parse_active_state("inactive"), SvcActiveState::Inactive);
        assert_eq!(parse_active_state("failed"), SvcActiveState::Failed);
        assert_eq!(parse_active_state("activating"), SvcActiveState::Other);
        assert_eq!(parse_active_state("deactivating"), SvcActiveState::Other);
        assert_eq!(parse_active_state("garbage"), SvcActiveState::Other);
    }
}
