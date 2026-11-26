# Xilinx 7-series database (Project X-Ray)

Helion builds 7-series bitstreams (ZedBoard `xc7z020clg484-1`, Arty A7-35T
`xc7a35t`) from the open Project X-Ray database. Helion does not ship it.
Without it the IDE's part list hides the 7-series parts and says so, and a
project that names one stops with "no Project X-Ray database found".

## Where Helion looks

1. `HELION_XILINX_DB`, if set: the directory that holds `artix7/` and `zynq7/`.
2. Otherwise `~/.cache/helion/prjxray-db`.

## What must be in it

| Part | Needs |
|------|-------|
| `xc7a35t` (Arty A7-35T) | `artix7/xc7a50t/tilegrid.json` and the `artix7` segbits |
| `xc7z020clg484-1` (ZedBoard) | `zynq7/mapping/parts.yaml` listing the part, `zynq7/xc7z020clg484-1/part.json`, the fabric's `tilegrid.json`, and `zynq7/xc7z020clg484-1/required_features.fasm` |

A `zynq7` part directory without `required_features.fasm` is an error:
the bitstream would miss always-on configuration bits.

## Getting it

The simplest way is to let Helion download it. It fetches one pinned commit
of `openXC7/prjxray-db` (CC0-1.0; about 44 MB, 575 MB unpacked for
`artix7`, `spartan7` and `zynq7`), checks every file against digests built
into Helion, and only then puts each family in place:

- IDE: **Create Project → part page → Download database** (with progress and Cancel).
- Command line: `helion-ide --fetch-xray-db`. A `--headless` run on a 7-series
  part asks first when the database is missing (at a terminal only; scripts
  get the command to run instead).

It installs into `HELION_XILINX_DB` when that is set, else
`~/.cache/helion/prjxray-db`. A family directory you put there yourself is
never replaced.

To use your own checkout instead:

Clone the prebuilt database (for example `f4pga/prjxray-db`) and point
Helion at it:

```sh
git clone https://github.com/f4pga/prjxray-db ~/.cache/helion/prjxray-db
# or: export HELION_XILINX_DB=/path/to/prjxray-db
```

Restart the IDE after setting the variable; the part list is read when the
New Project wizard opens.

Not yet checked: that a stock upstream checkout has every file above for
`xc7z020clg484-1`. If the IDE still hides the ZedBoard part, check each
file in the table above.

## Hardware status

Only the ZedBoard has run a Helion bitstream. Arty A7-35T, Basys 3 and
Nexys A7-100T loads use the matching openFPGALoader board names but are not
hardware-tested; the IDE marks them so.
