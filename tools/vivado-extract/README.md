# vivado-extract: learn from Vivado, keep the data out of the repo

Scripts that run Vivado in batch mode (on any SLURM host that has it) and pull
out what Helion needs to check itself against: the device model, the
configuration bits of each primitive feature, and Vivado's own timing,
utilization, power and DRC on reference designs.

**Data policy.** Everything Vivado writes (device dumps, bitstreams, reports,
SDF) may be AMD-licensed. It stays in a local data directory
(`$VX_DATA`, default `~/helion-vivado-data/`), never in this repository. The
repo holds only these scripts and tests Helion derives from its own code.

## Layout

| Path | What it does |
|---|---|
| `vx.sh` | Driver: pushes `tcl/ py/ rtl/` to the host, writes an sbatch file, submits, fetches `out/` back. |
| `tcl/inventory.tcl` | Every part and board part the install offers (`parts.tsv`, `boards.tsv`). No license. |
| `tcl/license_probe.tcl` | A 4-bit counter through synth/place/route/bitstream per part; records the stage a missing license stops. |
| `tcl/device.tcl` | Device model in pin-planning mode (no license): grid, sites, clock regions, IO banks, package pins, and per tile type its wires, PIPs and site-pin wiring (`speed` arg adds speed models; `full` adds nodes), per site type its BELs, pins, site PIPs. |
| `tcl/fuzz.tcl` | One routed design, then one bitstream per single property change (debug bitstreams, so every frame carries its address). |
| `tcl/reference.tcl` | Vivado's full flow on a design with every report Helion cross-checks, plus per-arc cell delays (`cell_arcs.tsv`) and per-load net delays (`net_delays.tsv`) as data. |
| `py/bitframes.py` | Reads a debug `.bit`(.gz): frames by address; `diff` lists every flipped bit as FAR/block/row/column/minor/word/bit. `selftest` checks the parser. |
| `py/mksweep.py` | Writes the fuzz sweep for `rtl/us_sink.v`; can add rows from attribute tables (`id attribute kind default variant`). |
| `py/fuzzsum.py` | Diffs every variant bitstream of a `fuzz.tcl` run against `base.bit.gz`: one `<id>.diff.tsv` each and a `summary.tsv` (zero-bit variants are results too). |
| `py/compare_db.py` | Compares a `device.tcl` dump with a Project X-Ray / prjuray style database (X-Ray `TYPE.DST.SRC` or prjuray `TYPE.PIP.DST.SRC` names; `--tilegrid` for the grid): grid, tile types, PIPs with no known bits, site types, package pins. |
| `py/compare_timing.py` | Compares `cell_arcs.tsv` / `summary.tsv` from `reference.tcl` with Helion's 7-series delay constants. |
| `rtl/us_sink.v`, `rtl/us_sink.sweep.tsv` | UltraScale+ fuzz base (LUT6, FDRE, CARRY8, DSP48E2, RAMB36E2, URAM288, MMCME4_ADV, BUFGCE, IOBUF) and its sweep. |

## Setup

```sh
export VX_HOST=<ssh alias of the Vivado host>        # BatchMode ssh must work
export VX_SETUP='module load <vivado module>'        # or: source <Vivado>/settings64.sh
```

Paths on the host must not contain spaces (Vivado refuses them).

## Recipes

```sh
cd tools/vivado-extract
./vx.sh submit inventory inventory.tcl @OUT@
./vx.sh submit lic license_probe.tcl @OUT@ xc7z020clg484-1 xcku3p-ffva676-1-e
VX_MEM=32G ./vx.sh submit dev-z020 device.tcl @OUT@ xc7z020clg484-1
./vx.sh status
./vx.sh fetch dev-z020 7series/xc7z020clg484-1/device
python3 py/compare_db.py ~/helion-vivado-data/7series/xc7z020clg484-1/device \
    ~/.cache/helion/prjxray-db/zynq7 --part-dir ~/.cache/helion/prjxray-db/zynq7/xc7z020clg484-1
```

Fuzzing (UltraScale+): put the RTL and sweep on the host, run, fetch, diff locally
(the host may have no Python):

```sh
python3 py/mksweep.py us --dsp-table <dsp table> --uram-table <uram table> > rtl/us_sink.sweep.tsv
./vx.sh submit fuzz-ku3p fuzz.tcl @OUT@ xcku3p-ffva676-1-e us_sink @SRC@/rtl/us_sink.v - @SRC@/rtl/us_sink.sweep.tsv
./vx.sh fetch fuzz-ku3p ultrascaleplus/xcku3p-ffva676-1-e/fuzz
python3 py/fuzzsum.py ~/helion-vivado-data/ultrascaleplus/xcku3p-ffva676-1-e/fuzz
```

Reference runs (sources: `.v/.sv/.vhd`, `+define+X`, `+incdir+D`; several XDCs comma-separated):

```sh
./vx.sh submit ref-blinky reference.tcl @OUT@ xc7z020clg484-1 zedboard_blinky <xdc> <rtl>...
./vx.sh fetch ref-blinky 7series/xc7z020clg484-1/ref/blinky
python3 py/compare_timing.py ~/helion-vivado-data/7series/xc7z020clg484-1/ref/*
```

## Things learned the hard way

- Pin-planning mode (`DESIGN_MODE PinPlanning` + `open_io_design`) reads the full
  device (tiles, sites, PIPs, nodes, speed models) with no synthesis license; a part
  your license does not cover can still be dumped.
- A routed design accepts `set_property` on most primitive attributes, and
  `write_bitstream` then encodes the new value without re-implementing, so a sweep
  costs one bitstream per variant, not one implementation.
- `BITSTREAM.GENERAL.DEBUGBITSTREAM YES` writes each frame as its own FDRI packet
  followed by a write of its frame address to LOUT (config register 0x08, header
  `0x30010001`): frames can be located without the device's frame-address sequence.
  xcku3p: 41,460 frames of 93 words.
- Vivado 2017.3 crashes (signal 11) in `get_speed_models` on UltraScale+ parts in
  pin-planning mode, hence the opt-in `speed` argument to `device.tcl`.
- `report_drc` blocks `write_bitstream` on unconnected primitive pins (e.g. RAMB36E2
  `REQP-1809` at default widths): give fuzz base cells explicit widths and tie-offs.
- Vivado 2017.3 takes about three minutes to start on a network filesystem; batch
  many parts or variants per session.
