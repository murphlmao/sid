# sid dev commands. `just --list` to see them. Every cargo call is niced and -j 3 so the
# machine stays usable while Murphy works; override with `just jobs=6 gate`.
set shell := ["bash", "-euo", "pipefail", "-c"]

jobs := "3"
nice := "nice -n 10"

default:
    @just --list

# Full 3-step gate (fmt, clippy -D warnings, test) with a single RESULT line.
gate:
    #!/usr/bin/env bash
    set -uo pipefail
    fail=0
    {{nice}} cargo fmt --all --check || fail=1
    {{nice}} cargo clippy --workspace --all-targets -j {{jobs}} -- -D warnings || fail=1
    {{nice}} cargo test --workspace -j {{jobs}} || fail=1
    if [[ $fail -eq 0 ]]; then echo "RESULT: GATE GREEN"; else echo "RESULT: GATE RED"; exit 1; fi

# Scoped gate for one crate: fmt (workspace-wide, rustfmt has no -p), clippy -p, test -p.
gate-fast pkg:
    {{nice}} cargo fmt --all --check
    {{nice}} cargo clippy -p {{pkg}} --all-targets -j {{jobs}} -- -D warnings
    {{nice}} cargo test -p {{pkg}} -j {{jobs}}

# Run one crate's tests.
test pkg:
    {{nice}} cargo test -p {{pkg}} -j {{jobs}}

# Headless capture of a tab in a theme -> target/caps/<tab>-<theme>.png (then Read the PNG).
# tab: ssh|database|network|workspaces|system|settings; theme: cosmos|void|dusk|cosmos-light.
# Extra sid-cap.sh flags go after, e.g. `just cap ssh cosmos --click 300,200`.
cap tab="ssh" theme="cosmos" *flags:
    mkdir -p target/caps
    scripts/sid-cap.sh --tab {{tab}} --theme {{theme}} --out target/caps/{{tab}}-{{theme}}.png {{flags}}

# Frame-cost harness (instructions/frame, idle vs scroll). Builds sid first. Flags: see scripts/perfcap.sh.
perf *flags:
    {{nice}} cargo build -p sid -j {{jobs}}
    mkdir -p target/perf
    scripts/perfcap.sh --bin target/debug/sid --out target/perf {{flags}}

# Docker sshd integration test for sid-ssh (needs docker).
test-ssh:
    scripts/test-ssh.sh

# Docker Postgres (+ TimescaleDB matrix) integration tests for sid-db (needs docker).
test-db:
    scripts/test-integration.sh
    scripts/test-db-matrix.sh

# Run the MCP server over stdio (crate `sid-mcp` lands with the agent-tooling work).
mcp:
    {{nice}} cargo run -q -p sid-mcp -j {{jobs}}

# Component gallery (SID_GALLERY=1 mounts it instead of the app) captured headlessly -> target/caps/gallery-<theme>.png.
gallery theme="cosmos":
    mkdir -p target/caps
    scripts/sid-cap.sh --env SID_GALLERY=1 --theme {{theme}} --size 1920x1600 --out target/caps/gallery-{{theme}}.png
