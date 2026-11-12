#!/bin/sh
# Vendor the Rune UI framework into third_party/rune at one exact commit.
#
#   scripts/sync-rune.sh [RUNE_CHECKOUT] [REV]
#
# RUNE_CHECKOUT defaults to ../rune (a clone of https://github.com/anomalyco/rune),
# REV to that checkout's HEAD. Only the crates helion-rune builds against are
# copied, with Rune's directory layout kept (some crates include_bytes! files
# from sibling directories). Rune is MIT-licensed; its LICENSE is kept.
#
# Why vendor: the IDE must build from a single Helion clone (or release
# tarball) with no sibling checkout, no extra git fetch and no dependence on
# an upstream branch staying put. The `rune` name on crates.io belongs to an
# unrelated project, so a registry dependency is not an option.
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
SRC=${1:-$ROOT/../rune}
SRC=$(cd "$SRC" && pwd)
REV=${2:-HEAD}
DEST=$ROOT/third_party/rune
UPSTREAM=https://github.com/anomalyco/rune

CRATES="rune rune-kit rune-motion rune-staging rune-taffy procwatch rune-mermaid rune-ref"
THEMES="opencode"

git -C "$SRC" rev-parse --verify --quiet "$REV^{commit}" >/dev/null \
  || { echo "sync-rune: $REV is not a commit in $SRC" >&2; exit 1; }
SHA=$(git -C "$SRC" rev-parse "$REV^{commit}")
if ! git -C "$SRC" branch -r --contains "$SHA" 2>/dev/null | grep -q .; then
  echo "sync-rune: warning: $SHA is not on any remote branch of $SRC (push it upstream first)" >&2
fi

PATHS="LICENSE Cargo.toml clippy.toml"
for c in $CRATES; do PATHS="$PATHS crates/$c"; done
for t in $THEMES; do PATHS="$PATHS themes/$t"; done

rm -rf "$DEST"
mkdir -p "$DEST"
# shellcheck disable=SC2086
git -C "$SRC" archive --format=tar "$SHA" -- $PATHS | tar -x -C "$DEST"

# The upstream workspace lists every Rune example/app; keep only what is vendored.
MEMBERS=""
for c in $CRATES; do MEMBERS="$MEMBERS    \"crates/$c\",\n"; done
for t in $THEMES; do MEMBERS="$MEMBERS    \"themes/$t\",\n"; done
awk -v members="$MEMBERS" '
  /^members = \[/ { printf "members = [\n%s]\n", members; skip = 1; next }
  skip && /^\]/   { skip = 0; next }
  skip            { next }
  /^exclude = /   { next }
  { print }
' "$DEST/Cargo.toml" | sed 's/\\n/\n/g' > "$DEST/Cargo.toml.new"
mv "$DEST/Cargo.toml.new" "$DEST/Cargo.toml"

cat > "$DEST/VENDORED.md" <<MD
# Vendored Rune

Upstream: $UPSTREAM
Commit:   $SHA
Crates:   $CRATES
Themes:   $THEMES

Do not edit files here. Change Rune upstream, then re-run
\`scripts/sync-rune.sh <rune-checkout> <commit>\` and commit the result.
MD
echo "sync-rune: vendored $UPSTREAM@$SHA into third_party/rune"
