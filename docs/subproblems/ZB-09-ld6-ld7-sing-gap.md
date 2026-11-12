# ZB-09: LD6 / LD7 on RIOB33_SING tiles (known X-Ray DB gap)

- Track: ZedBoard, backend plus database. Known gap, not a code bug.
- Branch: fm/zedboard-fix-carry4 (and the other d2d067a branches) carry
  the workaround commit: "zedboard: skip SING tile FASM (DB gap,
  documented)".
- Status: known gap, workaround in place. Excluded from every pass
  criterion.

## Goal

Decide, with evidence, whether LD6 and LD7 can ever be driven by Helion,
and if so make it work. If not, keep the exclusion documented and stop
spending time on it.

## Done when

Either (a) LD6 and LD7 light up on the board from a Helion bitstream, or
(b) the gap is proven unfillable from public data, with the exact missing
data named (which segbits file, which tile type, what a Vivado probe
would have to return). "Probably unfillable" is not done.

## What is already done

- The gap is identified: LD6 (U19) and LD7 (U14) sit on RIOB33_SING
  tiles, which have no segbits in the X-Ray DB. No bitstream Helion
  produces can configure those pins today.
- Workaround committed (d2d067a, on several branches): skip SING tile
  FASM, documented as a DB gap. The blinky and the t01-t05 tests exclude
  LD6/LD7 from all pass criteria because of it.
- The bring-up note from Oct 22, 2026 says LD6 / LD7 may stay dark even
  on a correct bitstream.

## What was tried and what failed

- Nothing tried beyond the workaround. No one has attempted to find or
  derive the missing segbits.

## What to do

1. Confirm the gap precisely: which tile types are involved, which
   segbits files are missing them, and what the Vivado bitstream for the
   same design sets in those frames (see ZB-10; a Vivado bitstream that
   lights LD6/LD7 contains the answer in its frames).
2. Check the public sources for the missing data: the Project X-Ray
   zynq7 segbits DBs, any newer prjxray-db material, and the BitFiltrator
   data. If the bits exist anywhere public, vendor them with provenance
   recorded (source, license, commit hash).
3. If no public source has them, the only path is a Vivado probe on the
   cluster (same class of work as the UltraScale+ Q6 probe, US-01). Write
   down exactly what the probe must return: tile type, base FAR, frame
   count, and the feature-to-bit mapping for the RIOB33_SING output
   driver. Do not run the probe speculatively; the license is the
   constraint.
4. If the bits are found, wire them in and run the blinky: LD6 and LD7
   must light. If they are proven unfillable, update this file and
   docs/HANDOFF.md with the exact missing data and close the sub-problem
   as "blocked on Vivado data", not as "done".

## Files and commands

- Code: crates/helion-xilinx7/src/emit.rs (the SING skip), src/device.rs
  (DB loader).
- DB: ~/workspace/xilinx-dbsrc, zynq7 family; check which segbits files
  cover RIOB33_SING (none do today).
- Pin map: examples/zedboard.xdc (LD6 U19, LD7 U14).
- Related: US-01 (the same class of problem: missing tile data, Vivado
  probe as the unblocker).

## Dependencies

Depends on ZB-10 for the Vivado frame evidence. Related to US-01.

## Rules

- No em dashes anywhere.
- LD6/LD7 stay excluded from every pass criterion until this is green.
- Do not tune or invent segbits. If the bits are not in a public source
  or a real Vivado bitstream, they do not exist.
