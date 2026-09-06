#!/usr/bin/env bash
# Thin wrapper: corpus-one.sh <manifest.json> <id>
# TIMEOUT_S = per-stage cap (default 75). Outer wall is always 600s.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CORPUS="${CORPUS_ROOT:-/workspace/fm-hel-corpus}"
STAGE_T="${TIMEOUT_S:-75}"
exec timeout 600 python3 "$CORPUS/harness/corpus_one.py" \
  "${1:?manifest}" "${2:?id}" "$ROOT" "$CORPUS" "$STAGE_T"
