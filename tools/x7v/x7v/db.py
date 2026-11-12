"""Project X-Ray database access (read-only, lazily cached)."""
from __future__ import annotations

import csv
import json
import os
from dataclasses import dataclass
from functools import cached_property, lru_cache

DEFAULT_ROOT = os.environ.get(
    "X7V_DB", os.path.expanduser("~/.cache/helion/prjxray-db/zynq7"))


@dataclass(frozen=True)
class SegBit:
    minor: int
    bit: int
    value: bool


def _parse_bit(tok: str) -> SegBit:
    neg = tok.startswith("!")
    m, b = tok.lstrip("!").split("_")
    return SegBit(int(m), int(b), not neg)


class Db:
    def __init__(self, root: str = DEFAULT_ROOT, fabric: str = "xc7z020",
                 package: str = "xc7z020clg484-1"):
        self.root = root
        self.fabric = fabric
        self.package = package

    # ---- part / grid -------------------------------------------------
    @cached_property
    def part(self) -> dict:
        return json.load(open(f"{self.root}/{self.package}/part.json"))

    @cached_property
    def tilegrid(self) -> dict:
        return json.load(open(f"{self.root}/{self.fabric}/tilegrid.json"))

    @cached_property
    def grid_index(self) -> dict[tuple[int, int], str]:
        return {(t["grid_x"], t["grid_y"]): name for name, t in self.tilegrid.items()}

    @cached_property
    def site_to_tile(self) -> dict[str, tuple[str, str]]:
        out = {}
        for tname, t in self.tilegrid.items():
            for site, stype in t.get("sites", {}).items():
                out[site] = (tname, stype)
        return out

    @cached_property
    def package_pins(self) -> dict[str, dict]:
        rows = list(csv.DictReader(open(f"{self.root}/{self.package}/package_pins.csv")))
        return {r["pin"]: r for r in rows}

    @cached_property
    def required_features(self) -> list[str]:
        p = f"{self.root}/{self.package}/required_features.fasm"
        if not os.path.exists(p):
            return []
        return [l.strip() for l in open(p) if l.strip() and not l.startswith("#")]

    # ---- per tile type -----------------------------------------------
    @lru_cache(maxsize=None)
    def segbits(self, tile_type: str, block: str = "CLB_IO_CLK") -> dict[str, tuple[SegBit, ...]]:
        suffix = "" if block == "CLB_IO_CLK" else "." + block.lower()
        p = f"{self.root}/segbits_{tile_type.lower()}{suffix}.db"
        out: dict[str, tuple[SegBit, ...]] = {}
        if not os.path.exists(p):
            return out
        for line in open(p):
            parts = line.split()
            if not parts:
                continue
            out[parts[0]] = tuple(_parse_bit(t) for t in parts[1:])
        return out

    @lru_cache(maxsize=None)
    def has_segbits(self, tile_type: str, block: str = "CLB_IO_CLK") -> bool:
        suffix = "" if block == "CLB_IO_CLK" else "." + block.lower()
        return os.path.exists(f"{self.root}/segbits_{tile_type.lower()}{suffix}.db")

    @lru_cache(maxsize=None)
    def ppips(self, tile_type: str) -> dict[str, str]:
        p = f"{self.root}/ppips_{tile_type.lower()}.db"
        out: dict[str, str] = {}
        if os.path.exists(p):
            for line in open(p):
                parts = line.split()
                if len(parts) == 2:
                    out[parts[0]] = parts[1]
        return out

    @lru_cache(maxsize=None)
    def tile_type(self, tile_type: str) -> dict:
        p = f"{self.root}/tile_type_{tile_type}.json"
        return json.load(open(p)) if os.path.exists(p) else {"wires": {}, "pips": {}, "sites": []}

    @cached_property
    def tileconn(self) -> list[dict]:
        return json.load(open(f"{self.root}/{self.fabric}/tileconn.json"))
