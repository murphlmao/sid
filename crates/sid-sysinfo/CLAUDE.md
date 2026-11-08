# sid-sysinfo (system info adapter)

`SysinfoProvider`: composed `sid_core::sys::SysProvider` over `sysinfo` + `netstat2` + `nix`.
The only crate naming those.

## Surface
`SysinfoProvider::new()`; modules `overview` (CPU/mem/swap), `processes`, `interfaces`, `ports`,
`default_route`, `kill`. The app holds it as `Arc<Mutex<SysinfoProvider>>` (it caches a `sysinfo::System`).

## Invariants
- Process commands fall back to the process name when cmdline is unreadable.
- Each `kill_process` call is independent (no state); the UI does a two-click confirm.
- Sync API: callers run it in background tasks, never in `render`.

## Test
`nice -n 10 cargo test -p sid-sysinfo -j 3` (reads the live host; tests assert shapes, not values).

## Landmines
- The System tab re-probes every 2s — that dominates idle frame profiles (see `scripts/perfcap.sh`).
- The POC's poll/broadcast service was deliberately not ported; don't add one without a second consumer.
