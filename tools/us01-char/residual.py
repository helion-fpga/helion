#!/usr/bin/env python3
"""residual.py - US-01 characterization (W-CHAR).

Explains the residual undriven-but-apparently-needed LUT input pins
from analyze-drive.py: for each, dumps the slice features that made
the output look live (FFMUX/OUTMUX selection), the CARRY8 DI mux value
for that letter, and whether the LUT's site output pin and the FF Q
have downstream PIP loads. Tests the hypothesis that the residual is
the carry-DI path (DI fed by the LUT O5, so the 6th input is moot).
Usage: python3 residual.py <fasm>
Output: JSON summary + up to 10 examples.
"""
import json
import re
import sys
from collections import Counter, defaultdict

DB = "/home/hatch/workspace/prjuray-db-ref/zynqusp"
PART = "xczu3eg-sbva484-1-e"
DEFAULT_INIT = 0x8000000080000000
LETTERS = "ABCDEFGH"

INIT_RE = re.compile(r"^(SLICE_X\d+Y\d+)\.([A-H])6LUT\.INIT\[63:0\] = 0x([0-9a-f]+)$")
FFMUX_RE = re.compile(r"^(SLICE_X\d+Y\d+)\.FFMUX([A-H])([12])\.(\w+)$")
OUTMUX_RE = re.compile(r"^(SLICE_X\d+Y\d+)\.OUTMUX([A-H])\.(\w+)$")
DI_RE = re.compile(r"^(SLICE_X\d+Y\d+)\.CARRY8\.DI(\d)\.(\w+)$")
PIP_RE = re.compile(r"^(.+?)\.PIP\.([^.]+)\.(.+)$")


def depends(init, i):
    for j in range(64):
        if ((init >> j) & 1) != ((init >> (j ^ (1 << i))) & 1):
            return True
    return False


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
    ffmux = defaultdict(dict)   # site -> {(letter,num): sel}
    outmux = defaultdict(dict)  # site -> {letter: sel}
    carry_di = {}               # (site, n) -> sel
    pips_dst = defaultdict(set)
    pips_src = defaultdict(set)
    cin_sites = set()           # sites with a CIN site PIP
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
            if ".PIP." in line:
                pm = PIP_RE.match(line)
                if pm:
                    tile, dst, src = pm.group(1), pm.group(2), pm.group(3)
                    pips_dst[(tile, dst)].add(src)
                    pips_src[(tile, src)].add(dst)
                    if dst.endswith("_SITE_0_CIN_PIN") \
                            and src.endswith("_SITE_0_CIN"):
                        cin_sites.add(tile)

    residual = []
    for (site, letter), init in inits.items():
        if init == DEFAULT_INIT:
            continue
        fsel = {k: v for k, v in ffmux.get(site, {}).items()
                if k[0] == letter}
        osel = outmux.get(site, {}).get(letter)
        n = LETTERS.index(letter)
        di = carry_di.get((site, n))
        tname, ttype, gx, gy = site_to_tile[site]
        pre = prefix(ttype)
        o5 = any(v == "D5" for v in fsel.values()) or osel == "D5"
        opartners = [p for p in conn.get((ttype, pre + letter + "_O"), [])
                     if p[0].startswith("INT")]
        oloads = 0
        if opartners:
            oot, oow, odx, ody = opartners[0]
            ontile = grid_to_tile.get((gx + odx, gy + ody))
            oloads = len(pips_src.get((ontile, oow), ()))
        # O6 is also live when the {L}_O site pin drives fabric directly
        # with no OUTMUX/FFMUX selection (see analyze-drive.py), when
        # CARRY8.DI{i}.DI{i} feeds the LUT O6 into the carry DI mux, or
        # when this slice's COUT is consumed (slice above has a CIN
        # site PIP), since S[i] is the LUT O6 output.
        m = re.match(r"SLICE_X(\d+)Y(\d+)$", site)
        above = None
        if m:
            above = f"SLICE_X{m.group(1)}Y{int(m.group(2)) + 1}"
        o6 = any(v in ("D6", "XORIN", "CY", "F7F8") for v in fsel.values()) \
            or osel in ("D6", "XORIN", "CY", "F7F8") or oloads > 0 \
            or di == f"DI{n}" or (above is not None and above in cin_sites)
        for i in range(6):
            # Same needed rule as analyze-drive.py: A1..A5 need any
            # live output to depend on them; A6 needs O6-live-only
            # (true 6-LUT mode) plus full dependence.
            if i < 5:
                needed = (o5 or o6) and depends(init, i)
            else:
                needed = o6 and not o5 and depends(init, i)
            if not needed:
                continue
            pin = f"{letter}{i+1}"
            partners = [p for p in conn.get((ttype, pre + pin), [])
                        if p[0].startswith("INT")]
            if not partners:
                continue
            ot, ow, dx, dy = partners[0]
            ntile = grid_to_tile.get((gx + dx, gy + dy))
            if (ntile, ow) in pips_dst:
                continue
            # downstream loads of the FF Q (LUT O loads already computed
            # above, before the needed test)
            qpartners = [p for p in conn.get((ttype, pre + letter + "Q"), [])
                         if p[0].startswith("INT")]
            qloads = 0
            if qpartners:
                qot, qow, qdx, qdy = qpartners[0]
                qntile = grid_to_tile.get((gx + qdx, gy + qdy))
                qloads = len(pips_src.get((qntile, qow), ()))
            residual.append({
                "site": site, "pin": pin, "init": hex(init),
                "ffmux": {f"{k[0]}{k[1]}": v for k, v in fsel.items()},
                "outmux": osel, "carry_di": di,
                "lut_out_loads": oloads, "ff_q_loads": qloads,
            })
    print(json.dumps({
        "fasm": fasm,
        "residual_count": len(residual),
        "by_pin": dict(Counter(r["pin"][-1] for r in residual)),
        "by_carry_di": dict(Counter(str(r["carry_di"]) for r in residual)),
        "by_outmux": dict(Counter(str(r["outmux"]) for r in residual)),
        "by_ffmux": dict(Counter(str(sorted(r["ffmux"].values())) for r in residual)),
        "lut_out_loads_zero": sum(1 for r in residual if r["lut_out_loads"] == 0),
        "ff_q_loads_zero": sum(1 for r in residual if r["ff_q_loads"] == 0),
        "both_loads_zero": sum(1 for r in residual
                               if r["lut_out_loads"] == 0 and r["ff_q_loads"] == 0),
        "either_load_nonzero": sum(1 for r in residual
                                   if r["lut_out_loads"] > 0 or r["ff_q_loads"] > 0),
        "examples": residual[:10],
    }, indent=1))


if __name__ == "__main__":
    main()
