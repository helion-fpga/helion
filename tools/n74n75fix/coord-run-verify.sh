#!/bin/bash
# Corrected copy of the N69/N72 coordinator verify script (nit N75).
# Source: ~/workspace/n69n72fix-logs/coord/coord-run-verify.sh.
# The coordinator ran the source script on merged impl branch e1a6eecc.
# N75 correction: the c5 port map is ~/workspace/routepack-logs/impl/counter5-port-map-AQ.json,
# the path Session 52 established. The stale path
# ~/workspace/routepack-logs/impl/verify-counter5/counter5-port-map-AQ.json does not exist
# (that directory holds only counter5-rtl.v and verify outputs, no port map).
# Decoder built from this checkout (nit N34: default decoder is stale pre-N7).
# Run from tools/us09v in the checkout that holds this script.
# US09V_DECODER_BIN defaults to this checkout's target/release/helion-bitgen;
# override it in the environment to use another decoder build.
# LOGDIR defaults to ~/workspace/n74n75fix-logs/verify; override it in the
# environment to write logs elsewhere. The coord-*.bit files the script verifies
# must be present under LOGDIR (symlink or copy them there); the script does
# not rebuild flows, it only verifies the bits.
set -u
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
export US09V_DECODER_BIN="${US09V_DECODER_BIN:-$REPO_ROOT/target/release/helion-bitgen}"
export PATH="$HOME/workspace/oss-cad-suite/bin:$PATH"
LOGDIR="${LOGDIR:-$HOME/workspace/n74n75fix-logs/verify}"
SUMMARY="$LOGDIR/coord-verify-summary.txt"
: > "$SUMMARY"
vb() {
  local tag="$1" bit="$2" design="$3" rtl="$4" top="$5" pmap="$6" cycles="$7"
  local log="$LOGDIR/verify-${tag}.log"
  python3 -m us09v verify-bit --bit "$bit" --design "$design" --rtl "$rtl" \
    --top "$top" --port-map "$pmap" --workdir "$LOGDIR/verify-${tag}" --cycles "$cycles" > "$log" 2>&1
  local ec=$?
  echo "coord-${tag}: exit=${ec} $(grep -E ': (PASS|FAIL)' "$log" | head -1) | $(grep -E 'comparisons' "$log" | head -1)" | tee -a "$SUMMARY"
}
vb "c16" "$LOGDIR/coord-c16.bit" chain16 "$HOME/workspace/stadin-logs/impl/chain16-rtl.v" chain16 "$HOME/workspace/n58-logs/impl/chain16-port-map-ho.json" 400
vb "c16-r2" "$LOGDIR/coord-c16-r2.bit" chain16 "$HOME/workspace/stadin-logs/impl/chain16-rtl.v" chain16 "$HOME/workspace/n58-logs/impl/chain16-port-map-ho.json" 400
vb "s32" "$LOGDIR/coord-s32.bit" shift32 "$HOME/workspace/n58-logs/impl/shift32-rtl.v" shift32 "$HOME/workspace/n58-logs/impl/shift32-port-map.json" 400
vb "s32-r2" "$LOGDIR/coord-s32-r2.bit" shift32 "$HOME/workspace/n58-logs/impl/shift32-rtl.v" shift32 "$HOME/workspace/n58-logs/impl/shift32-port-map.json" 400
vb "c12-s0" "$LOGDIR/coord-c12-s0.bit" counter12 "$HOME/workspace/critic42-runs/counter12-rtl.v" counter12 "$HOME/workspace/clk2leg-logs/impl/counter12-port-map-AQ.json" 400
vb "c8-s0" "$LOGDIR/coord-c8-s0.bit" counter8 "$HOME/workspace/routepack-logs/impl/verify-counter8/counter8-rtl.v" counter8 "$HOME/workspace/routepack-logs/impl/counter8-port-map-AQ.json" 400
vb "c8-s1" "$LOGDIR/coord-c8-s1.bit" counter8 "$HOME/workspace/routepack-logs/impl/verify-counter8/counter8-rtl.v" counter8 "$HOME/workspace/routepack-logs/impl/counter8-port-map-AQ.json" 400
vb "c2x4" "$LOGDIR/coord-c2x4.bit" counters2x4 "$HOME/workspace/spanclk-logs/baseline/verify-counters2x4/counters2x4-rtl.v" counters2x4 "$HOME/workspace/spanclk-logs/baseline/verify-counters2x4/port-map-AQ.json" 400
vb "c5" "$LOGDIR/coord-c5.bit" counter5 "$HOME/workspace/routepack-logs/impl/verify-counter5/counter5-rtl.v" counter5 "$HOME/workspace/routepack-logs/impl/counter5-port-map-AQ.json" 400
vb "counter" "$LOGDIR/coord-counter.bit" counter "$HOME/workspace/counter-logs/impl/verify-counter/counter-rtl.v" counter "$HOME/workspace/counter-logs/impl/verify-counter/counter-port-map.json" 400
vb "blinky" "$LOGDIR/coord-blinky.bit" blinky tests/fixtures/blinky/rtl.v blinky "$HOME/workspace/emitter-xczu3eg-logs/flow2/blinky-port-map.json" 400
vb "c16-s1" "$LOGDIR/coord-c16-s1.bit" counter16 "$HOME/workspace/n47-logs/impl/counter16-rtl.v" counter16 "$HOME/workspace/n47-logs/impl/counter16-port-map.json" 40000
echo "ALL COORD VERIFY-BITS DONE" | tee -a "$SUMMARY"
