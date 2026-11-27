# coord-fixtures.sh: single source of truth for the coordinator design set.
#
# N65 fix: the coord flow script and the coord verify script used to carry
# their own copies of this design set, with the fixture paths duplicated
# inline per call. A path edited in one script did not change the other, so
# the two scripts could silently disagree about which design they ran
# (Critic Session 42, N65; recurred in Sessions 47-55). This file is the
# one list both scripts source and iterate over. Change a path here and
# both scripts see the same change.
#
# Sourced by coord-run-flows.sh and coord-run-verify.sh. The calling script
# must set the lane variables below before sourcing this file. The list
# itself hardcodes no lane worktree path, so a future lane reuses it by
# setting the same variables to its own worktree, binary, and log dir.
#
# Required variables, set by the calling script:
#   COORD_WORKTREE      lane worktree root
#                       (this lane: $HOME/workspace/helion-n76n77n78fix-coord)
#   COORD_FIXDIR        flow .sv fixture dir
#                       (this lane: $HOME/workspace/intcols-logs/impl)
#   COORD_CRITIC42_DIR  critic42 runs dir, home of counter12.sv
#                       (this lane: $HOME/workspace/critic42-runs)
# The binary path and the log dir are variables the calling scripts set
# themselves (BIN and LOGDIR); this list never references them.
#
# One record per design, in run order. Fields, pipe separated:
#   tag | flow .sv | impl seed | flow label | design | verify RTL .v |
#   top | port map | cycles
# - tag: short tag used in log, .bit, .fasm, and summary file names.
# - flow .sv: SystemVerilog source for `helion impl`.
# - impl seed: seed passed to `helion impl`.
# - flow label: label printed in the flow summary line (differs from the
#   design name for c16-r2, whose label is chain16-r2).
# - design: design name passed to verify-bit --design.
# - verify RTL .v: RTL passed to verify-bit --rtl. The blinky entry keeps
#   its relative path tests/fixtures/blinky/rtl.v, which resolves relative
#   to tools/us09v, the verify script's working directory. Do not make it
#   absolute; the relative form is the preserved value.
# - top: top module passed to verify-bit --top (N34/N37 fail-loud rule:
#   --top is always passed).
# - port map: JSON passed to verify-bit --port-map.
# - cycles: cycles passed to verify-bit --cycles. c16-s1 uses 40000, every
#   other design uses 400. Preserved exactly.

: "${COORD_WORKTREE:?coord-fixtures.sh: COORD_WORKTREE must be set by the calling script}"
: "${COORD_FIXDIR:?coord-fixtures.sh: COORD_FIXDIR must be set by the calling script}"
: "${COORD_CRITIC42_DIR:?coord-fixtures.sh: COORD_CRITIC42_DIR must be set by the calling script}"

COORD_FIXTURES=(
  "c16|$COORD_FIXDIR/chain16.sv|0|chain16|chain16|$HOME/workspace/stadin-logs/impl/chain16-rtl.v|chain16|$HOME/workspace/n58-logs/impl/chain16-port-map-ho.json|400"
  "c16-r2|$COORD_FIXDIR/chain16.sv|0|chain16-r2|chain16|$HOME/workspace/stadin-logs/impl/chain16-rtl.v|chain16|$HOME/workspace/n58-logs/impl/chain16-port-map-ho.json|400"
  "s32|$COORD_FIXDIR/shift32.sv|0|shift32|shift32|$HOME/workspace/n58-logs/impl/shift32-rtl.v|shift32|$HOME/workspace/n58-logs/impl/shift32-port-map.json|400"
  "s32-r2|$COORD_FIXDIR/shift32.sv|0|shift32|shift32|$HOME/workspace/n58-logs/impl/shift32-rtl.v|shift32|$HOME/workspace/n58-logs/impl/shift32-port-map.json|400"
  "c12-s0|$COORD_CRITIC42_DIR/counter12.sv|0|counter12|counter12|$HOME/workspace/critic42-runs/counter12-rtl.v|counter12|$HOME/workspace/clk2leg-logs/impl/counter12-port-map-AQ.json|400"
  "c8-s0|$COORD_FIXDIR/counter8.sv|0|counter8|counter8|$HOME/workspace/routepack-logs/impl/verify-counter8/counter8-rtl.v|counter8|$HOME/workspace/routepack-logs/impl/counter8-port-map-AQ.json|400"
  "c8-s1|$COORD_FIXDIR/counter8.sv|1|counter8|counter8|$HOME/workspace/routepack-logs/impl/verify-counter8/counter8-rtl.v|counter8|$HOME/workspace/routepack-logs/impl/counter8-port-map-AQ.json|400"
  "c2x4|$COORD_FIXDIR/counters2x4.sv|0|counters2x4|counters2x4|$HOME/workspace/spanclk-logs/baseline/verify-counters2x4/counters2x4-rtl.v|counters2x4|$HOME/workspace/spanclk-logs/baseline/verify-counters2x4/port-map-AQ.json|400"
  "c5|$COORD_FIXDIR/counter5.sv|0|counter5|counter5|$HOME/workspace/routepack-logs/impl/verify-counter5/counter5-rtl.v|counter5|$HOME/workspace/routepack-logs/impl/counter5-port-map-AQ.json|400"
  "counter|$COORD_WORKTREE/examples/counter.sv|0|counter|counter|$HOME/workspace/counter-logs/impl/verify-counter/counter-rtl.v|counter|$HOME/workspace/counter-logs/impl/verify-counter/counter-port-map.json|400"
  "blinky|$COORD_WORKTREE/examples/blinky.sv|0|blinky|blinky|tests/fixtures/blinky/rtl.v|blinky|$HOME/workspace/emitter-xczu3eg-logs/flow2/blinky-port-map.json|400"
  "c16-s1|$COORD_FIXDIR/counter16.sv|1|counter16|counter16|$HOME/workspace/n47-logs/impl/counter16-rtl.v|counter16|$HOME/workspace/n47-logs/impl/counter16-port-map.json|40000"
)
