# Q6 PinPlanning probe package: HPIO tile inventory plus one HPIO tile FAR

Target part: `xcku3p-ffva676-1-e`. Target block: IO_CONFIG (55 bits,
blocked). This package is the exact missing data from the session 14
tilegrid research (Q6), narrowed to the minimal scope that unblocks
the v1 goldens: one row-0 HPIO tile base FAR, the offset ruling, and
the HPIO_RIGHT existence verdict.

Status: NOT RUN. No Vivado exists in this VM, no Vivado check was run
here, and nothing in this package claims a Vivado result. The owner runs
it on the cluster when next on campus or on the campus VPN. Do not run
any license-port connect check: a TCP connect proves nothing from this VM, and license state
is UNKNOWN.

## What is in this package

- `tcl/extract_device_info.tcl` and `tcl/helpers.tcl`: verbatim copies
  of the BitFiltrator scripts (MIT, Copyright (c) 2022 EPFL VLSC;
  license in `tcl/BITFILTRATOR-LICENSE`). Not modified. The device
  info script is the step 1 inventory source; do not summarize its
  output away, keep the raw JSON.
- `tcl/q6_hpio_inventory.tcl`: step 1 arbiter script (original Helion
  crew work). Lists every HPIO/HDIO/CMT tile with real Vivado name,
  column, row, clock region, IOB sites, and package pins, plus exact
  HPIO_RIGHT / HPIO_L counts.
- `q6_check_step1.py`: step 1 verdict checker (stdlib only, runs
  anywhere). Prints the verdict and the next step.
- `q6_gen_io_xdc.py`: step 2 input generator (stdlib only). Reads the
  step 1 inventory, picks the row-0 HPIO tiles and their real package
  pins, and writes `q6-io-probe.xdc` and `q6-io-probe.v`.
- `tcl/q6_one_hpio_tile.tcl`: steps 2 and 3 driver (original Helion
  crew work, modeled on BitFiltrator `one_ff_in_every_clb_column.tcl`).
  Places one IOB per probed site, generates the bitstream plus the
  logic-location (.ll) file.
- `tests/test_q6_probe.py`: tests for the checker and the generator
  (stdlib unittest). Run with `python3 tests/test_q6_probe.py`.
- `VERIFICATION.md`: the session 18 independent re-verification of
  the session 14 research, with exact commands and outputs.

## Step 0: get on the cluster and check Vivado exists

```sh
ssh <user>@<cluster-host>
vivado -version
```

If `vivado` is not found, stop and report that: the probe cannot run
without Vivado on the cluster. Copy this whole `q6-hpio` directory to
the cluster first, then run the steps from inside it.

## Step 1: tile inventory (PinPlanning, no bitstream, no license checkout)

```sh
vivado -mode batch -source tcl/extract_device_info.tcl -notrace \
  -tclargs xcku3p-ffva676-1-e xcku3p-device-info.json
vivado -mode batch -source tcl/q6_hpio_inventory.tcl -notrace \
  -tclargs xcku3p-ffva676-1-e q6-step1-hpio-inventory.txt
```

Keep both output files verbatim: `xcku3p-device-info.json` (raw, do
not summarize it away; losing it is how the tile inventory was lost
once already) and `q6-step1-hpio-inventory.txt`. Then check the
verdict (runs anywhere, no Vivado needed):

```sh
python3 q6_check_step1.py q6-step1-hpio-inventory.txt
```

The verdict is one of:

- `HPIO_RIGHT EXISTS`: go to step 2.
- `STOP, no HPIO_RIGHT, HPIO_L instead`: STOP. Do not run step 2.
  Bring back the two step 1 files and escalate to the CTO: the
  golden's tile type does not exist on this part, the
  `segbits_hpio_right.db` encoding does not apply to the real IO
  tiles, and no public HPIO_L encoding exists. A new HPIO_L fuzzing
  campaign would be needed, which is a separate work item.
- `STOP, no HPIO tile of either type`: STOP. The inventory is
  incomplete or the part string is wrong. Re-run step 1 and check
  the part string with `get_parts` in Vivado.

## Step 2: one HPIO tile base FAR plus the offset ruling (steps 2 and 3)

Only if step 1 says HPIO_RIGHT exists.

```sh
python3 q6_gen_io_xdc.py q6-step1-hpio-inventory.txt
vivado -mode batch -source tcl/q6_one_hpio_tile.tcl -notrace \
  -tclargs xcku3p-ffva676-1-e q6-io-probe.xdc q6-io-probe.v q6-io-probe.bit
```

License note: xcku3p is a WebPACK part (the BitFiltrator device file
records license Webpack), so `write_bitstream` is expected to work
under the free WebPACK license. If Vivado reports a license error
instead, STOP, paste the exact error into the run log, and bring back
nothing else from this step. Do not work around a license error.

Bring back all four files: `q6-io-probe.bit`, `q6-io-probe.ll`,
`q6-io-probe.rbt`, `q6-io-probe.dcp`. The `.ll` file is the ground
truth: it maps each placed IOB to its frame address. Also bring back
the full Vivado log and the generated `q6-io-probe.xdc`.

## What the crew does with the results

From the `.ll` file, per placed IOB: tile name, FAR block type, row
major, column major, minor, and the tile's frame count. From the
two tiles in row 0 (if both exist): whether they share a column major
and how the observed bit positions compare with the segbits DB bits,
which decides the offset question (Q4.3 item 4). That lands as a
one-tile table in the device file, the provisional HPIO bridge
(`HpioProvisionalFallback`, invented FAR 0x000) is deleted, and the
harness re-runs. IO_CONFIG goes green only on zero diff plus a
critic re-run.

## What does NOT unblock anything

- Re-running BitFiltrator's `create_device_summary.py` as-is: it has
  no IO step.
- Reading the minors table harder: 21 eight-minor columns on xcku3p,
  and five of six such columns on xczu3eg are not HPIO.
- Copying the xczu3eg tilegrid numbers (HPIO col 103) to xcku3p:
  different part, different column counts, different clock-region
  rows. The xczu3eg numbers are a schema example, not xcku3p data.
- Guessing an HPIO_L column from the minors table, reusing
  `segbits_hpio_right.db` for HPIO_L tiles, or relabeling the
  golden's tile type without a CTO ruling.

## Grounding

Every number in this package's background comes from two independent
public sources (prjuray-db, CC0 1.0, commit affbc5e; BitFiltrator,
MIT, commit 79b9bd6). See VERIFICATION.md for the exact commands and
outputs. No RapidWright .dat files and no FPGA Interchange DBs were
read or ingested. No em dashes anywhere.
