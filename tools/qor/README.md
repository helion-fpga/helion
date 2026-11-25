# qor: Helion's quality-of-results and runtime benchmark (ZedBoard)

```sh
HELION_XILINX_DB=~/.cache/helion/prjxray-db python3 tools/qor/run.py [--set quick|full] [--json out.json] [--timeout SECONDS] [--accept]
python3 tools/qor/run.py --selftest     # parser and gate only, builds nothing
python3 tools/qor/run.py --only pico     # one or more designs (comma-separated), any set
```

It builds `zedbit` (release), builds every design of the set with
`zedbit ... --ignore-timing` for `xc7z020clg484-1`, and reads zedbit's output:
LUT, FF, carry, RAMB18/RAMB36 and DSP48 counts after packing, LUT levels,
worst slack, its clock and Fmax, and time per step (synth, pack+place, route)
plus wall time. Logs and bitstreams go to `<target dir>/qor/` (`tail -f` a log
to watch a build).

| set | designs | time |
|---|---|---|
| `quick` | `zedboard_blinky` (built twice, see G22), svcheck `counter`, `complex`, `hier_bitsel` (clk + led, pins from `examples/zedboard.xdc`), PicoRV32 SoC | 21 min at load average 270-410 (2026-11-17, ae5deb8), 18 of them PicoRV32's route |
| `full` | quick + `zedboard_bram_fill`, `zedboard_dsp_fill_prims`, `zedboard_dsp_fill` (its plain-SV twin, DSPs inferred), Ibex (`HELION_DEFINES=VERILATOR`, pins in `ibex_obs.xdc`) | Ibex routing can take hours on the old router; `--timeout` bounds each build |

A design that fails to build, or is killed by `--timeout`, is recorded as a result (with zedbit's last
line), not an error. There is no timeout unless you give one.

## What fails it (exit 1)

Against `baseline.json`, Helion's own accepted numbers:

- LUT or FF count more than 2% higher;
- carry, RAMB18, RAMB36 or DSP count more than 2% lower (logic fell back to LUTs);
- worst slack more than 0.3 ns worse;
- a design that built in the baseline now fails;
- blinky's two builds no longer byte-identical, if they were in the baseline;
- wall time or route time over 2x the baseline (over 1.5x is a warning; changes
  under 10 s are ignored as noise). The load average at the end of each build is
  recorded (`load`), because a loaded machine alone can double routing time;
- a number in the baseline that zedbit's output no longer contains (zedbit's
  messages changed: update `parse()` in `run.py`).

## Goals, reported only

`vivado.json` holds Vivado 2017.3's numbers for the same RTL (2026-11-17,
`~/helion-zed-builds/vivado_jobs/COMPARISON.md`): `ref` is Vivado's own build,
`hx` is Vivado routing and timing Helion's placement from 354ff04. The run
prints each design's ratio to them, with the gap each one tracks in
`HELION_GAPS.md` (G1/G8 LUT ratio, G4 route time, G5 Fmax, G7 slack gap).
`targets.json` lists per-gap thresholds (for example G1: PicoRV32 at most 1.5x
Vivado's LUTs); each prints pass, FAIL, n/a or not run, and none of them changes
the exit code. When a gap closes, its target becomes the baseline's job.

## Accepting a new baseline

When a change makes numbers better (or worse on purpose), run the set again with
`--accept`. It rewrites the entries of the designs it ran in `baseline.json`
(entries of designs it did not run are kept) and exits 0. Commit
`baseline.json` with the change that moved the numbers, and say why in the
commit message.
