# sid-containers (docker / kubectl adapters)

CLI-shelling impls of `sid_core::containers::{ContainerProvider, KubeProvider}`.

## Surface
`DockerCliProvider::new()` (`src/docker/`), `KubectlCliProvider::new()` (`src/kube/`). Both use
`tokio::process::Command`; `docker`/`kubectl` are named only here.

## Invariants
- Stateless; callers hold `Arc<dyn ...>` and invoke only from spawned tasks.
- A missing binary or unreachable daemon/cluster yields a typed error the Network tab renders
  (`error_line`) — never a panic or a silent empty list.
- Parse JSON/`--format` output defensively; unknown fields ignored.

## Test
`nice -n 10 cargo test -p sid-containers -j 3` (parsers over captured output; no docker needed).

## Landmines
- Docker and Kubernetes are Network-tab sub-tabs (`ui/network_tab.rs`); output-shape changes
  break rendering there — capture with `scripts/sid-cap.sh --tab network`.
