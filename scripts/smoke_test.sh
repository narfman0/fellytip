#!/usr/bin/env bash
# smoke_test.sh — Build workspace, boot server + headless client, run all ralph
# scenarios, clean up, and exit with ralph's exit code.
#
# Usage: bash scripts/smoke_test.sh
#
# Requires: Rust toolchain in PATH, ports 15702 and 15703 free.

set -euo pipefail

WORKSPACE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${WORKSPACE}/target/debug"
EXE=""
if [[ "$OSTYPE" == "msys" || "$OSTYPE" == "cygwin" || "$OSTYPE" == "win"* ]]; then
    EXE=".exe"
fi

SERVER_PID=""
CLIENT_PID=""

cleanup() {
    [[ -n "$CLIENT_PID" ]] && kill "$CLIENT_PID" 2>/dev/null || true
    [[ -n "$SERVER_PID" ]] && kill "$SERVER_PID" 2>/dev/null || true
    wait 2>/dev/null || true
}
trap cleanup EXIT

# ── 1. Build ──────────────────────────────────────────────────────────────────
echo "--- Building (server, client, ralph) ---"
cargo build --manifest-path "${WORKSPACE}/Cargo.toml" \
    -p fellytip-server -p fellytip-client -p ralph

# ── 2. Start server ───────────────────────────────────────────────────────────
echo "--- Starting server (--no-idle-shutdown) ---"
"${TARGET}/fellytip-server${EXE}" --no-idle-shutdown \
    > "${WORKSPACE}/server.log" 2>&1 &
SERVER_PID=$!
echo "  server PID: ${SERVER_PID}"

# ── 3. Start headless client ──────────────────────────────────────────────────
echo "--- Starting headless client ---"
"${TARGET}/fellytip-client${EXE}" --headless \
    > "${WORKSPACE}/client.log" 2>&1 &
CLIENT_PID=$!
echo "  client PID: ${CLIENT_PID}"

# ── 4. Run ralph (handles BRP-ready polling internally) ───────────────────────
echo "--- Running ralph scenarios ---"
RALPH_EXIT=0
cargo run --manifest-path "${WORKSPACE}/Cargo.toml" -p ralph -- --scenario all \
    || RALPH_EXIT=$?

# ── 5. Report ─────────────────────────────────────────────────────────────────
if [[ $RALPH_EXIT -eq 0 ]]; then
    echo "=== SMOKE TEST PASSED ==="
else
    echo "=== SMOKE TEST FAILED (ralph exit ${RALPH_EXIT}) ==="
    echo "--- server.log (last 20 lines) ---"
    tail -20 "${WORKSPACE}/server.log" || true
    echo "--- client.log (last 20 lines) ---"
    tail -20 "${WORKSPACE}/client.log" || true
fi

exit $RALPH_EXIT
