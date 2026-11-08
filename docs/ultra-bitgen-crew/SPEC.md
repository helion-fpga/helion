# UltraScale+ bitgen: block-by-block to identical bitstreams

## Captain's intent (user's words, paraphrased)

Work the branch we were working on until Helion's bitstreams are identical to
the reference, every last bit. Block by block testing, greedy approach.

## Goal

`helion impl --part xcvu3p` emits a bitstream that is bit-identical to the
reference bitstream for the same placed and routed design. Zero diff, every
block, or a named blocker proving why a block cannot be closed without Vivado.

## What identical means (two tiers, both honest)

- Tier 1, checkable today with no Vivado: for the same FASM design, every
  bit Helion emits equals the bit implied by the public segbits databases
  (prjuray-db, CC0 1.0, and BitFiltrator arch JSON). The diff harness
  computes this per block. Zero diff at tier 1 means identical against every
  public source of truth that exists.
- Tier 2, needs Vivado: the same design built by Vivado, bit-for-bit equal.
  BLOCKED: license server 27000@license2.forest.usf.edu connection refused
  since Oct 22, 2026. Saksham follows up with the lab admin. The PinPlanning
  probe (license-free, exports part properties and one INT tile PIP list) is
  still not run; it unblocks the INT PIP completeness audit. Tier 2 runs the
  day license access exists. Any tier-1-green block that disagrees with
  Vivado is a real bug, not a measurement artifact.
- Control part first: xcku3p before xcvu3p. BitFiltrator ships a complete
  xcku3p device file and demo.py reconstruction. The pipeline must go green
  on xcku3p first, where an independent tool can validate it. xcvu3p then
  differs only by the device file. A bug found on xcku3p is a pipeline bug;
  a bug found only on xcvu3p is a device-file bug. Never debug both at once.

## Greedy block loop (the user's approach, locked)

1. Diff harness lists every block with its diff size, largest first.
2. Fix the largest diff block first. One block per fix, one fix per commit.
3. Re-run the full diff after each fix. A fix that regresses any green block
   is reverted on the spot.
4. A block goes green only when its diff is zero AND the critic has re-run
   the check.
5. Stop only at zero diff everywhere, or when a block is proven unfixable
   without Vivado. Then it becomes a named blocker with the exact missing
   data, never a silent skip.

## Blocks

LUT INIT, FF INIT/config, BRAM content, BRAM parity, INT PIP, IO config,
clocking (BUFGCE), DSP48E2 config, URAM288 content, frame addressing and
bitstream framing (sync words, FAR sequencing, CRC).

## Base and day-1 pieces

- Base branch: fm/ultra-bitstream at aee77e3 (day-1 scout report.md plus
  helion-ultrascale crate merged from fm/ultra-device 7a4146f).
- Device model: crates/helion-ultrascale (bel.rs, device.rs, fabric.rs).
  One slice per CLB, one CARRY8 per slice (UG574, CTO-signed correction).
- Consume, do not rewrite: pack-place staging (fm/ultra-packplace c9e96c4),
  router scaffold (fm/ultra-router 3d49b98), timing staging
  (fm/ultra-timing 7ff0007). They live as staging/ dirs on their branches;
  the integration worker ports them into the crate.
- FASM vocabulary: prjuray-db segbits (INT PIP 3778 entries, FASM format)
  and BitFiltrator ULTRASCALE_PLUS.json (LutLoc, RegLoc, BramMemLoc).
- BitFiltrator reference clone lives untracked in the base worktree at
  ~/workspace/helion-ultra-bitstream/bitfiltrator-ref/ (resources/archs/,
  resources/devices/, src/bit_locator.py, src/demo.py).

## Hard rules

- No em dashes anywhere: chat, files, code comments.
- Never fabricate terminal output or numbers. Every claim ships with the
  exact command and its output. Nothing is done or tested without a shown run.
- No merge to master without the user's explicit word. All work on
  fm/ultra-bitgen-* branches. Nothing pushed to GitHub without the user's word.
- Every fix ships with a regression test. A fix without a test is not a fix.
- Correctness before optimization. Never change semantics to improve a number.
  Never emit guessed bits: unknown encodings fail loud.
- Workers never address the user. All questions go through the orchestrator.
- Coordinate builds: cargo check freely, full builds through the orchestrator
  (machine OOMs on parallel release builds; -j1/-j2 max). Never clean target
  dirs; builds via nohup setsid, log to files.
- Legal: only CC0/public-domain or MIT sources (prjuray-db CC0, BitFiltrator
  MIT). RapidWright .dat and FPGA Interchange DBs are REJECTED (Xilinx EULA).
  Never ingest them.

## Out of scope

- SSI (multi-die) parts. Target is monolithic xcvu3p only.
- MMCM/PLL attribute fuzzing until a design needs it: tie to safe default,
  fail loud.
- URAM288 content until the device file lands: fail loud, never emit guessed bits.
- DSP48E2 config bits until segbits-vendor lands the encoding: fail loud.
- QoR work. This crew owns bit-correctness only.
