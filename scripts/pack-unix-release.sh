#!/bin/sh
# Pack helion + optional helion-ide / helion-prog + HAD + examples + licenses.
# A *windows* TRIPLE packs the .exe files into a .zip; it needs LIBUSB_COPYING
# (the bundled libusb's license; libusb is linked statically on Windows).
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
case "$TRIPLE" in *windows*) EXE=.exe ;; *) EXE= ;; esac
if [ -n "$EXE" ]; then
  [ -f "${LIBUSB_COPYING:-}" ] || { echo "LIBUSB_COPYING must name libusb's COPYING file" >&2; exit 1; }
fi
mkdir -p "$STAGE"

copy_bin() {
  src=$1
  dest=$2
  [ -f "$src" ] || return 1
  cp "$src" "$dest"
  chmod +x "$dest"
}

copy_bin "$BIN_DIR/helion$EXE" "$STAGE/helion$EXE" \
  || { echo "missing $BIN_DIR/helion$EXE" >&2; exit 1; }
for b in helion-ide helion-prog; do
  if [ -f "$BIN_DIR/$b$EXE" ]; then
    copy_bin "$BIN_DIR/$b$EXE" "$STAGE/$b$EXE"
  fi
done

mkdir -p "$STAGE/devices"
cp -R "$ROOT/devices/helion" "$STAGE/devices/helion"
mkdir -p "$STAGE/examples"
for f in "$ROOT/examples"/*; do
  [ -e "$f" ] || continue
  cp -R "$f" "$STAGE/examples/"
done
cp "$ROOT/LICENSE" "$ROOT/LICENSE-APACHE" "$ROOT/LICENSE-MIT" "$ROOT/README.md" "$STAGE/"
# helion-ide links the vendored Rune UI framework (MIT): ship its notice.
cp "$ROOT/third_party/rune/LICENSE" "$STAGE/LICENSE-RUNE"
printf '%s\n' "$VER" "$TRIPLE" > "$STAGE/VERSION"

if [ -n "$EXE" ]; then
  cp "$LIBUSB_COPYING" "$STAGE/LICENSE-LIBUSB"
  cat > "$STAGE/README-WINDOWS.txt" <<'TXT'
Helion for Windows x86_64. Unzip anywhere and run helion-ide.exe (or helion.exe
from a terminal). The .exe files are not code-signed, so SmartScreen may warn.

Programming a board over JTAG:
- Xilinx 7-series boards (ZedBoard and the other xc7 boards) are programmed
  only through openFPGALoader. Install openFPGALoader.exe and put it on PATH
  (or set HELION_OPENFPGALOADER to its full path), then use Program Device in
  the IDE or: helion.exe hw program --part <part> <run>\top.bit
- openFPGALoader opens the cable through libusb, so bind the cable's
  interface 0 (Digilent, FT2232H) to WinUSB with Zadig (https://zadig.akeo.ie).
  While it is bound, Vivado and other D2XX tools cannot use that cable;
  reinstall the vendor driver to undo it.
- helion-prog.exe and the native WinUSB path load Helion .hbits files
  (HL10 parts) only; they do not program 7-series or Lattice parts.
TXT
fi

mkdir -p "$OUT_DIR"
if [ -n "$EXE" ]; then
  ARCHIVE="$OUT_DIR/${NAME}.zip"
  # 7-Zip ships on the Windows runners; Info-ZIP elsewhere.
  if command -v 7z >/dev/null 2>&1; then
    (cd "$OUT_DIR" && 7z a -tzip -bd "${NAME}.zip" "$NAME" >/dev/null)
  else
    (cd "$OUT_DIR" && zip -qr "${NAME}.zip" "$NAME")
  fi
else
  ARCHIVE="$OUT_DIR/${NAME}.tar.gz"
  tar -C "$OUT_DIR" -czf "$ARCHIVE" "$NAME"
fi
rm -rf "$STAGE"
printf '%s\n' "$ARCHIVE"
