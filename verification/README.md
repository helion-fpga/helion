# Verification

## `compare_against_prjxray_fasm2frames.py`

The strongest check available on this machine: run Helion's FASM through
**Project X-Ray's own reference assembler** and compare the resulting
configuration frames against Helion's `.bit`, bit for bit.

This closes the biggest open gap in the earlier audits. The in-repo
`prjxray_diff` gate (byte-comparison against `xc7frames2bit`) **skips**
here — "no xc7frames2bit binary", "no prjxray checkout" — so every
"matches Vivado" claim rested on re-derivation by hand. This script actually
executes an independent implementation.

### Setup (once)

```bash
python3 -m venv /tmp/fasmvenv
/tmp/fasmvenv/bin/pip install fasm intervaltree numpy pyjson5 pyyaml simplejson
git clone https://github.com/f4pga/prjxray.git /tmp/prjxray-src
/tmp/fasmvenv/bin/pip install --no-deps -e /tmp/prjxray-src
```

`xc7frames2bit` is C++ and needs `gflags`/`absl`, so it is not built here.
`utils/fasm2frames.py` is the pure-Python half of the same reference and needs
no compiled helper, which is why it is the tool used.

### Run

```bash
python3 verification/compare_against_prjxray_fasm2frames.py \
    --fasm /tmp/x7/zedboard_blinky.fasm \
    --bit  /tmp/x7/zedboard_blinky.bit \
    --part xc7z020clg484-1
```

### What it does about DB layout

The reference resolves the database differently from Helion: it scans
`db_root` for `tile_type_*.json` (so `db_root` must be the **family**
directory, e.g. `prjxray-db/zynq7`), it reads `segbits_*.db` from that same
directory, it expects the **part** directory immediately beneath it, and it
needs `mapping/{parts,devices,resources}.yaml`. The script generates that
layout with symlinks and small generated YAML files. The real DB is not
modified beyond adding a family-level `mapping/` directory and per-fabric
symlinks to the family's segbits.

### Result on `examples/zedboard_blinky.sv` (xc7z020)

```
reference produced 8753844 bytes of frames
NOTE: the reference tool itself errored on 23 line(s),
      all for *_SING tiles that upstream prjxray has no segbits for.
helion frames: 9996   reference frames: 7802   common: 7802
frames differing: 2
words helion sets that the reference does not: 0
words the reference sets that helion does not: 4
OK: helion never sets a bit the independent reference does not.
```

### How to read that

* **Helion never sets a configuration bit the reference does not.** For every
  one of the 7802 frames the reference covers, Helion's value is a subset of
  the reference's. There is no fabricated or misaddressed configuration.
* **2 frames differ, by 4 words total**, in `0x4224a6` and `0x4224a7`. Those
  frames are owned by `RIOB33_SING_X73Y0` (and shared with the other tiles in
  that column). Helion deliberately does not write them: upstream
  `prjxray-db` ships **no segbits** for the `*_SING` tile types, and their
  frame geometry differs from the base type, so borrowing the base type's
  bit positions is invalid. The reference tool **errors on those same lines**:
  `frame_clear: invalid word address 102 in line: RIOB33_SING_X73Y49...`.
  So the 4 missing words are a documented upstream data gap, not a Helion
  miscompile. On the ZedBoard it affects LD6 (U19) and LD7 (U14), which keep
  default IOB configuration.
* **The reference covers fewer frames than Helion** (7802 vs 9996) because
  `fasm2frames.py` enumerates frames from the tilegrid union, which is 2194
  frames short on xc7z020. Helion uses every frame in `part.json`, which is
  the correct set for a bitstream (see `helion/AGENTS.md`). This is the same
  delta the earlier frame audit measured, and it is why the reference's frame
  list alone cannot validate the bitstream's total length.

### The same check on the shipped CLI (`helion bitstream`)

`helion bitstream examples/zedboard_blinky.sv --part xc7z020 \
    --xdc examples/zedboard.xdc -o /tmp/x7/cli_zed.bit`

produces a 4,045,681-byte `.bit` (xc7z020clg484-1, IDCODE 0x03727093, 9996
frames, 2697 FASM features). Feeding the FASM from that same run to
`fasm2frames.py` and comparing frames gives:

```text
CLI frames: 9996   ref frames: 7802   common: 7802
frames differing: 2  ['0x4224a6', '0x4224a7']
words CLI sets that ref does NOT: 0
```

Identical to the library path: only the `*_SING` gap, and nothing fabricated.

**A caution about comparing across runs.** An earlier comparison of the CLI
bitstream against a `.frm` generated from a *previous* run's FASM differed in
102 frames and showed 85 words the reference did not set. That was not a bug:
the two runs chose different (equally legal) routing pips and clock-spine
buffers, so the old reference simply described a different route. Compare a
bitstream only against a reference built from **its own** FASM; the useful
invariant is "Helion sets no bit the reference does not", which needs the FASM
to match and does not need the route to.

### Scope

Not hardware-validated: there is no JTAG cable on this machine, so
`openFPGALoader --detect` fails. Every claim here is about software
correctness of the configuration data.

---

## Board bring-up tests (`examples/t01`..`t05`)

Five single-purpose designs, each isolating one hardware path, all built for
the ZedBoard (`xc7z020clg484-1`) through the shipped CLI:

```bash
export HELION_XILINX_DB=<workspace>/work/prjxray-db
helion bitstream examples/t0N_*.sv --part xc7z020 \
    --xdc examples/zedboard.xdc -o /tmp/t0N.bit
```

| Design | Path under test | Pass criterion |
|---|---|---|
| `t01_iob_static` | IOB output driver; no clock, no logic | LD0-LD5 pattern, LD6/LD7 excluded (no X-Ray segbits) |
| `t02_input_passthrough` | IBUF -> LUT -> OBUF | each LED mirrors its button |
| `t03_lut_truth_table` | LUT6 INIT | LD0/2/4/6 on, LD1/3/5/7 off |
| `t04_ff_reset` | Hff `SR` pin, async reset | held reset sets the pattern; released, it toggles |
| `t05_shift_chain` | FF-to-FF routing across tiles | the lit LED walks LD0 -> LD7 and wraps |

All five produce a 4,045,681-byte `.bit` with a valid TLV header, the part name
`xc7z020clg484-1`, and the device IDCODE `0x03727093` in the configuration
stream (read out of the FDRI register writes, not the `b` record -- `b` holds
the ASCII part name, which is easy to mistake for an IDCODE).

`t02` is the test for the input-IOB fix: before it, any design with a data input
was unbuildable (`route: net \`btn_0\` has 0 drivers`).

### What is NOT verified here

These bitstreams are **not confirmed on silicon**. There is no JTAG cable
reachable from this machine (`openFPGALoader --detect` cannot open an FTDI
device) and no Vivado installed, so nothing here has been run on a board or
compared against a Vivado timing report. The claims above are structural: the
header, IDCODE, frame count, and agreement with the reference assembler. Whether
LD0 actually lights is a question only a board can answer.

### Routing used to be 99.9% of the runtime, and no longer is

The 40-minute Artix-35T figure that used to appear here was **not** a scaling
property of the fabric. It was three separate defects, now fixed:

* `XDevice` never implemented `RouteQuery::tile_xy`, so the trait's `None`
  default applied and the router's A* heuristic evaluated to 0 for every tile.
  A* had no direction at all and degenerated into flooding the whole fabric:
  routing one blinky net with 4 sinks expanded 4,534,484 distinct nodes and
  14,647,731 edges.
* On any routing failure the rip-up loop discarded **every** routed net
  (`for other in routed.keys()`, with no reference to what actually blocked the
  failed net) and re-queued all of them, so each failure threw away all
  completed work.
* Several copies of the same Artix probe were left running concurrently and
  competed for CPU.

Measured on `examples/zedboard_blinky.sv` (90 LUTs, `xc7z020`):

| stage | before | after |
|---|---|---|
| route | 164,205 ms | **729 ms** |
| whole bitstream | 165.5 s | **2.2 s** |
| `helion qor` | 145,149 ms | **5,123 ms** |
| `helion-xilinx7` lib suite | 82 s | **4.1 s** |

The bitstream is unchanged in content across this: 9996 frames, IDCODE
`0x03727093`, and the same 0 (blinky: 1, the `RIOB33_SING` gap) words set that
prjxray's `fasm2frames.py` does not set.

Fix 1 alone was not enough. With the heuristic working, the search started
preferring a shorter hop **through an unused BUFG slot**, which made one
configuration bit carry two sources and prjxray rejected the bitstream outright.
Two further defects surfaced the same way:

* The same-node stitch in `node_wires` unified the BUFG `_CK_MUXED*` inputs
  across all 17 slots, so the search stepped from one slot's input to the next
  for free and cascaded (`BUFGCTRL1_O -> FBG_OUT2 -> BUFGCTRL2_I0 -> ...`).
* A routed pip could land on a different buffer's input mux. Denying those wires
  to the search is wrong -- `CK_MUXED<n>` is shared by every BUFG in a tile, so
  blocking the wire also blocks the design's own legal entry, and the search then
  floods (the internal-clock design went from 0.2 s to 176 s). It is filtered at
  emit time instead, where the legitimate selects are known.

### Corpus result: 136 HDL files through the full flow

Every `.sv`/`.v` file in the repository (examples, the checked-in reproducers,
`corpus-pass/`, `tests/`, `ip/`) was run through the entire 7-series flow on
`xc7z020`:

| outcome | count | meaning |
|---|---|---|
| bitstream produced | 12 | valid `xc7z020clg484-1` image, IDCODE `0x03727093`, 9996 frames |
| refused: pin constraint missing | 117 | synthesized and packed; no `PACKAGE_PIN`, so no pad to place |
| refused: memory not lowerable | 7 | beyond the register-file path, with a named diagnostic |

**No crashes, no hangs, and no unexplained failures.** The two refusal classes
are correct behaviour, not defects: a design with no pin constraints genuinely
cannot be turned into a bitstream, and the memory refusals name the limit they
hit (`depth=16, width=32; limits are depth<=16, width<=32, depth*width<=128`).

The corpus is what found the three synthesis/emit defects listed above. In
particular `assign out = in;` and a LUT driving an FF clock both looked like
"Helion is broken" and were real bugs, while the 117 pin failures only looked
like failures because the harness ran without an XDC.

### Artix-35T needs its package named exactly

`xc7a35t` ships no tilegrid and borrows `xc7a50t`, whose fabric is larger than
the 35T `part.json` describes: the 50T tilegrid reaches frame address
`0x00c00100` while the 35T die's frames stop at `0x00401580`. Tiles out there
have no frame to program. This is a database gap, not a Helion defect, and it is
reported as such rather than as a bare `frame ... not in store`.

Routing is not the limitation: with the package named exactly
(`--part xc7a35tcsg324-1`), the Arty blinky places in 24 ms and routes 114 nets
in 774 ms with zero rip-ups.
