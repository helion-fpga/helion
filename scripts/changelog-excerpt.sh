#!/bin/sh
# Print the CHANGELOG body for one version (tag vX.Y.Z or X.Y.Z).
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
VER=${1:-}
[ -n "$VER" ] || { echo "usage: scripts/changelog-excerpt.sh vX.Y.Z" >&2; exit 1; }
VER=${VER#v}
FILE="$ROOT/CHANGELOG.md"
[ -f "$FILE" ] || { echo "missing $FILE" >&2; exit 1; }
awk -v ver="$VER" '
  $0 ~ ("^## \\[" ver "\\]") { p = 1; next }
  p && $0 ~ /^## \[/ { exit }
  p { print }
' "$FILE" | sed -e :a -e '/^\n*$/{$d;N;ba' -e '}'
