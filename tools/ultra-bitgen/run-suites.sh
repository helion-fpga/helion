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
# Usage: run-suites.sh [WORKTREE] [LOGDIR]
# Defaults mirror the prior lane scripts.
set -u
export PATH="$HOME/.cargo/bin:$PATH:$HOME/workspace/oss-cad-suite/bin"
WT="${1:-$HOME/workspace/helion-n67n68fix-impl}"
LOGDIR="${2:-$HOME/workspace/n67n68fix-logs/impl}"
SUMMARY="$LOGDIR/suite-summary.txt"

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
