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
# --prune-target (N70, grouping revised by N74) is an opt-in hygiene
# step that runs before the suites: for each crate in
# <target>/release/deps and <target>/debug/deps it keeps the
# newest-mtime hash per (crate, version, features, unit-kind, profile)
# group and deletes older-hash files, plus their matching .fingerprint
# dirs (found by hash, since fingerprint dirs are named by package
# name). Version comes from the dep-info .d file (registry paths carry
# it); features, kind and profile come from the unit JSON in
# .fingerprint. Distinct versions of one crate, distinct feature sets
# of one version, distinct unit kinds (bin vs test-bin), and distinct
# cargo profiles are all kept, so only genuinely stale same-group
# rebuilds are pruned. It deletes only files matching the strict cargo
# artifact pattern (lib<crate>-<16 hex> or <crate>-<16 hex>, plus an
# extension) inside the target dir of this worktree, never sources,
# never top-level binaries, never any other worktree's target dir. If
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

# --- N74 prune helpers (begin) ---
# Prune grouping key (N74): (crate, version, features, unit-kind,
# profile). A crate with several live versions (syn 0.15/1.0/2.0/3.0),
# several live feature sets of one version (memchr 2.8.3 built three
# ways, libc 0.2.189 built two ways), several live unit kinds (the
# helion bin plus its test-bin), or several live cargo profiles keeps
# one hash per group; only older same-group hashes are deleted. A "dup
# group" is a group with more than one hash. Cargo rebuilds anything it
# still needs, so a deleted hash that comes back on the next suite run
# was live in that build universe, not stale. The profile dimension is
# needed because cargo builds the same crate under different profiles
# in one target dir (verified: the 9 release hashes the first version
# of this fix pruned were all rebuilt by the suites, and every rebuilt
# pair differs in profile; the debug pairs share a profile and differ
# only in their dependency fingerprints, so they are a known residual,
# see the lane report).
# Version comes from the dep-info .d file: registry source paths look
# like .../registry/src/<idx>/<pkg>-<version>/src/... . Features, kind
# and profile come from the unit JSON in
# .fingerprint/*-<hash>/ (the glob by hash covers fingerprint dirs
# named by package name, which uses dashes where deps filenames use
# underscores). Unknowns are "-" (version, kind or profile) and "-"
# (features); known-empty features are "~". Unknowns group together,
# which is the old N70 behavior for those artifacts. Profile hashes
# are stable across runs and worktrees (the same values recur in
# unrelated target dirs), so grouping by them is sound.
# Filenames are parsed without assuming a lib prefix: only .rlib,
# .rmeta and .so files carry one, so libc-<hash>.d belongs to crate
# libc, not to a phantom crate "c" (N70 misread it that way, which also
# double-counted libc as a dup crate).
artifact_version() {
  local crate="$1" hash="$2" deps_dir="$3"
  local dfile="$deps_dir/$crate-$hash.d"
  [ -f "$dfile" ] || { printf '%s' "-"; return 0; }
  local dashed="${crate//_/-}"
  local hit
  hit="$(tr ' ' '\n' < "$dfile" | grep -m1 -E "/registry/src/[^/]+/(${crate}|${dashed})-[0-9][^/]*/" || true)"
  [ -n "$hit" ] || { printf '%s' "-"; return 0; }
  local seg
  seg="$(printf '%s' "$hit" | tr '/' '\n' | grep -m1 -E "^(${crate}|${dashed})-[0-9]" || true)"
  [ -n "$seg" ] || { printf '%s' "-"; return 0; }
  local rest="$seg"
  case "$rest" in
    "${crate}-"*) rest="${rest#${crate}-}" ;;
    "${dashed}-"*) rest="${rest#${dashed}-}" ;;
    *) printf '%s' "-"; return 0 ;;
  esac
  [ -n "$rest" ] || { printf '%s' "-"; return 0; }
  printf '%s' "$rest"
}

# Prints "features kind profile" for one (crate, hash) unit, from the
# unit JSON in its fingerprint dir (lib-<crate>.json,
# bin-<crate>.json, test-bin-<crate>.json,
# test-integration-test-<crate>.json, ...).
artifact_unit_info() {
  local crate="$1" hash="$2" fp_dir="$3"
  local json=""
  local d j
  for d in "$fp_dir"/*-"$hash"; do
    [ -d "$d" ] || continue
    for j in "$d"/*.json; do
      [ -f "$j" ] || continue
      json="$j"
      break 2
    done
  done
  if [ -z "$json" ]; then
    printf '%s %s %s' "-" "-" "-"
    return 0
  fi
  local stem kind
  stem="$(basename "$json" .json)"
  kind="${stem%-"$crate"}"
  local raw key prof
  raw="$(sed -n 's/.*"features":"\(\[[^]]*\]\)".*/\1/p' "$json" | head -n1)"
  prof="$(sed -n 's/.*"profile":\([0-9][0-9]*\).*/\1/p' "$json" | head -n1)"
  [ -n "$prof" ] || prof="-"
  if [ -z "$raw" ]; then
    printf '%s %s %s' "-" "$kind" "$prof"
    return 0
  fi
  key="$(printf '%s' "$raw" | sed -e 's/\\//g' -e 's/"//g' -e 's/\[//;s/\]//' -e 's/, */+/g')"
  [ -n "$key" ] || key="~"
  printf '%s %s %s' "$key" "$kind" "$prof"
}

# Parse find output lines "filename mtime path" into
# "crate hash mtime path", skipping files that do not match the strict
# cargo artifact shape. Only .rlib/.rmeta/.so files carry a lib prefix.
artifact_rows() {
  local fname mtime path rest crate hash
  while read -r fname mtime path; do
    [ -n "${fname:-}" ] || continue
    case "$fname" in
      lib*.rlib|lib*.rmeta|lib*.so)
        rest="${fname#lib}"
        crate="${rest%-*}"
        hash="${rest##*-}"
        hash="${hash%%.*}"
        ;;
      *)
        crate="${fname%-*}"
        hash="${fname##*-}"
        hash="${hash%%.*}"
        ;;
    esac
    [[ "$crate" =~ ^[A-Za-z0-9_.-]+$ ]] || continue
    [[ "$hash" =~ ^[0-9a-f]{16}$ ]] || continue
    printf '%s %s %s %s\n' "$crate" "$hash" "$mtime" "$path"
  done
}

# Count groups with more than one distinct hash in a deps dir (N74).
# Prints a single number. Files that do not match the cargo artifact
# pattern are ignored, never deleted.
count_dup_crates() {
  local deps_dir="$1"
  [ -d "$deps_dir" ] || { echo 0; return 0; }
  local fp_dir="$(dirname "$deps_dir")/.fingerprint"
  local raw grouped
  raw="$(mktemp)"
  grouped="$(mktemp)"
  find "$deps_dir" -maxdepth 1 -type f -printf '%f %T@ %p\n' 2>/dev/null | artifact_rows > "$raw"
  local crate hash mtime path version info
  while read -r crate hash mtime path; do
    version="$(artifact_version "$crate" "$hash" "$deps_dir")"
    info="$(artifact_unit_info "$crate" "$hash" "$fp_dir")"
    printf '%s %s %s %s\n' "$crate" "$version" "$info" "$hash"
  done < "$raw" | sort -u > "$grouped"
  rm -f "$raw"
  awk '{ c[$1" "$2" "$3" "$4" "$5]++ } END { d=0; for (k in c) if (c[k] > 1) d++; print d+0 }' "$grouped"
  rm -f "$grouped"
}

# Prune one profile dir (release or debug) under the worktree target
# dir: keep the newest-hash artifacts per (crate, version, features,
# unit-kind, profile) group, delete older hashes in each group.
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
  local raw="$LOGDIR/prune-$(basename "$profile_dir").raw"
  local table="$LOGDIR/prune-$(basename "$profile_dir").list"
  local deletelist="$LOGDIR/prune-$(basename "$profile_dir").delete"
  : > "$table"
  : > "$deletelist"
  # Rows are "crate version features kind profile hash mtime path",
  # sorted so the newest-mtime hash per group comes first.
  find "$deps_dir" -maxdepth 1 -type f -printf '%f %T@ %p\n' 2>/dev/null \
    | artifact_rows > "$raw"
  local crate hash mtime path version info
  while read -r crate hash mtime path; do
    version="$(artifact_version "$crate" "$hash" "$deps_dir")"
    info="$(artifact_unit_info "$crate" "$hash" "$fp_dir")"
    printf '%s %s %s %s %s %s\n' "$crate" "$version" "$info" "$hash" "$mtime" "$path"
  done < "$raw" | sort -k1,1 -k2,2 -k3,3 -k4,4 -k5,5 -k7,7nr -k6,6 > "$table"
  rm -f "$raw"
  # First row per group (newest mtime) names the kept hash; every later
  # row with a different hash is deleted. Output rows are
  # "crate hash path"; the path is last so read assigns the remainder
  # of the line to it even if it contains spaces.
  awk '{ key=$1 SUBSEP $2 SUBSEP $3 SUBSEP $4 SUBSEP $5; if (!(key in keep)) keep[key]=$6; if ($6 != keep[key]) { p=$8; for (i=9;i<=NF;i++) p=p" "$i; print $1" "$6" "p } }' "$table" > "$deletelist"
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
    [ -n "${fp_hash:-}" ] || continue
    [[ "$fp_hash" =~ ^[0-9a-f]{16}$ ]] || continue
    # Fingerprint dirs are named by package name (dashes), not by the
    # deps filename (underscores), so find them by hash. N70 looked
    # only for <crate>-<hash> with the deps token and silently missed
    # every dashed crate.
    for fp_path in "$fp_dir"/*-"$fp_hash"; do
      [ -d "$fp_path" ] || continue
      rm -rf -- "$fp_path" || { echo "ERROR: prune failed to delete $fp_path" >&2; return 1; }
      fp_deleted=$((fp_deleted + 1))
    done
  done < <(sort -u "$pairs")
  after_bytes="$(du -sb "$deps_dir" | cut -f1)"
  after_dups="$(count_dup_crates "$deps_dir")"
  echo "prune: $profile_dir: deps $before_bytes -> $after_bytes bytes (freed $((before_bytes - after_bytes))), dup groups $before_dups -> $after_dups, files deleted $deleted, fingerprint dirs deleted $fp_deleted" | tee -a "$SUMMARY"
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
# --- N74 prune helpers (end) ---

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
