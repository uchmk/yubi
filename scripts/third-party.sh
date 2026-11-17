#!/usr/bin/env bash
# Regenerates THIRD-PARTY-LICENSES.txt from `cargo metadata` and the licence texts the crates ship.
set -euo pipefail
cd "$(dirname "$0")/.."
python scripts/third_party.py
