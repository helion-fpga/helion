"""Routing fabric: tile wires joined by tileconn stitching, PIP graph
from the tile-type JSONs. Mirrors the x7v 7-series Fabric, ported to
the UltraScale+ prjuray-db layout.

A node is the set of (tile, wire) pairs joined by tileconn entries
between adjacent tiles. An edge inside a tile is an active PIP
feature (`PIP.<dst>.<src>`, physical direction src -> dst) or, for
site outputs, the site model itself.

No em dashes anywhere.
"""
from __future__ import annotations

from collections import defaultdict, deque

from .db import Db


class Fabric:
    def __init__(self, db: Db):
        self.db = db
        self._tt: dict[str, dict] = {}
        self._conn = None
        self.node_of: dict[tuple[str, str], int] = {}
        self.nodes: list[tuple[tuple[str, str], ...]] = []

    def tt(self, ttype: str) -> dict:
        if ttype not in self._tt:
            j = self.db.tile_type(ttype)
            pips_into: dict[str, list] = defaultdict(list)
            pairs: set[tuple[str, str]] = set()
            for p in j.get("pips", {}).values():
                s, d = p["src_wire"], p["dst_wire"]
                bid = p.get("is_directional") == "0"
                pips_into[d].append((s, bid))
                pairs.add((s, d))
                if bid:
                    pips_into[s].append((d, bid))
                    pairs.add((d, s))
            site_pin_wire: dict[str, tuple[int, str]] = {}
            for i, s in enumerate(j.get("sites", [])):
                for pin, info in s.get("site_pins", {}).items():
                    if info and info.get("wire"):
                        site_pin_wire[info["wire"]] = (i, pin)
            self._tt[ttype] = {
                "wires": set(j.get("wires", {})),
                "pips_into": dict(pips_into),
                "pip_pairs": pairs,
                "site_pin_wire": site_pin_wire,
                "sites": j.get("sites", []),
            }
        return self._tt[ttype]

    def _conn_index(self):
        if self._conn is None:
            idx: dict = defaultdict(list)
            for e in self.db.tileconn:
                a, b = e["tile_types"]
                dx, dy = e["grid_deltas"]
                for wa, wb in e["wire_pairs"]:
                    idx[(a, wa)].append((dx, dy, b, wb))
                    idx[(b, wb)].append((-dx, -dy, a, wa))
            self._conn = idx
        return self._conn

    def node(self, tile: str, wire: str) -> int:
        """Node id for (tile, wire), stitching across tileconn."""
        key = (tile, wire)
        if key in self.node_of:
            return self.node_of[key]
        idx = self._conn_index()
        grid = self.db.grid_index
        tg = self.db.tilegrid
        members = [key]
        seen = {key}
        q = deque([key])
        while q:
            t, w = q.popleft()
            g = tg.get(t)
            if g is None:
                continue
            for dx, dy, bt, bw in idx.get((g["type"], w), ()):
                other = grid.get((g["grid_x"] + dx, g["grid_y"] + dy))
                if other is None or tg[other]["type"] != bt:
                    continue
                k2 = (other, bw)
                if k2 not in seen:
                    seen.add(k2)
                    members.append(k2)
                    q.append(k2)
        nid = len(self.nodes)
        self.nodes.append(tuple(members))
        for m in members:
            self.node_of[m] = nid
        return nid

    def tile_type(self, tile: str) -> str | None:
        t = self.db.tilegrid.get(tile)
        return t["type"] if t else None

    def site_pin_wire(self, tile: str, site_index: int, pin: str) -> str | None:
        """Tile wire for a site pin, or None when the pin has none."""
        ttype = self.tile_type(tile)
        if ttype is None:
            return None
        tt = self.tt(ttype)
        sites = tt["sites"]
        if site_index >= len(sites):
            return None
        info = sites[site_index].get("site_pins", {}).get(pin) or {}
        return info.get("wire")

    def pips_into(self, tile: str, wire: str) -> list:
        """[(src_wire, bidirectional)] for pips driving `wire` in tile."""
        ttype = self.tile_type(tile)
        if ttype is None:
            return []
        return self.tt(ttype)["pips_into"].get(wire, [])
