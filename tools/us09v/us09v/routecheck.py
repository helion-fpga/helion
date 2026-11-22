"""Route legality from decoded FASM, checked against the public
tileconn.json and tile_types (not against Helion's own router, which
this oracle must not trust).

Checks, per design:
- every PIP feature names a real pip in its tile's tile_type JSON
  (FASM PIP.<dst>.<src> = physical direction src -> dst).
  Bidirectional BYPASS PIPs may carry a .FWD/.REV suffix; the
  suffix is normalized before the check (FWD keeps the direction,
  REV swaps it; see fasmio.parse_pip).
- no wire has two drivers (two active PIPs into one wire, or an
  active PIP into a wire also driven by a site output pin).
- every active PIP's source wire is driven.
- every used site input pin's wire is driven.
- every net is one tree: the wires reachable from a driver form a
  single connected directed tree; no wire is reachable from two
  different drivers.

Driver resolution here is a fresh local graph walk, independent of
extract.py's driver(): it only uses the tile_type pip lists and the
site_pin_wire maps. A net that fails here fails the build even if
the rebuild happened to produce a netlist.

When no Db/tilegrid is available (xcku3p goldens, provisional tile
coordinates), the checker reports "skipped" with the reason instead
of guessing.

No em dashes anywhere.
"""
from __future__ import annotations

import re
from collections import defaultdict, deque
from dataclasses import dataclass, field

from .db import Db
from .fabric import Fabric
from .fasmio import parse_pip

PIP_RE = re.compile(r"^PIP\.([^.]+)\.([^.]+)(?:\.(FWD|REV))?$")


@dataclass
class RouteReport:
    violations: list[str] = field(default_factory=list)
    notes: list[str] = field(default_factory=list)
    pips: int = 0
    nets: int = 0
    skipped: bool = False

    @property
    def ok(self) -> bool:
        return not self.violations and not self.skipped


def pip_exists_in_tile_type(db: Db, tile_type: str, dst: str, src: str) -> bool:
    """Tile-type level check: does tile_type have a pip src -> dst?
    Used for golden FASM designs whose tile coordinates are
    provisional and cannot be resolved through a tilegrid. The raw
    tile_type JSON stores pips as a dict of records."""
    tt = db.tile_type(tile_type)
    for p in tt.get("pips", {}).values():
        if p.get("src_wire") == src and p.get("dst_wire") == dst:
            return True
    return False


def _pin_wire(fabric: Fabric, ttype: str, site_index: int, pin: str) -> str | None:
    """Tile wire for a site pin (pin -> wire, the inverse of the
    fabric's site_pin_wire map)."""
    tt = fabric.tt(ttype)
    sites = tt["sites"]
    if site_index >= len(sites):
        return None
    return (sites[site_index].get("site_pins", {}).get(pin) or {}).get("wire")


def _slice_pin_wire(db: Db, fabric: Fabric, site: str, pin: str, provisional: bool) -> tuple[str, str] | None:
    """(tile owning the wire, wire) for a slice site pin. The tile is
    the grid tile when known, else the site name (provisional, with
    the CLEL_R tile type assumed)."""
    info = db.site_to_tile.get(site)
    if info is not None:
        tile, _stype = info
        ttype = db.tilegrid[tile]["type"]
        wire = _pin_wire(fabric, ttype, 0, pin)
        return (tile, wire) if wire else None
    wire = _pin_wire(fabric, "CLEL_R", 0, pin)
    return (site, wire) if wire else None


def _tile_type_for(db: Db, tile: str, provisional: bool) -> str | None:
    """Tile type, with a provisional fallback: infer the type from
    the tile name prefix (INT_X0Y0 -> INT) when the tile is not in
    the tilegrid and the prefix names a known tile type."""
    info = db.tilegrid.get(tile)
    if info is not None:
        return info["type"]
    if not provisional:
        return None
    m = re.match(r"^(.+?)_X\d+Y\d+$", tile)
    if m and db.has_tile_type(m.group(1)):
        return m.group(1)
    return None


def check_route_fasm(
    db: Db | None, tiles, fabric: Fabric | None = None,
    port_ins: dict[str, tuple[str, str]] | None = None,
    provisional: bool = False,
) -> RouteReport:
    """`port_ins`: (site, pin) primary inputs count as drivers for
    the sink check. `provisional`: tile types are inferred from tile
    name prefixes when the tile is not in the tilegrid."""
    rep = RouteReport()
    if db is None or fabric is None:
        rep.skipped = True
        rep.notes.append(
            "no tilegrid/tileconn: route checks skipped (provisional tiles)"
        )
        return rep
    port_in_pins = set((port_ins or {}).values())
    # Collect active PIPs per tile, checking existence.
    active_pips: dict[str, list[tuple[str, str]]] = {}
    for tile, feats in tiles.items():
        ttype = _tile_type_for(db, tile, provisional)
        if ttype is None:
            # Tiles the decoder names but the grid does not have
            # (e.g. SLICE tiles are site names, not tile names).
            if tile.startswith("SLICE_"):
                continue
            # Only PIP features are the route checker's business; a
            # tile with config features only (e.g. a BRAM site tile
            # carrying RAMB18E2 config) is not a route violation.
            if not any(PIP_RE.match(f) for f in feats):
                continue
            rep.violations.append(f"{tile}: PIP features on unknown tile")
            continue
        tt = fabric.tt(ttype)
        pips = tt.get("pip_pairs", set())
        seen: set[tuple[str, str]] = set()
        for f in feats:
            parsed = parse_pip(f)
            if parsed is None:
                continue
            # Normalized direction: .FWD keeps (dst, src), .REV
            # swaps them (parse_pip). A suffixed spelling and its
            # plain form are the same PIP, counted once.
            dst, src = parsed
            if (src, dst) in seen:
                continue
            seen.add((src, dst))
            if (src, dst) not in pips:
                rep.violations.append(
                    f"{tile}: PIP {dst}.{src} does not exist in {ttype}"
                )
                continue
            active_pips.setdefault(tile, []).append((src, dst))
            rep.pips += 1
    # Do not abort here: a PIP-existence violation on one tile must
    # not hide the net checks on the verified PIPs. The net checks
    # below cover only the PIPs that exist; the violations above
    # name the ones that could not be verified.
    if rep.violations:
        rep.notes.append("PIP existence violations found; net checks cover only verified PIPs")
    # Drivers and sinks are stitched fabric NODES (tileconn joins
    # (tile, wire) pairs into one electrical net), not raw wires: a
    # slice output pin drives its whole node, and a PIP source is
    # driven when its node has a driver.
    def node_of(tile: str, wire: str) -> int:
        return fabric.node(tile, wire)

    # Real drivers: configured slice output pins, VCC_WIRE, primary
    # input pins. (Non-slice site OUT pins are not counted: an IOB
    # tile's features do not say which of its pins drive the fabric;
    # IOB inputs arrive via port_ins, IOB outputs are sinks.)
    real_drivers: dict[int, list[str]] = defaultdict(list)
    # Local drivers per node: real drivers plus active PIP dsts.
    local_drivers: dict[int, list[str]] = defaultdict(list)
    for tile, feats in tiles.items():
        if tile.startswith("SLICE_"):
            for pin in _slice_output_pins(feats):
                got = _slice_pin_wire(db, fabric, tile, pin, provisional)
                if got is not None:
                    wtile, wire = got
                    nid = node_of(wtile, wire)
                    real_drivers[nid].append(f"{tile}.{pin}")
                    local_drivers[nid].append(f"{tile}.{pin}")
            continue
        ttype = _tile_type_for(db, tile, provisional)
        if ttype is None:
            continue
        tt = fabric.tt(ttype)
        if "VCC_WIRE" in tt.get("wires", set()):
            nid = node_of(tile, "VCC_WIRE")
            real_drivers[nid].append("const 1")
            local_drivers[nid].append("const 1")
    # Primary inputs drive their site pin's node.
    for _port, (site, pin) in (port_ins or {}).items():
        if site in db.site_to_tile:
            tile, idx = db.site_index(site)
            ttype = db.tilegrid[tile]["type"]
            wire = _pin_wire(fabric, ttype, idx, pin)
            got = (tile, wire) if wire else None
        else:
            got = _slice_pin_wire(db, fabric, site, pin, provisional)
        if got is not None:
            wtile, wire = got
            nid = node_of(wtile, wire)
            real_drivers[nid].append(f"primary {site}.{pin}")
            local_drivers[nid].append(f"primary {site}.{pin}")
    # Active PIPs drive their dst node.
    pip_edges: dict[int, list[int]] = defaultdict(list)
    for tile, plist in active_pips.items():
        for src, dst in plist:
            snid = node_of(tile, src)
            dnid = node_of(tile, dst)
            pip_edges[snid].append(dnid)
            local_drivers[dnid].append(f"{tile} PIP {dst}.{src}")
    rep.nets = len(real_drivers)
    for nid, ds in sorted(local_drivers.items()):
        if len(ds) > 1:
            members = ", ".join(f"{t}.{w}" for t, w in fabric.nodes[nid][:3])
            rep.violations.append(
                f"net {{{members}}} has {len(ds)} drivers: {', '.join(ds)}"
            )
    # Sinks that must be driven: active PIP sources, used site inputs.
    sinks: list[tuple[int, str]] = []
    for tile, plist in active_pips.items():
        for src, dst in plist:
            sinks.append((node_of(tile, src), f"PIP {dst}.{src} source on {tile}"))
    for tile, feats in tiles.items():
        if not tile.startswith("SLICE_"):
            continue
        for pin in _slice_input_pins(feats):
            if (tile, pin) in port_in_pins:
                continue  # driven by a primary input
            got = _slice_pin_wire(db, fabric, tile, pin, provisional)
            if got is not None:
                wtile, wire = got
                sinks.append((node_of(wtile, wire), f"{tile}.{pin}"))
    for nid, what in sinks:
        if not local_drivers.get(nid):
            members = ", ".join(f"{t}.{w}" for t, w in fabric.nodes[nid][:3])
            rep.violations.append(f"undriven sink: {what} on net {{{members}}}")
    # Net trees: every PIP-dst node must be reachable from a real
    # driver over PIP edges; otherwise the net is undriven (or a
    # driverless loop), even though each node has a local driver.
    reached: set[int] = set()
    q = deque(real_drivers.keys())
    while q:
        nid = q.popleft()
        if nid in reached:
            continue
        reached.add(nid)
        for nxt in pip_edges.get(nid, []):
            if nxt not in reached:
                q.append(nxt)
    for nid, ds in local_drivers.items():
        if nid in reached:
            continue
        if len(ds) == 1 and "PIP " in ds[0]:
            members = ", ".join(f"{t}.{w}" for t, w in fabric.nodes[nid][:3])
            rep.violations.append(
                f"net {{{members}}} is driven only by a PIP, not reachable "
                "from any real driver"
            )
    return rep


def _pin_dir(db: Db, stype: str, pin: str) -> str | None:
    if not db.has_site_type(stype):
        return None
    st = db.site_type(stype)
    return st.get("site_pins", {}).get(pin, {}).get("direction")


def _slice_output_pins(feats) -> set[str]:
    """Output pins of a slice that are driven by configured BELs."""
    from .extract import LETTERS
    from .fasmio import init_value

    pins = set()
    for c in LETTERS:
        for num in ("1", "2"):
            if any(f.startswith(f"FFMUX{c}{num}.") for f in feats):
                pins.add(f"{c}Q" if num == "1" else f"{c}Q2")
        # The {c}MUX pin is driven only through the OUTMUX: a LUT
        # with an INIT but no OUTMUX selection leaves its MUX pin
        # undriven (the extractor emits X for it).
        if any(f.startswith(f"OUTMUX{c}.") for f in feats):
            pins.add(f"{c}MUX")
        # The {c}_O pin is the LUT O6 output: it drives the fabric
        # when the LUT is live (INIT present) and the letter's
        # output is selected by an FFMUX or OUTMUX feature. Without
        # this, a LUT driving the fabric through its O pin (the
        # FFMUX BYP shape: LUT O through the INT to the AX pin)
        # fails as undriven / driven-only-by-PIP.
        if init_value(feats, c) is not None and (
            any(f.startswith(f"FFMUX{c}{num}.") for num in ("1", "2") for f in feats)
            or any(f.startswith(f"OUTMUX{c}.") for f in feats)
        ):
            pins.add(f"{c}_O")
    if any(
        f.startswith(f"FFMUX{c}") and f.endswith((".CY", ".XORIN"))
        or f.startswith(f"OUTMUX{c}") and f.endswith((".CY", ".XORIN"))
        for c in LETTERS
        for f in feats
    ):
        pins.add("COUT")
    return pins


def _slice_input_pins(feats) -> set[str]:
    """Input pins of a slice consumed by configured BELs: only LUT
    inputs the INIT actually depends on (an unused input is tied
    off inside the model, not a routing sink), the clock of each
    used FF group, and CE/SR only when the group uses them."""
    from .extract import GROUP_CE, GROUP_SEG, HALF_CLK, HALF_LETTERS, HALF_SR, LETTERS
    from .fasmio import group_attr, init_value, lut_depends

    pins = set()
    for c in LETTERS:
        init = init_value(feats, c)
        if init is None or init == 0:
            continue
        for k in range(6):
            if lut_depends(init, k):
                pins.add(f"{c}{k + 1}")
    for g, seg in GROUP_SEG.items():
        half = "ABCD" if g.startswith("ABCD") else "EFGH"
        letters = HALF_LETTERS[half]
        used = any(
            any(f.startswith(f"FFMUX{c}{2 if g.endswith('2') else 1}.") for f in feats)
            for c in letters
        )
        if not used:
            continue
        pins.add(HALF_CLK[half])
        if group_attr(feats, seg, "CEUSED") == 1:
            pins.add(GROUP_CE[g])
        if group_attr(feats, seg, "SRUSED") == 1:
            pins.add(HALF_SR[half])
    if any(f.startswith("CARRY8.CI.") for f in feats) or any(
        f.startswith(f"FFMUX{c}") and f.endswith((".CY", ".XORIN"))
        or f.startswith(f"OUTMUX{c}") and f.endswith((".CY", ".XORIN"))
        for c in LETTERS
        for f in feats
    ):
        # CIN is a routing sink only when the carry input comes from
        # the pin: CI.AX, or no CI feature at all. CI.V0/V1 make the
        # carry input a constant and the pin unused.
        if not any(
            f in ("CARRY8.CI.V0", "CARRY8.CI.V1") for f in feats
        ):
            pins.add("CIN")
    return pins
