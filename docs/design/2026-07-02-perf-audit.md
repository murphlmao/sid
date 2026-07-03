# Performance audit — findings (2026-07-02)

**Method:** 6 read-only subsystem finders → 25 adversarial verifiers (each re-read the cited
code + checked the fix preserves behavior AND the adapter architecture). Fable ranks/implements.

**Headline:** the architecture is healthy — **no I/O-in-render violations, no big-O disasters,
caching is mostly correct.** The audit's own adversarial pass *rejected* the tempting wrong
fixes (memoizing the composer → `behavior_risk: high` vs the attributive invariant; gating the
network filter by sub-tab → no real gain, medium risk). Net: **one real latency bug + six small
allocation cleanups.** Honest about scale — this is polish, not a rescue.

## Confirmed — implement (Fable, on the post-build-merge tree to avoid collisions)

| # | Where | Fix | Gain | Risk |
|:--|:--|:--|:--|:--|
| 1 | `sid/src/ui/session.rs` shell/sftp handles + `sid-core::ssh` trait + `sid-ssh` | **SshShell reader/writer split** — see below | medium | medium |
| 2 | `sid/src/ui/db_diagram.rs` render | Compute `table_bounds()` once/frame, thread into `edge_labels`(&) then move into `edges_canvas` | small | low |
| 3 | `sid-db/src/postgres.rs:573` | Drop `.to_string()` on the already-owned `String` arm of `render_pg_value` | small | low |
| 4 | `sid-db/src/sqlite.rs:426-432` | `render_sqlite_value` take `Value` by value, move out of `Text` arm (no `.clone()`) | small | low |
| 5 | `sid/src/ui/network_tab.rs` interfaces | Cache visible/hidden partitions on refresh/filter-change instead of recomputing in `interfaces_strip` render (mirror Ports/Services delegates) | small | low |
| 6 | `sid/src/ui/network_tab.rs:290,497` | `render_td` borrow `&self.ports[ix]`/`&self.services[ix]` instead of cloning whole row per cell | small | low |
| 7 | `sid/src/app.rs` seed/startup | `seed_if_empty` returns the (post-seed) lists so `reload_scopes`/`refresh`/`DbTabState::new` don't re-scan all 3 tables | small | low |

### Finding #1 (the one that matters) — SshShell mutex-across-await
`session.rs` holds `SharedShell = Arc<AsyncMutex<Box<dyn SshShell>>>` and does
`shell.lock().await.write(&bytes).await` — the guard lives for the **whole** inner `.await`.
`write` awaits SSH flow-control window availability (verified in russh 0.61.2:
`ChannelWriteHalf::data` → `send_data` waits on the window notifier), so during a large paste /
congested link / stalled remote, the write holds the lock and the **read loop can't drain
output** → a real, reproducible terminal freeze. This re-introduces, one layer up, exactly the
hazard the file's own doc says was fixed inside `RusshShell` via `Channel::split()`.

**Fix (real one, not a guard-drop micro-tweak — the async `&mut self` trait shape makes that
structurally impossible):** split `SshShell` into `SshShellReader` (`try_read`) + `SshShellWriter`
(`write`/`resize`/`close`); `open_shell` returns both. Reader needs **no** mutex — move it by
value into the single read-loop task. Only the writer stays behind `Arc<AsyncMutex<…>>`. Thread a
one-shot shutdown flag (not a per-call lock) from `Writer::close` to the reader. Blast radius is
exactly 4 files: `sid-core/src/ssh.rs`, `sid-ssh/src/client.rs`, `sid-ssh/src/shell.rs`,
`sid/src/ui/session.rs`. **This is an adapter-boundary change — Fable does it, post-merge, because
it collides with the in-flight test-extension (sid-core) and integration (sid-ssh/tests) branches.**

## Rejected by verification (do NOT do — recorded so we don't re-litigate)
- **Composer `compose()` clone-avoidance / memoization** — `behavior_risk: high`; the double-clone
  is real but the inputs are single-use owned Vecs, and any dirty-tracking/memo risks the
  attributive-composition invariant. Not worth it at personal-store scale.
- **Gate network filter by visible sub-tab** — no real gain (short lists), medium risk.
- **Workspace `config.toml` mtime cache / global dirty-tracking** — small gain, medium risk; the
  re-read is correct-by-default and cheap at this scale.
- **redb-browse full materialization** — already paged; rejected.

## Flagged → Fable's verdict: real, but DEFERRED (medium effort + functional risk)
- `session.rs render_grid()` → `screen.cells()` (`sid-term/src/screen.rs:108`) re-materializes the
  **entire** vt100 grid — a fresh `Vec<Vec<TermCell>>` with a `String` per cell — on every render
  (~1920 allocs for an 80×24 during active output). Real hot-path churn.
- **Why not done now:** the clean fix memoizes inside `Vt100Screen` (a `dirty` flag set by
  `feed()`/`resize()`, cache the grid, rebuild only when dirty; ideally have the `TerminalScreen`
  trait's `cells()` return a borrow so `render_grid` stops cloning too). A *wrong* invalidation
  shows **stale terminal output** — a visible functional regression, exactly the line Murphy drew
  ("not at the sacrifice of breaking functionality"). It can only be verified by observing a live
  terminal actually update (an observation gate), so it's not safe tail-of-session work.
- **Correct approach when picked up:** invalidate on `feed()` AND `resize()` only (vt100 screen
  state changes nowhere else); cursor is separate (`cursor_position`) so it doesn't affect the
  cell cache. Verify by driving `cat`/scrolling output over the docker sshd and confirming the
  grid tracks live. Medium gain, low-risk *if* verified — worth a dedicated increment.

## Caveats noted from the audit itself
- Finding #7 has a **regression trap**: a naive "reuse the pre-seed emptiness-check lists" breaks
  first-launch seeding (the demo rows are written *after* the check). The fix must return the
  **post-seed** lists. Guard with a test (there are none for `seed_if_empty` today).
- Two verifiers (#3 postgres, and the rejected redb one) returned placeholder-ish rationale — the
  underlying findings are trivially safe either way, but their auto-verdicts weren't real analysis;
  Fable eyeballs #3 before applying.
