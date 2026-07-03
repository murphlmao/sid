# Serious pass — SSH/SFTP + Database fixups, Network containers, DB integration matrix

Fable-designed (2026-07-03). Four tracks; A/B/C build in parallel worktrees, D is a
Fable-orchestrated integration probe → Fable writes the durable tests + fixes bugs.

## A — SSH/SFTP serious pass (`ui/ssh_home.rs`, `session.rs`, `app.rs` SSH region)
Bugs from live screenshots:
1. **Layout overlap:** the `AppState` status/error line renders *over* the Home sidebar
   (the long "secrets: encrypted-file vault … OS keyring unavailable …" text sits on top
   of the quick-connect box), and the quick-connect **input + `⏎` Go button overlap**
   (button painted over the placeholder). Fix: status/error becomes a **full-width bar
   between the tab strip and the [sidebar | main] split** (clipped, ellipsized, never over
   the sidebar); quick-connect row = input `flex_1` + Go button fixed-width beside it, no
   overlap, fits `SIDEBAR_WIDTH`.
2. **No obvious "add connection":** add a clear **`＋ Add connection`** affordance on the
   Home connections-tree sidebar header AND a **right-click context menu** on the tree
   (empty area + on a row) → "Add connection" (opens `HostForm::new_add`), plus per-row
   "Edit / Rename / Delete / Assign folder". 
3. **Tab-strip `＋` does nothing visible:** wire it. On Home, `＋` opens the add-connection
   form (not a no-op `go_home`). Keep `🏠` = go home.
Also give the session file-browser toolbar a quick sanity pass (it looked cramped) — but the
main SSH freeze/split work is done; this is UI polish + the add-connection flow.

## B — Database serious pass (`ui/db_tab.rs`)
**Connections belong on the LEFT, DBeaver-style** (Murphy: "on the left like dbeaver" — the
earlier right-rail was a misread; revert it). Layout: **LEFT = a unified panel with the
connections list on top (folders, origin badges, `＋ Add`, inline rename, right-click menu)
and the active connection's schema tree below it** (DBeaver's connections→schema tree);
CENTER = SQL editor + results; `diagram`/`Export`/`Run` where they are. Remove the
right-edge rail entirely. Keep folder grouping + inline rename (already built — just moved).

## C — Network: Docker + Kubernetes views (read-only)
New adapter seam + impl, two new Network sub-tabs. **Read-only now** (management later).
- **`sid-core::containers`** (new flat module): `ContainerProvider { list_containers() -> Result<Vec<ContainerInfo>, ContainerError> }` and
  `KubeProvider { list_contexts() -> …, list_pods(ctx) -> … }`. Types: `ContainerInfo { id, name, image, state, status, ports }`,
  `KubeContext { name, current: bool }`, `KubePod { namespace, name, ready, phase, restarts, node }`.
  `ContainerError`/`KubeError` (thiserror) incl. a `NotInstalled` variant.
- **`sid-containers`** (new crate): docker via `docker ps -a --format '{{json .}}'` (parse JSON
  lines); kube via `kubectl config get-contexts -o name` + `kubectl get pods -A -o json`. Both
  **shell out** (adapter rule: `docker`/`kubectl` named ONLY here), on the shared runtime, and
  **degrade gracefully** when the binary/socket/cluster is absent → `NotInstalled`/empty + a
  clear UI notice ("docker not running" / "kubectl not installed — no cluster").
- **UI:** add `Docker` and `Kubernetes` sub-tabs to the Network segmented control (network_tab.rs):
  Docker = containers table (name·image·state·status·ports) + filter; Kubernetes = context list +
  pods table, or the graceful-absence notice. Same refresh/cache/pure-from-render pattern as Ports.
- **Verify:** docker IS available here (a container is running) — verify the Docker view live.
  kubectl is ABSENT — the Kubernetes view ships but is **unverified without a cluster**; confirm it
  shows the graceful notice, and unit-test the pure JSON→type parsers with fabricated fixtures.

## D — Complex DB integration matrix (Fable orchestrates the probe, writes tests, fixes bugs)
Probe sid-db's `DbClient` against real backends via Docker, find driver bugs, then Fable writes
the consolidated `crates/sid-db/tests/*` (`#[ignore]`/feature-gated so default `cargo test` stays
fast) and fixes whatever breaks:
- **Postgres 16** — rich fixture (types incl. numeric/uuid/timestamptz/json/arrays, single +
  composite FK, cross-schema): `open`, `query_paged` paging, `schema_introspect`, `schema_graph`
  (exact FK/PK), TLS (`sslmode`), error paths (bad SQL, cancel).
- **TimescaleDB** (`timescale/timescaledb:latest-pg16`) — Postgres-wire-compatible; add a
  **hypertable** + verify `schema_introspect`/`schema_graph` treat it correctly (hypertables show
  as tables; their FKs/PKs resolve) — this is the "complex" case that exercises pg introspection
  against extension-created objects.
- **redb browse** — in-process (no container): the `redb_browse` client lists sid's own store
  tables + `query_paged` over them; assert the fixed table set + row shape.
- (MySQL explicitly out of scope per Murphy.)
Deliverable: green integration suite + any `sid-db` fix (Fable) + a note of what each backend
surfaced.

## Constraints (all tracks)
Adapter pattern; no blocking in render; conventional commits, no `Co-Authored-By`; push per commit.
Gate: `cargo test --workspace` + `clippy -D warnings` + `fmt --check` (real exit codes). Verify UI
live via `scripts/sid-shot.sh`/`sid-click.sh` on an isolated Hyprland workspace.
