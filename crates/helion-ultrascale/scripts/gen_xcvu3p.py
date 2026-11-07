#!/usr/bin/env python3
"""Generate the provisional xcvu3p.json device file from xcku3p.json.

The file is PROVISIONAL in full: no Vivado device file for xcvu3p
exists in the workspace, and Vivado confirmation is blocked on the
license server (connection refused since Oct 22, 2026). Nothing in
the output may be presented as confirmed. Every value not copied from
a confirmed source is flagged in the file's `helion_provisional`
object with its source, and this header records the construction so
a reader can reproduce or challenge it.

Sources used (confirmed):
- AMD XMP103 (UltraScale+ Product Selection Guide): xcvu3p resource
  counts, 394,080 LUTs, 788,160 FFs, 720 RAMB36E2, 2280 DSP48E2,
  320 URAM288 (URAM not modeled in the device-file schema).
- AMD UG570 Table 2: xcvu3p IDCODE X4B39093, production revision 1
  or later, so 0x14B39093.
- AMD UG570 Table 1: xcvu3p 71,820 configuration frames (see the
  frame-total discrepancy below).
- BitFiltrator `resources/parts_all.json`: the part string
  xcvu3p-ffvc1517-1-e exists as a real xcvu3p part.
- BitFiltrator `resources/devices/xcku3p.json` (MIT, EPFL VLSC):
  the structural template. Every column table in the output starts
  from xcku3p row 0 and is extended by the rules below, never drawn
  freehand.

Construction:
- 1 SLR (xcvu3p is monolithic; SSI out of scope), 7 FAR rows
  (0..6). Row count rationale, not a Vivado fact: 49,260 slices at
  60 CLBs per column is 821 CLB columns total across rows, and 7 is
  the unique row count up to 11 for which 821 splits into
  near-uniform rows: five rows of 117 columns and two rows of 118
  (5*117 + 2*118 = 821). The two 118-column rows are rows 0 and 6
  (the outer rows); that placement is arbitrary and flagged.
- Per row, the CLB column tables start from the xcku3p row-0 tables
  (113 columns, col majors 5..226) and append new columns at the
  right edge: new col majors 230, 231, ... Each new column gets
  num_minors 16, the value of all 113 template CLB columns
  (verified in the generator, asserted), and tile type CLEM_R, the
  modal type at the template's right edge. BRAM columns (10, majors
  0..9), BRAM reg majors, DSP columns (19), and the BRAM minors
  tables are copied verbatim from the template row in every row.
- DSP Y offsets accumulate across rows like the template (row r
  covers 24r..24r+23).
- Header counts are the XMP103 published values, not derived from
  the column tables. The loader never asserts header counts against
  geometry, and the confirmed xcku3p file already disagrees the
  same way (header 162,720 LUTs is 6 per slice against 8 in the
  geometry; header 360 BRAMs against 480 implied by the tables).
  See data/README.md.

Open discrepancies, recorded not hidden:
- Slice count 49,260 vs 49,250: AMD datasheet values in distributor
  listings give 49,260 CLBs for XCVU3P (49,260 * 8 = 394,080 LUTs,
  the XMP103 "394K"), while the Helion device model uses 49,250
  (394,000 / 8). This file uses 49,260 per the README. The CTO
  rules on both counts together; Vivado confirmation must re-check.
- Frame total 73,035 computed vs 71,820 UG570 Table 1: the
  construction OVERSHOOTS UG570 by 1,215 frames (1.7%). That is the
  opposite direction from xcku3p, whose file total 41,460 sits 16
  UNDER the UG570 count of 41,476 (4 per FAR row). Possible causes:
  the 7-row count is wrong, the per-row std table over-counts IO
  columns for VU3P, or UG570 Table 1 counts differently. Do NOT
  tune the tables to hit 71,820: the overshoot is evidence about
  the construction, and hiding it would be cooking the numbers.
  Vivado confirmation must resolve it. (This supersedes the
  71,664 anticipation in the first draft of data/README.md, which
  no rows x (std + BRAM) construction with these tables reproduces;
  the README is updated to the computed 73,035 with this note.)
- Header-vs-geometry gaps, same class as xcku3p: the tables imply
  840 RAMB36 sites (10 cols * 7 rows * 12 per column-row) against
  header 720, and 3,192 DSP sites (19 cols * 7 rows * 24) against
  header 2280. The loader does not assert these; they are listed
  here so no consumer mistakes the header for a table count.

Usage:
    python3 gen_xcvu3p.py            # writes ../data/xcvu3p.json
    python3 gen_xcvu3p.py --check    # verify the file, do not write
"""

import argparse
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
DATA = HERE.parent / "data"
TEMPLATE = DATA / "xcku3p.json"
OUTPUT = DATA / "xcvu3p.json"

# Confirmed sources (see header).
PART = "xcvu3p-ffvc1517-1-e"
IDCODE = "0x14B39093"  # UG570 Table 2, production revision 1+
NUM_SLICES = 49260  # XMP103: 394,080 LUTs / 8
NUM_LUTS = 394080  # XMP103
NUM_REGS = 788160  # XMP103
NUM_BRAMS = 720  # XMP103 RAMB36E2
NUM_DSPS = 2280  # XMP103 DSP48E2
UG570_FRAMES = 71820  # UG570 Table 1
CLBS_PER_COLUMN = 60  # UltraScale+ arch, NUM_CLB_PER_COLUMN
N_ROWS = 7
# CLB columns per row: rows 0 and 6 get the extra column.
CLB_COLS = {0: 118, 1: 117, 2: 117, 3: 117, 4: 117, 5: 117, 6: 118}
TEMPLATE_CLB_COLS = 113
NEW_CLB_MINORS = 16  # value of all 113 template CLB columns
NEW_CLB_TILE_TYPE = "CLEM_R"  # modal type at the template right edge


def build() -> dict:
    template = json.loads(TEMPLATE.read_text())

    # The template's CLB columns all carry 16 minors; the extension
    # rule depends on that, so verify it instead of assuming it.
    row0 = template["slrs"]["SLR0"]["rowMajors"]["0"]
    clb_minors = {
        row0["num_minors_per_std_colMajor"][major]
        for major in row0["clb_colMajors"].values()
    }
    assert clb_minors == {NEW_CLB_MINORS}, (
        f"template CLB minors are not all {NEW_CLB_MINORS}: {clb_minors}"
    )
    assert len(row0["clb_colMajors"]) == TEMPLATE_CLB_COLS
    assert max(row0["clb_colMajors"].values()) == 226
    assert len(row0["num_minors_per_std_colMajor"]) == 230

    rows = {}
    total_slices = 0
    total_frames = 0
    for r in range(N_ROWS):
        n_clb = CLB_COLS[r]
        n_new = n_clb - TEMPLATE_CLB_COLS
        clb_majors = dict(row0["clb_colMajors"])
        tile_types = dict(row0["clb_tileTypes"])
        std_minors = list(row0["num_minors_per_std_colMajor"])
        # New columns append at the right edge, past the template's
        # max col major 226. The std table grows with them.
        next_major = len(std_minors)  # 230
        for i in range(n_new):
            x = TEMPLATE_CLB_COLS + i
            clb_majors[str(x)] = next_major + i
            tile_types[str(x)] = NEW_CLB_TILE_TYPE
            std_minors.append(NEW_CLB_MINORS)
        row = {
            "bram_content_colMajors": dict(
                row0["bram_content_colMajors"]
            ),
            "bram_content_parity_colMajors": dict(
                row0["bram_content_parity_colMajors"]
            ),
            "bram_reg_colMajors": dict(row0["bram_reg_colMajors"]),
            "clb_colMajors": clb_majors,
            "clb_tileTypes": tile_types,
            "dsp_colMajors": dict(row0["dsp_colMajors"]),
            "max_dsp_y_ofst": 24 * r + 23,
            "min_dsp_y_ofst": 24 * r,
            "num_minors_per_bram_content_colMajor": list(
                row0["num_minors_per_bram_content_colMajor"]
            ),
            "num_minors_per_std_colMajor": std_minors,
        }
        rows[str(r)] = row
        total_slices += n_clb * CLBS_PER_COLUMN
        total_frames += sum(std_minors) + sum(
            row["num_minors_per_bram_content_colMajor"]
        )

    assert total_slices == NUM_SLICES, (total_slices, NUM_SLICES)

    notes = [
        {
            "field": "device",
            "value": "xcvu3p",
            "source": "crew target part; structure scaled from BitFiltrator xcku3p.json (MIT, EPFL VLSC)",
        },
        {
            "field": "part",
            "value": PART,
            "source": "BitFiltrator resources/parts_all.json lists xcvu3p-ffvc1517-1-e as a real xcvu3p part",
        },
        {
            "field": "license",
            "value": "Webpack",
            "source": "copied from the xcku3p template; every BitFiltrator device file carries Webpack, and xcvu3p appears in parts_webpack.json",
        },
        {
            "field": "num_slices",
            "value": str(NUM_SLICES),
            "source": "AMD XMP103: 394,080 LUTs / 8; see the 49,250 vs 49,260 discrepancy in data/README.md",
        },
        {
            "field": "num_luts",
            "value": str(NUM_LUTS),
            "source": "AMD XMP103",
        },
        {
            "field": "num_regs",
            "value": str(NUM_REGS),
            "source": "AMD XMP103",
        },
        {
            "field": "num_brams",
            "value": str(NUM_BRAMS),
            "source": "AMD XMP103 RAMB36E2; tables imply 840 sites, same header-vs-geometry class as xcku3p (360 vs 480)",
        },
        {
            "field": "num_dsps",
            "value": str(NUM_DSPS),
            "source": "AMD XMP103 DSP48E2; tables imply 3,192 sites, same header-vs-geometry class as xcku3p (1368 vs 1824)",
        },
        {
            "field": "slrs.SLR0.idcode",
            "value": IDCODE,
            "source": "AMD UG570 Table 2: X4B39093, production revision 1 or later",
        },
        {
            "field": "slrs.SLR0.max_far_row_idx",
            "value": str(N_ROWS - 1),
            "source": "construction: 7 FAR rows; 7 is the unique row count up to 11 for which 49,260 slices split into near-uniform 117/118-column rows; NOT Vivado-confirmed",
        },
        {
            "field": "slrs.SLR0.max_clock_region_row_idx",
            "value": str(N_ROWS - 1),
            "source": "construction: one clock-region row per FAR row, as in the xcku3p template; NOT Vivado-confirmed",
        },
        {
            "field": "slrs.SLR0.max_clock_region_col_idx",
            "value": "3",
            "source": "copied from the xcku3p template (4 clock-region columns); NOT Vivado-confirmed",
        },
        {
            "field": "slrs.SLR0.rowMajors.*.clb_colMajors",
            "value": "117 columns rows 1-5, 118 columns rows 0 and 6 (821 total)",
            "source": "construction: xcku3p row-0 tables (113 columns) extended at the right edge with new col majors 230+; the two 118-column rows are the outer rows, placement arbitrary",
        },
        {
            "field": "slrs.SLR0.rowMajors.*.clb_tileTypes new columns",
            "value": NEW_CLB_TILE_TYPE,
            "source": "construction: modal tile type at the template right edge; new columns are CLEM_R",
        },
        {
            "field": "slrs.SLR0.rowMajors.*.num_minors_per_std_colMajor new entries",
            "value": str(NEW_CLB_MINORS),
            "source": "construction: value of all 113 template CLB columns, verified in the generator",
        },
        {
            "field": "slrs.SLR0.rowMajors.*.bram_content_colMajors",
            "value": "10 columns, majors 0..9, copied from template",
            "source": "construction: copied verbatim from xcku3p row 0 in every row; NOT Vivado-confirmed for xcvu3p",
        },
        {
            "field": "slrs.SLR0.rowMajors.*.dsp_colMajors",
            "value": "19 columns, copied from template",
            "source": "construction: copied verbatim from xcku3p row 0 in every row; NOT Vivado-confirmed for xcvu3p",
        },
        {
            "field": "total_frames",
            "value": str(total_frames),
            "source": f"computed from the tables in this file; AMD UG570 Table 1 lists {UG570_FRAMES} for xcvu3p, so this construction overshoots by {total_frames - UG570_FRAMES}; see the generator header and data/README.md, do not tune the tables to match",
        },
    ]

    out = {
        "device": "xcvu3p",
        "helion_provisional": {"notes": notes, "provisional": True},
        "license": "Webpack",
        "num_brams": NUM_BRAMS,
        "num_dsps": NUM_DSPS,
        "num_luts": NUM_LUTS,
        "num_regs": NUM_REGS,
        "num_slices": NUM_SLICES,
        "num_slrs": 1,
        "part": PART,
        "slrs": {
            "SLR0": {
                "config_order_idx": 0,
                "idcode": IDCODE,
                "max_clock_region_col_idx": 3,
                "max_clock_region_row_idx": N_ROWS - 1,
                "max_far_row_idx": N_ROWS - 1,
                "min_clock_region_col_idx": 0,
                "min_clock_region_row_idx": 0,
                "min_far_row_idx": 0,
                "rowMajors": rows,
                "slr_idx": 0,
            }
        },
        "tileType_siteType_pairs": template["tileType_siteType_pairs"],
    }
    return out, total_slices, total_frames


def check(out: dict, total_slices: int, total_frames: int) -> None:
    """Self-consistency checks: the loader's validate_structure plus
    the slice and frame accounting. Fail loud on any violation."""
    assert out["num_slices"] == total_slices
    for r, row in out["slrs"]["SLR0"]["rowMajors"].items():
        n_std = len(row["num_minors_per_std_colMajor"])
        n_bram = len(row["num_minors_per_bram_content_colMajor"])
        for x, major in row["clb_colMajors"].items():
            assert major < n_std, f"row {r} CLB col {x} major {major} beyond {n_std}"
            assert x in row["clb_tileTypes"], f"row {r} CLB col {x} has no tile type"
        for name in (
            "bram_content_colMajors",
            "bram_content_parity_colMajors",
        ):
            for x, major in row[name].items():
                assert major < n_bram, f"row {r} {name} col {x} major {major} beyond {n_bram}"
        for x, major in row["bram_reg_colMajors"].items():
            assert major < n_std, f"row {r} BRAM reg col {x} major {major} beyond {n_std}"
        for x, major in row["dsp_colMajors"].items():
            assert major < n_std, f"row {r} DSP col {x} major {major} beyond {n_std}"
    assert total_frames > 0
    print(f"slices: {total_slices} (header {out['num_slices']})")
    print(f"frames: {total_frames} (UG570 Table 1: {UG570_FRAMES}; "
          f"overshoot {total_frames - UG570_FRAMES})")
    print(f"provisional notes: {len(out['helion_provisional']['notes'])}")
    print("self-checks PASS")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--check", action="store_true",
                    help="verify the existing file, do not write")
    args = ap.parse_args()
    if args.check:
        out = json.loads(OUTPUT.read_text())
        slices = sum(
            len(row["clb_colMajors"]) * CLBS_PER_COLUMN
            for row in out["slrs"]["SLR0"]["rowMajors"].values()
        )
        frames = sum(
            sum(row["num_minors_per_std_colMajor"])
            + sum(row["num_minors_per_bram_content_colMajor"])
            for row in out["slrs"]["SLR0"]["rowMajors"].values()
        )
        check(out, slices, frames)
        return
    out, slices, frames = build()
    check(out, slices, frames)
    OUTPUT.write_text(json.dumps(out, indent=2, sort_keys=True) + "\n")
    print(f"wrote {OUTPUT} ({OUTPUT.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
