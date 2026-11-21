"""Load the public prjuray-db database for one UltraScale+ part.

Sources, all license-free (CC0 1.0, see the COPYING file in the
prjuray-db clone):
- tilegrid.json, tileconn.json, package_pins.csv, part.json:
  prjuray-db/zynqusp/<part>/.
- tile_types/*.json, site_types/*.json: prjuray-db/zynqusp/.
- segbits_*.db: prjuray-db/zynqusp/ (feature existence checks only).

The database root is a prjuray-db `zynqusp` directory. Resolution
order for the default root: $US09V_DB_ROOT, then
~/workspace/prjuray-db-ref/zynqusp, then the crew harness reference
copy (which lacks tile_types and tileconn, so full route checks fail
loud there with the missing path named). Nothing is read from /tmp.
"""
from __future__ import annotations

import csv
import json
import os
import re

DEFAULT_PART = "xczu3eg-sbva484-1-e"


def default_db_root() -> str | None:
    env = os.environ.get("US09V_DB_ROOT")
    if env:
        return env
    cands = [
        os.path.expanduser("~/workspace/prjuray-db-ref/zynqusp"),
        os.path.expanduser(
            "~/workspace/ultra-bitgen-crew/harness/reference/prjuray-db/zynqusp"
        ),
    ]
    for c in cands:
        if os.path.isdir(c):
            return c
    return None


def verify_cc0(db_root: str) -> str:
    """Return the prjuray-db commit for a root, failing loud unless its
    COPYING file is the CC0 1.0 legal code. `db_root` is the zynqusp
    directory; the clone root is one level up."""
    clone = os.path.dirname(os.path.abspath(db_root))
    copying = os.path.join(clone, "COPYING")
    if not os.path.exists(copying):
        raise FileNotFoundError(
            f"no COPYING file at {copying}: cannot verify the database license"
        )
    with open(copying) as f:
        text = f.read()
    if "CC0 1.0 Universal" not in text:
        raise ValueError(
            f"{copying} is not the CC0 1.0 legal code: refusing to use this database"
        )
    return clone


class Db:
    """A prjuray-db zynqusp directory for one part."""

    def __init__(self, root: str | None = None, part: str = DEFAULT_PART):
        self.root = root or default_db_root()
        if not self.root:
            raise FileNotFoundError(
                "no prjuray-db zynqusp directory found: set US09V_DB_ROOT "
                "to a prjuray-db clone's zynqusp directory"
            )
        if not os.path.isdir(self.root):
            raise FileNotFoundError(f"database root is not a directory: {self.root}")
        self.part = part
        self.clone = verify_cc0(self.root)
        self._tilegrid = None
        self._tileconn = None
        self._tile_types: dict[str, dict] = {}
        self._site_types: dict[str, dict] = {}
        self._segbits: dict[str, dict[str, list]] = {}
        self._package_pins = None
        self._part_json = None
        self._site_to_tile: dict[str, tuple[str, str]] | None = None
        self._grid_index: dict[tuple[int, int], str] | None = None

    # ---- part files ----------------------------------------------------

    @property
    def part_dir(self) -> str:
        return os.path.join(self.root, self.part)

    @property
    def tilegrid(self) -> dict:
        if self._tilegrid is None:
            p = os.path.join(self.part_dir, "tilegrid.json")
            if not os.path.exists(p):
                raise FileNotFoundError(
                    f"no tilegrid.json for part {self.part} at {p}: "
                    "place and route checks need the public tilegrid"
                )
            with open(p) as f:
                self._tilegrid = json.load(f)
        return self._tilegrid

    @property
    def tileconn(self) -> list:
        if self._tileconn is None:
            p = os.path.join(self.part_dir, "tileconn.json")
            if not os.path.exists(p):
                raise FileNotFoundError(
                    f"no tileconn.json for part {self.part} at {p}: "
                    "route and connectivity checks need the public tileconn"
                )
            with open(p) as f:
                self._tileconn = json.load(f)
        return self._tileconn

    @property
    def grid_index(self) -> dict[tuple[int, int], str]:
        if self._grid_index is None:
            self._grid_index = {}
            for name, t in self.tilegrid.items():
                self._grid_index[(t["grid_x"], t["grid_y"])] = name
        return self._grid_index

    @property
    def site_to_tile(self) -> dict[str, tuple[str, str]]:
        """site name -> (tile name, site type)."""
        if self._site_to_tile is None:
            out = {}
            for tile, t in self.tilegrid.items():
                for site, stype in t.get("sites", {}).items():
                    out[site] = (tile, stype)
            self._site_to_tile = out
        return self._site_to_tile

    def site_index(self, site: str) -> tuple[str, int]:
        """site name -> (tile name, index into the tile type's sites
        list). The tilegrid sites dict preserves the tile_type sites
        list order, so the index is the site's position in the
        tile's sites dict."""
        tile, _stype = self.site_to_tile[site]
        for i, name in enumerate(self.tilegrid[tile].get("sites", {})):
            if name == site:
                return tile, i
        raise KeyError(f"{site}: not listed in tile {tile} sites")

    @property
    def package_pins(self) -> dict[str, str]:
        """site name -> package pin, from package_pins.csv."""
        if self._package_pins is None:
            p = os.path.join(self.part_dir, "package_pins.csv")
            out = {}
            if os.path.exists(p):
                with open(p, newline="") as f:
                    for row in csv.DictReader(f):
                        site = row.get("site") or row.get("Site")
                        pin = row.get("pin") or row.get("Pin")
                        if site and pin:
                            out[site] = pin
            self._package_pins = out
        return self._package_pins

    @property
    def part_json(self) -> dict:
        if self._part_json is None:
            p = os.path.join(self.part_dir, "part.json")
            if os.path.exists(p):
                with open(p) as f:
                    self._part_json = json.load(f)
            else:
                self._part_json = {}
        return self._part_json

    # ---- tile and site types -------------------------------------------

    def tile_type(self, ttype: str) -> dict:
        if ttype not in self._tile_types:
            p = os.path.join(self.root, "tile_types", f"tile_type_{ttype}.json")
            if not os.path.exists(p):
                raise FileNotFoundError(
                    f"no tile_type JSON for {ttype} at {p}"
                )
            with open(p) as f:
                self._tile_types[ttype] = json.load(f)
        return self._tile_types[ttype]

    def has_tile_type(self, ttype: str) -> bool:
        return os.path.exists(
            os.path.join(self.root, "tile_types", f"tile_type_{ttype}.json")
        )

    def site_type(self, stype: str) -> dict:
        if stype not in self._site_types:
            p = os.path.join(self.root, "site_types", f"site_type_{stype}.json")
            if not os.path.exists(p):
                raise FileNotFoundError(
                    f"no site_type JSON for {stype} at {p}"
                )
            with open(p) as f:
                self._site_types[stype] = json.load(f)
        return self._site_types[stype]

    def has_site_type(self, stype: str) -> bool:
        return os.path.exists(
            os.path.join(self.root, "site_types", f"site_type_{stype}.json")
        )

    # ---- segbits (feature existence only) -------------------------------

    _SEGBIT_RE = re.compile(r"^(\S+)\s+((?:!?)\d+_\d+(?:\s+(?:!?)\d+_\d+)*)\s*$")

    def segbits(self, ttype: str) -> dict[str, list]:
        """{feature: [(frame, bit, value), ...]} for one tile type,
        loaded from the segbits_*.db files whose names carry the tile
        type. Only the feature names are used by the checkers; the bit
        lists exist so a caller can confirm a feature is real."""
        if ttype in self._segbits:
            return self._segbits[ttype]
        out: dict[str, list] = {}
        # File names carry the tile type: segbits_clel_l.db holds
        # CLEL_L features. Read only the matching files.
        for name in sorted(os.listdir(self.root)):
            if not (name.startswith("segbits_") and name.endswith(".db")):
                continue
            if "origin_info" in name:
                continue
            stem = name[len("segbits_") : -len(".db")].upper()
            if stem != ttype:
                continue
            path = os.path.join(self.root, name)
            for line in open(path):
                line = line.strip()
                if not line or line.startswith("#"):
                    continue
                parts = line.split(None, 1)
                if len(parts) != 2:
                    continue
                feat, rest = parts
                prefix = feat.split(".", 1)[0]
                if prefix != ttype:
                    continue
                refs = []
                for tok in rest.split():
                    neg = tok.startswith("!")
                    tok = tok.lstrip("!")
                    if "_" not in tok:
                        continue
                    f, b = tok.split("_", 1)
                    try:
                        refs.append((int(f), int(b), not neg))
                    except ValueError:
                        continue
                out[feat] = refs
        self._segbits[ttype] = out
        return out

    def feature_exists(self, ttype: str, feature: str) -> bool:
        """True when `feature` (without tile-type prefix) is a real
        feature of tile type `ttype` in the segbits DB."""
        seg = self.segbits(ttype)
        if not seg:
            return False
        return f"{ttype}.{feature}" in seg
