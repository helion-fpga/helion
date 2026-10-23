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

### Scope

Not hardware-validated: there is no JTAG cable on this machine, so
`openFPGALoader --detect` fails. Every claim here is about software
correctness of the configuration data.
