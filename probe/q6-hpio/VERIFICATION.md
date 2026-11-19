# Session 18 verification: the session 14 tilegrid research, re-checked

Date: 2026-11-19. Worker: IO_CONFIG, session 18. Branch:
`fm/ultra-bitgen-ioconfig`, base 07d0d7d. Read-only verification plus
the Q6 probe package in this directory; no code in the crate was
changed, no merge, nothing pushed.

Verdict: the session 14 research is CONFIRMED in full, independently
reproduced from the two public sources (prjuray-db, CC0 1.0, commit
affbc5e; BitFiltrator, MIT, commit 79b9bd6). No part of IO_CONFIG is
genuinely encodable from public sources. IO_CONFIG stays blocked on
the Q6 Vivado probe. No em dashes anywhere.

## 1. xcku3p.json: HPIO_L only, no tile names, no HPIO columns

Command (run in the worktree):

```
python3 -c "import json; d=json.load(open(
'crates/helion-ultrascale/data/xcku3p.json')); txt=open(
'crates/helion-ultrascale/data/xcku3p.json').read();
print(d['part']); print('HPIO_RIGHT in file:', 'HPIO_RIGHT' in txt);
print('HPIO_L pairs:', [x for x in d['tileType_siteType_pairs']
if x[0].startswith('HPIO')])"
```

Output: part `xcku3p-ffva676-1-e`; `HPIO_RIGHT in file: False`; seven
HPIO_L pairs (`BIAS`, `HPIO_VREF_SITE`, `HPIOB_M`, `HPIOB_S`,
`HPIOB_SNGL`, `HPIOBDIFFINBUF`, `HPIOBDIFFOUTBUF`). Also confirmed:
no `SLICE_X` and no `INT_X` string anywhere in the file; the file is
a column-major index only (113 CLB columns, 10 BRAM, 19 DSP,
`num_minors_per_std_colMajor` length 230).

Byte identity with the BitFiltrator original:

```
md5sum crates/helion-ultrascale/data/xcku3p.json \
  ~/workspace/helion-ultra-bitstream/bitfiltrator-ref/resources/devices/xcku3p.json
e4e060cb73f243eccbe9dacf23072897  crates/helion-ultrascale/data/xcku3p.json
e4e060cb73f243eccbe9dacf23072897  .../bitfiltrator-ref/resources/devices/xcku3p.json
```

## 2. HPIO_RIGHT vs HPIO_L across all BitFiltrator device files

All 40 device files in `bitfiltrator-ref/resources/devices/` were
checked. Result: only xczu1/2/3eg, xczu1/2/3cg, and xazu1/2/3eg carry
HPIO_RIGHT (9 files); every other part, including xcku3p, xcku5p,
and all Kintex, Artix, and Virtex parts, carries HPIO_L only. This
matches the session 14 table exactly.

## 3. BitFiltrator arch file: no HPIO anywhere

`ULTRASCALE_PLUS.json` top-level keys: `BRAM`, `CLEL_L`, `CLEL_R`,
`CLEM`, `CLEM_R`. No HPIO string in the arch JSON, and `grep -rn
"HPIO\|HDIO"` over `bitfiltrator-ref/src/*.py` and the arch JSONs
returns nothing (exit 1). BitFiltrator covers LUT/FF/BRAM only.

## 4. prjuray-db: xczu3eg only, HPIO_RIGHT segbits only

`~/workspace/prjuray-db-ref/` contains only `zynqusp`. Its two
xczu3eg tilegrids (sbva484 and sfvc784) are byte-identical. The
tilegrid has 66,385 tiles, 158 types, 6 HPIO_RIGHT tiles, all sharing
column major 103, rows 0, 0, 1, 1, 2, 2, baseaddr 0x00006700 for row
0, `frames` 8 each, `offset` 5 for Y0/Y60/Y120 and 101 for
Y30/Y90/Y150. No `segbits_hpio_left.db` exists anywhere in the repo;
the only HPIO segbits file is `segbits_hpio_right.db` (1,564 lines
in the vendored copy).

## 5. The 55 bits, re-counted

```
python3 -c "import sys; sys.path.insert(0, '.'); \
from reference import Tier1Reference; ref = Tier1Reference(); \
[print(f, '->', len(ref.resolve_hpio('HPIO_RIGHT_X0Y0', f)), 'bits') \
for f in ['IOB_X0Y0.IOSTANDARD_IN.LVCMOS12', \
'IOB_X0Y1.IOSTANDARD_OUT.LVCMOS12_IDRIVE_I2_SLEW_SLEW_SLOW']]"
```

Output:

```
IOB_X0Y0.IOSTANDARD_IN.LVCMOS12 -> 21 bits, ones: 4 frames: [2, 3]
IOB_X0Y1.IOSTANDARD_OUT.LVCMOS12_IDRIVE_I2_SLEW_SLEW_SLOW -> 34 bits, ones: 24 frames: [2, 3]
```

21 + 34 = 55, matching the harness records (`total_bits: 55`,
`status: blocked`, `coord_system: tile-relative`) for blinky,
counter, and bram on xcku3p. The segbits encoding itself is present
and tile-relative; what is missing is only the absolute FAR, and
nothing public supplies it.

## 6. The INT rule correction, re-corroborated (already on this branch)

The type-dependent INT rule (CLEL_R: -1, others: +1) is already
implemented on this branch in `crates/helion-ultrascale/src/
bitgen_device.rs` (`int_col_major`, with the 76-minor validation)
and in the harness `reference.py` (`int_col_offset_from_clb`). I
re-ran the minors-table check from the xcku3p device file: 52 CLB
columns have a 76-minor left neighbour, all CLEL_R; 61 have one on
the right, all CLEL_L / CLEM / CLEM_R; 0 overlap, 0 neither. The
21 eight-minor columns are `[0, 14, 23, 35, 44, 59, 65, 71, 102,
108, 114, 126, 135, 150, 164, 179, 185, 191, 216, 222, 229]`;
frame count alone cannot identify an HPIO column, and column 229
is an analogy only, not ground truth. No INT work was needed in
this session.

## 7. Fail-loud behavior, still in place

The library `DeviceFile` impl returns `None` for every HPIO/HDIO/
CMT/DSP tile (`crates/helion-ultrascale/src/bitgen_device.rs`,
`resolve_tile` doc comment), and the test
`hpio_and_other_tiles_fail_loud` in
`crates/helion-ultrascale/tests/bitgen_device.rs` pins it for
`HPIO_RIGHT_X0Y0`, `HPIO_L_X0Y0`, both HDIO variants, `CMT_RIGHT_X0Y0`,
and `DSP_X0Y0`. The invented bridge (`HpioProvisionalFallback`,
base FAR 0x000) lives only in the `helion-bitgen` binary, is
admitted invented in its own comments, was not extended, and dies
when the probe lands. A design using IO without an encoding errors
clearly instead of emitting guessed bits.

Cargo test command and result: see the session 18 report to the
orchestrator (run as `~/.cargo/bin/cargo test -j2 -p
helion-ultrascale --test bitgen_device hpio_and_other_tiles_fail_loud`,
log `/tmp/ioconfig-cargo-test.log`).

## 8. What was checked and found NOT encodable

- A one-tile HPIO table grounded in both sources: impossible. Both
  sources agree the device file lists HPIO_L, not HPIO_RIGHT, and
  neither source gives any HPIO column major for xcku3p. The
  xczu3eg HPIO column (103) belongs to a different part and copying
  it would be inventing xcku3p data.
- HDIO/CMT encodings for the goldens: not exercised by the v1
  goldens (only the two HPIO lines), and equally tilegrid-blocked.
- Any other IO_CONFIG line: the goldens carry exactly the two HPIO
  lines above, both on the one tile `HPIO_RIGHT_X0Y0`.

## 9. Exact missing data (unchanged from session 14, Q6)

1. HPIO_RIGHT existence verdict on xcku3p-ffva676-1-e, plus real HPIO
   tile names, types, columns, rows, and sites: Vivado PinPlanning
   probe, step 1 of this package.
2. One HPIO tile base FAR (block, row, col, minor), frame count, and
   the offset rule for the second tile in the row: Vivado .ll /
   bitstream probe, steps 2 and 3 of this package.
3. Full xcku3p tilegrid: Vivado plus Project U-Ray style fuzzing,
   out of scope for the 55 bits.
4. HPIO_L segbits encoding, if step 1 finds no HPIO_RIGHT: a new
   fuzzing campaign; no public source has it.

No Vivado check was run in this session, no license-port connect
check was run, and license state remains UNKNOWN.
