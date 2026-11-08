# ZB-12: Remaining investigator findings (HIGH and MEDIUM cleanup)

- Track: ZedBoard, backend. Investigator findings H2, H4, H5, M1, M2,
  M3, M4, M5, L1, L2, L3. The leftovers after the critical findings got
  their own sub-problems.
- Branch: none. Worktree: ~/workspace/helion-zed-bringup.
- Status: not started. Lowest priority of the ZedBoard track; do the
  critical findings first.

## Goal

Close every remaining finding from the Oct 22, 2026 line-by-line
investigation, or record with evidence why a finding is benign.

## Done when

Each finding below is either fixed (with a test) or closed as benign with
a written reason and the evidence for it. "Looked at it" is not done.

## The findings

- H2 (emit.rs:280-315): `mux_for_ff` is a guess. It selects the FFMUX
  input (AX/BX/CX/DX) based on BEL letter pairing, and the comment says
  "the router/emitter can refine". If the LUT is not on the same BEL
  letter, the mux select is wrong and FF.D is disconnected. Verify
  against hardware or the Vivado diff (ZB-10). Silent failure if wrong.
- H4 (place.rs:216): `site_tile` may return an empty string.
  `site_tile.get(site).cloned().unwrap_or_default()` produces an invalid
  placement with an empty tile name if a site is reused but not in the
  map. Make it an error, never a default.
- H5 (route.rs:165-167): undriven nets silently dropped
  (`if loads.is_empty() { continue; }`). Likely benign, but it masks
  design issues. Decide with evidence: log it or error on it.
- M1 (route.rs:175-177): `is_clock_net` trusts the device query blindly.
  If xdevice.rs has a bug in clock_nets registration, non-clock nets use
  the clock spine. Verify the registration (backend.rs:62-67) or add a
  check that the net is actually driven by a BUFG.
- M2 (emit.rs:340): the unit test `routed_blinky` has empty bufgs, so the
  BUFG code path is untested. Partially covered since: the bufg_tieoff_paths
  test (9 pips) and the FF control verification (9b1c15f) now exercise
  BUFG-adjacent paths. Verify the coverage gap is actually closed, and
  add a BUFG emission assertion to the unit test if it is not.
- M3 (frames.rs:253-258): `canonical_feature` strips leading zeros
  (`FOO[00]` becomes `FOO[0]`). Probably fine, but unverified against the
  DB. Check whether the X-Ray DB ever distinguishes `FOO[00]` from
  `FOO[0]`; if it does, fix the stripping.
- M4 (timing.rs:1-12): the timing model is preliminary (Manhattan distance
  times hop delay). The doc admits it is not Vivado-correlated signoff
  STA. This sub-problem does not ask for a real timing model; it asks for
  the preliminary status to be recorded wherever timing numbers are shown,
  so no one mistakes them for signoff. A real timing DB is US-09's job.
- M5 (pack.rs:268-270): `insert_clock_spine` only handles FF clocks. If
  a clock drives only BRAM, DSP, or combinational logic (no FFs), no BUFG
  is inserted and the clock stays on general routing. Decide whether that
  is correct behaviour and document it, or fix it.
- L1 (emit.rs:160), L2 (place.rs:230), L3 (route.rs:188): `let _ = p;`,
  `let _ = fi;`, `let _ = internal;` suppress unused variables. Code
  smell indicating incomplete implementation. Remove each suppression by
  either using the variable or deleting it, and record what the
  incomplete logic was supposed to do.

## What was tried and what failed

- Nothing tried. These findings are untouched since Oct 22, 2026, except
  M2, which is partially covered by later tests (verify, do not assume).

## Files and commands

- Code: crates/helion-xilinx7/src/emit.rs, src/place.rs, src/route.rs,
  src/frames.rs, src/timing.rs, src/pack.rs (line numbers above).
- Source: ~/workspace/zedboard-crew/BRIEFS/investigator_report.md.

## Dependencies

Depends on nothing. Related to ZB-10 (the Vivado diff settles H2) and
US-09 (real timing DB).

## Rules

- No em dashes anywhere.
- A finding closed as benign needs the evidence written down, not a
  feeling.
- Follow the standing bug-fix rule: sweep the whole crate for the same
  bug class when fixing H4 (silent defaults) and H5 (silent drops).
