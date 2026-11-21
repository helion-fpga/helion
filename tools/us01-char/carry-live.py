#!/usr/bin/env python3
"""carry-live.py - US-01 characterization (W-CHAR).

Measures the two remaining carry-related LUT liveness paths that
analyze-drive.py does not model:
(a) CARRY8.DI{i}.DI{i}: the LUT O6 output feeds the carry DI mux
    directly (extract.py: DI{i}.DI{i} is the LUT-fed DI, unmodelled
    and fails loud). Any used LUT with DI{i}.DI{i} and no mux-live
    output and no O-pin loads is live ONLY via the carry DI.
(b) S[i] = LUT O6 always; S[i] feeds the carry chain, so if the
    slice above continues the chain (has a CIN site PIP, i.e. this
    slice's COUT is consumed) and this slice has no CY/XORIN/DI/CI
    selection at all, the LUT O6s still matter through CO propagation.
Usage: python3 carry-live.py <fasm>
Output: JSON summary.
"""
import json
import re
import sys
from collections import defaultdict

DB = "/home/hatch/workspace/prjuray-db-ref/zynqusp"
PART = "xczu3eg-sbva484-1-e"
DEFAULT_INIT = 0x8000000080000000
LETTERS = "ABCDEFGH"

INIT_RE = re.compile(r"^(SLICE_X\d+Y\d+)\.([A-H])6LUT\.INIT\[63:0\] = 0x([0-9a-f]+)$")
FFMUX_RE = re.compile(r"^(SLICE_X\d+Y\d+)\.FFMUX([A-H])([12])\.(\w+)$")
OUTMUX_RE = re.compile(r"^(SLICE_X\d+Y\d+)\.OUTMUX([A-H])\.(\w+)$")
DI_RE = re.compile(r"^(SLICE_X\d+Y\d+)\.CARRY8\.DI(\d)\.(\w+)$")
CI_RE = re.compile(r"^(SLICE_X\d+Y\d+)\.CARRY8\.CI(_TOP)?\.(\w+)$")
CIN_RE = re.compile(r"^(SLICE_X\d+Y\d+)\.PIP\.CLE_CLE_[LM]_SITE_0_CIN_PIN\.CLE_CLE_[LM]_SITE_0_CIN$")
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
    ffmux = defaultdict(dict)
    outmux = defaultdict(dict)
    carry_di = {}
    ci_sel = defaultdict(set)
    cin_sites = set()
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
                ffmux[m.group(1)][(m.group(2), m.group(3))] = m.group(4)
                continue
            m = OUTMUX_RE.match(line)
            if m:
                outmux[m.group(1)][m.group(2)] = m.group(3)
                continue
            m = DI_RE.match(line)
            if m:
                carry_di[(m.group(1), int(m.group(2)))] = m.group(3)
                continue
            m = CI_RE.match(line)
            if m:
                ci_sel[m.group(1)].add(m.group(3))
                continue
            if CIN_RE.match(line):
                cin_sites.add(CIN_RE.match(line).group(1))
                continue
            if ".PIP." in line:
                pm = PIP_RE.match(line)
                if pm:
                    pips_src[(pm.group(1), pm.group(3))].add(pm.group(2))

    # (a) DI-fed-only LUTs
    di_only = []
    for (site, letter), init in inits.items():
        if init == DEFAULT_INIT:
            continue
        fsel = [v for (l, n), v in ffmux.get(site, {}).items()
                if l == letter]
        osel = outmux.get(site, {}).get(letter)
        if any(v == "D5" for v in fsel) or osel == "D5":
            continue
        if any(v in ("D6", "XORIN", "CY", "F7F8") for v in fsel) \
                or osel in ("D6", "XORIN", "CY", "F7F8"):
            continue
        tname, ttype, gx, gy = site_to_tile[site]
        pre = prefix(ttype)
        op = [p for p in conn.get((ttype, pre + letter + "_O"), [])
              if p[0].startswith("INT")]
        oloads = 0
        if op:
            oot, oow, odx, ody = op[0]
            oloads = len(pips_src.get(
                (grid_to_tile.get((gx + odx, gy + ody)), oow), ()))
        if oloads > 0:
            continue
        n = LETTERS.index(letter)
        if carry_di.get((site, n)) == f"DI{n}":
            di_only.append((site, letter))

    # (b) slices whose COUT is consumed (slice above has CIN PIP) but
    # which have no CY/XORIN/DI/CI selection at all
    def site_xy(site):
        m = re.match(r"SLICE_X(\d+)Y(\d+)$", site)
        return int(m.group(1)), int(m.group(2))

    carry_slices = set(carry_di) | set(ci_sel)
    carry_slices = {s for (s, i) in carry_di} | set(ci_sel)
    for site in list(ffmux):
        for (l, n), v in ffmux[site].items():
            if v in ("CY", "XORIN"):
                carry_slices.add(site)
    for site in list(outmux):
        for l, v in outmux[site].items():
            if v in ("CY", "XORIN"):
                carry_slices.add(site)
    for (site, n), v in carry_di.items():
        if v == f"DI{n}":
            carry_slices.add(site)
    cout_only = []
    for site in carry_slices:
        x, y = site_xy(site)
        above = f"SLICE_X{x}Y{y+1}"
        if above in cin_sites:
            # slice above continues the chain; this slice's COUT matters
            # only if the slice has no other carry-use selection
            fsel = list(ffmux.get(site, {}).values())
            osel = list(outmux.get(site, {}).values())
            has_use = any(v in ("CY", "XORIN") for v in fsel + osel) \
                or any(carry_di.get((site, i)) == f"DI{i}"
                       for i in range(8)) \
                or bool(ci_sel.get(site))
            if not has_use:
                cout_only.append(site)
    print(json.dumps({
        "fasm": fasm,
        "di_fed_only_luts": len(di_only),
        "di_fed_only_examples": di_only[:10],
        "cout_continued_no_other_use_slices": len(cout_only),
        "cout_only_examples": cout_only[:10],
    }, indent=1))


if __name__ == "__main__":
    main()
