"""Decoded FASM -> flat gate-level netlist (bitstream LVS, UltraScale+).

Electrical model, all from the public prjuray-db database plus the
vendored segbits feature vocabulary:

* Nodes: tile wires joined by `tileconn.json` stitching rules.
* Edges: active PIP features (`PIP.<dst>.<src>`, physical direction
  src -> dst) and the VCC_WIRE constant. No pseudo-pip defaults exist
  for UltraScale+ in prjuray-db: an unselected mux input is undriven.
* Sites: per-site behavioural models driven by the site's features.
  A site output pin is driven by its site model only.
* LUT6_2: O6 = INIT[{A6..A1}], O5 = INIT[{0,A5..A1}] (A1 is the LSB).
* FFs: INIT/SRVAL polarity, the V suffix IS the value (V0 is 0,
  V1 is 1); the segbits bit is NOT the value (V0 sets the bit,
  V1 clears it, bit = NOT(INIT)). CEUSED, SRUSED, CLKINV, SRINV
  polarity V1 sets, V0 clears. SYNC.SYNC is a
  synchronous reset, SYNC.ASYNC (or absent) asynchronous. MODE.LATCH
  is an extraction error, not modelled.
* F7/F8/F9 (assumption A-F7F8, see README): F7AMUX = AX ? B : A,
  F7BMUX = CX ? D : C, F7CMUX = EX ? F : E, F7DMUX = GX ? H : G,
  F8AMUX = BX ? F7B : F7A, F8BMUX = FX ? F7D : F7C,
  F9MUX = HX ? F8B : F8A. The F7F8 input of each letter mux is
  A->F7A, B->F8A, C->F7B, D->F8A, E->F7C, F->F8B, G->F7D, H->F9.
  Every use is recorded as a warning: the mapping is not validated
  against a Vivado F7/F8 build yet.
* CARRY8: S[i] is the LUT O6 output, DI[i] is the DI mux selection
  (DI{i}.X{i} = bypass pin, DI{i}.DI{i} is unmodelled and fails loud),
  O[i] = S[i] ^ C[i], CO[i] = S[i] ? C[i] : DI[i]. SINGLE_CY8 ripples
  C[0]=CI through all 8 bits; DUAL_CY4 ripples the low nibble from CI
  and the high nibble from CI_TOP. CI defaults to the CIN pin; CI_TOP
  has no default in DUAL_CY4 mode and fails loud when absent.
* An unrouted CARRY8 DI bypass pin reads 0 (assumption A-DI0, pinned
  by the carry8 golden's stated 8-bit incrementer intent: with DI=1
  every bit would toggle every cycle instead of incrementing).
* LUTRAM and SRL modes are extraction errors (blocked, no public
  encoding). DSP, BRAM, URAM, clock and CMT sites are extraction
  errors when configured: this rebuild covers the CLB only.
* A missing FFMUX selection on a used FF is an extraction error. A
  missing OUTMUX selection is an error when the slice output is
  consumed by routing, else the output is X with a warning.

Nothing is assumed silently: undriven inputs that matter, multiply
driven nodes, and configured features the models do not understand
are collected in `Netlist.errors` / `Netlist.warnings`.

No em dashes anywhere.
"""
from __future__ import annotations

import re
from collections import Counter, defaultdict, deque
from dataclasses import dataclass, field

from .db import Db
from .fabric import Fabric
from .fasmio import ff_attr, group_attr, init_value, lut_depends, parse_pip

LETTERS = "ABCDEFGH"

# F7F8 input of each letter mux (assumption A-F7F8).
F7F8_OF = {
    "A": "F7A",
    "B": "F8A",
    "C": "F7B",
    "D": "F8A",
    "E": "F7C",
    "F": "F8B",
    "G": "F7D",
    "H": "F9",
}

# Control pins per half and per group (assumption A-CTRL):
# ABCD half uses CLK1/SRST1, EFGH half uses CLK2/SRST2;
# CE pins per FF group: ABCD-Q CKEN1, ABCD-Q2 CKEN2,
# EFGH-Q CKEN3, EFGH-Q2 CKEN4.
HALF_CLK = {"ABCD": "CLK1", "EFGH": "CLK2"}
HALF_SR = {"ABCD": "SRST1", "EFGH": "SRST2"}
GROUP_CE = {"ABCD": "CKEN1", "ABCD2": "CKEN2", "EFGH": "CKEN3", "EFGH2": "CKEN4"}
GROUP_HALF = {"ABCD": "ABCD", "ABCD2": "ABCD", "EFGH": "EFGH", "EFGH2": "EFGH"}
# segbits group name per FF group.
GROUP_SEG = {"ABCD": "ABCDFF", "ABCD2": "ABCDFF2", "EFGH": "EFGHFF", "EFGH2": "EFGHFF2"}
# Letters per half.
HALF_LETTERS = {"ABCD": "ABCD", "EFGH": "EFGH"}

SLICE_TYPES = {"SLICEL", "SLICEM"}
CLE_TILE_TO_SITE = {
    "CLEL_L": "SLICEL",
    "CLEL_R": "SLICEL",
    "CLEM": "SLICEM",
    "CLEM_R": "SLICEM",
}


@dataclass
class Site:
    name: str
    stype: str
    tile: str
    index: int
    features: dict[str, int | None] = field(default_factory=dict)

    def plain(self) -> set[str]:
        return set(self.features)


@dataclass
class Netlist:
    sites: dict[str, Site]
    lines: list[str] = field(default_factory=list)
    ports_in: dict[str, str] = field(default_factory=dict)
    ports_out: dict[str, str] = field(default_factory=dict)
    regs: list[tuple[str, str]] = field(default_factory=list)
    errors: list[str] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)
    clock_sinks: list[tuple[str, str]] = field(default_factory=list)
    unmodelled_sites: list[str] = field(default_factory=list)


def sig(site: str, pin: str) -> str:
    return re.sub(r"[^A-Za-z0-9_]", "_", f"s_{site}__{pin}")


class Extractor:
    def __init__(
        self,
        db: Db,
        tiles: dict[str, dict[str, int | None]],
        port_ins: dict[str, tuple[str, str]] | None = None,
        port_outs: dict[str, tuple[str, str]] | None = None,
    ):
        """db: the prjuray-db database (tilegrid + tileconn required).
        tiles: parsed FASM {tile: {feature: value}}.
        port_ins: {port name: (site, pin)} for testbench-driven inputs
          (slice-pin level fixtures, or IOB pads by site pin).
        port_outs: {port name: (site, pin)} for sampled outputs."""
        self.db = db
        self.tiles = tiles
        self.fab = Fabric(db)
        self.port_ins = port_ins or {}
        self.port_outs = port_outs or {}
        self._port_in_by_sitepin = {v: k for k, v in self.port_ins.items()}
        self._port_out_by_sitepin = {v: k for k, v in self.port_outs.items()}
        # Non-slice sites that are port pads (IOB sites): skipped in
        # build() instead of reported as unmodelled.
        self._port_sites = {s for (s, _p) in self._port_in_by_sitepin} | {
            s for (s, _p) in self._port_out_by_sitepin
        }
        self.sites: dict[str, Site] = {}
        self.site_by_tile_index: dict[tuple[str, int], Site] = {}
        self.errors: list[str] = []
        self.warnings: list[str] = []
        self._driver_memo: dict[int, tuple] = {}
        self.used_sites: set[str] = set()
        self.unmodelled_sites: list[str] = []
        self._consumed: dict[str, set[str]] = {}
        # Non-slice tiles with features: (tile, tile type).
        self._other_tiles: list[tuple[str, str]] = []
        self._index_sites()

    # ---- site discovery -------------------------------------------------

    def _index_sites(self):
        for tile, feats in self.tiles.items():
            m = re.match(r"^SLICE_X(\d+)Y(\d+)$", tile)
            if m:
                stype = self._slice_type(tile, feats)
                s = Site(tile, stype, tile, 0, dict(feats))
                self.sites[tile] = s
                self.site_by_tile_index[(tile, 0)] = s
                if feats:
                    self.used_sites.add(tile)
                continue
            if feats:
                ttype = self.db.tilegrid.get(tile, {}).get("type")
                if ttype is not None:
                    self._other_tiles.append((tile, ttype))

    def _slice_type(self, site: str, feats: dict) -> str:
        info = self.db.site_to_tile.get(site)
        if info is not None:
            tile, stype = info
            tile_type = self.db.tilegrid[tile]["type"]
            mapped = CLE_TILE_TO_SITE.get(tile_type)
            if mapped is None:
                self.errors.append(
                    f"{site}: CLB tile {tile} has unexpected tile type {tile_type}"
                )
                return "SLICEL"
            return mapped
        # Provisional site (no tilegrid entry, e.g. golden xcku3p
        # tiles): infer from MODE features, and say so.
        self.warnings.append(
            f"{site}: no tilegrid entry, site type inferred from features"
        )
        if any(f.endswith("LUT.MODE.RAM") or f.endswith("LUT.MODE.SRL") for f in feats):
            return "SLICEM"
        return "SLICEL"

    def _ttype(self, tile: str) -> str | None:
        """Tile type for driver resolution. Provisional SLICE tiles
        (golden xcku3p designs, no tilegrid entry) assume CLEL_R, a
        single-SLICEL tile; other unknown tiles have no type and
        contribute no drivers or pips."""
        t = self.db.tilegrid.get(tile)
        if t is not None:
            return t["type"]
        if tile.startswith("SLICE_"):
            info = self.db.site_to_tile.get(tile)
            if info is not None:
                return self.db.tilegrid[info[0]]["type"]
            return "CLEL_R"
        return None

    def _fabric_tile(self, site: Site) -> str:
        """The tilegrid tile owning a site's wires. A slice site's
        `tile` field is the site name (SLICE_XxYy), but tileconn
        stitching is keyed by the grid tile (CLEL_R_XxYy); using the
        site name as the tile silently makes every routed input
        undriven."""
        info = self.db.site_to_tile.get(site.name)
        return info[0] if info is not None else site.tile

    def site_at(self, tile: str, index: int) -> Site:
        key = (tile, index)
        if key in self.site_by_tile_index:
            return self.site_by_tile_index[key]
        # Non-slice sites reached as drivers: build a bare site record.
        # Features stay on the tile record (see _other_tiles); the site
        # record only carries identity for driver resolution.
        ttype = self._ttype(tile)
        if ttype is None:
            s = Site(f"{tile}_site{index}", "UNKNOWN", tile, index, {})
            self.sites[s.name] = s
            self.site_by_tile_index[key] = s
            return s
        # The site name is absolute (SLICE_XxYy) in the tilegrid
        # sites dict; the tile_type sites list uses tile-relative
        # names (X0Y0) and must not be used for identity.
        name = None
        stype = None
        grid_sites = self.db.tilegrid.get(tile, {}).get("sites", {})
        if grid_sites:
            names = list(grid_sites)
            if index < len(names):
                name = names[index]
                stype = grid_sites[name]
        if name is None:
            tt = self.fab.tt(ttype)
            if index < len(tt["sites"]):
                name = tt["sites"][index]["name"]
                stype = tt["sites"][index].get("type", ttype)
        if name is None:
            name = f"{tile}_site{index}"
            stype = ttype
        if name in self.sites:
            s = self.sites[name]
        else:
            s = Site(name, stype, tile, index, {})
            self.sites[name] = s
        self.site_by_tile_index[key] = s
        return s

    # ---- driver resolution ------------------------------------------------

    def driver(self, nid: int, _stack=None) -> tuple:
        """Resolve a node to ('site', site, pin) | ('const', v) |
        ('primary', port) | ('unmodelled', site, pin) | ('undriven',)."""
        if nid in self._driver_memo:
            return self._driver_memo[nid]
        stack = _stack if _stack is not None else set()
        if nid in stack:
            return ("loop",)
        stack.add(nid)
        sources = []
        for tile, wire in self.fab.nodes[nid]:
            ttype = self._ttype(tile)
            if ttype is None:
                continue
            tt = self.fab.tt(ttype)
            act = self.tiles.get(tile, {})
            if wire == "VCC_WIRE":
                sources.append(("const", 1, f"{tile}/VCC_WIRE"))
                continue
            if wire in tt["site_pin_wire"]:
                si, pin = tt["site_pin_wire"][wire]
                s = self.site_at(tile, si)
                direction = self._pin_dir(s.stype, pin)
                if direction == "OUT":
                    if (s.name, pin) in self._port_in_by_sitepin:
                        sources.append(("primary", self._port_in_by_sitepin[(s.name, pin)]))
                    elif s.stype in SLICE_TYPES:
                        sources.append(("site", s.name, pin))
                    elif self.tiles.get(tile):
                        sources.append(("unmodelled", s.name, pin))
                    # An unconfigured site drives nothing.
                    continue
            for src, _bid in tt["pips_into"].get(wire, ()):
                # A PIP driving `wire` from `src` may be spelled
                # plain (PIP.wire.src), FWD (PIP.wire.src.FWD), or
                # REV with the components swapped
                # (PIP.src.wire.REV); see fasmio.parse_pip.
                if (
                    f"PIP.{wire}.{src}" in act
                    or f"PIP.{wire}.{src}.FWD" in act
                    or f"PIP.{src}.{wire}.REV" in act
                ):
                    sources.append(("pip", tile, src))
        result: tuple = ("undriven",)
        resolved = []
        for s in sources:
            if s[0] in ("site", "const", "unmodelled"):
                resolved.append(s)
                continue
            r = self.driver(self.fab.node(s[1], s[2]), stack)
            if r[0] not in ("undriven", "loop"):
                resolved.append(r)
        uniq = set()
        for r in resolved:
            if r[0] == "site":
                uniq.add(("site", r[1], r[2]))
            elif r[0] == "const":
                uniq.add(("const", r[1]))
            elif r[0] == "primary":
                uniq.add(("primary", r[1]))
            else:
                uniq.add((r[0], r[1], r[2] if len(r) > 2 else ""))
        if len(uniq) > 1:
            self.errors.append(
                f"multiply driven node {self.fab.nodes[nid][:2]}: {sorted(uniq, key=str)}"
            )
        if resolved:
            r = resolved[0]
            if r[0] == "site":
                result = ("site", r[1], r[2])
            elif r[0] == "const":
                result = ("const", r[1])
            elif r[0] == "primary":
                result = ("primary", r[1])
            else:
                result = r
        stack.discard(nid)
        if _stack is None or result[0] != "undriven":
            self._driver_memo[nid] = result
        return result

    def _pin_dir(self, stype: str, pin: str) -> str:
        if not self.db.has_site_type(stype):
            return "?"
        pins = self.db.site_type(stype).get("site_pins", {})
        return pins.get(pin, {}).get("direction", "?")

    def pin_driver(self, site: Site, pin: str) -> tuple:
        if (site.name, pin) in self._port_in_by_sitepin:
            return ("primary", self._port_in_by_sitepin[(site.name, pin)])
        ttype = self._ttype(site.tile)
        if ttype is None:
            return ("nopin",)
        tt = self.fab.tt(ttype)
        wire = (tt["sites"][site.index].get("site_pins", {}).get(pin) or {}).get("wire")
        if wire is None:
            return ("nopin",)
        d = self.driver(self.fab.node(self._fabric_tile(site), wire))
        if d[0] == "site":
            self.used_sites.add(d[1])
        return d

    def expr(self, d: tuple) -> str:
        if d[0] == "site":
            return sig(d[1], d[2])
        if d[0] == "const":
            return f"1'b{d[1]}"
        if d[0] == "primary":
            # The port drives an internal signal named after the site
            # pin; the model references the signal, to_verilog wires
            # the port to it. (Returning the port name here used to
            # make to_verilog emit a self-assign.)
            site, pin = self.port_ins[d[1]]
            return sig(site, pin)
        if d[0] == "unmodelled":
            return "1'bx"
        return "1'bx"

    def inp(self, site: Site, pin: str, what: str, matters: bool = True) -> str:
        d = self.pin_driver(site, pin)
        if d[0] not in ("site", "const", "primary") and matters:
            self.errors.append(f"{site.name}.{pin} ({what}) is undriven")
        return self.expr(d)

    # ---- netlist build -----------------------------------------------------

    def build(self) -> Netlist:
        nl = Netlist(self.sites)
        done: set[str] = set()
        while True:
            todo = sorted(self.used_sites - done)
            if not todo:
                break
            for name in todo:
                done.add(name)
                s = self.sites[name]
                if s.stype in SLICE_TYPES:
                    self._model_slice(s, nl)
                elif s.name in self._port_sites:
                    continue
                elif s.features:
                    if s.stype.startswith(("CLK", "RCLK", "HCLK", "CFG")) or "BUFG" in s.stype:
                        self.warnings.append(
                            f"{s.name} ({s.stype}): clock site not modelled: "
                            f"{sorted(s.features)[:6]}"
                        )
                        self.unmodelled_sites.append(s.name)
                    else:
                        self.errors.append(
                            f"{s.name} ({s.stype}) is configured but has no model: "
                            f"{sorted(s.features)[:6]}"
                        )
                        self.unmodelled_sites.append(s.name)
                else:
                    self.warnings.append(
                        f"{s.name} ({s.stype}) drives logic but has no model"
                    )
        # Non-slice tiles with features (BRAM, DSP, IO, clock, ...):
        # the CLB rebuild does not model them. Report each once, with
        # its feature count, so a design using them fails here with the
        # block named instead of silently dropping the logic.
        for tile, ttype in self._other_tiles:
            feats = self.tiles.get(tile, {})
            if not feats:
                continue
            if ttype.startswith(("CLK", "RCLK", "HCLK", "CFG")) or "BUFG" in ttype:
                self.warnings.append(
                    f"{tile} ({ttype}): clock tile not modelled, "
                    f"{len(feats)} features"
                )
                self.unmodelled_sites.append(tile)
            elif ttype in ("INT", "INT_INTF_L", "INT_INTF_R",
                           "INT_INTF_L_PCIE4", "INT_INTF_R_PCIE4"):
                continue  # routing only, checked by routecheck
            elif all(f.startswith("PIP.") for f in feats):
                continue  # routing only (e.g. a CLE route-through PIP)
            elif any(f.startswith("IOB_") for f in feats):
                self.warnings.append(
                    f"{tile} ({ttype}): IO tile not modelled, "
                    f"{len(feats)} features"
                )
                self.unmodelled_sites.append(tile)
            else:
                self.errors.append(
                    f"{tile} ({ttype}): configured but has no model, "
                    f"{len(feats)} features"
                )
                self.unmodelled_sites.append(tile)
        # Ports.
        for port, (site, pin) in sorted(self.port_ins.items()):
            nl.ports_in[port] = sig(site, pin)
        for port, (site, pin) in sorted(self.port_outs.items()):
            s = self.sites.get(site)
            if s is None:
                self.errors.append(f"output port {port}: unknown site {site}")
                continue
            nl.ports_out[port] = sig(site, pin)
        # Unclassified features: every feature on a modelled slice must
        # be consumed by its model; anything else is reported, never
        # silent. PIP features are routing (checked by routecheck).
        for s in self.sites.values():
            if s.stype in SLICE_TYPES:
                for f in sorted(s.features):
                    if f.startswith("PIP."):
                        continue
                    if f not in self._consumed.get(s.name, set()):
                        self.warnings.append(f"unclassified feature {s.name}.{f}")
        nl.errors = self.errors
        nl.warnings = self.warnings
        nl.unmodelled_sites = sorted(set(self.unmodelled_sites))
        return nl

    # ---- slice model -------------------------------------------------------

    def _depends(self, init: int, k: int, width: int = 6) -> bool:
        return lut_depends(init, k, width)

    def _model_slice(self, s: Site, nl: Netlist):
        F = s.features
        self._consumed.setdefault(s.name, set())
        cons = self._consumed[s.name]
        # Control attributes are understood whether or not the FFs that
        # use them are present (the decoder emits the defaults on every
        # touched tile), so they are consumed up front and never
        # reported as unclassified.
        for g in ("ABCDFF", "ABCDFF2", "EFGHFF", "EFGHFF2"):
            for f in F:
                if f.startswith(g + "."):
                    cons.add(f)
        for f in F:
            if f.startswith("CARRY8."):
                cons.add(f)
            if re.match(r"^[A-H]LUT\.MODE\.", f):
                cons.add(f)
            if re.match(r"^[A-H]FF2?\.(INIT|SRVAL)(\.V[01])?$", f):
                cons.add(f)
        n = lambda pin: sig(s.name, pin)
        L = nl.lines
        L.append(f"// {s.name} ({s.stype}) @ {s.tile}")
        # LUTRAM / SRL: blocked, fail loud.
        for c in LETTERS:
            if f"{c}LUT.MODE.RAM" in F or f"{c}LUT.MODE.SRL" in F:
                mode = "RAM" if f"{c}LUT.MODE.RAM" in F else "SRL"
                self.errors.append(
                    f"{s.name}: {c}LUT in {mode} mode is not modelled (blocked)"
                )
                cons.add(f"{c}LUT.MODE.{mode}")
        # LUTs.
        lut_o6: dict[str, str] = {}
        lut_o5: dict[str, str] = {}
        for c in LETTERS:
            init = init_value(F, c)
            if init is None:
                continue
            cons.add(f"{c}6LUT.INIT[63:0]")
            for i in range(64):
                cons.add(f"{c}LUT.INIT[{i}]")
            ins6, ins5 = [], []
            for k in range(6):
                dep6 = self._depends(init, k)
                dep5 = k < 5 and self._depends(init & 0xFFFFFFFF, k, 5)
                used = (init != 0) and (dep6 or dep5)
                v = self.inp(s, f"{c}{k+1}", f"{c}LUT input", matters=used) if used else "1'b1"
                ins6.append(v if dep6 else "1'b1")
                ins5.append(v if dep5 else "1'b0")
            idx6 = "{" + ",".join(reversed(ins6)) + "}"
            idx5 = "{1'b0," + ",".join(reversed(ins5[:5])) + "}"
            L.append(f"wire [63:0] {n(c + '_INIT')} = 64'h{init:016x};")
            L.append(f"wire {n(c)} = {n(c + '_INIT')}[{idx6}];")
            L.append(f"wire {n(c + '_O5')} = {n(c + '_INIT')}[{idx5}];")
            lut_o6[c] = n(c)
            lut_o5[c] = n(c + "_O5")
        # Bypass pins, resolved lazily.
        byp: dict[str, str] = {}

        def X(c: str) -> str:
            if c not in byp:
                byp[c] = self.inp(s, f"{c}X", f"{c}X bypass", matters=False)
            return byp[c]

        # Wide muxes (assumption A-F7F8; warn on any use).
        f7: dict[str, str] = {}
        f8: dict[str, str] = {}
        f9 = ""
        wide_used = any(
            f.startswith(f"FFMUX{c}") and f.endswith(".F7F8")
            or f.startswith(f"OUTMUX{c}") and f.endswith(".F7F8")
            for c in LETTERS
            for f in F
        )
        if wide_used:
            self.warnings.append(
                f"{s.name}: F7/F8/F9 mux used (mapping A-F7F8 not yet "
                "validated against Vivado)"
            )
            f7["A"] = f"({X('A')} ? {lut_o6.get('B', "1'bx")} : {lut_o6.get('A', "1'bx")})"
            f7["B"] = f"({X('C')} ? {lut_o6.get('D', "1'bx")} : {lut_o6.get('C', "1'bx")})"
            f7["C"] = f"({X('E')} ? {lut_o6.get('F', "1'bx")} : {lut_o6.get('E', "1'bx")})"
            f7["D"] = f"({X('G')} ? {lut_o6.get('H', "1'bx")} : {lut_o6.get('G', "1'bx")})"
            f8["A"] = f"({X('B')} ? {f7['B']} : {f7['A']})"
            f8["B"] = f"({X('F')} ? {f7['D']} : {f7['C']})"
            f9 = f"({X('H')} ? {f8['B']} : {f8['A']})"
            for k, v in list(f7.items()):
                L.append(f"wire {n('F7' + k)} = {v};")
            for k, v in list(f8.items()):
                L.append(f"wire {n('F8' + k)} = {v};")
            L.append(f"wire {n('F9')} = {f9};")

        def wide(c: str) -> str:
            return n(F7F8_OF[c])

        # Carry8.
        carry_used = self._carry_used(s)
        co: dict[int, str] = {}
        xo: dict[int, str] = {}
        if carry_used:
            ci = self._carry_in(s, X)
            cons.add("CARRY8.CARRY_TYPE.SINGLE_CY8")
            cons.add("CARRY8.CARRY_TYPE.DUAL_CY4")
            dual = "CARRY8.CARRY_TYPE.DUAL_CY4" in F
            ci_top = None
            if dual:
                ci_top = self._carry_top_in(s, X)
            for i, c in enumerate(LETTERS):
                di = self._carry_di(s, i, c, X)
                si = lut_o6.get(c)
                if si is None:
                    self.errors.append(
                        f"{s.name}: CARRY8 bit {i} needs {c}LUT O6 (no INIT)"
                    )
                    si = "1'bx"
                c_in = ci if i == 0 else (ci_top if (dual and i == 4) else co[i - 1])
                L.append(f"wire {n(c + '_XOR')} = {si} ^ {c_in};")
                L.append(f"wire {n(c + '_CY')} = {si} ? {c_in} : {di};")
                co[i] = n(c + "_CY")
                xo[i] = n(c + "_XOR")
            L.append(f"wire {n('COUT')} = {co[7]};")
        else:
            L.append(f"wire {n('COUT')} = 1'bx;")
        # Flip-flops.
        ff_groups = ["ABCD", "ABCD2", "EFGH", "EFGH2"]
        for g in ff_groups:
            seg = GROUP_SEG[g]
            half = GROUP_HALF[g]
            letters = HALF_LETTERS[half]
            used = []
            for c in letters:
                num = "1" if g.endswith("2") else ""
                # FFMUX key: FFMUXA1 / FFMUXA2.
                key = f"FFMUX{c}{2 if g.endswith('2') else 1}"
                sel = [f.split(".", 1)[1] for f in F if f.startswith(key + ".")]
                if sel:
                    used.append((c, sel[0]))
            if not used:
                continue
            clk = self.inp(s, HALF_CLK[half], "clock")
            nl.clock_sinks.append((s.name, clk))
            ce = "1'b1"
            if group_attr(F, seg, "CEUSED") == 1:
                ce = self.inp(s, GROUP_CE[g], "clock enable")
            sr = "1'b0"
            if group_attr(F, seg, "SRUSED") == 1:
                sr = self.inp(s, HALF_SR[half], "set/reset")
            edge = "negedge" if group_attr(F, seg, "CLKINV") == 1 else "posedge"
            sync = f"{seg}.SYNC.SYNC" in F
            if f"{seg}.MODE.LATCH" in F:
                self.errors.append(f"{s.name}: {seg} in LATCH mode is not modelled")
                continue
            if group_attr(F, seg, "SRINV") == 1:
                sr = f"(~{sr})"
            L.append(f"wire {n('CE_' + g)} = {ce}; wire {n('SR_' + g)} = {sr};")
            for c, sel in used:
                num = "2" if g.endswith("2") else ""
                ff = f"{c}FF{num}"
                init = ff_attr(F, ff, "INIT")
                srval = ff_attr(F, ff, "SRVAL")
                if init is None:
                    self.errors.append(f"{s.name}.{ff}: no INIT feature")
                    init = 0
                if srval is None:
                    self.errors.append(f"{s.name}.{ff}: no SRVAL feature")
                    srval = 0
                cons.add(f"{ff}.INIT")
                cons.add(f"{ff}.INIT.V0")
                cons.add(f"{ff}.INIT.V1")
                cons.add(f"{ff}.SRVAL")
                cons.add(f"{ff}.SRVAL.V0")
                cons.add(f"{ff}.SRVAL.V1")
                d = self._mux_sel(s, c, sel, lut_o6, lut_o5, co, xo, wide, X)
                q = n(f"{c}Q{num}")
                L.append(f"reg {q} = 1'b{init};")
                if sync:
                    L.append(
                        f"always @({edge} {clk}) if ({n('SR_' + g)}) {q} <= 1'b{srval}; "
                        f"else if ({n('CE_' + g)}) {q} <= {d};"
                    )
                else:
                    L.append(
                        f"always @({edge} {clk} or posedge {n('SR_' + g)}) "
                        f"if ({n('SR_' + g)}) {q} <= 1'b{srval}; "
                        f"else if ({n('CE_' + g)}) {q} <= {d};"
                    )
                nl.regs.append((q, f"{s.name}.{ff}"))
                for f in F:
                    if f.startswith(f"FFMUX{c}{2 if g.endswith('2') else 1}."):
                        cons.add(f)
            for f in F:
                if f.startswith(seg + "."):
                    cons.add(f)
        # Output muxes.
        for c in LETTERS:
            sel = [f.split(".", 1)[1] for f in F if f.startswith(f"OUTMUX{c}.")]
            if not sel:
                # Unconfigured output: X, error only when consumed.
                mux_wire = self._mux_wire(s, c)
                if mux_wire is not None and self._wire_consumed(mux_wire):
                    self.errors.append(
                        f"{s.name}: {c}OUTMUX has no selection but {c}MUX is consumed"
                    )
                L.append(f"wire {n(c + 'MUX')} = 1'bx;")
                continue
            cons.add(f"OUTMUX{c}.{sel[0]}")
            e = self._mux_sel(s, c, sel[0], lut_o6, lut_o5, co, xo, wide, X)
            L.append(f"wire {n(c + 'MUX')} = {e};")
        # Q pins for letters with no FF: X.
        for c in LETTERS:
            for num in ("", "2"):
                if not any(f.startswith(f"FFMUX{c}{2 if num else 1}.") for f in F):
                    L.append(f"wire {n(c + 'Q' + num)} = 1'bx;")

    def _mux_sel(self, s, c, sel, lut_o6, lut_o5, co, xo, wide, X):
        i = LETTERS.index(c)
        if sel == "D6":
            e = lut_o6.get(c)
            if e is None:
                self.errors.append(f"{s.name}: {c}OUTMUX/FFMUX D6 needs {c}LUT O6")
                e = "1'bx"
            return e
        if sel == "D5":
            e = lut_o5.get(c)
            if e is None:
                self.errors.append(f"{s.name}: {c}OUTMUX/FFMUX D5 needs {c}LUT O5")
                e = "1'bx"
            return e
        if sel == "CY":
            if i not in co:
                self.errors.append(f"{s.name}: {c} mux CY needs CARRY8")
                return "1'bx"
            return co[i]
        if sel == "XORIN":
            if i not in xo:
                self.errors.append(f"{s.name}: {c} mux XORIN needs CARRY8")
                return "1'bx"
            return xo[i]
        if sel == "F7F8":
            return wide(c)
        if sel == "BYP":
            return X(c)
        self.errors.append(f"{s.name}: {c} mux selection {sel} not modelled")
        return "1'bx"

    def _mux_wire(self, s: Site, c: str):
        ttype = self._ttype(s.tile)
        if ttype is None:
            return None
        tt = self.fab.tt(ttype)
        info = tt["sites"][s.index].get("site_pins", {}).get(c + "MUX") or {}
        wire = info.get("wire")
        if wire is None:
            return None
        return (self._fabric_tile(s), wire)

    def _wire_consumed(self, tile_wire) -> bool:
        tile, wire = tile_wire
        nid = self.fab.node(tile, wire)
        # Consumed when the node drives something: an active PIP
        # sourced by a wire in the node, or a site input pin on a
        # wire in the node (a direct stitch, e.g. COUT into the
        # slice-above CIN). PIPs driving INTO the node do not count:
        # they are the node's drivers, not its consumers.
        for t, w in self.fab.nodes[nid]:
            for f in self.tiles.get(t, {}):
                # A PIP counts as sourced by `w` in its normalized
                # direction (plain, .FWD, or swapped .REV spelling).
                parsed = parse_pip(f)
                if parsed is not None and parsed[1] == w:
                    return True
            ttype = self._ttype(t)
            if ttype is None:
                continue
            tt = self.fab.tt(ttype)
            if w in tt["site_pin_wire"]:
                si, pin = tt["site_pin_wire"][w]
                stype = tt["sites"][si].get("type", ttype)
                if self._pin_dir(stype, pin) == "IN":
                    return True
        return False

    # ---- carry helpers -----------------------------------------------------

    def _carry_used(self, s: Site) -> bool:
        F = s.features
        for c in LETTERS:
            for f in F:
                if f.startswith(f"FFMUX{c}") and f.endswith((".CY", ".XORIN")):
                    return True
                if f.startswith(f"OUTMUX{c}") and f.endswith((".CY", ".XORIN")):
                    return True
        if any(f.startswith("CARRY8.CI.") or f.startswith("CARRY8.CI_TOP.") for f in F):
            return True
        if any(
            re.match(rf"^CARRY8\.DI{i}\.DI{i}$", f)
            for i in range(8)
            for f in F
        ):
            return True
        # COUT consumed by routing (chain continues upward).
        mux_wire = self._mux_wire_cout(s)
        if mux_wire is not None and self._wire_consumed(mux_wire):
            return True
        return False

    def _mux_wire_cout(self, s: Site):
        ttype = self._ttype(s.tile)
        if ttype is None:
            return None
        tt = self.fab.tt(ttype)
        info = tt["sites"][s.index].get("site_pins", {}).get("COUT") or {}
        wire = info.get("wire")
        return (self._fabric_tile(s), wire) if wire else None

    def _carry_in(self, s: Site, X) -> str:
        F = s.features
        if "CARRY8.CI.AX" in F:
            return X("A")
        if "CARRY8.CI.V0" in F:
            return "1'b0"
        if "CARRY8.CI.V1" in F:
            return "1'b1"
        return self.inp(s, "CIN", "carry in")

    def _carry_top_in(self, s: Site, X) -> str:
        F = s.features
        if "CARRY8.CI_TOP.EX" in F:
            return X("E")
        if "CARRY8.CI_TOP.V0" in F:
            return "1'b0"
        if "CARRY8.CI_TOP.V1" in F:
            return "1'b1"
        self.errors.append(
            f"{s.name}: DUAL_CY4 CARRY8 has no CI_TOP selection"
        )
        return "1'bx"

    def _carry_di(self, s: Site, i: int, c: str, X) -> str:
        F = s.features
        if f"CARRY8.DI{i}.DI{i}" in F:
            self.errors.append(
                f"{s.name}: CARRY8.DI{i}.DI{i} source is not modelled"
            )
            return "1'bx"
        # DI{i}.X{i} is the default selection (bypass pin); an unrouted
        # bypass reads 0 (assumption A-DI0).
        d = self.pin_driver(s, f"{c}X")
        if d[0] not in ("site", "const", "primary"):
            return "1'b0"
        return self.expr(d)


def extract(db: Db, tiles, port_ins=None, port_outs=None) -> Netlist:
    ex = Extractor(db, tiles, port_ins, port_outs)
    return ex.build()


def to_verilog(nl: Netlist, module: str = "extracted") -> str:
    ports = []
    for p in sorted(nl.ports_in):
        ports.append(f"input wire {p}")
    for p in sorted(nl.ports_out):
        ports.append(f"output wire {p}")
    out = [
        "// Netlist rebuilt from decoded UltraScale+ FASM by tools/us09v",
        "`timescale 1ns/1ps",
        f"module {module}(" + ", ".join(ports) + ");",
    ]
    decls, body = [], []
    for p in sorted(nl.ports_in):
        decls.append(f"wire {nl.ports_in[p]};")
        body.append(f"assign {nl.ports_in[p]} = {p};")
    for raw in nl.lines:
        if raw.lstrip().startswith(("//", "always")):
            body.append(raw)
            continue
        for stmt in (x.strip() for x in raw.split(";") if x.strip()):
            m = re.match(r"^wire\s+(\[[^\]]+\]\s+)?(\w+)\s*=\s*(.*)$", stmt, re.S)
            if m:
                decls.append(f"wire {m.group(1) or ''}{m.group(2)};")
                body.append(f"assign {m.group(2)} = {m.group(3)};")
            elif stmt.startswith(("reg ", "wire ")):
                decls.append(stmt + ";")
            else:
                body.append(stmt + ";")
    out.extend(decls)
    out.extend(body)
    for p in sorted(nl.ports_out):
        out.append(f"assign {p} = {nl.ports_out[p]};")
    out.append("endmodule")
    return "\n".join(out) + "\n"
