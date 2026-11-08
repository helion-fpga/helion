# US-11: Bitstream details (preamble, ending, CRC, padding, parse limit)

- Track: UltraScale+, bitgen. The small details where a bitstream can
  differ from Vivado's without any block noticing.
- Branch: fm/ultra-bitstream (bitgen-core lane fm/ultra-bitgen-core,
  5e26453). Worktree: ~/workspace/helion-ultra-bitstream.
- Status: OPEN. Several items are deliberately un-emitted or unexercised.

## Goal

Every bitstream detail must be either observed from a real Vivado
bitstream or proven correct. Nothing may be guessed.

## Done when

Each item below is either observed from Vivado, proven correct, or
recorded as a named blocker with the exact missing data.

## The items

1. Preamble registers. The preamble registers Vivado writes (TIMER,
   WBSTAR, POST_CRC, COR0, COR1, SWITCH, MASK, CTL0, CTL1) are
   deliberately NOT emitted, because UltraScale+ values are unknown in
   any open source. Guessed values are never emitted. Done when the real
   values are observed from a Vivado bitstream and emitted, or the
   registers are proven unnecessary for the parts in scope.
2. Frame-0 CRC state. The required post-preamble CRC state for the
   BitFiltrator example frame is 0x01e2bcae, derived by GF(2) linear
   algebra. Done when confirmed against a real Vivado bitstream, or
   proven correct by an independent derivation.
3. Ending sequence. The US+ ending and minimal preamble are mirrored
   from 7-series. Done when confirmed against a real Vivado bitstream.
   Tier-2 confirmation is blocked on the license server (see US-08).
4. Last-of-row 2-zero-padding. Critic session 7 flag F1: the last-of-row
   FAR 2-zero-padding path must be exercised before the generator is
   trusted for full-device bitstreams. It has never been exercised. Done
   when a test exercises it and passes.
5. All-zero-value parse limit. A valued feature whose value is all zero
   sets no bits and is indistinguishable from absent, so parse_bitstream
   drops it. Pinned by a test. Done when the limit is either fixed (the
   parser recovers all-zero valued features) or documented as a permanent
   limitation with the exact consequence stated.
6. make_synth_xcku3p_bit.py hardcoded reference path. Critic session 7
   flag F2, minor. Done when the path is a parameter, not a hardcoded
   string.
7. emit_frames zero-frame materialisation. Performance note from the
   bitgen-core lane: zero frames are materialised instead of skipped.
   Done when the materialisation is removed or proven harmless at
   full-device scale.

## What is already done

- Bitgen-core sessions 1-3 delivered (0f3aa2d, 88070a5, 5e26453): D1
  all-! parse fix, D2 START/DESYNC ending fix, F-A removal, polarity
  complement rule, SegbitsDb/BitRef reconciliation. Merge 004d7cf,
  critic sessions 8 and 11 PASS.
- BitRef.bit widened from u8 to u16 (98e282a) because frame bit numbers
  exceed 255.
- The all-zero-value parse limit is pinned by a test, so it cannot
  regress silently.

## What was tried and what failed

- The frame-0 CRC state was derived, not observed. The ending was
  mirrored, not observed. The padding path was flagged, not exercised.
  None of these have been confirmed against Vivado.

## Files and commands

- Code: crates/helion-ultrascale/src/bitgen.rs (emit/parse),
  src/frames.rs equivalent in the ultrascale crate, src/bitgen_device.rs.
- Harness: crates/helion-ultrascale/harness/ (roundtrip.py, tests/).
- Critic sessions 7, 8, 11 in docs/ultra-bitgen-crew/CRITIQUE.md.

## Dependencies

Items 1-3 depend on US-08 (a real Vivado bitstream). Item 4 depends on
US-07 (the generator must produce a last-of-row case). Items 5-7 depend
on nothing.

## Rules

- No em dashes anywhere.
- Guessed values are never emitted.
- A test that pins a limitation must state the limitation in its name or
  comment, so no one mistakes it for a pass.
