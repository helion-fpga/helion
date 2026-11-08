# US-12: Harness speed (the 100-design loop must finish in a sane window)

- Track: UltraScale+, bitgen plus harness. Performance only; the outputs
  are correct.
- Branch: fm/ultra-bitstream. Worktree:
  ~/workspace/helion-ultra-bitstream.
- Status: OPEN.

## Goal

Make the diff loop fast enough that 100 designs finish in a sane window.

## Done when

A full 2-design harness run takes a few minutes at most (currently about
9 minutes), with identical diff results to the current baseline.

## What is already done

- The harness works and is correct: 38 tests OK, runs/srval-complete and
  runs/tilegrid-int-rule diff.jsonl byte-identical, 153 crate tests pass.
- The performance facts are measured, not guessed: a full harness run
  takes about 9 minutes for 2 designs; the bits walk dominates; one bits
  run on a 16.5 MB bitstream took 111 s. Emitted bitstreams are
  full-device since the real DeviceFile merge: 16,585,732 bytes per design.
- The suspected hot spots are named: emit_frames zero-frame
  materialisation (see US-11 item 7) and the bits walk over full-device
  bitstreams.

## What was tried and what failed

- Nothing tried. No profiling or optimisation has been attempted. The
  9-minute figure is the baseline, not a regression.

## What to do

1. Profile a full 2-design run and write down where the time goes, with
   numbers. Do not optimise on a feeling.
2. Fix the biggest hot spot first. Likely candidates: skip zero frames
   in emit_frames instead of materialising them; make the bits walk
   incremental or block-scoped instead of walking the whole 16.5 MB
   bitstream every time; cache the DeviceFile load across designs.
3. After each change, re-run the full harness and confirm the diff
   results are byte-identical to runs/srval-complete (plus any new
   designs). A speedup that changes a diff result is a bug, not a speedup.
4. Record the new runtime honestly. The 100-design loop (US-07) plans
   its batches from this number.

## Files and commands

- Harness: ~/workspace/ultra-bitgen-crew/harness/ (diff.py,
  diff_engine.py, emitter.py), snapshot in
  crates/helion-ultrascale/harness/.
- Code: crates/helion-ultrascale/src/bitgen.rs (emit_frames).
- Baseline: ~/workspace/ultra-bitgen-crew/harness/runs/srval-complete/diff.jsonl.

## Dependencies

Depends on nothing. Unblocks US-07 (the 100-design loop's runtime).
Related to US-11 item 7 (zero-frame materialisation).

## Rules

- No em dashes anywhere.
- Profile first, with numbers. Do not optimise on a feeling.
- A speedup that changes a diff result is a bug.
