#!/usr/bin/env bash
# test-ssh.sh — Docker-sshd integration suite for sid-ssh.
#
# Brings up docker/docker-compose.test.yml's `sshd` service (a throwaway
# openssh-server with a disposable baked-in test keypair — see
# docker/ssh/Dockerfile), runs the docker-targeted `#[ignore]`d test in
# crates/sid-ssh/tests/live_sshd_smoke.rs against it (key auth + exec + an
# SFTP round-trip), tears down. Real exit code.
#
# Deliberately does NOT run `live_sshd_agent_exec_shell_sftp` (the OTHER
# `#[ignore]`d test in the same file) — that one needs a real ssh-agent +
# a trusted localhost sshd on the operator's own machine and is a manual gate,
# not something this harness can run unattended. Only
# `docker_sshd_key_auth_exec_and_sftp_round_trip` is selected by name.
#
# Usage:
#   scripts/test-ssh.sh          # up, test, down
#   scripts/test-ssh.sh --keep   # up, test, leave the container running
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
        echo "test-ssh: required tool '$bin' not found on PATH" >&2
        exit 1
    fi
done

cleanup() {
    if [[ "$KEEP" -eq 0 ]]; then
        echo "test-ssh: tearing down sshd container..." >&2
        docker compose -f "$COMPOSE_FILE" down -v >&2
    else
        echo "test-ssh: --keep passed, leaving sshd container running" >&2
    fi
}
trap cleanup EXIT

echo "test-ssh: building + starting sshd container..." >&2
if ! docker compose -f "$COMPOSE_FILE" up -d --build sshd >&2; then
    echo "test-ssh: docker compose up failed" >&2
    exit 1
fi

echo "test-ssh: waiting for sshd to report healthy..." >&2
WAIT_SECS=60
elapsed=0
while true; do
    status="$(docker inspect --format '{{.State.Health.Status}}' "$(docker compose -f "$COMPOSE_FILE" ps -q sshd)" 2>/dev/null || echo "unknown")"
    if [[ "$status" == "healthy" ]]; then
        break
    fi
    if [[ "$elapsed" -ge "$WAIT_SECS" ]]; then
        echo "test-ssh: sshd did not become healthy within ${WAIT_SECS}s (status: $status)" >&2
        docker compose -f "$COMPOSE_FILE" logs sshd >&2
        exit 1
    fi
    sleep 1
    elapsed=$((elapsed + 1))
done
echo "test-ssh: sshd is healthy after ${elapsed}s" >&2

chmod 600 "$REPO_ROOT/docker/ssh/test_id_ed25519" 2>/dev/null || true

export SID_TEST_SSH_HOST="localhost"
export SID_TEST_SSH_PORT="2222"
export SID_TEST_SSH_USER="sid_test"
export SID_TEST_SSH_KEY="$REPO_ROOT/docker/ssh/test_id_ed25519"

echo "test-ssh: running cargo test -p sid-ssh --test live_sshd_smoke docker_sshd_key_auth_exec_and_sftp_round_trip -- --ignored..." >&2
(cd "$REPO_ROOT" && cargo test -p sid-ssh --test live_sshd_smoke docker_sshd_key_auth_exec_and_sftp_round_trip -- --ignored --test-threads=1)
status=$?

if [[ "$status" -eq 0 ]]; then
    echo "test-ssh: PASSED" >&2
else
    echo "test-ssh: FAILED (exit $status)" >&2
fi
exit "$status"
