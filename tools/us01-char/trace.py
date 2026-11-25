#!/usr/bin/env python3
"""trace.py - US-01 characterization (W-CHAR).

Traces one BEL pin's connection through the decoded FASM plus the
public prjuray DBs (tilegrid, tileconn, tile_types, site_types).

Chain for a LUT input pin (sink):
  site pin wire (tile_type_CLE*.wires) --tileconn--> INT wire
  (e.g. CLE_CLE_L_SITE_0_A1 -> IMUX_W10) in the neighboring INT tile
  --INT PIP (FASM PIP.<dst>.<src>, src -> dst)--> INT_NODE wire
  --further PIPs / tileconn stitches--> driver (a LOGIC_OUTS wire
  from another slice, a BOUNCE wire, VCC, ...).

Chain for a BEL output pin (source) runs the same edges forward:
  AQ --tileconn--> LOGIC_OUTS_W14 --INT PIPs with src LOGIC_OUTS_W14-->
  INT_NODE wires --> ... --> sink IMUX wires of other slices.

Usage:
  python3 trace.py <fasm> <SITE_XxYy> <PIN> <sink|source> [max_depth]
Example:
  python3 trace.py P-11.fasm SLICE_X10Y46 A1 sink
Prints each hop with the exact FASM line (or tileconn entry) behind it.
"""
import json
import os
import re
import sys
from collections import defaultdict

DB = os.path.expanduser("~/workspace/prjuray-db-ref/zynqusp")
PART = "xczu3eg-sbva484-1-e"

PIP_RE = re.compile(r"^(.+?)_X\d+Y\d+\.PIP\.([^.]+)\.(.+)$")


def load_tilegrid():
    tg = json.load(open(f"{DB}/{PART}/tilegrid.json"))
    site_to_tile = {}
    grid_to_tile = {}
    for tname, t in tg.items():
        grid_to_tile[(t["grid_x"], t["grid_y"])] = tname
        for sname, stype in (t.get("sites") or {}).items():
            site_to_tile[sname] = (tname, t["type"], t["grid_x"], t["grid_y"])
    return tg, site_to_tile, grid_to_tile


def load_tileconn():
    tc = json.load(open(f"{DB}/{PART}/tileconn.json"))
    # (tile_type, wire) -> list of (other_type, other_wire, dx, dy)
    # entry: tile_types [A, B], grid_deltas [dx, dy] meaning B sits at
    # A + delta (verified empirically in the report).
    m = defaultdict(list)
    for e in tc:
        a, b = e["tile_types"]
        dx, dy = e["grid_deltas"]
        for wa, wb in e["wire_pairs"]:
            m[(a, wa)].append((b, wb, dx, dy))
            m[(b, wb)].append((a, wa, -dx, -dy))
    return m


def site_wire_prefix(tile_type):
    # CLE tile type -> wire prefix used in tile_type/tileconn wire names.
    # CLEL_L and CLEL_R both use CLE_CLE_L_SITE_0_*; CLEM and CLEM_R
    # both use CLE_CLE_M_SITE_0_* (verified against tileconn.json).
    if tile_type in ("CLEL_L", "CLEL_R", "CLEL_L_RBRK", "CLEL_L_TERM_B",
                     "CLEL_L_TERM_T", "CLEL_R_TERM_B", "CLEL_R_TERM_T"):
        return "CLE_CLE_L_SITE_0_"
    if tile_type in ("CLEM", "CLEM_R", "CLEM_RBRK"):
        return "CLE_CLE_M_SITE_0_"
    return None


def load_fasm_pips(path):
    # tile -> dst -> [src], and tile -> src -> [dst], with line numbers.
    dst_map = defaultdict(lambda: defaultdict(list))
    src_map = defaultdict(lambda: defaultdict(list))
    n = 0
    with open(path) as f:
        for ln, line in enumerate(f, 1):
            line = line.strip()
            if ".PIP." not in line:
                continue
            m = PIP_RE.match(line)
            if not m:
                continue
            tile = line.split(".PIP.")[0]
            dst, src = m.group(2), m.group(3)
            dst_map[tile][dst].append((src, ln))
            src_map[tile][src].append((dst, ln))
            n += 1
    return dst_map, src_map, n


def main():
    fasm, site, pin, direction = sys.argv[1:5]
    max_depth = int(sys.argv[5]) if len(sys.argv) > 5 else 6
    tg, site_to_tile, grid_to_tile = load_tilegrid()
    tc = load_tileconn()
    dst_map, src_map, npips = load_fasm_pips(fasm)
    print(f"FASM PIPs loaded: {npips}")
    if site not in site_to_tile:
        print(f"site {site} NOT in tilegrid")
        return 1
    tname, ttype, gx, gy = site_to_tile[site]
    print(f"site {site} -> tile {tname} type {ttype} grid ({gx},{gy})")
    prefix = site_wire_prefix(ttype)
    if prefix is None:
        print(f"no site-wire prefix known for tile type {ttype}")
        return 1
    wire = prefix + pin
    print(f"site-pin wire: {wire}")
    partners = tc.get((ttype, wire), [])
    print(f"tileconn partners for ({ttype}, {wire}): {len(partners)}")
    for (ot, ow, dx, dy) in partners:
        nname = grid_to_tile.get((gx + dx, gy + dy))
        print(f"  -> ({ot}, {ow}) delta ({dx},{dy}) neighbor tile {nname}")
    # Follow the first INT partner (the routing neighbor) if present.
    int_partners = [(ot, ow, dx, dy) for (ot, ow, dx, dy) in partners
                    if ot.startswith("INT")]
    if not int_partners:
        print("no INT tileconn partner: pin does not reach the INT fabric")
        return 0
    ot, ow, dx, dy = int_partners[0]
    ntile = grid_to_tile.get((gx + dx, gy + dy))
    print(f"following INT partner: {ntile} wire {ow}")
    if direction == "sink":
        # Walk backwards: who drives this wire?
        work = [(ntile, ow, 0)]
        seen = set()
        while work:
            tile, w, depth = work.pop(0)
            if (tile, w) in seen or depth > max_depth:
                continue
            seen.add((tile, w))
            drivers = dst_map.get(tile, {}).get(w, [])
            print(f"  {'  ' * depth}{tile} wire {w} driven by "
                  f"{len(drivers)} PIP(s):")
            for (src, ln) in drivers:
                print(f"  {'  ' * depth}  FASM line {ln}: "
                      f"{tile}.PIP.{w}.{src}  (src -> dst)")
                work.append((tile, src, depth + 1))
            if not drivers:
                # Stitched from a neighboring tile? Follow only the
                # stitch whose other tile type is the tile actually
                # present at the neighbor grid (tileconn lists every
                # tile type that could sit there; only one does).
                ttype2 = tg[tile]["type"]
                for (ot2, ow2, dx2, dy2) in tc.get((ttype2, w), []):
                    nt2 = grid_to_tile.get(
                        (tg[tile]["grid_x"] + dx2, tg[tile]["grid_y"] + dy2))
                    if nt2 is None or tg[nt2]["type"] != ot2:
                        continue
                    print(f"  {'  ' * depth}  tileconn stitch -> "
                          f"({ot2}, {ow2}) at {nt2}")
                    work.append((nt2, ow2, depth + 1))
    else:
        work = [(ntile, ow, 0)]
        seen = set()
        while work:
            tile, w, depth = work.pop(0)
            if (tile, w) in seen or depth > max_depth:
                continue
            seen.add((tile, w))
            loads = src_map.get(tile, {}).get(w, [])
            print(f"  {'  ' * depth}{tile} wire {w} drives "
                  f"{len(loads)} PIP(s):")
            for (dst, ln) in loads:
                print(f"  {'  ' * depth}  FASM line {ln}: "
                      f"{tile}.PIP.{dst}.{w}  (src -> dst)")
                work.append((tile, dst, depth + 1))
            if not loads:
                ttype2 = tg[tile]["type"]
                for (ot2, ow2, dx2, dy2) in tc.get((ttype2, w), []):
                    nt2 = grid_to_tile.get(
                        (tg[tile]["grid_x"] + dx2, tg[tile]["grid_y"] + dy2))
                    if nt2 is None or tg[nt2]["type"] != ot2:
                        continue
                    print(f"  {'  ' * depth}  tileconn stitch -> "
                          f"({ot2}, {ow2}) at {nt2}")
                    work.append((nt2, ow2, depth + 1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
