#!/usr/bin/env bash
# FM-HEL-HANG-1539: never leave orphan Ibex cargo. Hard-cap 75s.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
timeout 75s cargo test -p helion-sv --lib -- ysyx_ibex_lists_modules_and_synths_top --nocapture
