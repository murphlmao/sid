#!/usr/bin/env bash
# test-smoke-headless.sh — headless launch-survives smoke for the `sid`
# GPUI binary: builds docker/headless-smoke/'s image (Xvfb + Lavapipe
# software Vulkan, no GPU/compositor needed) and runs it. Real exit code.
#
# NOT a pixel/golden-image test — see
# docs/design/2026-07-02-testing-strategy.md ("Executive decision: headless
# GPUI smoke") for what this does and doesn't check, and why.
#
# Usage:
#   scripts/test-smoke-headless.sh

set -uo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." >/dev/null 2>&1 && pwd)"

if ! command -v docker >/dev/null 2>&1; then
    echo "test-smoke-headless: docker not found on PATH" >&2
    exit 1
fi

echo "test-smoke-headless: building docker/headless-smoke/ (this compiles the full workspace; a few minutes cold)..." >&2
if ! docker build -f "$REPO_ROOT/docker/headless-smoke/Dockerfile" -t sid-headless-smoke "$REPO_ROOT" >&2; then
    echo "test-smoke-headless: docker build failed" >&2
    exit 1
fi

echo "test-smoke-headless: running the smoke container..." >&2
docker run --rm sid-headless-smoke
status=$?

if [[ "$status" -eq 0 ]]; then
    echo "test-smoke-headless: PASSED" >&2
else
    echo "test-smoke-headless: FAILED (exit $status)" >&2
fi
exit "$status"
