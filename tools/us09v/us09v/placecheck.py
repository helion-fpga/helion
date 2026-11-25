"""Placement legality from decoded FASM, checked against the public
tilegrid and site_types (not against Helion's own pack/place
structures, which this oracle must not trust).

Checks, per design:
- site type: every SLICE tile in the FASM exists in the tilegrid with
  a CLE tile type; SLICEM-only features (LUT MODE.RAM/SRL) sit only
  on CLEM/CLEM_R tiles.
- capacity: at most 8 LUTs, 16 FFs, 1 CARRY8 per slice.
- no overlaps: no site is configured twice (defensive: a FASM dict
  cannot express it, but the placement-list input path can).
- control sets, per half CLB (assumption A-CTRL: ABCD half uses
  CLK1/SRST1, EFGH half uses CLK2/SRST2; CE pins CKEN1-4 map to the
  four FF groups ABCD-Q, ABCD-Q2, EFGH-Q, EFGH-Q2): at most 1
  distinct CLK net and 1 distinct SR net per half, at most 2
  distinct CE nets per half. Note: the crate's legality.rs applies
  stricter per-slice limits (1 CLK, 1 SR, 2 CE per slice); this
  checker applies the per-half rule from the brief, and the
  divergence is flagged in the README.
- carry chains: a CARRY8 slice whose CIN is driven by another slice
  must take it from the slice directly below (same X, Y-1), and the
  chain must be contiguous and vertical.

Inputs: either parsed FASM tiles (with a Db for the tilegrid and a
Fabric for driver resolution) or an explicit placement list
[(cell, site, bel)] for the overlap/capacity tests.

No em dashes anywhere.
"""
from __future__ import annotations

import re
from dataclasses import dataclass, field

from .db import Db
from .extract import CLE_TILE_TO_SITE, HALF_CLK, HALF_SR, LETTERS
from .fabric import Fabric

CLE_TYPES = set(CLE_TILE_TO_SITE)


@dataclass
class PlaceReport:
    violations: list[str] = field(default_factory=list)
    notes: list[str] = field(default_factory=list)
    slices: int = 0
    carry_chains: int = 0

    @property
    def ok(self) -> bool:
        return not self.violations


def _site_xy(site: str) -> tuple[int, int] | None:
    m = re.match(r"^SLICE_X(\d+)Y(\d+)$", site)
    return (int(m.group(1)), int(m.group(2))) if m else None


def check_place_fasm(
    db: Db | None,
    tiles: dict[str, dict[str, int | None]],
    fabric: Fabric | None = None,
    port_ins: dict[str, tuple[str, str]] | None = None,
    provisional: bool = False,
) -> PlaceReport:
    """`provisional`: tiles whose site has no tilegrid entry (golden
    xcku3p designs) are noted, not failed, for the site-type check.
    `port_ins` maps netlist port names to (site, pin) primary inputs
    so control pins driven by the testbench count as driven."""
    rep = PlaceReport()
    slices = {t: f for t, f in tiles.items() if t.startswith("SLICE_")}
    rep.slices = len(slices)
    if db is None:
        rep.notes.append("no tilegrid: site-type checks skipped (provisional tiles)")
    if provisional:
        rep.notes.append("provisional tiles: site-type checks are notes, not failures")
    resolver = None
    if fabric is not None:
        if db is None:
            raise ValueError("fabric needs a Db")
        from .extract import Extractor

        resolver = Extractor(db, tiles, port_ins=port_ins)
    for site, feats in sorted(slices.items()):
        xy = _site_xy(site)
        if xy is None:
            rep.violations.append(f"{site}: not a SLICE_XxYy site name")
            continue
        if db is not None:
            info = db.site_to_tile.get(site)
            if info is None:
                msg = f"{site}: no tilegrid entry"
                if provisional:
                    rep.notes.append(msg)
                else:
                    rep.violations.append(msg)
            else:
                tile, _stype = info
                ttype = db.tilegrid[tile]["type"]
                if ttype not in CLE_TYPES:
                    rep.violations.append(
                        f"{site}: tile {tile} has type {ttype}, not a CLE tile"
                    )
                elif ttype not in ("CLEM", "CLEM_R") and any(
                    f.endswith("LUT.MODE.RAM") or f.endswith("LUT.MODE.SRL")
                    for f in feats
                ):
                    rep.violations.append(
                        f"{site}: memory LUT on {ttype} (requires SLICEM)"
                    )
        luts = {c for c in LETTERS if f"{c}6LUT.INIT[63:0]" in feats or any(
            re.match(rf"^{c}LUT\.INIT\[\d+\]$", f) for f in feats
        )}
        if len(luts) > 8:
            rep.violations.append(f"{site}: {len(luts)} LUTs exceeds max 8")
        ffs = set()
        for c in LETTERS:
            for num in ("1", "2"):
                if any(f.startswith(f"FFMUX{c}{num}.") for f in feats):
                    ffs.add((c, num))
        if len(ffs) > 16:
            rep.violations.append(f"{site}: {len(ffs)} FFs exceeds max 16")
        if resolver is not None and ffs:
            _check_control_sets(site, feats, ffs, resolver, rep)
    if resolver is not None:
        _check_carry_chains(tiles, resolver, rep)
    return rep


def _group_srused(feats, group: str) -> bool:
    from .fasmio import group_attr

    seg = {"ABCD": "ABCDFF", "ABCD2": "ABCDFF2", "EFGH": "EFGHFF", "EFGH2": "EFGHFF2"}[group]
    return group_attr(feats, seg, "SRUSED") == 1


def _check_control_sets(site, feats, ffs, resolver, rep):
    s = resolver.sites[site]
    halves = set()
    for c, _num in ffs:
        halves.add("ABCD" if c in "ABCD" else "EFGH")
    for half in sorted(halves):
        clk = resolver.pin_driver(s, HALF_CLK[half])
        if clk[0] == "undriven":
            rep.violations.append(
                f"{site}: {half} half has FFs but {HALF_CLK[half]} is undriven"
            )
        groups = ("ABCD", "ABCD2") if half == "ABCD" else ("EFGH", "EFGH2")
        if any(_group_srused(feats, g) for g in groups):
            sr = resolver.pin_driver(s, HALF_SR[half])
            if sr[0] == "undriven":
                rep.violations.append(
                    f"{site}: {half} half uses SR but {HALF_SR[half]} is undriven"
                )
    for half, pins in (("ABCD", ("CKEN1", "CKEN2")), ("EFGH", ("CKEN3", "CKEN4"))):
        if half not in halves:
            continue
        nets = set()
        for pin in pins:
            d = resolver.pin_driver(s, pin)
            if d[0] in ("site", "const", "primary"):
                nets.add(d)
        if len(nets) > 2:
            rep.violations.append(
                f"{site}: {half} half has {len(nets)} distinct CE nets, max 2"
            )


def _check_carry_chains(tiles, resolver, rep):
    slices = {t: f for t, f in tiles.items() if t.startswith("SLICE_")}
    carry_sites = {}
    for site, feats in slices.items():
        used = any(
            f.startswith(f"FFMUX{c}") and f.endswith((".CY", ".XORIN"))
            or f.startswith(f"OUTMUX{c}") and f.endswith((".CY", ".XORIN"))
            for c in LETTERS
            for f in feats
        ) or any(
            f.startswith("CARRY8.CI.") or f.startswith("CARRY8.CI_TOP.")
            for f in feats
        )
        if used:
            carry_sites[site] = feats
    if not carry_sites:
        return
    rep.carry_chains = len(carry_sites)
    for site in sorted(carry_sites):
        xy = _site_xy(site)
        if xy is None:
            continue
        s = resolver.sites[site]
        d = resolver.pin_driver(s, "CIN")
        if d[0] == "site":
            src_site, src_pin = d[1], d[2]
            if src_pin != "COUT":
                rep.violations.append(
                    f"{site}: CIN driven by {src_site}.{src_pin}, not a COUT"
                )
                continue
            sxy = _site_xy(src_site)
            if sxy is None:
                rep.violations.append(
                    f"{site}: CIN driver site {src_site} is not a slice"
                )
                continue
            if sxy != (xy[0], xy[1] - 1):
                rep.violations.append(
                    f"{site}: CIN comes from {src_site}, not the slice "
                    f"directly below (SLICE_X{xy[0]}Y{xy[1] - 1})"
                )
                continue
            # The source slice must itself propagate carry: its COUT
            # is X when it has no CARRY8, so a chain that skips a
            # carry-less slice is broken even though the tileconn
            # stitch lands on the slice directly below.
            if src_site not in carry_sites:
                rep.violations.append(
                    f"{site}: CIN comes from {src_site}, which has no "
                    f"CARRY8 in use (chain gap)"
                )
        elif d[0] in ("const", "primary"):
            continue
        elif d[0] == "undriven":
            feats = carry_sites[site]
            if (
                "CARRY8.CI.V0" in feats
                or "CARRY8.CI.V1" in feats
                or "CARRY8.CI.AX" in feats
            ):
                continue
            rep.violations.append(
                f"{site}: CARRY8 in use but CIN is undriven and no CI feature"
            )


def check_place_list(placements: list[tuple[str, str, str]]) -> PlaceReport:
    """Placement-list path: [(cell, site, bel)] with no FASM. Checks
    overlaps (same site+bel twice) and capacity (8 LUTs, 16 FFs,
    1 CARRY8 per site)."""
    rep = PlaceReport()
    seen: dict[tuple[str, str], str] = {}
    per_site_lut: dict[str, int] = {}
    per_site_ff: dict[str, int] = {}
    per_site_carry: dict[str, int] = {}
    for cell, site, bel in placements:
        key = (site, bel)
        if key in seen:
            rep.violations.append(
                f"{site}.{bel}: used by both {seen[key]} and {cell}"
            )
        else:
            seen[key] = cell
        if "LUT" in bel:
            per_site_lut[site] = per_site_lut.get(site, 0) + 1
        elif "FF" in bel:
            per_site_ff[site] = per_site_ff.get(site, 0) + 1
        elif bel == "CARRY8":
            per_site_carry[site] = per_site_carry.get(site, 0) + 1
    rep.slices = len({s for _, s, _ in placements})
    for site, n in per_site_lut.items():
        if n > 8:
            rep.violations.append(f"{site}: {n} LUTs exceeds max 8")
    for site, n in per_site_ff.items():
        if n > 16:
            rep.violations.append(f"{site}: {n} FFs exceeds max 16")
    for site, n in per_site_carry.items():
        if n > 1:
            rep.violations.append(f"{site}: {n} CARRY8 exceeds max 1")
    return rep
