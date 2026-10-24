#!/bin/sh
# Unclean-exit recovery smoke for the embedded Fiber node.
#
# Phase 1 starts the node and hard-exits (exit code 9) without stopping it.
# Phase 2 starts against the same data directory and must recover, then stop
# cleanly. Any other exit code fails the smoke.
set -eu

cd "$(dirname "$0")/.."
DATA_DIR="${TMPDIR:-/tmp}/fiber-wallet-recovery-smoke"
BIN="core/target/debug/fnn_recovery_smoke"

echo "== recovery smoke: data dir $DATA_DIR"
rm -rf "$DATA_DIR"

(cd core && cargo build --bin fnn_recovery_smoke)

echo "== phase 1: crash"
set +e
"$BIN" "$DATA_DIR" crash
crash_code=$?
set -e
if [ "$crash_code" -ne 9 ]; then
  echo "FAIL: crash phase exited $crash_code, expected 9" >&2
  exit 1
fi

echo "== phase 2: recover"
"$BIN" "$DATA_DIR" recover

echo "== recovery smoke passed"
