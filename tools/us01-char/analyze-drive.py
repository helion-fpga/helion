#!/usr/bin/env python3
"""analyze-drive.py - US-01 characterization (W-CHAR).

Bulk mineability check on one decoded pair (default P-11):
for every SLICE LUT, decide used vs unused from INIT (default
0x8000000080000000 = unused, verified on the corner slice SLICE_X0Y0),
then for each LUT input pin A1..A6 resolve the tileconn IMUX wire in
the neighboring INT tile and check whether a FASM INT PIP drives it.
Also reports INIT-dependence per input (bit-flip test on the 64-bit
INIT, assuming INIT bit j = A1 + 2*A2 + 4*A3 + 8*A4 + 16*A5 + 32*A6),
FF AQ forward-drive counts, and CLK-pin tileconn partners.

Usage: python3 analyze-drive.py <fasm> [P-11|...]
Output: JSON on stdout.
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


def init_depends(init, pin_index):
    # pin_index 0..5 for A1..A6; INIT bit j uses A1 as LSB.
    for j in range(64):
        if ((init >> j) & 1) != ((init >> (j ^ (1 << pin_index))) & 1):
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
        if ttype.startswith("CLEL"):
            return "CLE_CLE_L_SITE_0_"
        if ttype.startswith("CLEM"):
            return "CLE_CLE_M_SITE_0_"
        return None

    inits = {}   # (site, letter) -> int
    ffmux = {}    # (site, letter, num) -> selection
    outmux = {}   # (site, letter) -> selection
    carry_di = {}  # (site, n) -> selection (AX default / DI{n})
    pips_dst = defaultdict(set)   # (tile, dst_wire) -> {src}
    pips_src = defaultdict(set)   # (tile, src_wire) -> {dst}
    cin_pips = set()
    with open(fasm) as f:
        for line in f:
            line = line.strip()
            m = INIT_RE.match(line)
            if m:
                inits[(m.group(1), m.group(2))] = int(m.group(3), 16)
                continue
            m = FFMUX_RE.match(line)
            if m:
                ffmux[(m.group(1), m.group(2), m.group(3))] = m.group(4)
                continue
            m = OUTMUX_RE.match(line)
            if m:
                outmux[(m.group(1), m.group(2))] = m.group(3)
                continue
            m = re.match(r"^(SLICE_X\d+Y\d+)\.CARRY8\.DI(\d)\.(\w+)$", line)
            if m:
                carry_di[(m.group(1), int(m.group(2)))] = m.group(3)
                continue
            if ".PIP." in line:
                pm = PIP_RE.match(line)
                if pm:
                    tile, dst, src = pm.group(1), pm.group(2), pm.group(3)
                    pips_dst[(tile, dst)].add(src)
                    pips_src[(tile, src)].add(dst)
                    if tile.startswith("SLICE_"):
                        cin_pips.add(tile)
                continue

    def live_outputs(site, letter):
        # O5 live if any D5 selection; O6 live if D6 / XORIN / CY / F7F8,
        # or if the LUT's {L}_O site pin has downstream INT PIP loads:
        # the O6 output drives fabric directly through the {L}_O pin
        # with NO OUTMUX/FFMUX selection (extract.py's OUTMUX-missing
        # error only concerns the {L}MUX pin, lines 660-697).
        # O6 is also live via the carry chain: CARRY8.DI{i}.DI{i} feeds
        # the LUT O6 straight into the carry DI mux (extract.py lines
        # 24-26, 810-815), and S[i] is the LUT O6 output, so when this
        # slice's COUT is consumed (the slice above has a CIN site PIP)
        # every S[i] matters through the ripple.
        o5 = o6 = False
        for (s, l, n), sel in ffmux.items():
            if s == site and l == letter:
                if sel == "D5":
                    o5 = True
                elif sel in ("D6", "XORIN", "CY", "F7F8"):
                    o6 = True
        sel = outmux.get((site, letter))
        if sel == "D5":
            o5 = True
        elif sel in ("D6", "XORIN", "CY", "F7F8"):
            o6 = True
        if o_loads.get((site, letter), 0) > 0:
            o6 = True
        n = LETTERS.index(letter)
        if carry_di.get((site, n)) == f"DI{n}":
            o6 = True
        m = re.match(r"SLICE_X(\d+)Y(\d+)$", site)
        if m:
            above = f"SLICE_X{m.group(1)}Y{int(m.group(2)) + 1}"
            if above in cin_pips:
                o6 = True
        return o5, o6

    def half_depends(init, pin_index):
        # dependence within the low 32-bit half (5-LUT mode, A1..A5)
        for j in range(32):
            if ((init >> j) & 1) != ((init >> (j ^ (1 << pin_index))) & 1):
                return True
        return False

    # Direct O6 output loads: PIPs driven by each LUT's {L}_O site pin
    # in the neighboring INT tile. Computed before the pin loop because
    # live_outputs() needs it.
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
        o_loads[(site, letter)] = len(pips_src.get((ntile, ow), ()))

    used_luts = {k: v for k, v in inits.items() if v != DEFAULT_INIT}
    stats = Counter()
    mode_counts = Counter()
    undriven_needed_examples = []
    no_live_output = 0
    for (site, letter), init in inits.items():
        used = init != DEFAULT_INIT
        o5, o6 = live_outputs(site, letter)
        if used:
            mode_counts[(o5, o6)] += 1
        if used and not o5 and not o6:
            no_live_output += 1
        tname, ttype, gx, gy = site_to_tile[site]
        pre = prefix(ttype)
        for i in range(6):
            pin = f"{letter}{i+1}"
            if i < 5:
                # A1..A5: needed if any live output's function depends on it.
                # (Full 64-bit test covers both halves; the O5 half test
                # is subsumed.)
                needed = (o5 or o6) and init_depends(init, i)
            else:
                # A6: a real input only in true 6-LUT mode, i.e. O6 live
                # WITHOUT O5 live. When both O5 and O6 are live the LUT
                # is two 5-LUTs sharing A1..A5 and A6 is not an input;
                # the low/high halves differ because they are two
                # different 5-input functions, not because A6 matters.
                needed = o6 and not o5 and init_depends(init, i)
            partners = [p for p in conn.get((ttype, pre + pin), [])
                        if p[0].startswith("INT")]
            if not partners:
                stats[("no_int_partner", used)] += 1
                continue
            ot, ow, dx, dy = partners[0]
            ntile = grid_to_tile.get((gx + dx, gy + dy))
            driven = (ntile, ow) in pips_dst
            stats[("pin", used, driven, needed)] += 1
            if used and needed and not driven and len(undriven_needed_examples) < 10:
                undriven_needed_examples.append(
                    (site, pin, hex(init), ntile, ow, o5, o6))

    # FF AQ forward drive: for slices with any FFMUX entry (FF in use)
    aq_stats = Counter()
    ff_slices = {s for (s, l, n) in ffmux}
    for site in ff_slices:
        tname, ttype, gx, gy = site_to_tile[site]
        pre = prefix(ttype)
        for letter in LETTERS:
            pin = f"{letter}Q"
            partners = [p for p in conn.get((ttype, pre + pin), [])
                        if p[0].startswith("INT")]
            if not partners:
                continue
            ot, ow, dx, dy = partners[0]
            ntile = grid_to_tile.get((gx + dx, gy + dy))
            loads = len(pips_src.get((ntile, ow), ()))
            aq_stats[("AQ_loads", min(loads, 3))] += 1
            break

    # CLK / control pin tileconn partners (static, same for all sites)
    ctrl = {}
    for pin in ["CLK1", "CLK2", "CKEN1", "SRST1", "LCLK", "WCKEN"]:
        ps = set()
        for ttype in ("CLEL_L", "CLEL_R", "CLEM", "CLEM_R"):
            pre = prefix(ttype)
            for (ot, ow, dx, dy) in conn.get((ttype, pre + pin), []):
                ps.add((ot, ow, dx, dy))
        ctrl[pin] = sorted(ps)

    print(json.dumps({
        "fasm": fasm,
        "lut_inits_total": len(inits),
        "used_luts": len(used_luts),
        "unused_luts": len(inits) - len(used_luts),
        "used_lut_modes_o5_o6": {str(k): v for k, v in sorted(mode_counts.items())},
        "used_luts_no_live_output": no_live_output,
        "pin_stats_used_driven_needed": {str(k): v for k, v in sorted(stats.items())},
        "undriven_needed_examples": undriven_needed_examples,
        "ff_slices_with_ffmux": len(ff_slices),
        "cin_pip_slices": len(cin_pips),
        "aq_loads": {str(k): v for k, v in sorted(aq_stats.items())},
        "ffmux_values": dict(Counter(ffmux.values()).most_common()),
        "outmux_values": dict(Counter(outmux.values()).most_common()),
        "ctrl_pin_partners": {k: [list(x) for x in v] for k, v in ctrl.items()},
    }, indent=1))


if __name__ == "__main__":
    main()
