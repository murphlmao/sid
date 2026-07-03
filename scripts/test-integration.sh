#!/usr/bin/env bash
# test-integration.sh — Docker-Postgres integration suite for sid-db.
#
# Brings up docker/docker-compose.test.yml's `postgres` service (seeded with
# the FK-rich fixture schema in docker/pg-init/), runs the `#[ignore]`d tests
# in crates/sid-db/tests/postgres_integration.rs against it, tears down.
# Real exit code: 0 only if the container came up healthy AND every test
# passed.
#
# Usage:
#   scripts/test-integration.sh          # up, test, down
#   scripts/test-integration.sh --keep   # up, test, leave the container running
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
        echo "test-integration: required tool '$bin' not found on PATH" >&2
        exit 1
    fi
done

cleanup() {
    if [[ "$KEEP" -eq 0 ]]; then
        echo "test-integration: tearing down postgres container..." >&2
        docker compose -f "$COMPOSE_FILE" down -v >&2
    else
        echo "test-integration: --keep passed, leaving postgres container running" >&2
    fi
}
trap cleanup EXIT

echo "test-integration: starting postgres container..." >&2
if ! docker compose -f "$COMPOSE_FILE" up -d postgres >&2; then
    echo "test-integration: docker compose up failed" >&2
    exit 1
fi

echo "test-integration: waiting for postgres to report healthy..." >&2
WAIT_SECS=60
elapsed=0
while true; do
    status="$(docker inspect --format '{{.State.Health.Status}}' "$(docker compose -f "$COMPOSE_FILE" ps -q postgres)" 2>/dev/null || echo "unknown")"
    if [[ "$status" == "healthy" ]]; then
        break
    fi
    if [[ "$elapsed" -ge "$WAIT_SECS" ]]; then
        echo "test-integration: postgres did not become healthy within ${WAIT_SECS}s (status: $status)" >&2
        docker compose -f "$COMPOSE_FILE" logs postgres >&2
        exit 1
    fi
    sleep 1
    elapsed=$((elapsed + 1))
done
echo "test-integration: postgres is healthy after ${elapsed}s" >&2

echo "test-integration: running cargo test -p sid-db --test postgres_integration -- --ignored..." >&2
(cd "$REPO_ROOT" && cargo test -p sid-db --test postgres_integration -- --ignored --test-threads=1)
status=$?

if [[ "$status" -eq 0 ]]; then
    echo "test-integration: PASSED" >&2
else
    echo "test-integration: FAILED (exit $status)" >&2
fi
exit "$status"
