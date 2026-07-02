# Network tab — Increment 1 (glanceable ports + kill + interfaces)

> **For agentic workers:** this is a **salvage/port**, not a greenfield design. The POC already
> built the whole adapter behind a clean, object-safe trait. Crib it almost verbatim; adapt only
> the module layout to the new sid (flat `sid-core` modules, not `adapters/`).

**Goal:** A working Network tab — a glanceable, sortable table of **listening ports**
(proto · port · pid · process) with **kill-by-pid** (two-click confirm, destructive), plus an
**interfaces** summary (name · addrs · up · rx/tx, default-route first) and a **refresh**. Purely
**live/ephemeral** — nothing persists to the store; no scope, no secrets, no redb.

Mockup intent (`docs/mockups/sid-mockup.html`, `#view-net`): *"glanceable · sortable ·
kill-by-pid / kill-by-port"*.

## Salvage map (POC → new sid)

Source: `~/vcs/sid-poc`. All of these port near-verbatim.

| POC | New sid | Notes |
|:--|:--|:--|
| `sid-core/src/adapters/sys.rs` | `crates/sid-core/src/sys.rs` (+ `pub mod sys;` in lib.rs) | Flat module to match `db.rs`/`ssh.rs`/`term.rs`. Types: `Pid`, `Signal`, `Protocol`, `SocketState`, `ListeningPort`, `NetInterface`, `ProcessInfo`, `SysError`, trait `SysProvider`. **Drop the `serde` derives** — nothing serializes these (live/ephemeral). Keep `thiserror` on `SysError`. |
| `sid-sysinfo/src/{lib,ports,interfaces,processes,kill,default_route}.rs` | new crate `crates/sid-sysinfo` | Deps: `sysinfo`, `netstat2`, `nix`, `sid-core`. Add to workspace `members` + a workspace dep entry. **Keep `kill.rs`'s guards verbatim** (pid-0 rejection + `i32::try_from` overflow rejection — they stop `u32::MAX → -1` process-group broadcasts; security-load-bearing). |
| `sid-core/src/sys_probe.rs` (polling service) | **do not port** | Over-built for inc-1. The UI holds `Arc<Mutex<dyn SysProvider>>` and calls `list_*` on the shared runtime on refresh. `// ponytail:` skip the broadcast service until a second consumer needs it. |

## Architecture / constraints (binding)
- **Adapter pattern:** `sysinfo`/`netstat2`/`nix` are named **only** in `sid-sysinfo`. `gpui`/
  `gpui-component` **only** in `crates/sid`. `crates/sid` names the `sid_core::sys` trait seam +
  the one concrete constructor (`SysinfoProvider::new()`), never the libs.
- **No blocking in `render`:** `list_listening_ports`/`list_interfaces`/`kill_process` run on the
  shared runtime (`crate::ui::session::ssh_runtime()` — already the app's tokio handle), results
  cached on the entity; `render` is pure-from-cache. Refresh = spawn → update cache → `cx.notify()`.
- **Kill is destructive → confirm.** Reuse the **two-click confirm** pattern already used by the SSH
  host-row delete (arm on first click, execute on second; auto-disarm on any other interaction). No
  modal needed. Default signal SIGTERM; SIGKILL is a secondary action. (ponytail: two-click over a
  modal — it already exists in this app.)

## Tasks

### N1 — Port `sid-core::sys` + `sid-sysinfo` crate *(pure backend, no UI, no app.rs)*
- Port the trait + types (drop serde) and the impl crate per the salvage map. Wire workspace
  membership + `sid-sysinfo.workspace = true` dep entry; add `sid-sysinfo` to `crates/sid`'s deps.
- **TDD (load-bearing only):** `parse_proc_net_route` (default-route row vs none — the POC doctest),
  `kill_process` rejects pid 0, `kill_process` rejects a pid that overflows `i32` (e.g. `u32::MAX`),
  and the netstat→`ListeningPort` protocol/state mapping. **No** rendering tests, **no** exhaustive
  permutations, **no** live-socket integration test (observation-gated).
- **Deliverable:** `cargo test -p sid-sysinfo` green; `SysinfoProvider` lists ports/interfaces and
  kills a pid behind the `sid_core::sys::SysProvider` trait.

### N2 — Network tab UI *(observation-gated rendering)*
- **Files:** `crates/sid/src/ui/network_tab.rs` (new) + `mod.rs` export; `app.rs` (`AppState.network`
  field + `Tab::Network => self.network_tab(window, cx)` arm + init). **Keep `app.rs` edits localized.**
- Ports table via `gpui-component` `Table`/`TableDelegate` — **crib the usage from
  `crates/sid/src/ui/db_tab.rs`** (results grid). Columns: proto · port · pid · process · [kill].
  A `⟳ refresh` button. Kill button per row → two-click confirm → `kill_process(pid, Term)`; surface
  `SysError` in the status line (esp. `PermissionDenied`).
- Interfaces strip: name · addrs · up/down · rx/tx (humanized), default-route iface first.
- **TDD:** none new (I/O + rendering, observation-gated). Any pure humanize/sort helper gets one test.
- **Deliverable:** the Network tab lists live listening ports, refreshes, and kills a pid.

### N3 — Gate
- `cargo test --workspace`, `clippy -D warnings`, `fmt --check` (real exit codes — don't trust a
  piped `tail`).
- **Observation (needs Murphy):** open Network; see real listening ports (start a `python -m
  http.server` and watch it appear on refresh); kill it (two-click) and watch it vanish; a
  root-owned port surfaces a `PermissionDenied` status, not a crash. Interfaces show the WAN first.
- **Deferred to inc-2:** cpu/mem columns (needs the process-refresh join), sortable headers,
  filter box, established (non-listening) connections, kill-by-port-distinct-from-pid.

## Durability (worktree agents)
`git push -u origin HEAD` after **every** commit — the worktree is torn down at the session
boundary and unpushed local branches are lost. This has bitten us repeatedly.
