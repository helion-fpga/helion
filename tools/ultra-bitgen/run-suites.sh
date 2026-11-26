#!/bin/bash
# Canonical UltraScale+ bitgen suite runner.
# SERIAL, one suite at a time, -j1. Never run cargo suites and checkers concurrently.
# The tileconn DB variables are exported ONLY for the ultrascale suite, so
# its tileconn_route tests run for real instead of skipping vacuously.
# They are unset before the CLI suite: helion-cli ultra tests assume no
# pin-wire map is loaded (the *_stops_at_route_without_pin_wire_map tests
# expect the flow to fail at ROUTE, and counter xczu3eg asserts every PIP
# is on INT_X1Y0); with the map loaded those three tests fail. The
# checkers do not read these variables (they use US09V_DB_ROOT or a
# default path), so they run unset too, as in the prior lanes.
# Fails loud (nonzero exit) if any suite fails.
# Usage: run-suites.sh [--prune-target] [WORKTREE] [LOGDIR]
# All arguments are optional. WORKTREE defaults to the git top-level of
# the repository containing this script (resolved with git rev-parse, and
# the script fails loud with a clear error if that fails), so any lane
# worktree can run it with no arguments. LOGDIR defaults to
# $HOME/workspace/<worktree-name>-logs, derived from the worktree name
# after WORKTREE is known. Explicit WORKTREE and LOGDIR arguments work
# exactly as before.
# --prune-target (N70) is an opt-in hygiene step that runs before the
# suites: for each crate in <target>/release/deps and
# <target>/debug/deps it keeps the newest-hash artifacts and deletes
# older-hash files, plus their matching .fingerprint/<crate>-<hash>
# dirs. It deletes only files matching the strict cargo artifact
# pattern (lib<crate>-<16 hex> or <crate>-<16 hex>, plus an extension)
# inside the target dir of this worktree, never sources, never
# top-level binaries, never any other worktree's target dir. If
# CARGO_TARGET_DIR is set to a directory outside the worktree, pruning
# is skipped with a clear message instead of touching that directory.
# A plain run without the flag never deletes anything. Cargo rebuilds
# anything it still needs, so a post-prune rebuild plus the same suite
# totals prove the prune was safe.
set -u
export PATH="$HOME/.cargo/bin:$PATH:$HOME/workspace/oss-cad-suite/bin"

PRUNE_TARGET=0
if [ "${1:-}" = "--prune-target" ]; then
  PRUNE_TARGET=1
  shift
fi
if [ -n "${1:-}" ]; then
  WT="$1"
else
  WT="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)" || {
    echo "ERROR: could not resolve the worktree: git -C \"$(dirname "$0")\" rev-parse --show-toplevel failed. Pass WORKTREE explicitly." >&2
    exit 1
  }
fi
if [ -n "${2:-}" ]; then
  LOGDIR="$2"
else
  LOGDIR="$HOME/workspace/$(basename "$WT")-logs"
fi
SUMMARY="$LOGDIR/suite-summary.txt"

# --- N70 prune helpers (begin) ---
# Count crates with more than one distinct hash in a deps dir (N70).
# Prints a single number. Files that do not match the cargo artifact
# pattern are ignored, never deleted.
count_dup_crates() {
  local deps_dir="$1"
  [ -d "$deps_dir" ] || { echo 0; return 0; }
  find "$deps_dir" -maxdepth 1 -type f -printf '%f\n' 2>/dev/null \
    | sed -E -n 's/^(lib)?([A-Za-z0-9_.-]+)-([0-9a-f]{16})(\.[A-Za-z0-9]+)?$/\2 \3/p' \
    | sort -u \
    | awk '{ c[$1]++ } END { d=0; for (k in c) if (c[k] > 1) d++; print d+0 }'
}

# Prune one profile dir (release or debug) under the worktree target
# dir: keep the newest-hash artifacts per crate, delete older hashes.
prune_profile_dir() {
  local profile_dir="$1"
  local deps_dir="$profile_dir/deps"
  local fp_dir="$profile_dir/.fingerprint"
  if [ ! -d "$deps_dir" ]; then
    echo "prune: $profile_dir: no deps dir, nothing to prune" | tee -a "$SUMMARY"
    return 0
  fi
  local before_bytes before_dups after_bytes after_dups
  before_bytes="$(du -sb "$deps_dir" | cut -f1)"
  before_dups="$(count_dup_crates "$deps_dir")"
  local table="$LOGDIR/prune-$(basename "$profile_dir").list"
  local deletelist="$LOGDIR/prune-$(basename "$profile_dir").delete"
  : > "$table"
  : > "$deletelist"
  # Rows are "crate hash mtime path", newest first per crate. The sed
  # pattern is the strict cargo artifact shape; anything else is left
  # alone.
  find "$deps_dir" -maxdepth 1 -type f -printf '%f %T@ %p\n' 2>/dev/null \
    | sed -E -n 's/^(lib)?([A-Za-z0-9_.-]+)-([0-9a-f]{16})(\.[A-Za-z0-9]+)? ([0-9.]+) (.*)$/\2 \3 \5 \6/p' \
    | sort -k1,1 -k3,3nr -k2,2 > "$table"
  # First row per crate (newest mtime) names the kept hash; every later
  # row with a different hash is deleted. Output rows are
  # "crate hash path"; the path is last so read assigns the remainder
  # of the line to it even if it contains spaces.
  awk '{ if (!($1 in keep)) keep[$1]=$2; if ($2 != keep[$1]) { p=$4; for (i=5;i<=NF;i++) p=p" "$i; print $1" "$2" "p } }' "$table" > "$deletelist"
  local pairs="$LOGDIR/prune-$(basename "$profile_dir").pairs"
  : > "$pairs"
  local deleted=0
  local fp_deleted=0
  local del_crate del_hash del_path
  while read -r del_crate del_hash del_path; do
    [ -n "${del_path:-}" ] || continue
    rm -f -- "$del_path" || { echo "ERROR: prune failed to delete $del_path" >&2; return 1; }
    deleted=$((deleted + 1))
    echo "$del_crate $del_hash" >> "$pairs"
  done < "$deletelist"
  local fp_crate fp_hash fp_path
  while read -r fp_crate fp_hash; do
    [ -n "${fp_crate:-}" ] || continue
    case "$fp_crate" in "."|"..") continue;; esac
    [[ "$fp_hash" =~ ^[0-9a-f]{16}$ ]] || continue
    fp_path="$fp_dir/$fp_crate-$fp_hash"
    if [ -d "$fp_path" ]; then
      rm -rf -- "$fp_path" || { echo "ERROR: prune failed to delete $fp_path" >&2; return 1; }
      fp_deleted=$((fp_deleted + 1))
    fi
  done < <(sort -u "$pairs")
  after_bytes="$(du -sb "$deps_dir" | cut -f1)"
  after_dups="$(count_dup_crates "$deps_dir")"
  echo "prune: $profile_dir: deps $before_bytes -> $after_bytes bytes (freed $((before_bytes - after_bytes))), dup crates $before_dups -> $after_dups, files deleted $deleted, fingerprint dirs deleted $fp_deleted" | tee -a "$SUMMARY"
  return 0
}

# Resolve the target dir to prune and prune release and debug, if they
# exist. Refuses to touch a CARGO_TARGET_DIR outside this worktree.
prune_target() {
  local target_dir="${CARGO_TARGET_DIR:-$WT/target}"
  case "$target_dir" in
    "$WT"/*) ;;
    *) echo "prune: CARGO_TARGET_DIR=$target_dir is outside worktree $WT; skipping prune" | tee -a "$SUMMARY"; return 0 ;;
  esac
  prune_profile_dir "$target_dir/release" || return 1
  prune_profile_dir "$target_dir/debug" || return 1
  return 0
}
# --- N70 prune helpers (end) ---

if [ -z "${US09V_DECODER_BIN:-}" ]; then
  echo "ERROR: US09V_DECODER_BIN is unset. Set it to a helion-bitgen decoder binary built from the current branch before running the checkers; the stale default fails decode with exactly 41 band bits (N34)." >&2
  exit 1
fi
if [ ! -x "$US09V_DECODER_BIN" ]; then
  echo "ERROR: US09V_DECODER_BIN=$US09V_DECODER_BIN is not an executable file." >&2
  exit 1
fi

mkdir -p "$LOGDIR"
: > "$SUMMARY"

cd "$WT" || exit 1
FAIL=0

if [ "$PRUNE_TARGET" -eq 1 ]; then
  prune_target || exit 1
fi

export HELION_TILECONN_DB_ROOT="$HOME/workspace/prjuray-db-ref/zynqusp"
export HELION_TILECONN_PART="xczu3eg-sbva484-1-e"
cargo test --no-fail-fast -j1 -p helion-ultrascale > "$LOGDIR/suite-ultrascale.log" 2>&1
ULTRA_EXIT=$?
unset HELION_TILECONN_DB_ROOT HELION_TILECONN_PART
[ "$ULTRA_EXIT" -ne 0 ] && FAIL=1
echo "ultrascale exit=$ULTRA_EXIT $(grep -oE '[0-9]+ passed; [0-9]+ failed; [0-9]+ ignored' "$LOGDIR/suite-ultrascale.log" | awk -F'[ ;]+' '{p+=$1; f+=$3; i+=$5} END {print "passed="p" failed="f" ignored="i}')" | tee -a "$SUMMARY"

cargo test --release -j1 -p helion-cli > "$LOGDIR/suite-cli-release.log" 2>&1
CLI_EXIT=$?
[ "$CLI_EXIT" -ne 0 ] && FAIL=1
echo "cli-release exit=$CLI_EXIT $(grep -oE '[0-9]+ passed; [0-9]+ failed; [0-9]+ ignored' "$LOGDIR/suite-cli-release.log" | awk -F'[ ;]+' '{p+=$1; f+=$3; i+=$5} END {print "passed="p" failed="f" ignored="i}')" | tee -a "$SUMMARY"

cd "$WT/tools/us09v" || exit 1
python3 -m unittest discover -s tests > "$LOGDIR/checkers-us09.log" 2>&1
CHECK_EXIT=$?
[ "$CHECK_EXIT" -ne 0 ] && FAIL=1
echo "checkers exit=$CHECK_EXIT $(tail -2 "$LOGDIR/checkers-us09.log" | tr '\n' ' ')" | tee -a "$SUMMARY"

if [ "$FAIL" -ne 0 ]; then
  echo "SUITES FAILED (see $SUMMARY)" | tee -a "$SUMMARY"
  exit 1
fi
echo "ALL SUITES DONE" | tee -a "$SUMMARY"
