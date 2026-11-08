//! `adapter_check` — CLAUDE.md rule-1 violations (adapter pattern).
//!
//! Rules (kept in step with `crates/sid-ui/tests/hygiene.rs`, which enforces the third):
//! 1. GPUI-family crates are named only in `sid` and `sid-ui`.
//! 2. Concrete adapter crates are never named in `sid-core`.
//! 3. `crates/sid/src` (outside `main.rs`) does not name `gpui_component::` directly —
//!    it goes through `sid_ui::component`.

use std::path::Path;

use regex::Regex;

use super::crate_map::{GPUI_CRATES, dep_names};

/// GPUI-family identifiers (underscore spelling, as written in source).
pub const GPUI_IDENTS: &[&str] = &[
    "gpui",
    "gpui_component",
    "gpui_platform",
    "gpui_base",
    "gpui_kit_assets",
];

/// Concrete crates `sid-core` must never name.
pub const CONCRETE_IDENTS: &[&str] = &[
    "redb",
    "russh",
    "russh_sftp",
    "git2",
    "rusqlite",
    "tokio_postgres",
    "sysinfo",
    "vt100",
    "netstat2",
    "nix",
    "zbus",
];

/// A single violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// Rule label.
    pub rule: &'static str,
    /// `path:line` (or just `path` for manifest hits).
    pub at: String,
    /// What was found.
    pub detail: String,
}

/// Lines outside `#[cfg(test)]` modules (same convention as the hygiene test: such a
/// module closes with a `}` at column 0). `(1-based line, text)`.
pub fn shipping_lines(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut in_test = false;
    let mut pending = false;
    for (ix, line) in text.lines().enumerate() {
        if in_test {
            if line == "}" {
                in_test = false;
            }
            continue;
        }
        if line.trim() == "#[cfg(test)]" {
            pending = true;
            continue;
        }
        if pending {
            pending = false;
            if line.starts_with("mod ") || line.starts_with("pub mod ") {
                in_test = !line.trim_end().ends_with(';');
                continue;
            }
        }
        out.push((ix + 1, line));
    }
    out
}

/// Strip a line comment; returns the code part.
fn code_part(line: &str) -> &str {
    let t = line.trim_start();
    if t.starts_with("//") {
        return "";
    }
    match line.find(" //") {
        Some(i) => &line[..i],
        None => line,
    }
}

fn ident_regex(idents: &[&str]) -> Regex {
    Regex::new(&format!(r"\b(?:{})\s*::", idents.join("|"))).expect("static regex")
}

/// Scan one source text for `idents::` path uses.
pub fn scan_source(text: &str, idents: &[&str], skip_tests: bool) -> Vec<(usize, String, String)> {
    let re = ident_regex(idents);
    let lines: Vec<(usize, &str)> = if skip_tests {
        shipping_lines(text)
    } else {
        text.lines().enumerate().map(|(i, l)| (i + 1, l)).collect()
    };
    let mut out = Vec::new();
    for (n, line) in lines {
        let code = code_part(line);
        if let Some(m) = re.find(code) {
            let name = m.as_str().trim_end_matches(':').trim().to_string();
            out.push((n, name, line.trim().to_string()));
        }
    }
    out
}

fn norm(s: &str) -> String {
    s.replace('-', "_")
}

/// Run all rules against the workspace at `root`.
pub fn check(root: &Path) -> Result<Vec<Violation>, String> {
    let crates_dir = root.join("crates");
    let rd = std::fs::read_dir(&crates_dir).map_err(|e| format!("read crates/: {e}"))?;
    let mut dirs: Vec<_> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
    dirs.sort();
    let mut out = Vec::new();
    for dir in dirs.iter().filter(|d| d.join("Cargo.toml").is_file()) {
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap_or_default();
        let deps: Vec<String> = manifest
            .parse::<toml::Table>()
            .map(|t| dep_names(&t).into_iter().map(|d| norm(&d)).collect())
            .unwrap_or_default();
        let rel = |p: &Path| {
            p.strip_prefix(root)
                .unwrap_or(p)
                .to_string_lossy()
                .into_owned()
        };

        // Rule 1: GPUI only in sid / sid-ui.
        if !GPUI_CRATES.contains(&name.as_str()) {
            let gp: Vec<&str> = deps
                .iter()
                .filter(|d| GPUI_IDENTS.contains(&d.as_str()) || d.starts_with("gpui_pre"))
                .map(String::as_str)
                .collect();
            for d in gp {
                out.push(Violation {
                    rule: "gpui-only-in-frontend",
                    at: rel(&dir.join("Cargo.toml")),
                    detail: format!("depends on `{d}`"),
                });
            }
            // sid-mcp names these crates as data (rule tables, fixtures), not as code.
            let src_files = if name == "sid-mcp" {
                Vec::new()
            } else {
                crate::util::files_with_ext(dir, "rs")
            };
            for f in src_files {
                let Ok(text) = std::fs::read_to_string(&f) else {
                    continue;
                };
                for (n, id, line) in scan_source(&text, GPUI_IDENTS, false) {
                    out.push(Violation {
                        rule: "gpui-only-in-frontend",
                        at: format!("{}:{n}", rel(&f)),
                        detail: format!("names `{id}`: {}", crate::util::clip(&line, 100)),
                    });
                }
            }
        }

        // Rule 2: no concrete crates in sid-core.
        if name == "sid-core" {
            for d in deps
                .iter()
                .filter(|d| CONCRETE_IDENTS.contains(&d.as_str()))
            {
                out.push(Violation {
                    rule: "no-concrete-in-core",
                    at: rel(&dir.join("Cargo.toml")),
                    detail: format!("depends on concrete crate `{d}`"),
                });
            }
            for f in crate::util::files_with_ext(dir, "rs") {
                let Ok(text) = std::fs::read_to_string(&f) else {
                    continue;
                };
                for (n, id, line) in scan_source(&text, CONCRETE_IDENTS, false) {
                    out.push(Violation {
                        rule: "no-concrete-in-core",
                        at: format!("{}:{n}", rel(&f)),
                        detail: format!("names `{id}`: {}", crate::util::clip(&line, 100)),
                    });
                }
            }
        }

        // Rule 3 (mirrors hygiene.rs): app crate does not name gpui_component directly.
        if name == "sid" {
            for f in crate::util::files_with_ext(&dir.join("src"), "rs") {
                if f.ends_with("sid/src/main.rs") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&f) else {
                    continue;
                };
                for (n, _, line) in scan_source(&text, &["gpui_component"], true) {
                    out.push(Violation {
                        rule: "app-uses-sid-ui-not-gpui-component",
                        at: format!("{}:{n}", rel(&f)),
                        detail: format!(
                            "use sid_ui::component instead: {}",
                            crate::util::clip(&line, 100)
                        ),
                    });
                }
            }
        }
    }
    Ok(out)
}

/// Compact text rendering.
pub fn render(v: &[Violation]) -> String {
    if v.is_empty() {
        return "adapter_check: OK - no rule-1 violations (gpui only in sid/sid-ui; sid-core free of concrete crates; app avoids gpui_component::)\n".into();
    }
    let mut s = format!("adapter_check: {} violation(s)\n", v.len());
    for x in v {
        s.push_str(&format!("[{}] {} - {}\n", x.rule, x.at, x.detail));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_ignores_comments_and_finds_paths() {
        let src = "// gpui::x\nuse gpui::App;\nlet a = redb::Database::create();\nlet z = 1; // nix::foo\n";
        let g = scan_source(src, GPUI_IDENTS, false);
        assert_eq!(g.len(), 1);
        assert_eq!((g[0].0, g[0].1.as_str()), (2, "gpui"));
        let c = scan_source(src, CONCRETE_IDENTS, false);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].1, "redb");
    }

    #[test]
    fn word_boundary_avoids_substring_hits() {
        assert!(scan_source("my_gpui::x; sidnix::y", &["gpui", "nix"], false).is_empty());
    }

    #[test]
    fn shipping_lines_skips_test_modules_and_resumes() {
        let src = "a\n#[cfg(test)]\nmod t {\n    gpui_component::x\n}\nb gpui_component::y\n";
        let hits = scan_source(src, &["gpui_component"], true);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, 6);
    }

    fn write(p: &Path, body: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn check_flags_gpui_in_wrong_crate_and_concrete_in_core() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        write(
            &r.join("crates/sid-ui/Cargo.toml"),
            "[package]\nname=\"sid-ui\"\n[dependencies]\ngpui={package=\"gpui-pre\",version=\"0.3\"}\n",
        );
        write(&r.join("crates/sid-ui/src/lib.rs"), "use gpui::App;\n");
        write(
            &r.join("crates/sid-core/Cargo.toml"),
            "[package]\nname=\"sid-core\"\n[dependencies]\nredb=\"4\"\n",
        );
        write(
            &r.join("crates/sid-core/src/lib.rs"),
            "fn f(){ russh::go(); }\n",
        );
        write(
            &r.join("crates/sid-git/Cargo.toml"),
            "[package]\nname=\"sid-git\"\n[dependencies]\ngpui-component=\"1\"\n",
        );
        write(
            &r.join("crates/sid-git/src/lib.rs"),
            "use gpui_component::Root;\n",
        );
        write(
            &r.join("crates/sid/Cargo.toml"),
            "[package]\nname=\"sid\"\n",
        );
        write(
            &r.join("crates/sid/src/lib.rs"),
            "use gpui_component::Foo;\n",
        );
        write(
            &r.join("crates/sid/src/main.rs"),
            "use gpui_component::Root;\n",
        );
        let v = check(r).unwrap();
        let rules: Vec<_> = v.iter().map(|x| (x.rule, x.at.as_str())).collect();
        assert!(rules.contains(&("no-concrete-in-core", "crates/sid-core/Cargo.toml")));
        assert!(rules.contains(&("no-concrete-in-core", "crates/sid-core/src/lib.rs:1")));
        assert!(rules.contains(&("gpui-only-in-frontend", "crates/sid-git/Cargo.toml")));
        assert!(rules.contains(&("gpui-only-in-frontend", "crates/sid-git/src/lib.rs:1")));
        assert!(rules.contains(&(
            "app-uses-sid-ui-not-gpui-component",
            "crates/sid/src/lib.rs:1"
        )));
        // sid-ui may name gpui; main.rs is the composition root.
        assert!(
            !v.iter()
                .any(|x| x.at.contains("sid-ui") || x.at.contains("main.rs"))
        );
        assert_eq!(v.len(), 5);
    }
}
