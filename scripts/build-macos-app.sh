#!/bin/sh
# Assemble dist/Helion.app for Apple Silicon (aarch64-apple-darwin).
# POSIX sh. No Docker, no Rosetta, no vendor bitstreams.
#
#   rustup target add aarch64-apple-darwin
#   ./scripts/build-macos-app.sh
#   open dist/Helion.app
#
# This Linux VM cannot link aarch64-apple-darwin. Without --layout-only the
# script errors clearly. --layout-only writes Info.plist + HAD + icon only.
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
TARGET=${HELION_TARGET_TRIPLE:-aarch64-apple-darwin}
DIST=${HELION_DIST:-"$ROOT/dist"}
LAYOUT_ONLY=0
SKIP_BUILD=${HELION_SKIP_BUILD:-0}

die() { printf '%s\n' "$*" >&2; exit 1; }

usage() {
    cat <<'USAGE'
usage: scripts/build-macos-app.sh [--layout-only] [--out DIR]

  rustup target add aarch64-apple-darwin
  cargo build --release --target aarch64-apple-darwin
  assemble dist/Helion.app (MacOS, Info.plist, Resources/devices, icon)

  --layout-only   HAD + Info.plist + icon, no cargo (Linux smoke / no SDK)
  --out DIR       bundle parent (default: <repo>/dist)
USAGE
}

while [ $# -gt 0 ]; do
    case "$1" in
        --layout-only) LAYOUT_ONLY=1; shift ;;
        --out)
            [ $# -ge 2 ] || die "--out needs a directory"
            DIST=$2
            shift 2
            ;;
        --out=*)
            DIST=${1#--out=}
            shift
            ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown argument $1" ;;
    esac
done

if [ "$LAYOUT_ONLY" -eq 1 ]; then
    SKIP_BUILD=1
fi

APP="$DIST/Helion.app"
MACOS="$APP/Contents/MacOS"
RES="$APP/Contents/Resources"
uname_s=$(uname -s)
uname_m=$(uname -m)

PLIST=
ICON=
for d in "$ROOT/packaging/macos" "$ROOT/scripts/macos"; do
    if [ -z "$PLIST" ] && [ -f "$d/Info.plist" ]; then
        PLIST="$d/Info.plist"
    fi
    if [ -z "$ICON" ] && [ -f "$d/AppIcon.png" ]; then
        ICON="$d/AppIcon.png"
    fi
done
[ -n "$PLIST" ] || die "missing Info.plist (packaging/macos or scripts/macos)"

IDE=""
CLI=""

if [ "$SKIP_BUILD" != "1" ]; then
    if [ "$uname_s" != "Darwin" ]; then
        die "error: scripts/build-macos-app.sh must run on macOS with the Apple SDK.
this host is $uname_s/$uname_m; $TARGET cannot be linked here.
bundle layout only: $0 --layout-only
on Apple Silicon:
  rustup target add $TARGET
  ./scripts/build-macos-app.sh
  open dist/Helion.app"
    fi
    if ! command -v xcrun >/dev/null 2>&1 || ! xcrun --show-sdk-path >/dev/null 2>&1; then
        die "error: Apple SDK not found (xcrun --show-sdk-path failed)."
    fi
    command -v rustup >/dev/null 2>&1 || die "error: rustup not on PATH; needed for: rustup target add $TARGET"
    command -v cargo >/dev/null 2>&1 || die "error: cargo not on PATH"
    printf 'rustup target add %s\n' "$TARGET"
    rustup target add "$TARGET" || die "error: rustup target add $TARGET failed"
    if ! rustup target list --installed | grep -qx "$TARGET"; then
        die "error: rustup target $TARGET is not installed"
    fi
    printf 'cargo build --release --target %s\n' "$TARGET"
    cargo build --release --target "$TARGET" --manifest-path "$ROOT/Cargo.toml" \
        -p helion-cli -p helion-gui \
        || die "error: cargo --target $TARGET failed (Apple linker / SDK required). this host is $uname_s/$uname_m."
    BIN_DIR="$ROOT/target/$TARGET/release"
    [ -f "$BIN_DIR/helion-ide" ] || die "missing $BIN_DIR/helion-ide"
    IDE="$BIN_DIR/helion-ide"
    if [ -f "$BIN_DIR/helion" ]; then
        CLI="$BIN_DIR/helion"
    fi
else
    BIN_DIR=${HELION_BIN_DIR:-}
    if [ -n "$BIN_DIR" ]; then
        if [ -f "$BIN_DIR/helion-ide" ]; then
            IDE="$BIN_DIR/helion-ide"
        elif [ -f "$BIN_DIR/Helion" ]; then
            IDE="$BIN_DIR/Helion"
        fi
        if [ -f "$BIN_DIR/helion" ]; then
            CLI="$BIN_DIR/helion"
        fi
    fi
    printf 'layout-only: assembling Helion.app without cargo (host %s/%s, not a Mac binary)\n' "$uname_s" "$uname_m"
fi

rm -rf "$APP"
mkdir -p "$MACOS" "$RES/devices" "$RES/examples"
cp "$PLIST" "$APP/Contents/Info.plist"
if [ -n "$ICON" ]; then
    cp "$ICON" "$RES/AppIcon.png"
fi
printf 'placeholder AppIcon — replace with AppIcon.icns via iconutil on macOS\n' > "$RES/ICON.txt"
printf 'APPLHDSN' > "$APP/Contents/PkgInfo"

write_stub() {
    dest=$1
    name=$2
    printf '#!/bin/sh\necho %s: layout-only placeholder — rebuild on aarch64-apple-darwin\nexit 1\n' "$name" > "$dest"
    chmod +x "$dest"
}

if [ -n "$IDE" ]; then
    cp "$IDE" "$MACOS/Helion"
    chmod +x "$MACOS/Helion"
    cp "$IDE" "$MACOS/helion-ide"
    chmod +x "$MACOS/helion-ide"
else
    write_stub "$MACOS/Helion" helion-ide
    write_stub "$MACOS/helion-ide" helion-ide
fi
if [ -n "$CLI" ]; then
    # Default APFS is case-insensitive: `helion` would clobber `Helion` (the IDE).
    if [ "$uname_s" = Darwin ]; then
        cp "$CLI" "$MACOS/helion-cli"
        chmod +x "$MACOS/helion-cli"
    else
        cp "$CLI" "$MACOS/helion"
        chmod +x "$MACOS/helion"
    fi
else
    write_stub "$MACOS/helion-cli" helion
fi

# Runtime HAD: Device::devices_dir looks at Contents/MacOS/../Resources/devices/helion
if [ -d "$ROOT/devices/helion" ]; then
    cp -R "$ROOT/devices/helion" "$RES/devices/helion"
else
    die "missing $ROOT/devices/helion"
fi
[ -f "$RES/devices/helion/parts/HL10T-C32-1.toml" ] || die "HAD missing in bundle"

if [ -d "$ROOT/examples" ]; then
    for f in "$ROOT/examples"/*; do
        [ -e "$f" ] || continue
        cp -R "$f" "$RES/examples/"
    done
fi

printf 'layout: %s\n' "$APP"
printf '  HAD %s\n' "$RES/devices/helion/parts/HL10T-C32-1.toml"
if [ "$SKIP_BUILD" = "1" ]; then
    printf 'layout-only: skipping rustup/cargo (not a running Mac .app)\n'
    exit 0
fi
printf 'built %s\n' "$APP"
printf '  MacOS: Helion + helion-ide + helion-cli\n'
printf 'run:    open %s\n' "$APP"
printf 'doctor: %s/helion doctor\n' "$MACOS"
