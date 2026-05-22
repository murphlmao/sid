//! `crate_info` tool — Cargo metadata + LOC + test count + pub items for one crate.

use std::path::Path;

use serde::Serialize;
use walkdir::WalkDir;

use crate::error::SidMcpError;

/// Result returned by the `crate_info` tool.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CrateInfo {
    /// Workspace-relative crate name (e.g., `"sid-core"`).
    pub name: String,
    /// Path relative to workspace root (e.g., `"crates/sid-core"`).
    pub path: String,
    /// Total lines of Rust source code (`.rs` files under `src/`).
    pub loc: usize,
    /// Number of test annotations (`#[test]` / `#[tokio::test]`) found under
    /// `src/` and `tests/`.
    pub test_count: usize,
    /// Number of public items found (`pub fn`, `pub struct`, `pub trait`,
    /// `pub enum`) under `src/`.
    pub pub_item_count: usize,
    /// Whether the crate has a `tests/` directory (integration tests).
    pub has_integration_tests: bool,
    /// Whether the crate has a `benches/` directory (criterion benchmarks).
    pub has_benches: bool,
    /// Whether the crate is one of the critical-path crates (95% coverage bar).
    pub is_critical_path: bool,
}

/// Crates that CLAUDE.md flags as 95% critical-path.
const CRITICAL_PATH: &[&str] = &["sid-store", "sid-core", "sid-job", "sid-secrets"];

/// Implementation entry point.
///
/// # Errors
///
/// Returns [`SidMcpError::UnknownCrate`] if `name` is not a member of the
/// workspace (i.e., `crates/<name>/Cargo.toml` does not exist).
pub async fn run(workspace_root: &Path, name: &str) -> Result<CrateInfo, SidMcpError> {
    let crate_dir = workspace_root.join("crates").join(name);
    if !crate_dir.join("Cargo.toml").exists() {
        return Err(SidMcpError::UnknownCrate(name.to_string()));
    }

    let src_dir = crate_dir.join("src");
    let tests_dir = crate_dir.join("tests");
    let benches_dir = crate_dir.join("benches");

    let mut loc = 0usize;
    let mut test_count = 0usize;
    let mut pub_item_count = 0usize;

    for dir in [&src_dir, &tests_dir] {
        if !dir.exists() {
            continue;
        }
        for entry in WalkDir::new(dir).into_iter().flatten() {
            if entry.path().extension().map(|e| e == "rs").unwrap_or(false) {
                if let Ok(content) = tokio::fs::read_to_string(entry.path()).await {
                    loc += content.lines().count();
                    for line in content.lines() {
                        let t = line.trim();
                        if t.starts_with("#[test]")
                            || t.starts_with("#[tokio::test")
                            || t.starts_with("#[tokio::test]")
                        {
                            test_count += 1;
                        }
                        // Pub items only counted under src/, not tests/.
                        if dir == &src_dir && is_pub_item_decl(t) {
                            pub_item_count += 1;
                        }
                    }
                }
            }
        }
    }

    Ok(CrateInfo {
        name: name.to_string(),
        path: format!("crates/{name}"),
        loc,
        test_count,
        pub_item_count,
        has_integration_tests: tests_dir.exists(),
        has_benches: benches_dir.exists(),
        is_critical_path: CRITICAL_PATH.contains(&name),
    })
}

/// Return true if the trimmed line declares a public item we count for
/// the pub-item-count metric.
fn is_pub_item_decl(trimmed: &str) -> bool {
    // Skip `pub use`, `pub mod`, `pub const`, `pub static` — they're not
    // the same shape of "API surface" CLAUDE.md cares about.
    for prefix in [
        "pub fn ",
        "pub struct ",
        "pub trait ",
        "pub enum ",
        "pub async fn ",
    ] {
        if trimmed.starts_with(prefix) {
            return true;
        }
    }
    // `pub(crate)` etc don't count — they're not the public surface.
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_pub_item_decl_recognises_each_kind() {
        assert!(is_pub_item_decl("pub fn foo() -> i32 {"));
        assert!(is_pub_item_decl("pub struct Bar {"));
        assert!(is_pub_item_decl("pub trait Baz {"));
        assert!(is_pub_item_decl("pub enum Qux {"));
        assert!(is_pub_item_decl("pub async fn quux() {"));
    }

    #[test]
    fn is_pub_item_decl_rejects_non_api_pubs() {
        // boundary: pub use shouldn't bloat the count.
        assert!(!is_pub_item_decl("pub use foo::Bar;"));
        assert!(!is_pub_item_decl("pub mod foo;"));
        assert!(!is_pub_item_decl("pub const FOO: u32 = 1;"));
        assert!(!is_pub_item_decl("pub(crate) fn private() {"));
        assert!(!is_pub_item_decl("fn private() {"));
    }

    #[tokio::test]
    async fn unknown_crate_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let err = run(tmp.path(), "no-such-crate").await.unwrap_err();
        assert!(matches!(err, SidMcpError::UnknownCrate(_)));
    }

    #[tokio::test]
    async fn counts_loc_and_tests_in_synthetic_crate() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let cdir = root.join("crates/example");
        std::fs::create_dir_all(cdir.join("src")).unwrap();
        std::fs::write(
            cdir.join("Cargo.toml"),
            "[package]\nname=\"example\"\nversion=\"0.0.1\"\nedition=\"2024\"\n",
        )
        .unwrap();
        std::fs::write(
            cdir.join("src/lib.rs"),
            "pub fn one() {}\npub struct Two {}\n\n#[test]\nfn t1() {}\n#[tokio::test]\nasync fn t2() {}\n",
        )
        .unwrap();

        let info = run(root, "example").await.unwrap();
        assert_eq!(info.name, "example");
        assert_eq!(info.test_count, 2);
        assert_eq!(info.pub_item_count, 2);
        assert!(info.loc >= 6);
        assert!(!info.has_integration_tests);
        assert!(!info.has_benches);
        // Boundary: example isn't critical-path.
        assert!(!info.is_critical_path);
    }

    #[tokio::test]
    async fn flags_critical_path_crate() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let cdir = root.join("crates/sid-store");
        std::fs::create_dir_all(cdir.join("src")).unwrap();
        std::fs::write(
            cdir.join("Cargo.toml"),
            "[package]\nname=\"sid-store\"\nversion=\"0.0.1\"\nedition=\"2024\"\n",
        )
        .unwrap();
        std::fs::write(cdir.join("src/lib.rs"), "").unwrap();

        let info = run(root, "sid-store").await.unwrap();
        assert!(info.is_critical_path);
    }
}
