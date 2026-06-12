//! UX-v2 detail pane for the Network tab's interfaces sidebar.
//!
//! [`build_form_spec`] constructs a [`FormSpec`] from a live [`NetInterface`]
//! snapshot and user prefs; [`prefs_from_values`] extracts a
//! [`NetInterfacePrefs`] back out of a submitted [`FormValues`] map.

use sid_core::adapters::sys::NetInterface;

use crate::form::{FormField, FormSection, FormSpec, FormValues, SectionKind, Validate};
use crate::modal::Field;

/// Sid-level per-interface user preferences persisted in the settings store.
///
/// # Examples
///
/// ```
/// use sid_widgets::network::detail_pane::NetInterfacePrefs;
/// let p = NetInterfacePrefs { pinned: true, alias: "work-lan".into() };
/// assert!(p.pinned);
/// assert_eq!(p.alias, "work-lan");
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetInterfacePrefs {
    /// Whether this interface is pinned to the top of the sidebar list.
    pub pinned: bool,
    /// User-chosen display name; empty means "use the raw interface name".
    pub alias: String,
}

/// Build a [`FormSpec`] for the UX-v2 interface detail pane.
///
/// The spec has three sections:
/// 1. Info — observable interface data (status, addresses, RX/TX bytes).
/// 2. Editable — sid-level prefs (pin-to-top toggle, display alias text).
/// 3. Info — OS-level disclaimer note.
///
/// # Examples
///
/// ```
/// use sid_core::adapters::sys::NetInterface;
/// use sid_widgets::network::detail_pane::{build_form_spec, NetInterfacePrefs};
/// use sid_widgets::form::SectionKind;
///
/// let iface = NetInterface { name: "eth0".into(), addrs: vec![], rx_bytes: 0, tx_bytes: 0, is_up: true };
/// let spec = build_form_spec(&iface, &NetInterfacePrefs::default(), false);
/// assert_eq!(spec.sections.len(), 3);
/// assert_eq!(spec.sections[0].kind, SectionKind::Info);
/// assert_eq!(spec.sections[1].kind, SectionKind::Editable);
/// assert_eq!(spec.sections[2].kind, SectionKind::Info);
/// ```
pub fn build_form_spec(
    iface: &NetInterface,
    prefs: &NetInterfacePrefs,
    is_default_route: bool,
) -> FormSpec {
    let status_label = if iface.is_up { "up" } else { "down" };
    let default_badge = if is_default_route { " (default route)" } else { "" };
    let status_str = format!("{status_label}{default_badge}");

    let addrs_str = if iface.addrs.is_empty() {
        "(none)".to_string()
    } else {
        iface.addrs.join(", ")
    };

    let rx_str = format_bytes(iface.rx_bytes);
    let tx_str = format_bytes(iface.tx_bytes);

    let info_section = FormSection {
        title: "Interface".into(),
        kind: SectionKind::Info,
        fields: vec![
            FormField::new(
                "status",
                Field::Display {
                    label: "Status".into(),
                    body: status_str,
                },
            ),
            FormField::new(
                "addresses",
                Field::Display {
                    label: "Addresses".into(),
                    body: addrs_str,
                },
            ),
            FormField::new(
                "rx",
                Field::Display {
                    label: "RX".into(),
                    body: rx_str,
                },
            ),
            FormField::new(
                "tx",
                Field::Display {
                    label: "TX".into(),
                    body: tx_str,
                },
            ),
        ],
    };

    let edit_section = FormSection {
        title: "sid prefs".into(),
        kind: SectionKind::Editable,
        fields: vec![
            FormField::new(
                "pinned",
                Field::Toggle {
                    label: "Pin to top".into(),
                    value: prefs.pinned,
                },
            ),
            FormField::new(
                "alias",
                Field::Text {
                    label: "Display alias".into(),
                    value: prefs.alias.clone(),
                    placeholder: Some("e.g. work-lan".into()),
                },
            )
            .with_validate(vec![Validate::MaxLen(40)]),
        ],
    };

    let disclaimer_section = FormSection {
        title: "".into(),
        kind: SectionKind::Info,
        fields: vec![FormField::new(
            "os_note",
            Field::Display {
                label: "".into(),
                body: "OS-level interface configuration is not supported here.".into(),
            },
        )],
    };

    FormSpec::new(
        format!("network.interface_prefs:{}", iface.name),
        format!("Interface: {}", iface.name),
        vec![info_section, edit_section, disclaimer_section],
    )
}

/// Extract [`NetInterfacePrefs`] from a submitted [`FormValues`] map.
///
/// Returns `None` if required keys are absent (should not happen in
/// practice since the form always initialises them).
///
/// [`FormValues`] is `BTreeMap<String, String>`. The `pinned` key holds
/// `"true"` or `"false"` (as produced by the substrate's `Toggle` field
/// via `value_string()`); anything else returns `None`. The `alias` key
/// is trimmed.
///
/// # Examples
///
/// ```
/// use std::collections::BTreeMap;
/// use sid_widgets::network::detail_pane::prefs_from_values;
///
/// let mut map: BTreeMap<String, String> = BTreeMap::new();
/// map.insert("pinned".to_string(), "true".to_string());
/// map.insert("alias".to_string(), "home".to_string());
/// let prefs = prefs_from_values(&map).unwrap();
/// assert!(prefs.pinned);
/// assert_eq!(prefs.alias, "home");
/// ```
pub fn prefs_from_values(values: &FormValues) -> Option<NetInterfacePrefs> {
    let pinned = match values.get("pinned")?.as_str() {
        "true" => true,
        "false" => false,
        _ => return None,
    };
    let alias = values.get("alias")?.trim().to_string();
    Some(NetInterfacePrefs { pinned, alias })
}

/// Format a byte count as a short human string (KB / MB / GB).
fn format_bytes(b: u64) -> String {
    const KB: u64 = 1_024;
    const MB: u64 = KB * 1_024;
    const GB: u64 = MB * 1_024;
    if b >= GB {
        format!("{:.1} GB", b as f64 / GB as f64)
    } else if b >= MB {
        format!("{:.1} MB", b as f64 / MB as f64)
    } else if b >= KB {
        format!("{:.1} KB", b as f64 / KB as f64)
    } else {
        format!("{b} B")
    }
}

/// Build the settings key for the pinned flag of a given interface.
///
/// # Examples
///
/// ```
/// use sid_widgets::network::detail_pane::pinned_key;
/// assert_eq!(pinned_key("eth0"), "network.iface.eth0.pinned");
/// assert_eq!(pinned_key("wlan0"), "network.iface.wlan0.pinned");
/// ```
pub fn pinned_key(name: &str) -> String {
    format!("network.iface.{name}.pinned")
}

/// Build the settings key for the display alias of a given interface.
///
/// # Examples
///
/// ```
/// use sid_widgets::network::detail_pane::alias_key;
/// assert_eq!(alias_key("eth0"), "network.iface.eth0.alias");
/// ```
pub fn alias_key(name: &str) -> String {
    format!("network.iface.{name}.alias")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn sample_iface() -> NetInterface {
        NetInterface {
            name: "eth0".into(),
            addrs: vec!["10.0.0.1".into(), "fe80::1".into()],
            rx_bytes: 2_048,
            tx_bytes: 1_024,
            is_up: true,
        }
    }

    #[test]
    fn build_form_spec_has_three_sections() {
        let spec = build_form_spec(&sample_iface(), &NetInterfacePrefs::default(), false);
        assert_eq!(spec.sections.len(), 3);
        assert_eq!(spec.sections[0].kind, SectionKind::Info);
        assert_eq!(spec.sections[1].kind, SectionKind::Editable);
        assert_eq!(spec.sections[2].kind, SectionKind::Info);
    }

    #[test]
    fn build_form_spec_default_route_badge_present() {
        let spec = build_form_spec(&sample_iface(), &NetInterfacePrefs::default(), true);
        let status_field = &spec.sections[0].fields[0];
        if let Field::Display { body, .. } = &status_field.field {
            assert!(body.contains("default route"), "expected badge in: {body}");
        } else {
            panic!("status field is not Display");
        }
    }

    #[test]
    fn build_form_spec_no_badge_when_not_default() {
        let spec = build_form_spec(&sample_iface(), &NetInterfacePrefs::default(), false);
        let status_field = &spec.sections[0].fields[0];
        if let Field::Display { body, .. } = &status_field.field {
            assert!(!body.contains("default route"), "unexpected badge in: {body}");
        }
    }

    #[test]
    fn build_form_spec_down_interface_shows_down() {
        let mut iface = sample_iface();
        iface.is_up = false;
        let spec = build_form_spec(&iface, &NetInterfacePrefs::default(), false);
        if let Field::Display { body, .. } = &spec.sections[0].fields[0].field {
            assert!(body.starts_with("down"), "expected 'down' prefix, got: {body}");
        }
    }

    #[test]
    fn build_form_spec_empty_addrs_shows_none() {
        let mut iface = sample_iface();
        iface.addrs.clear();
        let spec = build_form_spec(&iface, &NetInterfacePrefs::default(), false);
        if let Field::Display { body, .. } = &spec.sections[0].fields[1].field {
            assert_eq!(body, "(none)");
        }
    }

    #[test]
    fn build_form_spec_prefs_roundtrip() {
        let prefs = NetInterfacePrefs { pinned: true, alias: "home-net".into() };
        let spec = build_form_spec(&sample_iface(), &prefs, false);
        // Toggle field should carry the pinned value.
        if let Field::Toggle { value, .. } = &spec.sections[1].fields[0].field {
            assert!(*value);
        } else {
            panic!("expected Toggle");
        }
        // Text field should carry the alias.
        if let Field::Text { value, .. } = &spec.sections[1].fields[1].field {
            assert_eq!(value, "home-net");
        } else {
            panic!("expected Text");
        }
    }

    #[test]
    fn prefs_from_values_happy_path() {
        // FormValues is BTreeMap<String, String>; Toggle yields "true"/"false".
        let mut map = BTreeMap::new();
        map.insert("pinned".into(), "false".into());
        map.insert("alias".into(), "  trimmed  ".into());
        let prefs = prefs_from_values(&map).unwrap();
        assert!(!prefs.pinned);
        assert_eq!(prefs.alias, "trimmed");
    }

    #[test]
    fn prefs_from_values_missing_key_returns_none() {
        let map: FormValues = BTreeMap::new();
        assert!(prefs_from_values(&map).is_none());
    }

    #[test]
    fn prefs_from_values_wrong_value_returns_none() {
        let mut map = BTreeMap::new();
        // pinned must be "true" or "false"; anything else returns None.
        map.insert("pinned".into(), "yes".into());
        map.insert("alias".into(), "x".into());
        assert!(prefs_from_values(&map).is_none());
    }

    #[test]
    fn pinned_key_format() {
        assert_eq!(pinned_key("eth0"), "network.iface.eth0.pinned");
        assert_eq!(pinned_key("docker0"), "network.iface.docker0.pinned");
    }

    #[test]
    fn alias_key_format() {
        assert_eq!(alias_key("wlan0"), "network.iface.wlan0.alias");
    }

    #[test]
    fn format_bytes_boundaries() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GB");
    }

    #[test]
    fn disclaimer_section_is_last_and_info() {
        let spec = build_form_spec(&sample_iface(), &NetInterfacePrefs::default(), false);
        let last = spec.sections.last().unwrap();
        assert_eq!(last.kind, SectionKind::Info);
        if let Field::Display { body, .. } = &last.fields[0].field {
            assert!(
                body.contains("OS-level"),
                "disclaimer not found; got: {body}"
            );
        }
    }

    #[test]
    fn form_id_embeds_interface_name() {
        let spec = build_form_spec(&sample_iface(), &NetInterfacePrefs::default(), false);
        assert_eq!(spec.id.0, "network.interface_prefs:eth0");
    }

    #[test]
    fn max_len_validator_works() {
        assert!(Validate::MaxLen(40).check("short").is_none());
        let long: String = "x".repeat(41);
        assert!(Validate::MaxLen(40).check(&long).is_some());
    }
}
