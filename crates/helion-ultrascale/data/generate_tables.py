#!/usr/bin/env python3
"""Extract the LUT/FF/BRAM bit-location tables from the BitFiltrator arch JSON.

Source: bitfiltrator-ref/resources/archs/ULTRASCALE_PLUS.json (BitFiltrator,
EPFL VLSC, MIT License, Copyright (c) 2022 EPFL VLSC). Only the four tables
the lut-ff-bram worker consumes are copied, verbatim, with no values changed:
LutLoc and RegLoc for all four CLB tile types (CLEL_L, CLEL_R, CLEM, CLEM_R),
and BramMemLoc plus BramMemParityLoc. Everything else in the source file is
dropped.

Usage:
    python3 generate_tables.py [path/to/ULTRASCALE_PLUS.json] [output.json]

Defaults: source is the bitfiltrator-ref clone in the fm/ultra-bitstream
worktree, output is ultrascale_plus_lut_ff_bram.json next to this script.
The script prints the source SHA-256 and the extracted table counts; the
SHA-256 must match the value recorded in README.md, or the source changed
and the extraction needs a fresh review.
"""

import hashlib
import json
import sys
from pathlib import Path

DEFAULT_SOURCE = Path(
    "/home/hatch/workspace/helion-ultra-bitstream/bitfiltrator-ref"
    "/resources/archs/ULTRASCALE_PLUS.json"
)
EXPECTED_SHA256 = (
    "c6320fe7322d918fc3995f9122f62b0f4ab7b282b517bf8231d0c63ac70b436c"
)

CLB_TILE_TYPES = ["CLEL_L", "CLEL_R", "CLEM", "CLEM_R"]


def main() -> int:
    source = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_SOURCE
    output = (
        Path(sys.argv[2])
        if len(sys.argv) > 2
        else Path(__file__).with_name("ultrascale_plus_lut_ff_bram.json")
    )
    raw = source.read_bytes()
    digest = hashlib.sha256(raw).hexdigest()
    print(f"source: {source}")
    print(f"source sha256: {digest}")
    if digest != EXPECTED_SHA256:
        print(
            "WARNING: source SHA-256 differs from README.md. "
            "The upstream file changed; review the diff before using "
            "the extracted tables."
        )
    data = json.loads(raw)
    out = {}
    for tile_type in CLB_TILE_TYPES:
        out[tile_type] = {
            "LutLoc": data[tile_type]["LutLoc"],
            "RegLoc": data[tile_type]["RegLoc"],
        }
    out["BRAM"] = {
        "BramMemLoc": data["BRAM"]["BramMemLoc"],
        "BramMemParityLoc": data["BRAM"]["BramMemParityLoc"],
    }
    # Sanity counts, matching the critic base review (P4).
    for tile_type in CLB_TILE_TYPES:
        y_lut = out[tile_type]["LutLoc"]["Y_ofst"]
        y_reg = out[tile_type]["RegLoc"]["Y_ofst"]
        assert len(y_lut) == 60, f"{tile_type} LutLoc Y entries"
        assert len(y_reg) == 60, f"{tile_type} RegLoc Y entries"
        for y in y_lut.values():
            assert len(y["frame_ofst"]) == 8 and len(y["minor"]) == 8
            for lut in y["frame_ofst"].values():
                assert len(lut) == 64
        for y in y_reg.values():
            assert len(y["frame_ofst"]) == 16 and len(y["minor"]) == 16
    for table, count in (
        ("BramMemLoc", 16384),
        ("BramMemParityLoc", 2048),
    ):
        y_entries = out["BRAM"][table]["Y_ofst"]
        assert len(y_entries) == 24, f"{table} Y entries"
        for y in y_entries.values():
            assert len(y["frame_ofst"]) == count
            assert len(y["minor"]) == count
    output.write_text(json.dumps(out, separators=(",", ":")))
    print(f"wrote: {output} ({output.stat().st_size} bytes)")
    print("counts: LutLoc 60 Y x 8 LUTs x 64 bits, "
          "RegLoc 60 Y x 16 FFs, "
          "BramMemLoc 24 Y x 16384 bits, "
          "BramMemParityLoc 24 Y x 2048 bits")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
