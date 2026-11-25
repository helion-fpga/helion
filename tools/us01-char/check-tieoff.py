#!/usr/bin/env python3
"""check-tieoff.py - US-01 characterization (W-CHAR).

Decisive test of the VCC-default rule: for every used LUT input pin
that the decoded FASM leaves without a driving PIP (the 700-pin
residual in P-11 under the final live-output rule, 1,824 in P-13),
look the same IMUX wire up in the raw prjuray baseline FASM. Expected: each is a `.VCC_WIRE` tie-off there (the
decoder drops VCC PIPs because the segbits DB encodes them as all
`!` bits, i.e. the mux default state; see NORMALIZATION_REPORT.md
rules R5 and section 2). Any pin with a real (non-VCC) baseline PIP
would be a decoder miss; any pin with neither would be genuinely
undetermined.
Usage: python3 check-tieoff.py <decoded-fasm> <baseline-fasm>
Output: JSON summary.
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
PIP_RE = re.compile(r"^(.+?)\.PIP\.([^.]+)\.(.+)$")


def depends(init, i):
    for j in range(64):
        if ((init >> j) & 1) != ((init >> (j ^ (1 << i))) & 1):
            return True
    return False


def load_pips(path, int_only=False):
    dst = defaultdict(set)
    src = defaultdict(set)
    with open(path) as f:
        for line in f:
            line = line.strip()
            if ".PIP." not in line:
                continue
            m = PIP_RE.match(line)
            if not m:
                continue
            tile, d, s = m.group(1), m.group(2), m.group(3)
            if int_only and not tile.startswith("INT_"):
                continue
            dst[(tile, d)].add(s)
            src[(tile, s)].add(d)
    return dst, src


def main():
    dec_fasm, base_fasm = sys.argv[1:3]
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
    cin_sites = set()
    with open(dec_fasm) as f:
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
            m = re.match(r"^(SLICE_X\d+Y\d+)\.CARRY8\.DI(\d)\.(\w+)$", line)
            if m:
                carry_di[(m.group(1), int(m.group(2)))] = m.group(3)
                continue
            if ".PIP." in line:
                pm = PIP_RE.match(line)
                if pm and pm.group(2).endswith("_SITE_0_CIN_PIN") \
                        and pm.group(3).endswith("_SITE_0_CIN"):
                    cin_sites.add(pm.group(1))
    dec_pips, dec_src = load_pips(dec_fasm)
    base_pips, _ = load_pips(base_fasm, int_only=True)

    # Direct O6 output loads per LUT (needed for the live-output rule:
    # a {L}_O pin with downstream INT PIP loads makes O6 live with no
    # OUTMUX/FFMUX selection; see analyze-drive.py).
    o_loads = {}
    for (site, letter), init in inits.items():
        tname, ttype, gx, gy = site_to_tile[site]
        pre = prefix(ttype)
        partners = [p for p in conn.get((ttype, pre + letter + "_O"), [])
                    if p[0].startswith("INT")]
        if not partners:
            continue
        ot, ow, dx, dy = partners[0]
        ntile = grid_to_tile.get((gx + dx, gy + dy))
        o_loads[(site, letter)] = len(dec_src.get((ntile, ow), ()))

    verdicts = Counter()
    problems = []
    for (site, letter), init in inits.items():
        if init == DEFAULT_INIT:
            continue
        fsel = {k: v for k, v in ffmux.get(site, {}).items()
                if k[0] == letter}
        osel = outmux.get(site, {}).get(letter)
        o5 = any(v == "D5" for v in fsel.values()) or osel == "D5"
        # O6 live paths: mux selection, direct {L}_O pin loads,
        # CARRY8.DI{i}.DI{i} feed, or COUT consumed by the slice above
        # (S[i] is the LUT O6 output; see analyze-drive.py).
        n = LETTERS.index(letter)
        m = re.match(r"SLICE_X(\d+)Y(\d+)$", site)
        above = f"SLICE_X{m.group(1)}Y{int(m.group(2)) + 1}" if m else None
        o6 = any(v in ("D6", "XORIN", "CY", "F7F8") for v in fsel.values()) \
            or osel in ("D6", "XORIN", "CY", "F7F8") \
            or o_loads.get((site, letter), 0) > 0 \
            or carry_di.get((site, n)) == f"DI{n}" \
            or (above is not None and above in cin_sites)
        tname, ttype, gx, gy = site_to_tile[site]
        pre = prefix(ttype)
        for i in range(6):
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
                verdicts["no_int_partner"] += 1
                continue
            ot, ow, dx, dy = partners[0]
            ntile = grid_to_tile.get((gx + dx, gy + dy))
            if (ntile, ow) in dec_pips:
                srcs = dec_pips[(ntile, ow)]
                verdicts["real_driven"] += 1
                if len(srcs) != 1:
                    problems.append((site, pin, "multi_src_decoded",
                                     sorted(srcs)))
                continue
            bsrcs = base_pips.get((ntile, ow), set())
            if bsrcs == {"VCC_WIRE"}:
                verdicts["vcc_tied"] += 1
            elif bsrcs:
                verdicts["baseline_real_only"] += 1
                problems.append((site, pin, "baseline_real_only",
                                 sorted(bsrcs)))
            else:
                verdicts["truly_undriven"] += 1
                problems.append((site, pin, "truly_undriven", []))
    print(json.dumps({
        "decoded": dec_fasm,
        "baseline": base_fasm,
        "verdicts": dict(verdicts),
        "problems": problems[:20],
        "n_problems": len(problems),
    }, indent=1))


if __name__ == "__main__":
    main()
