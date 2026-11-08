# ZB-07: Site allocators must respect the used set

- Track: ZedBoard, backend. Investigator finding C5.
- Branch: none (no fix committed anywhere). Worktree:
  ~/workspace/helion-zed-bringup.
- Status: not started.

## Goal

When a design uses more than one BUFG, BRAM, or DSP, each one must be
placed on a different site. Today they all collide on the first site.

## Done when

A two-clock design programs and both clocks work on the board (or, at
minimum, a unit test with two BUFGs asserts they land on different
sites, and the same for two BRAMs and two DSPs). A design where two
BUFGs share one site is a failure of this sub-problem.

## What is already done

- Nothing except the finding. Investigator C5: `bufg_site()` in
  xdevice.rs lines 118-120 returns
  `self.device.bufg_sites().into_iter().next()`, always the first site.
  The `used` set is not consulted. place.rs inserts into used_sites
  (lines 147-156), but the next call still returns the first site. The
  same bug is in `ramb_site()` and `dsp_site()`.

## What was tried and what failed

- Nothing tried. The finding is untouched since Oct 22, 2026. The blinky
  has only one clock, so this bug has never shown up on the board; it will
  show up the first time a two-clock design is tried.

## What to do

1. Fix all three functions in xdevice.rs (`bufg_site`, `ramb_site`,
   `dsp_site`) to skip sites already in the `used` set and return the
   first free one. If no site is free, return an error, never a used
   site and never a default.
2. Audit every other `*_site()` style helper in the backend for the same
   pattern (returns `.next()` without consulting `used`). The standing
   bug-fix rule (Oct 13, 2026) says: when a bug is found, sweep EVERY
   surface for the same bug class before moving on; each fix names the
   class, greps the whole crate for the pattern, fixes every instance,
   and reports class plus instances.
3. Add unit tests: two BUFGs land on different sites; two BRAMs land on
   different sites; two DSPs land on different sites; exhausting all
   sites returns an error.
4. Verify on the board with a two-clock design if one exists in the
   corpus; if not, the unit tests plus a successful two-clock build are
   the proof, and the board test is recorded as still open.

## Files and commands

- Code: crates/helion-xilinx7/src/xdevice.rs (lines 118-120 and the
  ramb_site / dsp_site equivalents), src/place.rs (lines 147-156).
- Grep for the pattern across the crate before and after the fix.

## Dependencies

Depends on nothing. Related to ZB-02 (BUFG site selection is a suspect
if v3 stays dark) and ZB-08 (CARRY4 placement has the same class of
problem, but with adjacency constraints on top).

## Rules

- No em dashes anywhere.
- Sweep the whole crate for the same bug class; do not fix one function
  and leave its siblings broken.
- Never return a used site or a silent default when no site is free.
