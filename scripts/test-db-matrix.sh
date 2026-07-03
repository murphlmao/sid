#!/usr/bin/env bash
# test-db-matrix.sh — full Docker integration matrix for sid-db: plain
# Postgres (rich-type rendering, cancel, auth) AND TimescaleDB (internal-
# schema exclusion). Complements scripts/test-integration.sh (which only
# covers postgres_integration.rs's FK-rich fixture) by also bringing up the
# `timescale` service and running every other `#[ignore]`d sid-db test file.
#
# Real exit code: 0 only if BOTH containers came up healthy AND every test
# passed.
#
# Usage:
#   scripts/test-db-matrix.sh          # up, test, down
#   scripts/test-db-matrix.sh --keep   # up, test, leave containers running
#
# See docs/design/2026-07-02-testing-strategy.md.

set -uo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." >/dev/null 2>&1 && pwd)"
COMPOSE_FILE="$REPO_ROOT/docker/docker-compose.test.yml"

KEEP=0
if [[ "${1:-}" == "--keep" ]]; then
    KEEP=1
fi

for bin in docker cargo; do
    if ! command -v "$bin" >/dev/null 2>&1; then
        echo "test-db-matrix: required tool '$bin' not found on PATH" >&2
        exit 1
    fi
done

cleanup() {
    if [[ "$KEEP" -eq 0 ]]; then
        echo "test-db-matrix: tearing down postgres+timescale containers..." >&2
        docker compose -f "$COMPOSE_FILE" down -v >&2
    else
        echo "test-db-matrix: --keep passed, leaving containers running" >&2
    fi
}
trap cleanup EXIT

wait_healthy() {
    local service="$1"
    local wait_secs=90
    local elapsed=0
    echo "test-db-matrix: waiting for '$service' to report healthy..." >&2
    while true; do
        local cid status
        cid="$(docker compose -f "$COMPOSE_FILE" ps -q "$service" 2>/dev/null)"
        status="$(docker inspect --format '{{.State.Health.Status}}' "$cid" 2>/dev/null || echo "unknown")"
        if [[ "$status" == "healthy" ]]; then
            echo "test-db-matrix: '$service' is healthy after ${elapsed}s" >&2
            return 0
        fi
        if [[ "$elapsed" -ge "$wait_secs" ]]; then
            echo "test-db-matrix: '$service' did not become healthy within ${wait_secs}s (status: $status)" >&2
            docker compose -f "$COMPOSE_FILE" logs "$service" >&2
            return 1
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
}

echo "test-db-matrix: starting postgres+timescale containers..." >&2
if ! docker compose -f "$COMPOSE_FILE" up -d postgres timescale >&2; then
    echo "test-db-matrix: docker compose up failed" >&2
    exit 1
fi

wait_healthy postgres || exit 1
wait_healthy timescale || exit 1

overall_status=0

run_tests() {
    local test_bin="$1"
    echo "test-db-matrix: running cargo test -p sid-db --test $test_bin -- --ignored..." >&2
    if ! (cd "$REPO_ROOT" && cargo test -p sid-db --test "$test_bin" -- --ignored --test-threads=1); then
        echo "test-db-matrix: $test_bin FAILED" >&2
        overall_status=1
    fi
}

# Plain-Postgres suites (docker/pg-init/ fixtures).
run_tests postgres_integration
run_tests postgres_rich_types
run_tests postgres_cancel
run_tests postgres_auth

# TimescaleDB suite (docker/timescale-init/ fixture).
run_tests postgres_timescale

if [[ "$overall_status" -eq 0 ]]; then
    echo "test-db-matrix: PASSED" >&2
else
    echo "test-db-matrix: FAILED" >&2
fi
exit "$overall_status"
