#!/usr/bin/env python3
"""classify-fasm.py - US-01 characterization (W-CHAR).

Classifies every FASM line in a decoded pair file into:
  (a) BEL-pin-touching features (SLICE LUT/FF/CARRY/MUXF config, etc.)
  (b) INT PIP features (INT* tiles, .PIP.)
  (c) CLB-internal / site PIP features (PIP on non-INT tiles)
  (d) everything else (IO, BRAM, clock, config, WIRE, etc.)
Also splits (a) into LUT / FF / CARRY8 / MUXF / other-SLICE and (d)
into BRAM / DSP / clock / IO / other, and records exact example lines.

Line shape: TILE.FEATURE[.SUB...] [= VALUE] or TILE.FEATURE[hi:lo] = VALUE.
TILE is like SLICE_X0Y0, INT_X0Y1, BRAM_X0Y0, CMT_RIGHT_X32Y0.
Usage: python3 classify-fasm.py <fasm-file> [<fasm-file> ...]
Output: one JSON object per file on stdout.
"""
import json
import re
import sys
from collections import Counter

TILE_RE = re.compile(r"^([A-Z0-9_]+?)_X\d+Y\d+\.(.*)$")
LUT_PINS = ("A1", "A2", "A3", "A4", "A5", "A6", "B1", "B2", "B3", "B4",
            "B5", "B6", "C1", "C2", "C3", "C4", "C5", "C6", "D1", "D2",
            "D3", "D4", "D5", "D6", "E1", "E2", "E3", "E4", "E5", "E6",
            "F1", "F2", "F3", "F4", "F5", "F6", "G1", "G2", "G3", "G4",
            "G5", "G6", "H1", "H2", "H3", "H4", "H5", "H6")


def tile_type(tile):
    m = re.match(r"^([A-Z]+(?:_[A-Z]+)*?)_X\d+Y\d+$", tile)
    if m:
        return m.group(1)
    # tiles like CMT_RIGHT_X32Y0, INT_INTF_L_X0Y0, RCLK_BRAM_INTF_TD_L_X0Y0
    m = re.match(r"^(.*)_X\d+Y\d+$", tile)
    return m.group(1) if m else tile


def classify(line):
    line = line.strip()
    if not line:
        return None, None, None
    feat = line.split(" =", 1)[0].split("=", 1)[0].strip()
    m = TILE_RE.match(feat)
    if not m:
        return "unparsed", line, feat
    tile, rest = m.group(1), m.group(2)
    # tile here is the full tile name minus the _X..Y.. suffix? No:
    # TILE_RE group(1) is lazy up to _X..Y.. ; rest is the feature part.
    # Rebuild full tile name:
    full_tile = feat[: len(feat) - len(rest) - 1]
    ttype = tile_type(full_tile)
    first = rest.split(".")[0]
    is_pip = rest.startswith("PIP.")
    if is_pip:
        if ttype.startswith("INT"):
            return "b_int_pip", line, feat
        if full_tile.startswith("SLICE_") or full_tile.startswith("BRAM") \
                or full_tile.startswith("DSP") or "IOB" in ttype:
            return "c_site_pip", line, feat
        return "d_pip_other", line, feat
    if full_tile.startswith("SLICE_"):
        # BEL-pin-touching SLICE config
        if re.match(r"^[A-H]6LUT", first) or first in ("A5LUT", "B5LUT",
                "C5LUT", "D5LUT", "E5LUT", "F5LUT", "G5LUT", "H5LUT"):
            return "a_lut", line, feat
        if re.match(r"^[A-H]FF", first) or first in ("AFF2", "BFF2",
                "CFF2", "DFF2", "EFF2", "FFF2", "GFF2", "HFF2"):
            return "a_ff", line, feat
        if first.startswith("CARRY8") or "CARRY" in first:
            return "a_carry8", line, feat
        if first.startswith("F7MUX") or first.startswith("F8MUX") or \
                first.startswith("F9MUX") or first in ("MUXF7", "MUXF8"):
            return "a_muxf", line, feat
        return "a_slice_other", line, feat
    if ttype.startswith("CMT") or "CLK" in ttype or ttype.startswith("RCLK") \
            or "MMCM" in rest or "PLL" in rest or ttype.startswith("XIPHY"):
        return "d_clock", line, feat
    if full_tile.startswith("BRAM") or full_tile.startswith("RAMB18") \
            or full_tile.startswith("RAMB36"):
        return "d_bram", line, feat
    if full_tile.startswith("DSP"):
        return "d_dsp", line, feat
    if "IOB" in ttype or ttype.startswith("HPIO") or ttype.startswith("HDIO") \
            or "IO" in ttype:
        return "d_io", line, feat
    if ".WIRE." in rest:
        return "d_wire", line, feat
    return "d_other", line, feat


def main():
    for path in sys.argv[1:]:
        counts = Counter()
        examples = {}
        sub = Counter()
        with open(path) as f:
            for line in f:
                cls, raw, feat = classify(line)
                if cls is None:
                    continue
                counts[cls] += 1
                if cls not in examples:
                    examples[cls] = raw.strip()
                # sub-counts for SLICE feature first tokens
                if cls.startswith("a_"):
                    first = feat.split(".", 1)[1].split(".")[0] \
                        if "." in feat else feat
                    sub[first] += 1
        total = sum(counts.values())
        print(json.dumps({
            "file": path,
            "total": total,
            "counts": dict(sorted(counts.items())),
            "examples": examples,
            "slice_first_tokens_top": dict(sub.most_common(40)),
        }, indent=1))


if __name__ == "__main__":
    main()
