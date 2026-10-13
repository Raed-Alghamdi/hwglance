#!/usr/bin/env bash
# HW Glance check: unit, interaction and render tests (snapshots included), clippy, and the
# bar widget's model checks. Set HWGLANCE_SHOTS=<dir> to also write the renders as PNGs.
# The golden snapshot tests are GPU-rendered; HWGLANCE_SKIP_GOLDEN=1 makes them pass without
# comparing (CI sets it).
set -euo pipefail
cd "$(dirname "$0")/.."

echo "== cargo test"
cargo test --workspace --quiet
echo "== cargo clippy (warnings are reported, not fatal)"
cargo clippy --workspace --all-targets --quiet
echo "== bar widget model checks"
node omarchy-plugin/harness/model-test.js
echo "check passed"
