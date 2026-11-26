#!/bin/bash
# Synthetic test for the N76/N77/N78 prune fix in
# tools/ultra-bitgen/run-suites.sh.
# Builds a fake target tree (no cargo invocation, no real build) and
# proves, using the exact prune functions shipped in run-suites.sh
# (extracted by marker, never copied by hand):
#  (a) same 5-field key, different deps vector: BOTH units survive the
#      profile prune (N77/N78 universe pairs).
#  (b) same 5-field key AND same deps vector, older mtime: the older
#      unit is deleted, the newer kept (genuinely stale rebuild). This
#      also guards the "local" exclusion: the two units differ only in
#      the own-hash local dep_info path, so including "local" in the key
#      would wrongly keep both.
#  (c) a true orphan fingerprint dir (hash nowhere in deps/, build/,
#      examples/, kind lib) is deleted by the orphan sweep (N76).
#  (d) build-script, run-build-script, and example fingerprint dirs
#      survive the sweep even though their hashes are absent from
#      deps/ (and, in this tree, from build/ and examples/ too, so only
#      the kind guard saves them). The build-script stem uses the trap
#      form build-script-build-script-build.
#  (e) a hash-only sweep would wrongly delete 4 release dirs while the
#      safe rule deletes 1 (and 1 vs 0 in debug).
# Exit 0 on PASS, 1 on the first FAIL. No em dashes in this file.
set -u

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
RUN_SUITES="$SCRIPT_DIR/../ultra-bitgen/run-suites.sh"
[ -f "$RUN_SUITES" ] || { echo "FAIL: run-suites.sh not found at $RUN_SUITES"; exit 1; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
LOGDIR="$WORK/logs"
mkdir -p "$LOGDIR"
SUMMARY="$LOGDIR/suite-summary.txt"
: > "$SUMMARY"

# Source the exact shipped prune functions (marker-delimited block).
sed -n '/prune helpers (begin)/,/prune helpers (end)/p' "$RUN_SUITES" > "$WORK/prune-fns.sh"
grep -q "artifact_deps_closure" "$WORK/prune-fns.sh" || { echo "FAIL: extracted block has no artifact_deps_closure"; exit 1; }
grep -q "sweep_fingerprint_orphans" "$WORK/prune-fns.sh" || { echo "FAIL: extracted block has no sweep_fingerprint_orphans"; exit 1; }
# shellcheck disable=SC1090
. "$WORK/prune-fns.sh"

expect_exists() {
  if [ -e "$1" ]; then echo "PASS: exists: $2"; else echo "FAIL: missing: $2 ($1)"; exit 1; fi
}
expect_gone() {
  if [ ! -e "$1" ]; then echo "PASS: gone: $2"; else echo "FAIL: still present: $2 ($1)"; exit 1; fi
}
expect_eq() {
  if [ "$1" = "$2" ]; then echo "PASS: $3 is $1"; else echo "FAIL: $3 expected $2, got $1"; exit 1; fi
}

# make_deps_unit <profile_dir> <crate> <hash> <version> <features>
#   <profile> <deps-json> <mtime>
# A lib unit with deps artifacts (.rlib, .rmeta, .d) and a fingerprint
# dir with a lib-<crate>.json in real cargo format (features as an
# escaped string, deps as a one-line array, local carrying the own hash).
make_deps_unit() {
  local profile_dir="$1" crate="$2" hash="$3" version="$4" features="$5" profile="$6" deps="$7" mtime="$8"
  local deps_dir="$profile_dir/deps" fp_dir="$profile_dir/.fingerprint"
  local unit_dir="$fp_dir/$crate-$hash"
  mkdir -p "$deps_dir" "$unit_dir"
  printf 'fake rlib\n' > "$deps_dir/lib$crate-$hash.rlib"
  printf 'fake rmeta\n' > "$deps_dir/lib$crate-$hash.rmeta"
  printf '%s: /home/hatch/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/%s-%s/src/lib.rs\n' "$deps_dir/$crate-$hash.d" "$crate" "$version" > "$deps_dir/$crate-$hash.d"
  cat > "$unit_dir/lib-$crate.json" <<EOF
{"rustc":1,"features":"$features","declared_features":"[]","target":1,"profile":$profile,"path":1,"deps":$deps,"local":[{"CheckDepInfo":{"dep_info":"release/.fingerprint/$crate-$hash/dep-lib-$crate","checksum":false}}],"rustflags":[],"config":1,"compile_kind":0}
EOF
  printf '0123456789abcdef\n' > "$unit_dir/lib-$crate"
  printf 'fake dep info\n' > "$unit_dir/dep-lib-$crate"
  : > "$unit_dir/invoked.timestamp"
  touch -d "$mtime" "$deps_dir/lib$crate-$hash.rlib" "$deps_dir/lib$crate-$hash.rmeta" "$deps_dir/$crate-$hash.d"
}

# make_fp_only_unit <profile_dir> <dir-name> <json-stem> <features>
#   <profile> <deps-json>
# A fingerprint dir with no deps artifacts (build-script,
# run-build-script, example, and true-orphan units have none).
make_fp_only_unit() {
  local profile_dir="$1" dirname="$2" stem="$3" features="$4" profile="$5" deps="$6"
  local unit_dir="$profile_dir/.fingerprint/$dirname"
  mkdir -p "$unit_dir"
  cat > "$unit_dir/$stem.json" <<EOF
{"rustc":1,"features":"$features","declared_features":"[]","target":1,"profile":$profile,"path":1,"deps":$deps,"local":[],"rustflags":[],"config":1,"compile_kind":0}
EOF
  printf '0123456789abcdef\n' > "$unit_dir/$stem"
}

REL="$WORK/target/release"
DBG="$WORK/target/debug"
mkdir -p "$REL" "$DBG"

# (a) N77/N78 universe pair: same crate/version/features/kind/profile,
# deps differ in the libc fourth value only.
make_deps_unit "$REL" democrate aaaaaaaa11111111 1.0.0 '[\"default\"]' 111 '[[111,"libc",true,999],[222,"object",false,888]]' "2026-11-26 10:00:00"
make_deps_unit "$REL" democrate bbbbbbbb22222222 1.0.0 '[\"default\"]' 111 '[[111,"libc",true,777],[222,"object",false,888]]' "2026-11-26 09:00:00"
# (b) Genuinely stale pair: same 5 fields AND same deps vector.
make_deps_unit "$REL" stalecrate cccccccc33333333 2.0.0 '[]' 222 '[[333,"cfg_if",true,444]]' "2026-11-26 10:00:00"
make_deps_unit "$REL" stalecrate dddddddd44444444 2.0.0 '[]' 222 '[[333,"cfg_if",true,444]]' "2026-11-26 09:00:00"
# (c) True orphan: kind lib, hash nowhere.
make_fp_only_unit "$REL" orphancrate-eeeeeeee55555555 lib-orphancrate '[\"default\"]' 333 '[]'
# (d) Protected kinds, hashes absent from deps/ (and from build/ and
# examples/ in this tree, so only the kind guard saves them).
make_fp_only_unit "$REL" buildcrate-ffffffff66666666 build-script-build-script-build '[]' 0 '[]'
make_fp_only_unit "$REL" runcrate-11111111aaaaaaaa run-build-script-build-script-build '[]' 0 '[]'
make_fp_only_unit "$REL" examplecrate-22222222bbbbbbbb example-my_example '[]' 0 '[]'
# Debug: one stale same-closure pair and one protected build-script dir.
make_deps_unit "$DBG" dbgcrate aaaaaaaa99999999 3.0.0 '[\"default\"]' 444 '[[555,"cfg_if",true,666]]' "2026-11-26 10:00:00"
make_deps_unit "$DBG" dbgcrate bbbbbbbb88888888 3.0.0 '[\"default\"]' 444 '[[555,"cfg_if",true,666]]' "2026-11-26 09:00:00"
make_fp_only_unit "$DBG" dbgbuild-cccccccc77777777 build-script-build-script-build '[]' 0 '[]'

# (e) What a hash-only sweep would delete on the untouched tree.
hash_only_count() {
  local profile_dir="$1"
  local tmp="$WORK/hashonly.$RANDOM"
  collect_name_hashes "$profile_dir/deps" "$tmp"
  local n=0 d h
  for d in "$profile_dir"/.fingerprint/*; do
    [ -d "$d" ] || continue
    h="${d##*-}"
    grep -qxF -- "$h" "$tmp" || n=$((n + 1))
  done
  rm -f "$tmp"
  echo "$n"
}
expect_eq "$(hash_only_count "$REL")" 4 "hash-only sweep release count"
expect_eq "$(hash_only_count "$DBG")" 1 "hash-only sweep debug count"

# Dup groups before prune: exactly the stale pairs (a) is split by deps.
expect_eq "$(count_dup_crates "$REL/deps")" 1 "release dup groups before prune"
expect_eq "$(count_dup_crates "$DBG/deps")" 1 "debug dup groups before prune"

prune_profile_dir "$REL"
prune_profile_dir "$DBG"

# (a) both universe members survive.
expect_exists "$REL/deps/libdemocrate-aaaaaaaa11111111.rlib" "(a) universe member 1 kept"
expect_exists "$REL/deps/libdemocrate-bbbbbbbb22222222.rlib" "(a) universe member 2 kept"
expect_exists "$REL/.fingerprint/democrate-aaaaaaaa11111111" "(a) universe member 1 fingerprint kept"
expect_exists "$REL/.fingerprint/democrate-bbbbbbbb22222222" "(a) universe member 2 fingerprint kept"
# (b) newer kept, older deleted with its fingerprint dir.
expect_exists "$REL/deps/libstalecrate-cccccccc33333333.rlib" "(b) newer stale-pair member kept"
expect_gone "$REL/deps/libstalecrate-dddddddd44444444.rlib" "(b) older stale-pair member deleted"
expect_gone "$REL/deps/libstalecrate-dddddddd44444444.rmeta" "(b) older rmeta deleted"
expect_gone "$REL/deps/stalecrate-dddddddd44444444.d" "(b) older dep-info deleted"
expect_gone "$REL/.fingerprint/stalecrate-dddddddd44444444" "(b) older fingerprint dir deleted"
expect_exists "$REL/.fingerprint/stalecrate-cccccccc33333333" "(b) newer fingerprint dir kept"
# Debug stale pair behaves the same.
expect_exists "$DBG/deps/libdbgcrate-aaaaaaaa99999999.rlib" "(b-debug) newer kept"
expect_gone "$DBG/deps/libdbgcrate-bbbbbbbb88888888.rlib" "(b-debug) older deleted"
expect_gone "$DBG/.fingerprint/dbgcrate-bbbbbbbb88888888" "(b-debug) older fingerprint dir deleted"
# Dup groups after prune.
expect_eq "$(count_dup_crates "$REL/deps")" 0 "release dup groups after prune"
expect_eq "$(count_dup_crates "$DBG/deps")" 0 "debug dup groups after prune"

sweep_fingerprint_orphans "$REL"
sweep_fingerprint_orphans "$DBG"

# (c) true orphan deleted; (d) protected kinds survive.
expect_gone "$REL/.fingerprint/orphancrate-eeeeeeee55555555" "(c) true orphan deleted"
expect_exists "$REL/.fingerprint/buildcrate-ffffffff66666666" "(d) build-script dir survives"
expect_exists "$REL/.fingerprint/runcrate-11111111aaaaaaaa" "(d) run-build-script dir survives"
expect_exists "$REL/.fingerprint/examplecrate-22222222bbbbbbbb" "(d) example dir survives"
expect_exists "$DBG/.fingerprint/dbgbuild-cccccccc77777777" "(d-debug) build-script dir survives"
# Kept lib dirs survive the sweep too.
expect_exists "$REL/.fingerprint/democrate-aaaaaaaa11111111" "kept lib dir survives sweep"
expect_exists "$REL/.fingerprint/stalecrate-cccccccc33333333" "kept stale-pair winner survives sweep"
# Safe-rule delete counts from the audit lists.
expect_eq "$(wc -l < "$LOGDIR/orphan-release.delete")" 1 "safe sweep release deletes"
expect_eq "$(wc -l < "$LOGDIR/orphan-debug.delete")" 0 "safe sweep debug deletes"
grep -qx "orphancrate-eeeeeeee55555555 lib" "$LOGDIR/orphan-release.delete" || { echo "FAIL: orphan delete list names the wrong dir"; exit 1; }
echo "PASS: orphan delete list names the true orphan"

echo "ALL SYNTHETIC PRUNE TESTS PASSED"
exit 0
