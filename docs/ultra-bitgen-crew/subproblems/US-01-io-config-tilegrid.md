# US-01: IO_CONFIG tilegrid and the HPIO_RIGHT vs HPIO_L question

- Track: UltraScale+, bitgen. Largest blocked block (55 bits).
- Branch: fm/ultra-bitstream (base) plus fm/ultra-bitgen-tilegrid
  (a63f49c, the INT rule fix). Worktree:
  ~/workspace/helion-ultra-bitstream.
- Status: BLOCKED. No tilegrid exists in any public source. The unblocker
  is the Q6 Vivado probe, which needs the license or the cluster.

## Goal

Produce the xcku3p tilegrid: the map from tile names to absolute frame
addresses, so IO_CONFIG bits can be emitted and diffed. And settle
whether the part has HPIO_RIGHT tiles at all.

## Done when

IO_CONFIG reads green on both designs (blinky and counter) with no other
block regressed, and the HPIO_RIGHT vs HPIO_L question is answered with
evidence from a real Vivado probe or a public source, not a guess.

## What is already done

- The tilegrid research report (BRIEFS/tilegrid-research.md, 1,173 lines,
  0 em dashes) proves there is no xcku3p tilegrid in any public source:
  BitFiltrator has none for any part; prjuray-db covers xczu3eg only;
  xcku3p.json is a column-major index with no tile names and no HPIO/HDIO/
  CMT columns.
- New flag from that research: the device file lists HPIO_L, not
  HPIO_RIGHT. HPIO_RIGHT occurs 0 times in BitFiltrator's xcku3p.json and
  ULTRASCALE_PLUS.json; HPIO_L occurs 7 times. If HPIO_RIGHT does not
  exist on xcku3p, IO_CONFIG needs a new HPIO_L fuzzing campaign, not
  just a tilegrid.
- Grounded fix merged inside the same work (commit a63f49c, merge
  060b230): the type-dependent INT rule in bitgen_device.rs int_col_major
  (+1 branch, -1 restricted to CLEL_R) and harness reference.py
  (int_col_offset_from_clb). Researcher finding: CLEL_R serves INT at
  column -1, the other three CLB types at column +1 (452 CLB columns
  checked: 208 left-only, all CLEL_R; 244 right-only, all other types; 0
  both, 0 neither). Critic session 16 PASS, no defects, independently
  reproduced the table from raw JSON and cross-checked the xczu3eg
  tilegrid (8,820 CLB tiles, zero exceptions). 153 tests pass; harness run
  byte-identical to runs/srval-complete.
- The HPIO bridge is the current workaround: `HpioProvisionalFallback` in
  crates/helion-ultrascale/src/bin/helion-bitgen.rs maps HPIO_RIGHT_X0Y0
  to tile type HPIO_RIGHT, base FAR 0x000, an INVENTED value pinned by a
  test, never ground truth. It exists because the library DeviceFile impl
  returns None for HPIO tiles but the v1 goldens carry HPIO_RIGHT_X0Y0
  lines; literal deletion fails emit with UnknownTile and regresses every
  green block to blocked (critic session 14 reproduced this). It dies when
  the tilegrid lands.

## What was tried and what failed

- Every public source was searched: BitFiltrator, prjuray-db, the
  BitFiltrator ULTRASCALE_PLUS.json, the xcku3p.json device file. No
  tilegrid exists for xcku3p anywhere public. The search is closed; do
  not repeat it.
- The INT column rule was the one grounded fix that came out of the
  tilegrid work. The tilegrid itself stays blocked.

## What to do

1. Run the Q6 Vivado probe (the exact missing data, from the research
   report): step 1, tile inventory (extract_device_info.tcl plus
   `TILE_TYPE =~ HPIO*` arbiter queries, which alone settles HPIO_RIGHT
   vs HPIO_L); step 2, one row-0 HPIO tile base FAR plus frame count;
   step 3, the offset ruling. It cannot run from this VM; it needs Vivado
   on the cluster or a working license checkout (see the license section
   in docs/HANDOFF.md; port 27000 answered Nov 8, checkout unverified).
2. If step 1 says HPIO_RIGHT does not exist on xcku3p, stop and re-scope:
   IO_CONFIG needs an HPIO_L fuzzing campaign, and the goldens' HPIO_RIGHT
   lines need re-examination. Do not force the tilegrid to fit HPIO_RIGHT.
3. If the probe returns tile data, build the tilegrid, wire it into the
   DeviceFile impl, delete the HPIO bridge, and re-run the full harness.
   IO_CONFIG must read green on both designs.
4. If the probe cannot run, this sub-problem stays blocked and the
   blocker is recorded with the exact missing data. Do not invent tile
   data.

## Files and commands

- Research: docs/ultra-bitgen-crew/BRIEFS/tilegrid-research.md.
- Code: crates/helion-ultrascale/src/bitgen_device.rs (int_col_major),
  src/device_file.rs, src/bin/helion-bitgen.rs (HpioProvisionalFallback).
- Harness: crates/helion-ultrascale/harness/reference.py
  (int_col_offset_from_clb), diff.py.
- Probe scripts: ~/workspace/your_files/vivado-fuzzing-instructions.md,
  vivado-fuzzing-package.zip (job 1 is the xcvu3p device file; the Q6
  probe is the xcku3p equivalent).
- Data: crates/helion-ultrascale/data/ (vendored equivalents of the
  reference data; the loader never reads /tmp).

## Dependencies

Blocked on the Vivado license / cluster (Q6 probe). Unblocks IO_CONFIG
green and kills the HPIO bridge. Related to ZB-09 (same class: missing
tile data, Vivado probe as unblocker).

## Rules

- No em dashes anywhere.
- Never invent tile data. A provisional value must be flagged
  `provisional: true` with its source, and must die when real data lands.
- A fix that regresses a green block is reverted on the spot.
- Never claim Vivado-equivalence without a real Vivado diff at zero.
