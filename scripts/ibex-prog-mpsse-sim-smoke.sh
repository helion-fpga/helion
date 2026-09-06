#!/usr/bin/env bash
# FM-HEL-TOP: capped Ibex/pin-wrap build (≤120s) + helion-prog --cable mpsse-sim STAT smoke.
# Never uncapped. Proves sim fabric DONE only — not board/USB DONE.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export HELION_ROOT="$ROOT"
export IBEX_IMPL_CAP_SEC="${IBEX_IMPL_CAP_SEC:-120}"
export IBEX_SMOKE_DESIGNS="${IBEX_SMOKE_DESIGNS:-reduced,pinwrap,bare}"
export IBEX_SMOKE_SKIP_BUILD="${IBEX_SMOKE_SKIP_BUILD:-0}"
exec python3 "$ROOT/scripts/ibex-prog-mpsse-sim-smoke.py"
