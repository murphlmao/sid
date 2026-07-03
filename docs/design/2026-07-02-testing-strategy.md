# sid — integration + automation test harness

**Date:** 2026-07-02 · **Scope:** Docker-backed integration tests for the DB and SSH
adapters, a headless launch-survives smoke for the GPUI binary, and a lean CI workflow.
Built in an isolated worktree per Murphy's brief; touches only `docker/**`,
`.github/workflows/**`, new `scripts/*.sh` runners, `crates/sid-db/tests/**`,
`crates/sid-ssh/tests/**`, and this doc.

This is additive to CLAUDE.md's "pragmatic testing" mode, not a reversal of it: unit tests
stay targeted-per-feature, and this harness only covers the paths a unit test structurally
cannot — a live Postgres `pg_catalog` walk, a live sshd round-trip, and "does the binary
actually start."

## What's automated, and where it lives

| Layer | What | Where | Gate? |
|---|---|---|---|
| Unit | Pure logic (`tls_choice`, FK/PK assembly, lexer, etc.) | `crates/*/src/**` `#[cfg(test)]` | Yes — `cargo test --workspace`, every push |
| Docker integration — DB | `PostgresClient` against a real `postgres:16` container, FK-rich fixture schema | `crates/sid-db/tests/postgres_integration.rs` | Yes — CI `integration` job |
| Docker integration — SSH | `RusshClientFactory` against a real disposable sshd container, key auth + SFTP round-trip | `crates/sid-ssh/tests/live_sshd_smoke.rs` (`docker_sshd_key_auth_exec_and_sftp_round_trip`) | Yes — CI `integration` job |
| Headless smoke | `sid` binary starts, opens a window, survives, exits under Xvfb+Lavapipe | `docker/headless-smoke/` | Informational (`continue-on-error`) — see below |
| Manual gate | Real ssh-agent + trusted localhost sshd, styled-cell terminal assertions | `crates/sid-ssh/tests/live_sshd_smoke.rs` (`live_sshd_agent_exec_shell_sftp`) | No — run by hand before an SSH-tab release gate; needs *your* agent/host |
| Visual | Screenshots for human review | `scripts/sid-shot.sh` / `scripts/sid-click.sh` (pre-existing, unmodified) | No — observation-gated by design (see "Why no golden-image tests" below) |

## 1. DB adapter — Docker Postgres integration

**Why:** the demo SQLite fixture can't exercise `PostgresClient::schema_graph`'s live
`pg_catalog` walk (composite FKs via `unnest(...) WITH ORDINALITY`, cross-schema joins) —
that code path was previously only unit-tested against *fabricated rows*
(`assemble_foreign_keys`/`assemble_primary_keys` in `postgres.rs`), never against a real
server's actual FK/PK catalog output. This closes that gap.

**Fixture** (`docker/pg-init/01-schema.sql`, loaded automatically by postgres:16's
`/docker-entrypoint-initdb.d` convention): `public.customers`, `public.warehouses`,
`public.bins` (composite PK `(warehouse_id, bin_id)`), `public.orders` (a single-column FK
to `customers` *and* a composite FK to `bins`), and `billing.invoices` — a schema-qualified
table in a non-`public` namespace referencing `public.orders`, so the `ns.nspname <>
refns.nspname` cross-schema join path in `schema_graph`'s SQL is actually exercised, not
just the same-schema case.

**Tests** (`crates/sid-db/tests/postgres_integration.rs`, all `#[ignore]`d):
- `sslmode_disable_local_connect_and_query_paged_returns_rows` — plaintext local connect +
  `query_paged` row/column shape.
- `query_paged_pagination_walks_all_rows_via_cursor` — cursor pagination across multiple
  pages on a real connection (page_size=1 over 3 rows).
- `execute_ddl_dml_round_trip` — `execute()` against DDL + DML, asserts `rows_affected`.
- `schema_introspect_lists_fixture_tables_and_columns` — table/column listing across both
  the `public` and `billing` schemas.
- `schema_graph_matches_fixture_exactly` — the headline test: asserts the *exact* FK list
  (all 4 edges, including the composite and cross-schema ones) and the *exact* PK map
  (including the composite PK's column order) against the live server.

**TLS scope:** `tls_choice`'s decision logic (remote-always-TLS, local-honors-`sslmode`) is
already fully covered by the pure unit tests in `postgres.rs`. This harness additionally
proves the *plaintext local* leg round-trips against a real server
(`sslmode=disable` on `localhost`). A full `sslmode=require`/`verify-full` round-trip against
the container is **not** covered — rustls only implements verify-full (see
`build_rustls_connector`'s doc comment in `postgres.rs`), which needs a certificate chain
actually trusted for the `localhost` hostname. Standing up a throwaway CA + leaf cert for a
disposable test container is real PKI machinery for a property the unit tests already pin
at the decision-function level; deferred as not worth it. If a real self-signed/pinned-cert
mode is ever added to `PostgresClient` (`build_rustls_connector`'s own ponytail note flags
this as deferred product work), that's when this container fixture should grow a cert leg.

**Run locally:**
```
scripts/test-integration.sh            # up postgres, run, down
scripts/test-integration.sh --keep     # leave the container running after
```
Actually run for this harness: **5/5 passed** against a live `postgres:16` container.

## 2. SSH adapter — Docker sshd integration

**Why:** `live_sshd_smoke.rs` existed before this work but was permanently `#[ignore]`d and
un-runnable in CI — it requires *your* ssh-agent and a trusted sshd on `localhost:22`. It has
never actually run in an automated setting. This adds a second, CI-runnable test in the same
file that needs neither.

**Fixture** (`docker/ssh/Dockerfile` + `docker/ssh/test_id_ed25519{,.pub}`): a minimal
Debian + `openssh-server` image with one throwaway keypair generated solely for this harness
baked in as `sid_test`'s `authorized_keys`. `PasswordAuthentication no`, pubkey-only.
Committing a private key is normally a red flag — this one is a disposable test fixture with
no bearing outside an ephemeral container (regenerable with one `ssh-keygen` call; nothing
it protects exists outside `docker compose down -v`).

**Test:** `docker_sshd_key_auth_exec_and_sftp_round_trip` — connects with `SshAuth::Key`
(no agent), runs `exec("echo ok")`, then an SFTP round trip: lists the baked-in fixture dir,
`put`s new bytes to a pre-existing remote file, `get`s them back, and asserts byte equality.
This is a stronger check than a bare `list()` — it proves upload and download actually move
correct bytes, not just that a directory listing is non-empty.

**A real finding, out of scope to fix here:** writing this test surfaced a genuine gap in
`sid-ssh`'s `put()`. `RusshSftp::put` (`crates/sid-ssh/src/sftp.rs`) wraps `russh-sftp`
2.3's `SftpSession::write`, which opens with `OpenFlags::WRITE` only (no `CREATE`) — so it
can **overwrite an existing remote file but cannot create a new one**. The round-trip test
therefore targets a file the fixture image already contains
(`sftp-fixture/writable.txt`) rather than a fresh path. This is product code
(`crates/sid-ssh/src/sftp.rs`), out of this harness's file ownership to fix — flagged here
for whoever owns that crate next. A one-line fix is `session.open_with_flags(path,
OpenFlags::CREATE | OpenFlags::WRITE)` in place of the bare `write()` convenience call.

**Run locally:**
```
scripts/test-ssh.sh            # build+up sshd, run, down
scripts/test-ssh.sh --keep     # leave the container running after
```
Actually run for this harness: **1/1 passed** against a live sshd container.

The pre-existing `live_sshd_agent_exec_shell_sftp` test is untouched and still `#[ignore]`d
— it remains the manual, agent-based gate for the full shell/vt100 path this harness doesn't
attempt to automate (a real interactive PTY + your own agent identity is a different, more
manual, category of test than "does key auth + SFTP work against a known server").

## 3. Headless GPUI launch smoke — executive decision

**The question:** can `target/debug/sid` be launched, produce a window, and be torn down
cleanly inside a container with no real GPU and no real compositor?

**What I found out (by reading gpui 0.2.2's own source, not assuming):**
- GPUI's Linux backend (`gpui::platform::linux::current_platform` /
  `guess_compositor`) picks a backend **by environment variable inspection**: `WAYLAND_DISPLAY`
  set → Wayland client; else `DISPLAY` set → X11 client; else (or `ZED_HEADLESS` set) → its
  own `HeadlessClient`.
- That native `HeadlessClient` is a dead end for this purpose: its `open_window` **always
  returns an error** ("neither DISPLAY nor WAYLAND_DISPLAY is set"). It exists for
  executor-only use (background work with no UI), not for testing that a window opens. `sid`'s
  `main.rs` calls `cx.open_window(..).unwrap()`, which would panic under it immediately.
- So a real window needs either a nested Wayland compositor (`sway --headless` /
  `weston --backend=headless-backend.so`, as the brief suggested) or an X11 server
  (`Xvfb`) — GPUI's `x11` feature is already enabled by default on Linux (see
  `crates/sid/Cargo.toml`'s comment: "Linux default features pull in font-kit + wayland + x11
  automatically"), so Xvfb needs zero product-code changes to reach.
- **Decision: Xvfb, not a nested Wayland compositor.** Both are "a fake display GPUI will
  accept"; Xvfb is the much more standard, better-documented, lower-variance choice for CI
  (used by e.g. Zed's own Linux CI, and by most GUI-toolkit CI recipes generally) — a nested
  wlroots-headless-backend compositor brings its own protocol/driver quirks on top of an
  already-uncommon rendering path. Since GPUI treats X11 as a first-class backend (not an
  emulation shim), this isn't a compromise.
- **Rendering still needs a Vulkan implementation.** GPUI's Linux renderer (`blade-graphics`,
  vendored from Zed) targets Vulkan only — there is no GL/GLES fallback path to fall back to.
  `mesa-vulkan-drivers` ships **Lavapipe**, Mesa's CPU/software Vulkan ICD, which is what
  makes this work with zero GPU passthrough.

**What I built:** `docker/headless-smoke/` — a two-stage Dockerfile (full `rust:bookworm`
builder with the gpui/gpui-component Linux dev-header set; slim `debian:bookworm-slim`
runtime with `xvfb` + `mesa-vulkan-drivers` + `xdotool`) and `run-smoke.sh`, which:
1. starts `Xvfb :99`,
2. launches `sid` against it with hermetic `XDG_*` dirs (same pattern as `scripts/sid-shot.sh`'s
   default non-`--real` mode — a fresh demo-seeded store, never a real one),
3. polls (`xdotool search --onlyvisible --pid`) for a mapped window, up to
   `SMOKE_WINDOW_TIMEOUT_SECS` (default 15s),
4. sleeps `SMOKE_SURVIVE_SECS` (default 5s) and reconfirms the process is still alive,
5. sends `SIGTERM`, waits up to `SMOKE_TERM_GRACE_SECS` (default 5s), escalates to `SIGKILL`
   only as a failure path,
6. treats exit code `0` or `143` (the expected code for a SIGTERM'd process with no custom
   handler — gpui installs none) as PASS; anything else (segfault=139, abort=134, a Rust
   panic's default 101, a required SIGKILL) as FAIL, dumping the app's stdout/stderr either way.

This is a **launch-survives** smoke — starts, opens a window, survives N seconds, exits
without crashing — not a correctness check of what's rendered.

**Result: it works, reliably.** Built the image, then ran it **4/4 clean passes** locally —
window found immediately (0s poll), survived the 5s window, exited with code 143 (clean
SIGTERM) within 1s every time. Xvfb + Lavapipe genuinely renders `sid`'s GPUI window with
zero GPU passthrough. This was the open question going in; it's resolved positively.

**CI placement:** wired as `.github/workflows/ci.yml`'s `smoke` job, currently with
`continue-on-error: true` — **not** because the technique is unreliable (4/4 says otherwise),
but because this is its first run on GitHub's *hosted* runners, an environment it hasn't been
validated against yet (different kernel/cgroup/Mesa-package-version than this development
machine). Once it's shown green on hosted runners for a run or two, promote it to a hard gate
by dropping `continue-on-error` — there's no technical reason left not to. Until then, treat a
red `smoke` job as "go look, likely an environment difference," not "assume the technique
failed."

### Why no golden-image / pixel-diff tests

Explicitly out of scope, by design, not by time pressure. sid's UI changes every session —
CLAUDE.md's vertical-slice, iterate-fast philosophy directly conflicts with the maintenance
tax of golden images that need re-baselining on every visual tweak. A screenshot-diff suite
would spend more of every session's time updating baselines than it would ever spend
catching regressions. `scripts/sid-shot.sh` / `scripts/sid-click.sh` (pre-existing, untouched
by this work) already give a human a real screenshot to look at on demand — that stays the
right tool for "does this look right," gated by observation, per CLAUDE.md's own rule that "a
rendering spike is gated by observation, not unit tests; that is correct, not a shortcut."
This harness's headless smoke answers a narrower, machine-checkable question ("does it start
at all") that sits below where a human needs to look.

## 4. CI workflow (`.github/workflows/ci.yml`)

Three jobs:
- **`fast`** (every push/PR, no Docker): `cargo fmt --all --check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo test --workspace` (the default, non-`#[ignore]`d
  suite — stays fast and green with no external services, per the brief's requirement).
- **`integration`** (needs `fast`): runs `scripts/test-integration.sh` and
  `scripts/test-ssh.sh` directly — the same two scripts a developer runs locally, so there is
  exactly one recipe for "run the Docker integration suite," not a CI-only variant that can
  drift from the local one.
- **`smoke`** (needs `fast`, `continue-on-error: true`): builds and runs
  `docker/headless-smoke/`.

Kept lean deliberately: no matrix builds, no artifact uploads, no release packaging — those
are exactly the "heavy CI" CLAUDE.md defers. `Swatinem/rust-cache` is the one caching
convenience included, because a cold `cargo build` of the full workspace (gpui +
gpui-component pull in a large dependency tree) is otherwise the dominant cost of every run.

## How to run everything locally

```
cargo test --workspace                                    # fast suite (no Docker)
cargo fmt --all --check                                   # formatting gate
cargo clippy --workspace --all-targets -- -D warnings      # lint gate

scripts/test-integration.sh                                # DB adapter, docker postgres
scripts/test-ssh.sh                                         # SSH adapter, docker sshd

docker build -f docker/headless-smoke/Dockerfile -t sid-headless-smoke .
docker run --rm sid-headless-smoke                          # headless launch-survives smoke

scripts/sid-shot.sh --tab database                          # human-observed screenshot (unchanged)
```

All of `docker/pg-init/`, `docker/ssh/`, and `docker/headless-smoke/` are driven from
`docker/docker-compose.test.yml` (postgres + sshd services) or a direct `docker build`
(headless-smoke, which needs the whole workspace as build context, not just one
service's fixture directory).
