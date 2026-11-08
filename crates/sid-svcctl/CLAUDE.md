# sid-svcctl (systemctl adapter)

CLI-shelling `sid_core::svc::ServiceProvider` over `systemctl`.

## Surface
`SvcctlProvider::new()` (stateless; `Arc<dyn ServiceProvider>` suffices). Modules `client`
(`tokio::process::Command`), `parse` (list-units output), `classify` (stderr -> typed error:
Access denied / interactive auth required / ...).

## Invariants
- `tokio::process`, never `std::process` — calls run inside spawned tasks and must yield.
- Parsing and error classification are pure and unit-tested against captured output.
- `SvcScope::System` vs user scope is explicit at every call.

## Test
`nice -n 10 cargo test -p sid-svcctl -j 3` (no systemd required — fixtures).

## Landmines
- Never invoke privileged actions implicitly; surface "needs auth" as the classified error.
- systemd output formats vary by version — extend `parse` tests with real samples when fixing.
