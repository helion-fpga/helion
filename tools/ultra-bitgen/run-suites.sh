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
# --prune-target (N70, grouping revised by N74 and by N77/N78) is an
# opt-in hygiene step that runs before the suites: for each crate in
# <target>/release/deps and <target>/debug/deps it keeps the
# newest-mtime hash per (crate, version, features, unit-kind, profile,
# deps-closure) group and deletes older-hash files, plus their matching
# .fingerprint dirs (found by hash, since fingerprint dirs are named by
# package name). Deps-closure is the unit JSON deps array verbatim,
# normalized to a canonical string (see artifact_deps_closure); it
# separates the bin-vs-test and debug dependency-universe pairs, which
# share the other five fields but differ in their deps fourth-u64
# values (N77, N78). Version comes from the dep-info .d file (registry
# paths carry it); features, kind and profile come from the unit JSON in
# .fingerprint. Distinct versions of one crate, distinct feature sets
# of one version, distinct unit kinds (bin vs test-bin), distinct cargo
# profiles, and distinct dependency closures are all kept, so only
# genuinely stale same-group rebuilds are pruned. After each profile
# prune, a fingerprint-orphan sweep (N76) deletes fingerprint dirs
# whose hash is absent from deps/ filenames AND build/ dir names AND
# examples/ filenames AND whose unit kind is not build-script,
# run-build-script, or example; on every real target inspected that
# sweep deletes 0 dirs, which is the correct result, because the
# deps-orphan dirs are live build-script and example units. It deletes
# only files matching the strict cargo artifact pattern (lib<crate>-<16
# hex> or <crate>-<16 hex>, plus an extension) inside the target dir of
# this worktree, never sources, never top-level binaries, never any
# other worktree's target dir. If CARGO_TARGET_DIR is set to a directory
# outside the worktree, pruning is skipped with a clear message instead
# of touching that directory.
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

# --- N74/N76/N77/N78 prune helpers (begin) ---
# Prune grouping key (N74, extended by N77/N78): (crate, version,
# features, unit-kind, profile, deps-closure). A crate with several live
# versions (syn 0.15/1.0/2.0/3.0), several live feature sets of one
# version (memchr 2.8.3 built three ways, libc 0.2.189 built two ways),
# several live unit kinds (the helion bin plus its test-bin), several
# live cargo profiles, or several live dependency closures (the
# bin-vs-test universe pairs, e.g. serde built against two different
# serde_derive fingerprints) keeps one hash per group; only older
# same-group hashes are deleted. A "dup group" is a group with more than
# one hash. Cargo rebuilds anything it still needs, so a deleted hash
# that comes back on the next suite run was live in that build universe,
# not stale. The profile dimension is needed because cargo builds the
# same crate under different profiles in one target dir (verified: the 9
# release hashes the first version of this fix pruned were all rebuilt
# by the suites, and every rebuilt pair differs in profile; the debug
# pairs share a profile and differ only in their dependency
# fingerprints, so they are a known residual, see the lane report).
# The deps-closure dimension is needed because the debug pairs (N77)
# and the release bin-vs-test pairs (N78) share all five other fields
# and differ only in the fourth u64 of one or more deps entries. The
# deps vector needs no resolution to unit hashes (no file on disk maps
# a fourth-u64 value to a unit hash); transitive differences propagate
# into the direct vector, so the raw vector separates every observed
# universe pair (verified: 54 release and 3 debug dup groups on the
# N74/N75 targets, 0 sharing a deps vector). Never key on the sibling
# fingerprint-hash file (the bare 16-hex file named after the JSON
# stem): it differs between every pair member but is a hash of the whole
# fingerprint, so keying on it would split stale rebuilds whose dep
# closure is unchanged and the prune would stop deleting stale hashes.
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

# Prints the canonical deps-closure string for one unit JSON (N77/N78):
# the JSON "deps" array verbatim, normalized by sorting the entries and
# rendering each as <first-u64>:<dep-name>:<public-bool>:<fourth-u64>,
# joined with commas. Commas and colons cannot occur in a dep name
# (cargo target names are [A-Za-z0-9_-]), so the string is unambiguous
# and contains no spaces, which keeps it a single field in the prune
# tables. The JSON "local" field is EXCLUDED: its dep_info path carries
# the unit own hash, so including it would make every unit its own
# group. A known-empty deps array prints "~" (same convention as
# known-empty features); an unreadable or malformed JSON prints "-".
# Extracted with python3 reading the JSON, not by hand-parsing fourth
# values with grep: the array is one line and the entries are
# positional. Prints "-" on any failure.
artifact_deps_closure() {
  local json="$1"
  local out
  out="$(python3 -c 'import json,sys
try:
    d=json.load(open(sys.argv[1]))
    p=sorted("%s:%s:%s:%s"%(e[0],e[1],str(e[2]).lower(),e[3]) for e in (d.get("deps") or []))
    print(",".join(p) if p else "~")
except Exception:
    sys.exit(1)' "$json" 2>/dev/null)" || { printf '%s' "-"; return 0; }
  [ -n "$out" ] || out="-"
  printf '%s' "$out"
}

# Prints "features kind profile deps-closure" for one (crate, hash)
# unit, from the unit JSON in its fingerprint dir (lib-<crate>.json,
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
    printf '%s %s %s %s' "-" "-" "-" "-"
    return 0
  fi
  local stem kind deps
  stem="$(basename "$json" .json)"
  kind="${stem%-"$crate"}"
  deps="$(artifact_deps_closure "$json")"
  local raw key prof
  raw="$(sed -n 's/.*"features":"\(\[[^]]*\]\)".*/\1/p' "$json" | head -n1)"
  prof="$(sed -n 's/.*"profile":\([0-9][0-9]*\).*/\1/p' "$json" | head -n1)"
  [ -n "$prof" ] || prof="-"
  if [ -z "$raw" ]; then
    printf '%s %s %s %s' "-" "$kind" "$prof" "$deps"
    return 0
  fi
  key="$(printf '%s' "$raw" | sed -e 's/\\//g' -e 's/"//g' -e 's/\[//;s/\]//' -e 's/, */+/g')"
  [ -n "$key" ] || key="~"
  printf '%s %s %s %s' "$key" "$kind" "$prof" "$deps"
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
  awk '{ c[$1" "$2" "$3" "$4" "$5" "$6]++ } END { d=0; for (k in c) if (c[k] > 1) d++; print d+0 }' "$grouped"
  rm -f "$grouped"
}

# Prune one profile dir (release or debug) under the worktree target
# dir: keep the newest-hash artifacts per (crate, version, features,
# unit-kind, profile, deps-closure) group, delete older hashes in each group.
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
  # Rows are "crate version features kind profile deps-closure hash
  # mtime path", sorted so the newest-mtime hash per group comes first.
  find "$deps_dir" -maxdepth 1 -type f -printf '%f %T@ %p\n' 2>/dev/null \
    | artifact_rows > "$raw"
  local crate hash mtime path version info
  while read -r crate hash mtime path; do
    version="$(artifact_version "$crate" "$hash" "$deps_dir")"
    info="$(artifact_unit_info "$crate" "$hash" "$fp_dir")"
    printf '%s %s %s %s %s %s %s\n' "$crate" "$version" "$info" "$hash" "$mtime" "$path"
  done < "$raw" | sort -k1,1 -k2,2 -k3,3 -k4,4 -k5,5 -k6,6 -k8,8nr -k7,7 > "$table"
  rm -f "$raw"
  # First row per group (newest mtime) names the kept hash; every later
  # row with a different hash is deleted. Output rows are
  # "crate hash path"; the path is last so read assigns the remainder
  # of the line to it even if it contains spaces.
  awk '{ key=$1 SUBSEP $2 SUBSEP $3 SUBSEP $4 SUBSEP $5 SUBSEP $6; if (!(key in keep)) keep[key]=$7; if ($7 != keep[key]) { p=$9; for (i=10;i<=NF;i++) p=p" "$i; print $1" "$7" "p } }' "$table" > "$deletelist"
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

# Map a fingerprint JSON stem to its unit kind by matching the fixed
# kind prefixes in longest-first order (N76). The case patterns are
# tried in order, so the longest match wins: a build-script stem is
# "build-script-build-script-build" (kind build-script, target
# build-script-build), and a run-build-script stem is
# "run-build-script-build-script-build"; a naive split on the first
# hyphen misreads both. Unknown stems print "-".
fingerprint_unit_kind() {
  local stem="$1"
  case "$stem" in
    run-build-script-*) printf '%s' "run-build-script" ;;
    build-script-*) printf '%s' "build-script" ;;
    test-integration-test-*) printf '%s' "test-integration-test" ;;
    test-bin-*) printf '%s' "test-bin" ;;
    test-lib-*) printf '%s' "test-lib" ;;
    lib-*) printf '%s' "lib" ;;
    bin-*) printf '%s' "bin" ;;
    example-*) printf '%s' "example" ;;
    *) printf '%s' "-" ;;
  esac
}

# Collect the 16-hex hashes that occur as a -<16 hex> token (followed
# by a dot or end of name) in the names under one dir, one per line,
# sorted unique. Missing dir means an empty set.
collect_name_hashes() {
  local dir="$1" out="$2"
  : > "$out"
  [ -d "$dir" ] || return 0
  local n
  for n in "$dir"/*; do
    [ -e "$n" ] || continue
    n="$(basename "$n")"
    if [[ "$n" =~ -([0-9a-f]{16})(\.|$) ]]; then
      printf '%s\n' "${BASH_REMATCH[1]}" >> "$out"
    fi
  done
  sort -u -o "$out" "$out"
}

# Fingerprint-orphan sweep (N76): delete a fingerprint dir ONLY if all
# four parts of the rule hold: its hash is absent from <profile>/deps/
# filenames AND absent from <profile>/build/ dir names AND absent from
# <profile>/examples/ filenames AND its JSON stem kind is not
# build-script, run-build-script, or example. Never delete on
# deps-absence alone: build-script and run-build-script units have no
# deps artifact (their compiled scripts and outputs live in
# <profile>/build/<crate>-<hash>/), and example units have no deps
# artifact either (their binaries live in <profile>/examples/), so a
# hash-only sweep would delete 73 live release dirs (57,898 bytes) and
# 50 live debug dirs on the reference target and force cargo to rebuild
# them. The kind check is defense in depth: a dir of one of those kinds
# is live by construction even if the build/ or examples/ listing were
# ever stale. Dirs with no JSON or an unknown kind are never deleted.
# On every real target inspected this sweep deletes 0 dirs and 0 bytes,
# which is the CORRECT result: N76 as filed is a mislabelled live set,
# not dead accumulation. Deleted dirs are listed in
# $LOGDIR/orphan-<profile>.delete for audit.
sweep_fingerprint_orphans() {
  local profile_dir="$1"
  local fp_dir="$profile_dir/.fingerprint"
  if [ ! -d "$fp_dir" ]; then
    echo "orphan-sweep: $profile_dir: no .fingerprint dir, nothing to sweep" | tee -a "$SUMMARY"
    return 0
  fi
  local tmp
  tmp="$(mktemp -d)"
  local deps_hashes="$tmp/deps.hashes"
  local build_hashes="$tmp/build.hashes"
  local ex_hashes="$tmp/examples.hashes"
  collect_name_hashes "$profile_dir/deps" "$deps_hashes"
  collect_name_hashes "$profile_dir/build" "$build_hashes"
  collect_name_hashes "$profile_dir/examples" "$ex_hashes"
  local list="$LOGDIR/orphan-$(basename "$profile_dir").delete"
  : > "$list"
  local deleted=0 freed=0
  local dir name hash json j stem kind bytes
  for dir in "$fp_dir"/*; do
    [ -d "$dir" ] || continue
    name="$(basename "$dir")"
    hash="${name##*-}"
    [[ "$hash" =~ ^[0-9a-f]{16}$ ]] || continue
    grep -qxF -- "$hash" "$deps_hashes" && continue
    grep -qxF -- "$hash" "$build_hashes" && continue
    grep -qxF -- "$hash" "$ex_hashes" && continue
    json=""
    for j in "$dir"/*.json; do
      [ -f "$j" ] || continue
      json="$j"
      break
    done
    [ -n "$json" ] || continue
    stem="$(basename "$json" .json)"
    kind="$(fingerprint_unit_kind "$stem")"
    case "$kind" in
      build-script|run-build-script|example|-) continue ;;
    esac
    bytes="$(du -sb "$dir" | cut -f1)"
    rm -rf -- "$dir" || { echo "ERROR: orphan sweep failed to delete $dir" >&2; rm -rf "$tmp"; return 1; }
    echo "$name $kind" >> "$list"
    deleted=$((deleted + 1))
    freed=$((freed + bytes))
  done
  rm -rf "$tmp"
  echo "orphan-sweep: $profile_dir: fingerprint dirs deleted $deleted, bytes freed $freed" | tee -a "$SUMMARY"
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
  sweep_fingerprint_orphans "$target_dir/release" || return 1
  prune_profile_dir "$target_dir/debug" || return 1
  sweep_fingerprint_orphans "$target_dir/debug" || return 1
  return 0
}
# --- N74/N76/N77/N78 prune helpers (end) ---

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
