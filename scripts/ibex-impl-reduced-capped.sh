#!/usr/bin/env bash
# FM-HEL-TOP: reduced Ibex-scale implement ≤120s. Never uncapped.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export HELION_ROOT="$ROOT"
export IBEX_IMPL_CAP_SEC="${IBEX_IMPL_CAP_SEC:-120}"
exec python3 "$ROOT/scripts/ibex-impl-reduced-capped.py"
