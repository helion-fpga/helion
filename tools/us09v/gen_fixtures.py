"""Generate the hand-built complete small fixtures for us09v tests.

Each fixture is a complete small design on a real xczu3eg slice
(SLICE_X10Y10, tile CLEL_R_X6Y10): RTL, complete FASM (LUT INITs,
FFMUX/OUTMUX selections, group attributes, and BFS-found INT PIP
routes for every feedback net), and a slice-pin port map.

The route finder BFS runs over stitched tileconn nodes plus PIP
edges, restricted to a bounding box around the slice so it stays
local. Routes from different source nets reserve their wires, so
no two nets share a wire (which would be a double driver). A route
that cannot be found fails loud here, at generation time, instead
of shipping a broken fixture.

Run: python3 gen_fixtures.py [--check]
  --check only verifies the PIPs in the generated FASM files exist
  in the INT tile_type JSON, without regenerating.

No em dashes anywhere.
"""
from __future__ import annotations

import json
import os
import re
import sys
from collections import deque

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from us09v.db import Db, default_db_root
from us09v.fabric import Fabric
from us09v.routecheck import pip_exists_in_tile_type

SITE = "SLICE_X10Y10"
TILE = "CLEL_R_X6Y10"
FIX_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "tests", "fixtures")

GROUP_ATTRS = [
    "ABCDFF.CEUSED.V0", "ABCDFF.SRUSED.V0", "ABCDFF.CLKINV.V0",
    "ABCDFF.SYNC.ASYNC", "ABCDFF.MODE.FF",
]


def tile_xy(tile: str) -> tuple[int, int]:
    m = re.match(r"^.+_X(\d+)Y(\d+)$", tile)
    return (int(m.group(1)), int(m.group(2))) if m else (0, 0)


def site_pin_wire(db: Db, fabric: Fabric, site: str, pin: str) -> tuple[str, str]:
    tile, index = db.site_index(site)
    tt = fabric.tt(db.tilegrid[tile]["type"])
    info = tt["sites"][index].get("site_pins", {}).get(pin) or {}
    wire = info.get("wire")
    if wire is None:
        raise ValueError(f"{site}.{pin}: no wire in tile type")
    return (tile, wire)


def pip_adjacency(fabric: Fabric, ttype: str) -> dict[str, list[str]]:
    """src wire -> [dst wires] for a tile type (directed PIPs only)."""
    tt = fabric.tt(ttype)
    adj: dict[str, list[str]] = {}
    for src, dst in tt["pip_pairs"]:
        adj.setdefault(src, []).append(dst)
    return adj


def find_route(
    db: Db,
    fabric: Fabric,
    src: tuple[str, str],
    dst: tuple[str, str],
    reserved: set[tuple[str, str]],
    radius: int = 3,
) -> list[tuple[str, str, str]]:
    """BFS from stitched node(src) to stitched node(dst). Returns the
    PIPs as [(tile, dst_wire, src_wire)]. `reserved` wires (used by
    other nets) are never stepped on. Tiles outside the bounding box
    around src/dst are never entered."""
    adj_cache: dict[str, dict[str, list[str]]] = {}

    def adj(ttype: str) -> dict[str, list[str]]:
        if ttype not in adj_cache:
            adj_cache[ttype] = pip_adjacency(fabric, ttype)
        return adj_cache[ttype]

    cx, cy = tile_xy(src[0])
    tx, ty = tile_xy(dst[0])
    x0, x1 = min(cx, tx) - radius, max(cx, tx) + radius
    y0, y1 = min(cy, ty) - radius, max(cy, ty) + radius

    def in_box(tile: str) -> bool:
        x, y = tile_xy(tile)
        return x0 <= x <= x1 and y0 <= y <= y1

    start_node = fabric.node(*src)
    target_node = fabric.node(*dst)
    if start_node == target_node:
        return []
    # BFS over node ids. prev[node] = (prev node, tile, dst, src).
    prev: dict[int, tuple[int, str, str, str] | None] = {start_node: None}
    q = deque([start_node])
    while q:
        nid = q.popleft()
        for tile, wire in fabric.nodes[nid]:
            if not in_box(tile):
                continue
            ttype = db.tilegrid[tile]["type"]
            # Only INT tiles are routing fabric. CLE and other tiles
            # carry site-internal pseudo-PIPs (LUT route-throughs,
            # CIN->CIN_PIN); using one as a routing wire would
            # corrupt the BEL it belongs to.
            if ttype != "INT":
                continue
            for dst_wire in adj(ttype).get(wire, []):
                nxt = fabric.node(tile, dst_wire)
                if nxt in prev:
                    continue
                members = fabric.nodes[nxt]
                if any(m in reserved for m in members):
                    continue
                if not all(in_box(t) for t, _w in members):
                    continue
                prev[nxt] = (nid, tile, dst_wire, wire)
                if nxt == target_node:
                    # Reconstruct.
                    pips: list[tuple[str, str, str]] = []
                    cur = nxt
                    while prev[cur] is not None:
                        p, tile2, d2, s2 = prev[cur]
                        pips.append((tile2, d2, s2))
                        cur = p
                    pips.reverse()
                    return pips
                q.append(nxt)
    raise ValueError(f"no route from {src} to {dst} within radius {radius}")


def build_fixture(db, fabric, name, rtl, site_feats, routes, port_map):
    """routes: [(src site pin, dst site pin)] on SITE. Returns the
    FASM text. PIPs from the routes are added to the fixture tiles."""
    tiles: dict[str, dict[str, int | None]] = {SITE: dict(site_feats)}
    # Reserve wires per source net so different nets never share one.
    nets: dict[str, list[tuple[str, str]]] = {}
    for src_pin, dst_pin in routes:
        nets.setdefault(src_pin, []).append((src_pin, dst_pin))
    # Whole stitched nodes used by already-routed nets: reserving
    # only the PIP wires is not enough, because two routes can use
    # different wires of one tileconn-stitched node and short
    # together. Nodes of the net currently being routed are shared
    # by its fanout and are only reserved for other nets once the
    # net is done.
    used_nodes: set[tuple[str, str]] = set()
    # Reserve the nodes of every site pin except the route
    # endpoints: a route through a wire stitched to an unused site
    # pin would drive that pin, which is a real connection (e.g. a
    # DI bypass pin would stop reading 0 and take the routed net).
    endpoint_pins = {p for pair in routes for p in pair}
    site_tile, _stype = db.site_to_tile[SITE]
    ttype = db.tilegrid[site_tile]["type"]
    for pin in fabric.tt(ttype)["sites"][0].get("site_pins", {}):
        if pin in endpoint_pins:
            continue
        try:
            w = site_pin_wire(db, fabric, SITE, pin)
        except ValueError:
            continue
        used_nodes.update(fabric.nodes[fabric.node(*w)])
    for src_pin, pairs in nets.items():
        src_wire = site_pin_wire(db, fabric, SITE, src_pin)
        reserved: set[tuple[str, str]] = set(used_nodes)
        for other_pin, other_pairs in nets.items():
            if other_pin == src_pin:
                continue
            for _s, d in other_pairs:
                reserved.update(fabric.nodes[fabric.node(*site_pin_wire(db, fabric, SITE, d))])
        # Also reserve other nets' sources (whole nodes).
        for other_pin in nets:
            if other_pin != src_pin:
                reserved.update(fabric.nodes[fabric.node(*site_pin_wire(db, fabric, SITE, other_pin))])
        net_nodes: set[tuple[str, str]] = set()
        net_nodes.update(fabric.nodes[fabric.node(*src_wire)])
        for _s, dst_pin in pairs:
            dst_wire = site_pin_wire(db, fabric, SITE, dst_pin)
            pips = find_route(db, fabric, src_wire, dst_wire, reserved)
            for tile, dst_w, src_w in pips:
                tiles.setdefault(tile, {})[f"PIP.{dst_w}.{src_w}"] = None
                net_nodes.update(fabric.nodes[fabric.node(tile, dst_w)])
                net_nodes.update(fabric.nodes[fabric.node(tile, src_w)])
            net_nodes.update(fabric.nodes[fabric.node(*dst_wire)])
        used_nodes.update(net_nodes)
    lines = []
    for tile in sorted(tiles):
        for feat in sorted(tiles[tile]):
            v = tiles[tile][feat]
            if v is None:
                lines.append(f"{tile}.{feat}")
            else:
                lines.append(f"{tile}.{feat} = 0x{v:x}")
    return "\n".join(lines) + "\n"


def ff_feats(letter: str) -> dict[str, None]:
    # All fixture registers reset to 0, so INIT and SRVAL are V0
    # under the V-suffix-is-the-value reading (see fasmio.ff_attr).
    return {f"{letter}FF.INIT.V0": None, f"{letter}FF.SRVAL.V0": None}


def write_fixture(name: str, rtl: str, fasm: str, port_map: dict[str, str]):
    d = os.path.join(FIX_DIR, name)
    os.makedirs(d, exist_ok=True)
    open(os.path.join(d, "rtl.v"), "w").write(rtl)
    open(os.path.join(d, f"{name}.fasm"), "w").write(fasm)
    open(os.path.join(d, "port_map.json"), "w").write(json.dumps(port_map, indent=2) + "\n")
    print(f"{name}: {len(fasm.splitlines())} FASM lines")


def gen(db: Db, fabric: Fabric):
    # and2: y = a & b, one LUT.
    fasm = build_fixture(
        db, fabric, "and2",
        "module and2(input a, b, output y);\n  assign y = a & b;\nendmodule\n",
        {"A6LUT.INIT[63:0]": 0x8888888888888888, "OUTMUXA.D6": None},
        [],
        {"a": f"{SITE}.A1", "b": f"{SITE}.A2", "y": f"{SITE}.AMUX"},
    )
    write_fixture("and2",
        "module and2(input a, b, output y);\n  assign y = a & b;\nendmodule\n",
        fasm, {"a": f"{SITE}.A1", "b": f"{SITE}.A2", "y": f"{SITE}.AMUX"})

    # blinky: q <= ~q, one LUT + one FF + feedback AQ -> A1.
    feats = {"A6LUT.INIT[63:0]": 0x5555555555555555, "FFMUXA1.D6": None}
    feats.update(ff_feats("A"))
    for g in GROUP_ATTRS:
        feats[g] = None
    fasm = build_fixture(
        db, fabric, "blinky",
        "module blinky(input clk, output led);\n"
        "  reg q = 1'b0;\n"
        "  always @(posedge clk) q <= ~q;\n"
        "  assign led = q;\nendmodule\n",
        feats, [("AQ", "A1")],
        {"clk": f"{SITE}.CLK1", "led": f"{SITE}.AQ"},
    )
    write_fixture("blinky",
        "module blinky(input clk, output led);\n"
        "  reg q = 1'b0;\n"
        "  always @(posedge clk) q <= ~q;\n"
        "  assign led = q;\nendmodule\n",
        fasm, {"clk": f"{SITE}.CLK1", "led": f"{SITE}.AQ"})

    # counter4: LUT-computed sums, 4 LUTs + 4 FFs.
    feats = {
        "A6LUT.INIT[63:0]": 0x5555555555555555,
        "B6LUT.INIT[63:0]": 0x6666666666666666,
        "C6LUT.INIT[63:0]": 0x7878787878787878,
        "D6LUT.INIT[63:0]": 0x7F807F807F807F80,
        "FFMUXA1.D6": None, "FFMUXB1.D6": None,
        "FFMUXC1.D6": None, "FFMUXD1.D6": None,
    }
    for c in "ABCD":
        feats.update(ff_feats(c))
    for g in GROUP_ATTRS:
        feats[g] = None
    # The golden INITs pin the input assignment: A 0x5555 is ~A1,
    # B 0x6666 is B1^B2, C 0x7878 is C3^(C1&C2), D 0x7f80... is
    # D4^(D1&D2&D3). So the sum bit itself goes on the highest
    # input of each LUT and the lower bits on the lower inputs.
    routes = [
        ("AQ", "A1"), ("AQ", "B2"), ("AQ", "C2"), ("AQ", "D3"),
        ("BQ", "B1"), ("BQ", "C1"), ("BQ", "D2"),
        ("CQ", "C3"), ("CQ", "D1"), ("DQ", "D4"),
    ]
    fasm = build_fixture(
        db, fabric, "counter4",
        "module counter4(input clk, output [3:0] led);\n"
        "  reg [3:0] cnt = 0;\n"
        "  always @(posedge clk) cnt <= cnt + 1;\n"
        "  assign led = cnt;\nendmodule\n",
        feats, routes,
        {"clk": f"{SITE}.CLK1", "led[0]": f"{SITE}.AQ", "led[1]": f"{SITE}.BQ",
         "led[2]": f"{SITE}.CQ", "led[3]": f"{SITE}.DQ"},
    )
    write_fixture("counter4",
        "module counter4(input clk, output [3:0] led);\n"
        "  reg [3:0] cnt = 0;\n"
        "  always @(posedge clk) cnt <= cnt + 1;\n"
        "  assign led = cnt;\nendmodule\n",
        fasm, {"clk": f"{SITE}.CLK1", "led[0]": f"{SITE}.AQ", "led[1]": f"{SITE}.BQ",
               "led[2]": f"{SITE}.CQ", "led[3]": f"{SITE}.DQ"})

    # carry8-inc: 8 LUTs (S[i] = cnt[i]) + CARRY8 (DI=0, CI=1) + 8 FFs.
    feats = {"CARRY8.CARRY_TYPE.SINGLE_CY8": None, "CARRY8.CI.V1": None}
    for i, c in enumerate("ABCDEFGH"):
        feats[f"{c}6LUT.INIT[63:0]"] = 0xAAAAAAAAAAAAAAAA
        feats[f"CARRY8.DI{i}.{c}X"] = None
        feats[f"FFMUX{c}1.XORIN"] = None
        feats.update(ff_feats(c))
    for g in GROUP_ATTRS:
        feats[g] = None
    routes = [(f"{c}Q", f"{c}1") for c in "ABCDEFGH"]
    fasm = build_fixture(
        db, fabric, "carry8-inc",
        # clk2 is the same clock net reaching the EFGH half's CLK2
        # pin: one clock drives both half-clock pins in real
        # hardware (via the global clock wires), and the testbench
        # toggles clk and clk2 identically.
        "module carry8inc(input clk, input clk2, output [7:0] led);\n"
        "  reg [7:0] cnt = 0;\n"
        "  always @(posedge clk) cnt <= cnt + 1;\n"
        "  assign led = cnt;\nendmodule\n",
        feats, routes,
        {"clk": f"{SITE}.CLK1", "clk2": f"{SITE}.CLK2",
         **{f"led[{i}]": f"{SITE}.{c}Q" for i, c in enumerate("ABCDEFGH")}},
    )
    write_fixture("carry8-inc",
        "module carry8inc(input clk, input clk2, output [7:0] led);\n"
        "  reg [7:0] cnt = 0;\n"
        "  always @(posedge clk) cnt <= cnt + 1;\n"
        "  assign led = cnt;\nendmodule\n",
        fasm, {"clk": f"{SITE}.CLK1", "clk2": f"{SITE}.CLK2",
               **{f"led[{i}]": f"{SITE}.{c}Q" for i, c in enumerate("ABCDEFGH")}})

    # wide-mux F7: y = s ? b : a. Pins the A-F7F8 mapping behavior
    # (F7A = AX ? B6 : A6); it does NOT validate the mapping.
    feats = {
        "A6LUT.INIT[63:0]": 0xAAAAAAAAAAAAAAAA,
        "B6LUT.INIT[63:0]": 0xAAAAAAAAAAAAAAAA,
        "OUTMUXA.F7F8": None,
    }
    fasm = build_fixture(
        db, fabric, "wide-mux-f7",
        "module widemux(input a, b, s, output y);\n"
        "  assign y = s ? b : a;\nendmodule\n",
        feats, [],
        {"a": f"{SITE}.A1", "b": f"{SITE}.B1", "s": f"{SITE}.AX", "y": f"{SITE}.AMUX"},
    )
    write_fixture("wide-mux-f7",
        "module widemux(input a, b, s, output y);\n"
        "  assign y = s ? b : a;\nendmodule\n",
        fasm, {"a": f"{SITE}.A1", "b": f"{SITE}.B1", "s": f"{SITE}.AX", "y": f"{SITE}.AMUX"})


def check(db: Db):
    """Verify every PIP in every generated fixture FASM exists in
    its tile's tile_type JSON."""
    bad = 0
    for name in sorted(os.listdir(FIX_DIR)):
        fasm_path = os.path.join(FIX_DIR, name, f"{name}.fasm")
        if not os.path.exists(fasm_path):
            continue
        for line in open(fasm_path):
            line = line.strip()
            if ".PIP." not in line:
                continue
            tile, feat = line.split(".", 1)
            m = re.match(r"^PIP\.([^.]+)\.([^.]+)$", feat)
            if not m:
                print(f"{name}: malformed PIP feature {line}")
                bad += 1
                continue
            ttype = db.tilegrid[tile]["type"]
            if not pip_exists_in_tile_type(db, ttype, m.group(1), m.group(2)):
                print(f"{name}: PIP {feat} does not exist in {ttype}")
                bad += 1
    print(f"fixture PIP check: {'0 bad' if bad == 0 else f'{bad} bad'}")
    return bad


if __name__ == "__main__":
    db = Db(default_db_root())
    fabric = Fabric(db)
    if "--check" in sys.argv:
        sys.exit(1 if check(db) else 0)
    gen(db, fabric)
    sys.exit(1 if check(db) else 0)
