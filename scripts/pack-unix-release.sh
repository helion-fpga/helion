#!/bin/sh
# Pack helion + optional helion-ide / helion-prog + HAD + examples + licenses.
#
#   scripts/pack-unix-release.sh VERSION TRIPLE BIN_DIR OUT_DIR
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
VER=${1:-}
TRIPLE=${2:-}
BIN_DIR=${3:-}
OUT_DIR=${4:-}

[ -n "$VER" ] && [ -n "$TRIPLE" ] && [ -n "$BIN_DIR" ] && [ -n "$OUT_DIR" ] \
  || { echo "usage: scripts/pack-unix-release.sh VERSION TRIPLE BIN_DIR OUT_DIR" >&2; exit 1; }

VER=${VER#v}
NAME="helion-${VER}-${TRIPLE}"
STAGE="$OUT_DIR/$NAME"
mkdir -p "$STAGE"

copy_bin() {
  src=$1
  dest=$2
  [ -f "$src" ] || return 1
  cp "$src" "$dest"
  chmod +x "$dest"
}

copy_bin "$BIN_DIR/helion" "$STAGE/helion" \
  || { echo "missing $BIN_DIR/helion" >&2; exit 1; }
if [ -f "$BIN_DIR/helion-ide" ]; then
  copy_bin "$BIN_DIR/helion-ide" "$STAGE/helion-ide"
fi
if [ -f "$BIN_DIR/helion-prog" ]; then
  copy_bin "$BIN_DIR/helion-prog" "$STAGE/helion-prog"
fi
# Drop debug symbols so tarballs stay ≪ commercial CAD.
for b in helion helion-ide helion-prog; do
  [ -f "$STAGE/$b" ] || continue
  strip "$STAGE/$b" >/dev/null 2>&1 || true
done

mkdir -p "$STAGE/devices"
cp -R "$ROOT/devices/helion" "$STAGE/devices/helion"
mkdir -p "$STAGE/examples"
for f in "$ROOT/examples"/*; do
  [ -e "$f" ] || continue
  # ip_ingest is a CLI test corpus, not a stranger-clone lab.
  case "$(basename "$f")" in
    ip_ingest) continue ;;
  esac
  cp -R "$f" "$STAGE/examples/"
done
cp "$ROOT/LICENSE-APACHE" "$ROOT/LICENSE-MIT" "$ROOT/README.md" "$STAGE/"
printf '%s\n' "$VER" "$TRIPLE" > "$STAGE/VERSION"

mkdir -p "$OUT_DIR"
tar -C "$OUT_DIR" -czf "$OUT_DIR/${NAME}.tar.gz" "$NAME"
rm -rf "$STAGE"
printf '%s\n' "$OUT_DIR/${NAME}.tar.gz"
