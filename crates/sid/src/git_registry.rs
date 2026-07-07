//! `git_registry` — the binary's wiring seam for the git provider factory, mirroring
//! `db_registry`'s shape for a single adapter instead of `DbKind`-keyed ones (there is
//! only one git backend, so no per-kind lookup table is needed here).
//!
//! This is the one file in `crates/sid` allowed to name `sid_git`'s concrete
//! `Git2Provider` — everything downstream (the Workspaces tab) works through the
//! `sid_core::git::GitProvider` trait object this hands back. Swapping the git backend
//! later (or adding a second one, keyed the way `DbRegistry` keys on `DbKind`) is: a new
//! `GitProvider` impl in its own crate + one line here.
//!
//! `sid-git`'s `Git2Provider` is a real git2-backed port landing on a parallel branch;
//! on THIS branch every method still returns `GitError::Other("sid-git port in
//! progress")` — the Workspaces tab is built and observation-gated entirely against
//! that honest error state (see `docs/superpowers/plans/2026-07-06-workspaces-v1.md`'s
//! BUILD ADDENDUM).

use sid_core::git::GitProvider;

/// The git provider factory — a stateless handle; call `.open(path)` on it to bind a
/// per-repo handle. Fresh on every call (the concrete `Git2Provider` is a zero-sized
/// unit struct), so callers never need to cache this themselves.
pub(crate) fn factory() -> Box<dyn GitProvider> {
    sid_git::Git2Provider::factory()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_reaches_the_real_concrete_type_through_the_trait_seam() {
        // Smoke test for the wiring itself: `crates/sid` resolves to the real
        // `Git2Provider`, which maps a non-repo path to the typed `NotARepo` (the
        // variant the workspace list's "not a git repo" chip keys on).
        // `Box<dyn GitProvider>` isn't `Debug`, so `unwrap_err` doesn't apply — match.
        match factory().open(std::path::Path::new("/nonexistent")) {
            Ok(_) => panic!("expected a non-repo path to error"),
            Err(e) => assert!(
                matches!(e, sid_core::git::GitError::NotARepo(_)),
                "want NotARepo, got: {e}"
            ),
        }
    }
}
