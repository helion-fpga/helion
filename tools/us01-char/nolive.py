#!/usr/bin/env python3
"""nolive.py - US-01 characterization (W-CHAR).

Breaks down used LUTs with no FFMUX/OUTMUX live output: by tile
type (SLICEL vs SLICEM), by CARRY8 DI mux value for the letter, by
LUT MODE (RAM/SRL/LUT), and by whether the LUT site output pin
({L}_O -> LOGIC_OUTS) has downstream PIP loads.
Usage: python3 nolive.py <fasm>
Output: JSON summary.
"""
import json
import os
import re
import sys
from collections import Counter, defaultdict

DB = os.path.expanduser("~/workspace/prjuray-db-ref/zynqusp")
PART = "xczu3eg-sbva484-1-e"
DEFAULT_INIT = 0x8000000080000000
LETTERS = "ABCDEFGH"

INIT_RE = re.compile(r"^(SLICE_X\d+Y\d+)\.([A-H])6LUT\.INIT\[63:0\] = 0x([0-9a-f]+)$")
FFMUX_RE = re.compile(r"^(SLICE_X\d+Y\d+)\.FFMUX([A-H])([12])\.(\w+)$")
OUTMUX_RE = re.compile(r"^(SLICE_X\d+Y\d+)\.OUTMUX([A-H])\.(\w+)$")
DI_RE = re.compile(r"^(SLICE_X\d+Y\d+)\.CARRY8\.DI(\d)\.(\w+)$")
MODE_RE = re.compile(r"^(SLICE_X\d+Y\d+)\.([A-H])LUT\.MODE\.(\w+)$")
PIP_RE = re.compile(r"^(.+?)\.PIP\.([^.]+)\.(.+)$")


def main():
    fasm = sys.argv[1]
    tg = json.load(open(f"{DB}/{PART}/tilegrid.json"))
    site_to_tile = {}
    for tname, t in tg.items():
        for sname in (t.get("sites") or {}):
            site_to_tile[sname] = (tname, t["type"], t["grid_x"], t["grid_y"])
    grid_to_tile = {(t["grid_x"], t["grid_y"]): n for n, t in tg.items()}
    tc = json.load(open(f"{DB}/{PART}/tileconn.json"))
    conn = defaultdict(list)
    for e in tc:
        a, b = e["tile_types"]
        dx, dy = e["grid_deltas"]
        for wa, wb in e["wire_pairs"]:
            conn[(a, wa)].append((b, wb, dx, dy))
            conn[(b, wb)].append((a, wa, -dx, -dy))

    def prefix(ttype):
        return "CLE_CLE_L_SITE_0_" if ttype.startswith("CLEL") \
            else "CLE_CLE_M_SITE_0_"

    inits = {}
    ffmux = defaultdict(set)
    outmux = defaultdict(set)
    carry_di = {}
    modes = {}
    pips_src = defaultdict(set)
    with open(fasm) as f:
        for line in f:
            line = line.strip()
            m = INIT_RE.match(line)
            if m:
                inits[(m.group(1), m.group(2))] = int(m.group(3), 16)
                continue
            m = FFMUX_RE.match(line)
            if m:
                ffmux[m.group(1)].add(m.group(2))
                continue
            m = OUTMUX_RE.match(line)
            if m:
                outmux[m.group(1)].add(m.group(2))
                continue
            m = DI_RE.match(line)
            if m:
                carry_di[(m.group(1), int(m.group(2)))] = m.group(3)
                continue
            m = MODE_RE.match(line)
            if m:
                modes[(m.group(1), m.group(2))] = m.group(3)
                continue
            if ".PIP." in line:
                pm = PIP_RE.match(line)
                if pm:
                    pips_src[(pm.group(1), pm.group(3))].add(pm.group(2))

    rows = []
    for (site, letter), init in inits.items():
        if init == DEFAULT_INIT:
            continue
        if letter in ffmux.get(site, set()) or letter in outmux.get(site, set()):
            continue
        tname, ttype, gx, gy = site_to_tile[site]
        pre = prefix(ttype)
        di = carry_di.get((site, LETTERS.index(letter)))
        opartners = [p for p in conn.get((ttype, pre + letter + "_O"), [])
                     if p[0].startswith("INT")]
        oloads = 0
        if opartners:
            oot, oow, odx, ody = opartners[0]
            ontile = grid_to_tile.get((gx + odx, gy + ody))
            oloads = len(pips_src.get((ontile, oow), ()))
        mpartners = [p for p in conn.get((ttype, pre + letter + "MUX"), [])
                     if p[0].startswith("INT")]
        mloads = 0
        if mpartners:
            mot, mow, mdx, mdy = mpartners[0]
            mntile = grid_to_tile.get((gx + mdx, gy + mdy))
            mloads = len(pips_src.get((mntile, mow), ()))
        rows.append({
            "site": site, "letter": letter,
            "tile_type": ttype, "mode": modes.get((site, letter)),
            "carry_di": di, "o_loads": oloads, "mux_loads": mloads,
        })
    print(json.dumps({
        "fasm": fasm,
        "no_live_output_used_luts": len(rows),
        "by_tile_type": dict(Counter(r["tile_type"] for r in rows)),
        "by_mode": dict(Counter(str(r["mode"]) for r in rows)),
        "by_carry_di": dict(Counter(str(r["carry_di"]) for r in rows)),
        "o_loads_zero": sum(1 for r in rows if r["o_loads"] == 0),
        "mux_loads_zero": sum(1 for r in rows if r["mux_loads"] == 0),
        "both_loads_zero": sum(1 for r in rows
                               if r["o_loads"] == 0 and r["mux_loads"] == 0),
        "examples": rows[:10],
    }, indent=1))


if __name__ == "__main__":
    main()
