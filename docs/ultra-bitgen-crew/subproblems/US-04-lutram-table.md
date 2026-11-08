# US-04: LUTRAM table

- Track: UltraScale+, bitgen.
- Branch: fm/ultra-bitstream. Worktree:
  ~/workspace/helion-ultra-bitstream.
- Status: BLOCKED. No LUTRAM table exists in any public source.

## Goal

Encode LUT RAM so designs that use distributed RAM can diff to zero.

## Done when

LUTRAM reads green on a golden design that uses LUT RAM, or it is proven
with evidence that only Vivado logic-location data can supply the table,
with the exact missing data named.

## What is already done

- Nothing. LUTRAM is blocked, separate from LUT_INIT, in the block
  registry (INTERFACES.md). No public source has a LUTRAM table: the
  prjuray-db reference clone has no UltraScale+ reference.bit files,
  and BitFiltrator's ULTRASCALE_PLUS.json does not document LUTRAM bit
  locations.

## What was tried and what failed

- Nothing tried. No table has been built or sought beyond the public
  sources already searched for the tilegrid (US-01); those sources do not
  have LUTRAM either.

## What to do

1. Check whether the LUTRAM bit locations can be derived from the LUT_INIT
   encoding plus the LUTRAM mode bits already in the public DBs. If yes,
   build the table, vendor it under crates/helion-ultrascale/data/ with
   provenance, and diff a LUTRAM golden to zero.
2. If not, the only path is Vivado logic-location data (a Vivado run that
   reports where each LUTRAM bit lands). Write down exactly what data is
   needed and which Vivado command produces it. Do not run it
   speculatively; the license is the constraint (see US-08).
3. Until the table exists, a design using LUTRAM must fail loud with a
   clear error, never emit guessed bits.
4. If the table is proven unfillable without Vivado data, record the exact
   missing data and close this sub-problem as "blocked on Vivado data",
   not as "done".

## Files and commands

- Contracts: docs/ultra-bitgen-crew/INTERFACES.md (block registry).
- Code: crates/helion-ultrascale/src/ (bitgen.rs, segbits.rs, bitloc.rs).
- Harness: crates/helion-ultrascale/harness/fasm.py (classify).
- Related: US-01 (same class: missing public data, Vivado as unblocker).

## Dependencies

Blocked on public data or the Vivado license. Unblocks LUTRAM coverage
in US-07 (the random generator).

## Rules

- No em dashes anywhere.
- Never invent bits. Fail loud on missing encodings.
- "Probably unfillable" is not done; name the exact missing data.
