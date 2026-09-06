#!/usr/bin/env bash
# FM-HEL-TOP: Ibex-scale implement (bitstream) with hard wall-clock cap.
# Never uncapped. Mac-safe: python3 process-group alarm (no GNU timeout).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export HELION_ROOT="$ROOT"
export IBEX_IMPL_CAP_SEC="${IBEX_IMPL_CAP_SEC:-120}"
exec python3 "$ROOT/scripts/ibex-impl-capped.py"
