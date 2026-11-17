"""Project X-Ray database access (read-only, lazily cached)."""
from __future__ import annotations

import csv
import json
import os
from dataclasses import dataclass
from functools import cached_property, lru_cache

DEFAULT_ROOT = os.environ.get(
    "X7V_DB", os.path.expanduser("~/.cache/helion/prjxray-db/zynq7"))
# Directory holding the database families (zynq7/, artix7/, ...).
DB_PARENT = os.environ.get("X7V_DB_ROOT", os.path.dirname(DEFAULT_ROOT.rstrip("/")))
DEFAULT_PART = "xc7z020clg484-1"


def _yaml_map(path: str) -> dict[str, dict[str, str]]:
    """The database's mapping/*.yaml: `key:` lines, indented `field: value`."""
    out: dict[str, dict[str, str]] = {}
    cur = None
    for line in open(path):
        body = line.split("#", 1)[0]
        if not body.strip() or ":" not in body:
            continue
        k, v = body.split(":", 1)
        unq = lambda x: x.strip().strip('"').strip("'")
        if body[0] in " \t":
            if cur is not None:
                out[cur][unq(k)] = unq(v)
        else:
            cur = unq(k)
            out.setdefault(cur, {})
    return out


def resolve_part(name: str, parent: str = DB_PARENT) -> tuple[str, str, str]:
    """Part name -> (family directory, fabric, part), from the database's own
    mapping/parts.yaml and mapping/devices.yaml (written independently of
    Helion's resolver)."""
    short = {"xc7z020": "xc7z020clg484-1", "xc7a35t": "xc7a35tcsg324-1"}
    name = short.get(name, name)
    fams = sorted(d for d in os.listdir(parent) if os.path.isfile(os.path.join(parent, d, "mapping", "parts.yaml")))
    for fam in fams:
        famdir = os.path.join(parent, fam)
        parts = _yaml_map(os.path.join(famdir, "mapping", "parts.yaml"))
        if name not in parts:
            continue
        device = parts[name]["device"]
        fabric = _yaml_map(os.path.join(famdir, "mapping", "devices.yaml"))[device]["fabric"]
        if not os.path.isfile(os.path.join(famdir, name, "part.json")):
            raise ValueError(f"part {name}: {famdir}/{name}/part.json is missing")
        return famdir, fabric, name
    raise ValueError(f"unknown part {name} (database families under {parent}: {', '.join(fams) or 'none'})")


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

    @classmethod
    def for_part(cls, name: str) -> "Db":
        """The database for any part X-Ray describes (`xc7a35tcsg324-1`)."""
        if name == DEFAULT_PART and "X7V_DB" in os.environ:
            return cls()
        return cls(*resolve_part(name))

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
        if tile_type in ("DSP_L", "DSP_R"):
            # X-Ray has no AREG_1/BREG_1. Vivado 2017.3 writes AREG = 1 as
            # AREG_0's frame-27 bit alone (all 384 slices of the two ZedBoard
            # DSP fill builds), so AREG_1 is that bit with AREG_0's others clear.
            for k, bits in list(out.items()):
                if k.endswith((".AREG_0", ".BREG_0")):
                    out[k[:-1] + "1"] = tuple(SegBit(b.minor, b.bit, b.minor == 27) for b in bits)
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
