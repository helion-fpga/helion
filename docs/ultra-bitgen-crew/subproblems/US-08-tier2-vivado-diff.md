# US-08: Tier 2 Vivado diff (real Vivado bitstreams, 100 designs)

- Track: UltraScale+, bitgen plus validation. The proof that the goal is
  met.
- Branch: fm/ultra-bitstream. Worktree:
  ~/workspace/helion-ultra-bitstream. The hourly driver cron
  bitgen-100-driver checks the license every run.
- Status: never run. License checkout unverified.

## Goal

Diff Helion bitstreams against real Vivado bitstreams for the same 100
designs, to zero. This is the only proof of Vivado-equivalence that
counts.

## Done when

100/100 designs green at tier 2 (zero diff vs a real Vivado bitstream
for the same design). A design counts only when green at both tiers.

## What is already done

- The tier definitions are locked in SPEC.md: tier 1 is a zero diff
  against the public segbits DBs (prjuray-db CC0 1.0 plus BitFiltrator
  arch JSON), checkable with no Vivado. Tier 2 is a zero diff against a
  real Vivado bitstream for the same design. Any tier-1-green block that
  disagrees with Vivado is a real bug.
- The license history: the license server 27000@license2.forest.usf.edu
  refused connections from Oct 22 through Nov 7, 2026. On Nov 8 at about
  10:30 EST the port appeared to answer, but that finding was proven VOID
  later the same day (about 13:30 EST): a control test showed direct TCP
  connects from this VM succeed to every port, including known-closed ones
  (license2:1, example.com:81), so the sandbox gateway accepts the
  handshake and the check proves nothing about the license server. The
  license state is UNKNOWN. A real license checkout is still UNVERIFIED:
  no lmutil or Vivado exists on this VM.
- Vivado 2017.3 lives on the USF cluster, host sc.rc.usf.edu, login
  srivastava1. The sandbox cannot reach it (campus / VPN only). The user
  was following up with the lab admin as of Oct 22, 2026.
- The fuzzing package (jobs 1-5) is the mechanism for getting Vivado
  data off the cluster: ~/workspace/your_files/vivado-fuzzing-
  instructions.md plus vivado-fuzzing-package.zip (30 files).
- Only 2 designs exist so far (blinky and counter). Tier 2 needs US-07
  (the 100-design loop) to produce the designs first.

## What was tried and what failed

- Nothing tried. No real Vivado bitstream has ever been diffed against a
  Helion bitstream on this part. The license was down for 17 days (Oct 22
  through Nov 7, by connection-refused checks). A Nov 8 "port answered"
  report was later proven void by a control test, so the license state is
  unknown, and no checkout has been proven.

## What to do

1. Verify a real license checkout is possible. The real test is Vivado on
   the cluster (sc.rc.usf.edu, campus/VPN only) or the PinPlanning probe.
   Do not claim the license works because the port answers.
2. When a checkout works, generate Vivado bitstreams for the same 100
   designs as US-07 (same FASM, same placement, same seed), and run the
   tier 2 diff on each.
3. Any tier-1-green block that disagrees with Vivado is a real bug. File
   it, fix it through the greedy loop (largest diff first, one fix per
   commit, full re-run, critic re-run), and record it.
4. Report tier-2 progress through the driver: first tier-2 diff, every
   10-design tier-2 milestone, tier-2 100/100.

## Files and commands

- Contracts: docs/ultra-bitgen-crew/SPEC.md (tier definitions),
  INTERFACES.md.
- Harness: ~/workspace/ultra-bitgen-crew/harness/ (diff.py and friends).
- Fuzzing package: ~/workspace/your_files/vivado-fuzzing-instructions.md,
  vivado-fuzzing-package.zip.
- Driver: cron bitgen-100-driver (hourly).

## Dependencies

Depends on US-07 (the 100 designs) and on a verified Vivado license
checkout. Unblocks the goal itself. Related to ZB-10 (the same job at
7-series scale, 1 design).

## Rules

- No em dashes anywhere.
- Never claim Vivado-equivalence without a real Vivado diff at zero.
- A port answering is not a checkout. Verify the checkout.
