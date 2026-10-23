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

### Scaling is the open performance problem

Synthesis is not the bottleneck: an 86-LUT blinky on the Artix-35T
(`xc7a35tcsg324-1`, 18,055 tiles) synthesizes in ~1 ms. Placement and routing on
that fabric run for **over 40 minutes** without finishing, where the same design
on the smaller zynq fabric completes in ~70-150 s. The 35T tilegrid is larger
(18,055 vs. the 29,359-tile z020, but with a different routing-graph shape and
far more IOB/clock tiles), and place/route appear to scale worse than linearly
in tile count. Any claim about "running every part" has to answer this first.
