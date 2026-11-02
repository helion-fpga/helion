# Helion ZedBoard debug — handoff

State of the `xc7z020clg484-1` (ZedBoard) bring-up when this branch was handed off.
Written for the next person or AI picking this up. Every number here was
re-measured on this branch, not carried forward from memory.

**Branch:** `zedboard-handoff`
**Fork point:** `194e354` — this branch is a **stale fork of `master`**. See
[Pushing](#pushing) before you try to merge it.

> **Superseded document.** The previous handoff (2026-10-23) said the LED
> "lights but does not toggle". That was true then and is **no longer true** —
> the freezing defects it described were found and fixed afterwards. It is
> archived as `HANDOFF-2026-10-23-superseded.md`; its defect tables are still
> useful history, but its status line is wrong.

---

## TL;DR

| Question | Answer |
| --- | --- |
| Does the blinky bitstream blink? | **Believed yes — never tested on hardware.** |
| Does it match Vivado bit-for-bit? | **No.** 70 differing bits, 6 tiles of 15,000+. |
| Do those 70 bits affect behaviour? | **No** — verified by simulation, not by a board. |
| Does a register file work? | **Yes**, as of this branch. |
| Was anything flashed? | **No. Ever.** The flash gate was never opened. |

The single most important sentence in this document: **everything here is
verified digitally.** The fabric simulator is a model built from the X-Ray
database. It is not the fabric. Two earlier attempts at this flow produced
structurally perfect bitstreams with a dead LED (see [War stories](#war-stories)).

---

## What the numbers actually are

Regenerate the artifact, then re-run these. Do not trust the table without
re-running — these tools are cheap.

```bash
cd helion
HELION_XILINX_DB=/Users/saksham/Documents/zedboard-helion-debug/work/prjxray-db \
  cargo test --release -p helion-xilinx7 --test blinky_reg_bit
# -> writes /tmp/x7/blinky_reg_fixed.bit
```

Result on this branch:

```
4,045,666 bytes   md5 8bb37a4c72295e1b2be6a4611ba74c01
```

| Check | Command | Result |
| --- | --- | --- |
| Bit diff vs Vivado | `python3 docs/zedboard-tools/rawdiff.py <ours> work/ref/GT-blinky.bit` | **70 bits, 45 clusters, 70 NAMED, 0 UNNAMED** |
| Tile-level diff | `python3 docs/zedboard-tools/blockdiff.py <ours> <ref>` | **17 of 19 blocks clean**, 6 differing tiles |
| Mux safety | `python3 docs/zedboard-tools/altmux.py <ours> <ref>` | **0 double-selected** on both sides |
| Clock network | `python3 docs/zedboard-tools/clkdiff.py <ours>` | 0 REAL diffs, 0 unnamed clock words |
| Behaviour | `behaviour_counter` + `docs/zedboard-tools/bitsim.py` | counter matches Vivado digit-for-digit, 0 X's |
| Test suite | `cargo test --release --no-fail-fast` | **50 failures — the pre-existing baseline, 0 new** |

The 50 test failures are **pre-existing and required** for this count:
`HELION_XILINX_DB` must be set. Without it you get 110 failures, almost all
of which are `db::load::*` unable to find the database. That is an environment
problem, not a regression. Compare failure *sets*, not just counts:

```bash
HELION_XILINX_DB=... cargo test --release --no-fail-fast 2>&1 \
  | grep -E '^test .* FAILED$' | sort > /tmp/fail.txt
```

---

## The oracle

**`work/ref/GT-blinky.bit` is the only trustworthy reference.** It is Vivado's
output for a design known to blink LD0 on real hardware.

Before comparing against *anything*, run:

```bash
python3 docs/zedboard-tools/oracle.py
```

This checksums every candidate and refuses non-Vivado files. It exists because
for several rounds `BLINK-242.bit` was used as the golden — and it is a
**Helion** build, not Vivado. A whole round of "diffing against the oracle" was
actually diffing Helion against Helion, which is circular and always agrees.

| File | md5 | Size | Producer |
| --- | --- | --- | --- |
| `GT-blinky.bit` | `3f6d7cfa…` | 4,045,666 | **Vivado — the oracle** |
| `GT2-blinky.bit` | `b06c9299…` | 4,045,666 | Vivado, second build |
| `BLINK-242.bit` | `3cdb9a46…` | 4,045,681 | Helion — **not** an oracle |
| `OURS-r25.bit` | `59baed5e…` | 4,045,681 | Helion — **not** an oracle, and dead |

File size alone identifies the producer: 4,045,666 = Vivado, 4,045,681 = Helion.

**The reference's source is `blinky.vhd`, not the SystemVerilog looks-alike.**
The VHDL registers the output (`LD0 <= count(24)` inside the clocked block);
the stand-in drove it combinationally. That is one fewer flop, and it sent an
investigation chasing a "missing flop" that was never missing. Use
`work/ref/blinky_reg.sv` as the faithful equivalent.

---

## The 70 remaining bits

All of them are in six tiles, all on the counter column around `Y39`–`Y42`:

```
CLBLM_R_X71Y39   INT_L_X70Y39   INT_R_X69Y39
INT_R_X71Y39     INT_R_X71Y41   INT_R_X71Y42
```

Everything outside rows Y32..Y47 is **bit-identical** to Vivado. The clock
backbone (`BYP_ALT{0..7}` on Y40..Y45) is identical.

There are four causes, and three of them are *Vivado's* choices rather than
our bugs:

1. **An inert `ALUT` INIT** on `CLBLM_R_X71Y39`. Ours `0xffffffff00000000`,
   Vivado's `0xff00ff0000000000`. Verified dead at the pin level: **zero** mux
   segbits are set for that `SLICEL_X1` on either side, so nothing selects the
   LUT and no flop samples it. Vivado is canonicalising an unread LUT.
2. **Y39 `BYP_ALT0`**: Vivado arms it from a data wire (`EL1END0`); we tie
   `GFAN0`.
3. **Y41 `FAN_ALT1`** is contested: Vivado routes via `NL1BEG_N3`, we burn an
   arc into `NR1BEG0` and land `IMUX4` on a cross-row `FAN_BOUNCE_S3_0`. Both
   still reach `FAN_BOUNCE1`.
4. **Y42**: one extra `FAN_ALT0.NR1END0`.

### Do not "fix" this by ranking costs

Cause 3 was chased as a router-cost problem for three separate rounds
(33, 34, 37). It is a **4-unit tie**:

```
FAN_BOUNCE1   cur_g 140   (our pick)
SR1BEG_S0     cur_g 136   (Vivado's pick)
DEFAULT_HOP_PS        45
```

Instrumenting the cost function showed the intended source sits at
`g` = 230 against a `DEFAULT_HOP_PS` of 45 — a gap of 139. No sane penalty
closes that. Adding `SR1BEG_S0` to `arm_penalty` made the image **worse**
(70 → 84 bits) because that list applies fabric-wide. Forcing the choice is a
real quality loss traded for a cosmetic bit-match.

**All three rounds agree. Do not re-open this by ranking.** If you want fewer
bits, the honest framing is "match Vivado's router on this one contested mux",
and that needs a different idea, not a bigger penalty.

---

## What actually got fixed

### The register file read back the top word for every address

The last real bug found on this branch. Commit `8cf4818`.

`unpacked_read_mux_expr` in `crates/helion-sv/src/lib.rs` builds `mem[addr]`
as an OR over words. The **top word was emitted with no address guard**, so it
became an unconditional OR term of every other word:

```
depth 2:   mem1 | (mem0 & ~addr)      instead of      addr ? mem1 : mem0
```

Wrong whenever `addr=0` and the top word holds a 1. Fixed by starting the
accumulator at `Const(false)` and applying the selector to *every* word.

**This is the one to read if you want to understand how this flow hides bugs.**
The existing test asserted that *a driver reached the output* — which a mux
with a missing guard satisfies perfectly. And the first truth-table written to
catch it **passed against known-broken code**, because it set the top word to
`0x00` — the one value that cannot expose a leak.

Two evaluator bugs nearly hid it too:

- Gates were matched to output nets by **cell name**, but a gate's output net
  is whatever its `O` pin drives (`u_wshare_l41` drives `u_wshare_n41`). The
  evaluator resolved nothing and blamed the wrong thing.
- For a 1-bit address the net is `addr`, not `addr_0`, so forcing `addr_0`
  left the address constant.

The regression tests (`read_returns_the_right_word_depth_{2,4,8,16}`) were
verified **non-vacuous** by running them against the pre-fix source: all four
fail there, all four pass after. Do the same for any test you add here.

A 16×8 register file now builds end to end: 465 nets, 0 rip-ups, 11,269
features, 4,045,681 bytes — about 27 minutes.

### The carry chain's entry lane read the wrong pin

Earlier commit. `emit_carry4_map` put every lane's operand on `I5`, which
reaches slice pin `A6`. Correct for body lanes, wrong for the **entry** lane,
whose mask reads `A3`. Nothing drove `A3`, so `~A3` was 0 forever, bit 0 could
never carry, and the counter sat at zero.

Both images had identical INIT and identical `FFMUX.XOR`, so no
configuration-level check could see it. Only the *pins* differed.

### The `n_nx_` prefix

`count := count + 1; LD0 <= count(24)` puts one bit on the wire under two
names: `n_nx_count_24` (carry lane) and `count_24` (registered). Every
comparison was an exact string match, so nothing linked the chain to the
register reading it. `same_bit()` in `emit.rs` resolves the two. This cost the
most time of anything on this branch.

---

## War stories — patterns that will bite you again

These are the expensive lessons. They generalise beyond this project.

### A structurally perfect image can be completely dead

Every structural check on this flow — frame count, IDCODE, feature diff,
clock reachability, pad presence, `ZINI`/`ZRST` census, `done=1` — passed on
bitstreams whose LED stayed dark. Twice.

**Structure is not behaviour.** If a property matters, decode the image and
run it (`crates/helion-xilinx7/tests/behaviour_counter.rs` does exactly this).

Tell-tale: a flop that is configured, clocked and reset correctly but never
changes state. The right question is not "is the flop right?" but **"does
anything drive its `D`?"**

### A simulator that agrees with itself proves nothing

Three separate times on this branch, a simulation harness was the actual bug:

- `bitsim.py` filters to FFs whose mux is `XOR` (carry lanes only). A design
  with no registered adder has an **empty** FF set, so it never updates and
  reports live designs as frozen. Use `sim3.py` (HNF-based, any design) unless
  the question is specifically about the blinky's carry chain.
- The register-file evaluator matched gates by cell name (above).
- The truth table used the one value that couldn't expose the bug (above).

**Validate any harness against a case whose answer you know first.** A plain
hand-written `y = d[s]` 4:1 mux was enough to prove the register-file
evaluator sound before trusting it.

### Verify the premise before trusting the diff

Two investigations died on unexamined assumptions:

- The "missing flop" was the source files being different designs.
- A routing topology was disproved by asking the node graph what it actually
  does, after a hand-derived version had looked plausible.

Before comparing two artifacts, confirm they are supposed to be the same thing.

### `active_features` already adds the tile offset

`word = geom["CLB_IO_CLK"]["offset"] + bp // 32`. A hand-rolled bit patcher
that indexes the frame directly writes the wrong word, silently, and every "the
patch changed nothing" conclusion drawn from it is void.

### Bits that look missing may be undecodable, not absent

`CLBLM_R_X71Y39` has **no `OUTMUX` feature at all** in the database, so nothing
can drive its `LOGIC_OUTS5`. Vivado's arm there is a dangling stub. Same class:
`CLK1.GCLK_B6` vs `GCLK_B11` are both reachable with identical 0.058 ns delay —
a free selector choice that produces a **320-bit** diff from one decision,
because the coarse+fine encoding shares no bits.

---

## Known gaps (not bugs, and deliberately so)

- **No BRAM inference in `helion-sv`.** `pico_acceptance::picorv32_with_bram_end_to_end`
  fails at synthesis: a 32×8 memory exceeds the register-file caps
  (depth ≤ 16, width ≤ 32, depth×width ≤ 128) and there is no block-RAM
  lowering. It **refuses loudly**, which is correct — a half-lowered memory
  carries read data with no driver.
  **Do not "fix" this by raising the register-file limits.** That turns one
  BRAM into thousands of flops.
- **LED7 (`U14`, `RIOB33_SING_X73Y0`) is unprogrammable** from upstream X-Ray
  segbits — no `segbits_*` ships for `_SING` variants, and its half of the
  shared word budget cannot be reached. LED6 (`U19`) was recovered by
  borrowing; LED7 genuinely cannot be. Confirmed, do not retry.
- **`examples/zedboard.xdc` lists pins that are not in the package database.**
  `H21` and `K22` failed placement during this work. Any design with a full
  8-bit input bus will hit this. I worked around it with a locally generated
  XDC; the example itself was **not** fixed.
- **A full 16×8 register-file build takes ~27 minutes.** Correct but slow.
- **No UltraScale+ support exists.** `helion-xilinx7` is 7-series only. If
  someone has asked for UltraScale+, that request is **unmet**.

---

## Invariants that must not regress

1. **No `BYP_ALT*`/`FAN_ALT*` mux may have two arms selected** anywhere on the
   die. prjxray encodes arms coarse+fine one-hot, so two arms select *neither*
   input and the output is undefined — while every structural check still
   passes. Audit: `python3 docs/zedboard-tools/altmux.py <ours.bit> <ref.bit>`.
2. **Interior rows must keep all 8 `BYP_ALT` ties** plus `FAN_ALT1/3/5`.
   Deleting `BYP_ALT1.GFAN0`, `BYP_ALT4.GFAN0` and `FAN_ALT3.NL1END_S3_0` from
   a middle row **froze LD0 on real hardware**. This is the one hardware-verified
   fact on this branch.
3. **The blinky image md5 must stay `8bb37a4c…`** unless you intend to change
   it. Any routing change that moves it needs a full re-verification.

---

## Tooling

| Tool | Purpose |
| --- | --- |
| `docs/zedboard-tools/oracle.py` | Identify a `.bit` by md5. **Run this first.** |
| `docs/zedboard-tools/rawdiff.py <ours> <ref>` | Bit-level diff, bit→feature attribution |
| `docs/zedboard-tools/blockdiff.py <ours> <ref> [--y A:B]` | Block-by-block (X-Ray style) comparison |
| `docs/zedboard-tools/altmux.py <ours> <ref>` | Double-selected mux audit |
| `docs/zedboard-tools/clkdiff.py <ours>` | Whole-device clock spine check |
| `docs/zedboard-tools/bitsim.py` | Fabric simulator — **carry-chain FFs only**, see above |
| `docs/zedboard-tools/sim3.py` | HNF simulator, works for any design |
| `docs/zedboard-tools/vbits.py` | Compare decoded features |

### Where the tools live, and the env vars

The debug scripts ship in `docs/zedboard-tools/` so this document is
self-contained. They need the X-Ray database and the reference bitstreams,
which are **not** in this repo (the DB alone is ~600 MB):

```bash
export HELION_XILINX_DB=/path/to/prjxray-db      # contains zynq7/
export HELION_REF_DIR=/path/to/ref               # contains GT-blinky.bit
python3 docs/zedboard-tools/oracle.py            # should report GT-blinky.bit as the oracle
```

`x7decode.py` (imported by several tools) and `memsim.py` also read
`HELION_XILINX_DB`; `memsim.py` additionally reads `HELION_REPO` to locate the
`helion` binary.

**Never diff FASM text.** prjxray silently drops unmatched lines, so a clean
assemble proves self-consistency only. Decode both images through the segbits
DB and compare feature sets.

### Two import-time traps

```python
import sys; sys.argv = ['bitsim']   # bitsim.py reads sys.argv AT IMPORT
```

`work/bisect.py` **shadows the stdlib `bisect` module**. Any script in `work/`
that imports `random` will execute it. Run such scripts via
`runpy.run_path(...)`.

---

## Pushing

This branch is a **stale fork**: 128 commits ahead of, and 410 commits behind,
`helion-fpga/helion@master`. The fork point is `194e354`.

**This branch does not merge cleanly and should not be force-pushed onto
`master`.** It is published as a reviewable artifact: the diff against the
fork point is the deliverable, not a merge candidate. If the work is wanted
upstream, it needs rebasing onto current `master` — 100 of the commits touch
`crates/helion-xilinx7` and `crates/helion-sv`, which have likely moved.

```bash
git remote add github https://github.com/helion-fpga/helion.git
git push -u github zedboard-handoff
```

---

## If you only do one thing

Re-run the five checks in [What the numbers actually are](#what-the-numbers-actually-are)
and confirm they still agree. Every past regression on this branch looked fine
in isolation and was caught by a comparison it hadn't been run through.

Then, if you want the goal actually closed: **put it on a ZedBoard.** No amount
of simulation closes it. The flash gate stayed closed for this entire effort
because the user verifies hardware personally — that is the only remaining step,
and it is not one an AI can take.
