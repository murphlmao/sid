//! `crate_map` — workspace crates, purpose, intra-workspace edges, GPUI allowance.

use std::collections::BTreeSet;
use std::path::Path;

/// Crates allowed to name GPUI (CLAUDE.md rule 1).
pub const GPUI_CRATES: &[&str] = &["sid", "sid-ui"];

/// GPUI-family crate names (Cargo package or dependency-key spelling).
pub const GPUI_NAMES: &[&str] = &[
    "gpui",
    "gpui-pre",
    "gpui-pre-platform",
    "gpui_platform",
    "gpui-component",
    "gpui_component",
    "gpui-base",
    "gpui_base",
    "gpui-kit-assets",
    "gpui_kit_assets",
];

/// One workspace crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrateInfo {
    /// Package name.
    pub name: String,
    /// One-line purpose.
    pub purpose: String,
    /// Workspace crates this one depends on (any dependency kind).
    pub deps: BTreeSet<String>,
    /// May this crate name GPUI?
    pub gpui_allowed: bool,
    /// Does its manifest actually depend on a GPUI-family crate?
    pub uses_gpui: bool,
}

/// Parse `[workspace].members` from the root manifest text.
pub fn members(root_toml: &str) -> Vec<String> {
    let Ok(v) = root_toml.parse::<toml::Table>() else {
        return Vec::new();
    };
    v.get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(|m| m.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// First `//!` line of a source file, de-markdowned of leading `sid-x —` noise.
pub fn first_doc_line(src: &str) -> Option<String> {
    src.lines()
        .map(str::trim)
        .take_while(|l| l.starts_with("//!") || l.is_empty())
        .find_map(|l| {
            let t = l.trim_start_matches("//!").trim();
            (!t.is_empty()).then(|| t.to_string())
        })
}

/// Dependency keys (and their `package = ` renames) across all dependency tables.
pub fn dep_names(manifest: &toml::Table) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut tables: Vec<&toml::Value> = Vec::new();
    for k in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(t) = manifest.get(k) {
            tables.push(t);
        }
    }
    if let Some(targets) = manifest.get("target").and_then(|t| t.as_table()) {
        for cfg in targets.values() {
            for k in ["dependencies", "dev-dependencies", "build-dependencies"] {
                if let Some(t) = cfg.get(k) {
                    tables.push(t);
                }
            }
        }
    }
    for t in tables {
        if let Some(t) = t.as_table() {
            for (k, v) in t {
                out.insert(k.clone());
                if let Some(p) = v.get("package").and_then(|p| p.as_str()) {
                    out.insert(p.to_string());
                }
            }
        }
    }
    out
}

/// Build the crate list for the workspace at `root`.
pub fn collect(root: &Path) -> Result<Vec<CrateInfo>, String> {
    let root_toml = std::fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|e| format!("read root Cargo.toml: {e}"))?;
    let mut raw = Vec::new();
    for m in members(&root_toml) {
        let dir = root.join(&m);
        let Ok(text) = std::fs::read_to_string(dir.join("Cargo.toml")) else {
            continue;
        };
        let Ok(table) = text.parse::<toml::Table>() else {
            continue;
        };
        let name = table
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or(&m)
            .to_string();
        let desc = table
            .get("package")
            .and_then(|p| p.get("description"))
            .and_then(|d| d.as_str())
            .map(str::to_string);
        let purpose = desc
            .or_else(|| {
                ["src/lib.rs", "src/main.rs"].iter().find_map(|f| {
                    std::fs::read_to_string(dir.join(f))
                        .ok()
                        .and_then(|s| first_doc_line(&s))
                })
            })
            .unwrap_or_else(|| "(no description)".into());
        raw.push((name, purpose, dep_names(&table)));
    }
    let names: BTreeSet<String> = raw.iter().map(|(n, _, _)| n.clone()).collect();
    let mut out: Vec<CrateInfo> = raw
        .into_iter()
        .map(|(name, purpose, deps)| {
            let uses_gpui = deps.iter().any(|d| GPUI_NAMES.contains(&d.as_str()));
            CrateInfo {
                gpui_allowed: GPUI_CRATES.contains(&name.as_str()),
                uses_gpui,
                deps: deps
                    .into_iter()
                    .filter(|d| names.contains(d) && *d != name)
                    .collect(),
                name,
                purpose,
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Compact text rendering.
pub fn render(crates: &[CrateInfo]) -> String {
    let mut s = format!(
        "{} workspace crates (GPUI allowed only in: sid, sid-ui)\n",
        crates.len()
    );
    for c in crates {
        let flag = match (c.gpui_allowed, c.uses_gpui) {
            (true, _) => " [gpui ok]",
            (false, true) => " [GPUI VIOLATION]",
            (false, false) => "",
        };
        s.push_str(&format!("- {}{} - {}\n", c.name, flag, c.purpose));
        if !c.deps.is_empty() {
            let d: Vec<&str> = c.deps.iter().map(String::as_str).collect();
            s.push_str(&format!("    -> {}\n", d.join(", ")));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        std::fs::write(
            r.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/a\", \"crates/b\", \"crates/sid-ui\"]\n",
        )
        .unwrap();
        for (n, body, src) in [
            (
                "a",
                "[package]\nname=\"a\"\nversion=\"0\"\ndescription=\"A thing\"\n",
                "",
            ),
            (
                "b",
                "[package]\nname=\"b\"\nversion=\"0\"\n[dependencies]\na={path=\"../a\"}\ngpui={package=\"gpui-pre\",version=\"0.3\"}\n",
                "//! The b crate.\n//! More.\n",
            ),
            (
                "sid-ui",
                "[package]\nname=\"sid-ui\"\nversion=\"0\"\n[dev-dependencies]\nb={path=\"../b\"}\ngpui-component=\"0.6\"\n",
                "",
            ),
        ] {
            std::fs::create_dir_all(r.join("crates").join(n).join("src")).unwrap();
            std::fs::write(r.join("crates").join(n).join("Cargo.toml"), body).unwrap();
            std::fs::write(r.join("crates").join(n).join("src/lib.rs"), src).unwrap();
        }
        d
    }

    #[test]
    fn collects_purpose_edges_and_gpui_flags() {
        let d = fixture();
        let c = collect(d.path()).unwrap();
        assert_eq!(c.len(), 3);
        let a = &c[0];
        assert_eq!(a.purpose, "A thing");
        assert!(!a.uses_gpui);
        let b = &c[1];
        assert_eq!(b.purpose, "The b crate.");
        assert_eq!(b.deps.iter().collect::<Vec<_>>(), vec!["a"]);
        assert!(b.uses_gpui && !b.gpui_allowed);
        let ui = &c[2];
        assert!(ui.gpui_allowed && ui.uses_gpui);
        let text = render(&c);
        assert!(text.contains("- b [GPUI VIOLATION] - The b crate."));
        assert!(text.contains("- sid-ui [gpui ok]"));
        assert!(text.contains("    -> a"));
    }

    #[test]
    fn first_doc_line_skips_blank_and_stops_at_code() {
        assert_eq!(first_doc_line("\n//! hi\n//! x"), Some("hi".into()));
        assert_eq!(first_doc_line("use x;\n//! late"), None);
    }
}
