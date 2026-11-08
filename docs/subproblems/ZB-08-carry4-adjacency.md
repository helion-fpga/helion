# ZB-08: CARRY4 vertical adjacency in placement

- Track: ZedBoard, backend. Investigator finding C6.
- Branch: none (no fix committed anywhere). Worktree:
  ~/workspace/helion-zed-bringup.
- Status: not started. Depends on ZB-04 for anything to place.

## Goal

CARRY4 cells must be placed in vertically adjacent slices in the same
column, so the carry-out of one drives the carry-in of the next.

## Done when

A design with a multi-bit adder or counter has its CARRY4s in a
contiguous vertical chain in the same column (verified in the placed
netlist), and test t03 passes on the board.

## What is already done

- Nothing except the finding. Investigator C6: place.rs lines 167-178
  places each CARRY4 on a free slice via `slice_near(ax, ay, &used_sites)`.
  Carry chains must be vertically adjacent in the same column (CO of one
  drives CI of the next). Placing them on random free slices breaks the
  chain. Even with correct FASM, the physical chain is broken.

## What was tried and what failed

- Nothing tried. The finding is untouched since Oct 22, 2026. Note that
  there is currently nothing to place: helion-sv never creates Carry4
  cells (see ZB-04), so the placer has never seen a CARRY4 in a real
  design.

## What to do

1. Wait for ZB-04 (inference) to produce Carry4 cells, or build the
   placer change against a synthetic placed netlist with Carry4 cells.
   Do not wait silently; if ZB-04 is not started, build against a
   synthetic netlist and say so.
2. Change the CARRY4 placement in place.rs to allocate a vertical run:
   given a chain of N CARRY4s, pick a column and N vertically adjacent
   slices in it, mark them all used, and record the chain order (which
   cell's CO drives which cell's CI).
3. If no vertical run of the needed length is free in any column, return
   an error. Never scatter the chain and hope.
4. Add a unit test: a chain of 4 CARRY4s lands in one column in adjacent
   rows, in order.
5. Verify with t03 on the board together with ZB-04.

## Files and commands

- Code: crates/helion-xilinx7/src/place.rs (lines 167-178).
- Test: ~/workspace/zedboard-hw-loop/tests/t03_carry4_counter.sv.

## Dependencies

Depends on ZB-04 (nothing to place until inference exists). The hardware
proof also depends on ZB-01 (clock). Related to ZB-07 (same used-set
discipline, plus adjacency on top).

## Rules

- No em dashes anywhere.
- Never scatter a carry chain. If no vertical run fits, fail loudly.
