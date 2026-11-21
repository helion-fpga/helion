"""Port resolution for real designs: XDC package pins to extractor
port maps.

Inputs come from the IOB site pin "I" (direction OUT in the DB:
the pad drives the fabric through the input buffer). Outputs are
traced backwards from the IOB site pin "OP" (direction IN: the
fabric drives the pad through the output buffer) through active
PIPs and tileconn stitching to the driving slice output pin, so the
extractor's output port is the slice signal, not an unmodelled IOB.

The IOB site itself is never modelled: the pad is a primary port
for inputs, and the traced slice pin for outputs. A design whose
clock goes through a BUFG/BUFGCE still fails loud in the extractor
(clock sites are out of scope for v1); a directly routed clock
works because driver resolution finds the IOB "I" pin.

No em dashes anywhere.
"""
from __future__ import annotations

import re
from collections import deque

from .cosim import parse_xdc
from .db import Db
from .fabric import Fabric


def port_name(site: str, pin: str) -> str:
    """Netlist port name for a slice-pin port (explicit mode)."""
    return re.sub(r"[^A-Za-z0-9_]", "_", f"p_{site}_{pin}")


def explicit_ports(port_map: dict[str, str]) -> tuple[dict, dict, dict]:
    """Explicit port map {rtl bit: 'SITE.PIN'} -> (port_ins,
    port_outs, cosim pin_of). Direction is decided by the caller
    (verify_fasm): this helper only parses the map. Use
    explicit_ports_in_out with the RTL port directions instead."""
    raise NotImplementedError("use explicit_ports_in_out")


def explicit_ports_in_out(
    port_map: dict[str, str], in_bits: set[str], out_bits: set[str]
) -> tuple[dict[str, tuple[str, str]], dict[str, tuple[str, str]], dict[str, str]]:
    """{rtl bit: 'SITE.PIN'} plus the RTL port directions ->
    (port_ins, port_outs, cosim pin_of)."""
    port_ins: dict[str, tuple[str, str]] = {}
    port_outs: dict[str, tuple[str, str]] = {}
    pin_of: dict[str, str] = {}
    for bit, sitepin in port_map.items():
        if "." not in sitepin:
            raise ValueError(f"port map {bit}: {sitepin!r} is not SITE.PIN")
        site, pin = sitepin.rsplit(".", 1)
        name = port_name(site, pin)
        pin_of[bit] = name
        if bit in in_bits:
            port_ins[name] = (site, pin)
        elif bit in out_bits:
            port_outs[name] = (site, pin)
        else:
            raise ValueError(f"port map {bit}: not an RTL port bit")
    return port_ins, port_outs, pin_of


def trace_pad_to_slice_pin(
    db: Db, tiles, fabric: Fabric, iob_tile: str, iob_site_index: int
) -> tuple[str, str] | None:
    """Trace an IOB output backwards from the site's OP pin wire to
    the driving slice output pin. Returns (slice site, pin) or None
    when the route cannot be traced (the caller reports it)."""
    tt = fabric.tt(db.tilegrid[iob_tile]["type"])
    sites = tt["sites"]
    if iob_site_index >= len(sites):
        return None
    op_wire = (sites[iob_site_index].get("site_pins", {}).get("OP") or {}).get("wire")
    if op_wire is None:
        return None
    # Reverse BFS over (tile, wire): predecessors are tileconn
    # neighbours (undirected) and active PIPs in this tile whose dst
    # is the current wire (go to src). Stop at a slice site OUT pin.
    start = (iob_tile, op_wire)
    seen = {start}
    q = deque([start])
    while q:
        tile, wire = q.popleft()
        ttype = db.tilegrid[tile]["type"]
        ttt = fabric.tt(ttype)
        # Is this wire a slice site output pin?
        if (wire in ttt["site_pin_wire"]) and tile.startswith("SLICE_"):
            si, pin = ttt["site_pin_wire"][wire]
            stype = ttt["sites"][si].get("type", ttype)
            if _pin_dir(db, stype, pin) == "OUT":
                return (tile, pin)
        # Tileconn neighbours.
        nid = fabric.node(tile, wire)
        for t2, w2 in fabric.nodes[nid]:
            if (t2, w2) not in seen:
                seen.add((t2, w2))
                q.append((t2, w2))
        # Active PIPs into this wire (reverse direction).
        for f, _v in tiles.get(tile, {}).items():
            m = re.match(r"^PIP\.([^.]+)\.([^.]+)$", f)
            if m and m.group(1) == wire:
                src = m.group(2)
                if (tile, src) not in seen:
                    seen.add((tile, src))
                    q.append((tile, src))
    return None


def _pin_dir(db: Db, stype: str, pin: str) -> str | None:
    if not db.has_site_type(stype):
        return None
    return db.site_type(stype).get("site_pins", {}).get(pin, {}).get("direction")


def ports_from_xdc(
    db: Db, tiles, fabric: Fabric, xdc_path: str, in_bits: set[str], out_bits: set[str]
) -> tuple[dict[str, tuple[str, str]], dict[str, tuple[str, str]], dict[str, str], list[str]]:
    """XDC -> (port_ins, port_outs, cosim pin_of, errors). port_ins
    keys are package pins; port_outs keys are package pins."""
    pin_of, _clocks = parse_xdc(xdc_path)
    pin_to_site = {pin: site for site, pin in db.package_pins().items()}
    port_ins: dict[str, tuple[str, str]] = {}
    port_outs: dict[str, tuple[str, str]] = {}
    errors: list[str] = []
    for bit, pkg in sorted(pin_of.items()):
        site = pin_to_site.get(pkg)
        if site is None:
            errors.append(f"package pin {pkg} (port {bit}): no site in package_pins.csv")
            continue
        if site not in db.site_to_tile:
            errors.append(f"site {site} (port {bit}, pin {pkg}): no tilegrid entry")
            continue
        tile, site_index = db.site_index(site)
        if bit in in_bits:
            port_ins[pkg] = (site, "I")
        elif bit in out_bits:
            traced = trace_pad_to_slice_pin(db, tiles, fabric, tile, site_index)
            if traced is None:
                errors.append(
                    f"output {bit} (pin {pkg}, site {site}): could not trace "
                    "the route back to a slice output pin"
                )
                continue
            port_outs[pkg] = traced
        else:
            errors.append(f"XDC port {bit}: not an RTL port bit")
    return port_ins, port_outs, pin_of, errors
