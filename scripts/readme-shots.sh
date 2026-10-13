#!/usr/bin/env bash
# Regenerate the README screenshots in docs/screenshots/ from the demo data.
# The renders use the GPU (wgpu), so they come from whatever adapter the machine has; run it on
# a desktop, not in CI. PNGs are shrunk with oxipng when it is installed.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo test -p hwglance --test readme_shots -- --ignored --nocapture
if command -v oxipng >/dev/null; then
    oxipng -q -o 4 --strip safe docs/screenshots/*.png
fi
ls -l docs/screenshots
