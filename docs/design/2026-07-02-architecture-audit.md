# Architecture audit — adapter/port pattern conformance

**Date:** 2026-07-02 · **Auditor:** Fable (full pass, dependency graph + leak greps + usage classification)
**Verdict: the adapter architecture is sound.** Zero layer violations. Every external library
lives in exactly one impl crate behind a `sid-core` trait. Findings below are modularity
*improvements*, not rule breaches.

## Method
1. Dependency-graph extraction from every crate's `Cargo.toml` (hard evidence — a layer
   violation must appear here).
2. Leak greps for concrete names (`gpui`, `russh`, `vt100`, `tokio_postgres`, `rusqlite`,
   `sysinfo`, `netstat2`, `nix`, `redb`, `postcard`, `keyring`) outside their home crates,
   with every hit classified (code vs doc-comment vs test-only vs false positive).
3. Constructor-confinement check: where the frontend names concrete adapter types.
4. OS-touch inventory in the frontend (`std::fs`, `env::var`).

## Verified clean
| Rule | Evidence |
|:--|:--|
| GPUI only in `crates/sid` | zero `gpui` references in any other crate |
| One lib ⇄ one impl crate | russh/russh-sftp→`sid-ssh` · vt100→`sid-term` · tokio-postgres/rusqlite/rustls→`sid-db` · sysinfo/netstat2/nix→`sid-sysinfo` · redb/postcard/toml→`sid-store` · keyring-core/zbus-store→`sid-secrets` |
| `sid-core` is a pure seam | deps: async-trait, serde, thiserror only (serde_json is dev-only, tests) |
| No adapter-to-adapter coupling | `sid-ssh→sid-term` and `sid-store→sid-secrets` are **dev-dependencies** (live-smoke test / secret-boundary test). The one real cross dep, `sid-db→sid-store`, is intentional: `redb_browse` browses sid's own store *through the `GlobalStore` API*, still surfaced via the `DbClient` trait. |
| Constructor-only rule in frontend | concrete types appear at wiring points only: `db_registry.rs` (all DB clients), `ssh_connect.rs`/`session.rs` (`RusshClientFactory`, `Vt100Screen`), `network_tab.rs` (`SysinfoProvider::new()`, documented in its header). All other grep hits are doc-comments or `#[cfg(test)]` fakes (`FakeKeyring`, `MemorySecretStore`). |
| Grep false positives | `std::os::unix::` matches `nix::` — `known_hosts.rs` is clean. |

## Improvement queue (prioritized; none blocking)
1. **`LocalFiles` port (MEDIUM).** `downloads_dir()` + `fs::write` are duplicated in
   `ui/session.rs` (SFTP download) and `ui/db_tab.rs` (CSV export) — the one OS-integration
   class currently done inline in the frontend. Introduce `sid_core::files` (downloads dir,
   guarded write; later: open-in-editor, reveal-in-file-manager) with a Linux impl crate or a
   module in the composition root. Kills the duplication and pre-seams editor-launch, which
   the binding rules already name as a future trait point. *Do after the in-flight branches
   land (both files contested).* 
2. **Shared runtime misnamed/misplaced (LOW).** `ssh_runtime()` lives in `ui/session.rs` but
   serves db_tab and network_tab too. Move to `crates/sid/src/runtime.rs` as `app_runtime()`;
   callers are one-line changes. *Same contention constraint.*
3. **SSH wiring consolidation (LOW).** DB has `db_registry.rs` as its single wiring point;
   SSH construction is spread across `ssh_connect.rs` + `session.rs`. Consolidate into a
   registry-style module **when the multi-tab shell rebuild rewrites `session.rs` anyway**
   (fold into that plan — zero extra cost).
4. **Clipboard via GPUI (ACCEPTED, no action).** The binding rules list clipboard as an
   OS-trait point; GPUI already abstracts it and only the rendering surface consumes it.
   A wrapper trait adds ceremony with no swappability gain until a non-GPUI consumer exists.
   GPUI *is* the clipboard adapter. Revisit only if core/domain ever needs clipboard access.
5. **Composition-root OS touches (ACCEPTED).** `app.rs`'s store-path resolution, demo seed
   file creation, and env reads are composition-root duties — the one module allowed to
   touch everything to wire the app.

## Pattern to preserve
New OS/external surfaces keep following the established shape (seen in `sys.rs` and the
in-flight `svc.rs`): flat trait module in `sid-core`, `thiserror` domain errors, impl crate
naming the concrete libs, frontend consumes the trait + names one constructor at one wiring
point, pure-logic tests in the impl crate.
