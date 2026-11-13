"""Decoded features -> flat gate-level netlist (bitstream LVS).

Electrical model, all from the Project X-Ray database:

* Nodes: tile wires joined by `tileconn.json` stitching rules.
* Edges: active pip features (`DST.SRC`), pseudo-pips marked `always`, and
  defaults, taken only when nothing else drives the node: pseudo-pips marked
  `default` (an unselected IMUX reads VCC) and mux selections encoded with
  every bit clear (a global buffer input on its fixed IMUX), which the
  decoder cannot see because they set no bit.
* Sites: per-site-type behavioural models driven by the site's features.
  `ZINV_*` features mean "inversion disabled"; their absence inverts.

Nothing is assumed silently: undriven inputs that matter, multiply driven
nodes, and configured features the models do not understand are collected
in `Netlist.errors` / `Netlist.warnings`.
"""
from __future__ import annotations

import re
from collections import defaultdict, deque
from dataclasses import dataclass, field
from functools import lru_cache

from .db import Db
from .decode import DesignDecode

LETTERS = "ABCD"


@dataclass
class TT:
    """Per tile-type cached routing data."""
    wires: set
    pips_into: dict          # dst -> [(src, bidirectional)]
    pip_pairs: set           # {(src, dst)} including both dirs of bidir pips
    ppips: dict              # "DST.SRC" -> kind
    zero_pips: set           # "DST.SRC" pips encoded as all bits clear
    site_pin_wire: dict      # wire -> (site_index, pin)
    sites: list              # tile_type sites entries


class Fabric:
    def __init__(self, db: Db):
        self.db = db
        self._tt: dict[str, TT] = {}
        self._conn = None
        self.node_of: dict[tuple[str, str], int] = {}
        self.nodes: list[tuple[tuple[str, str], ...]] = []
        self._site_dir: dict[str, dict] = {}

    def tt(self, ttype: str) -> TT:
        if ttype not in self._tt:
            j = self.db.tile_type(ttype)
            pips_into = defaultdict(list)
            pairs = set()
            for p in j.get("pips", {}).values():
                s, d = p["src_wire"], p["dst_wire"]
                bid = p.get("is_directional") == "0"
                pips_into[d].append((s, bid))
                pairs.add((s, d))
                if bid:
                    pips_into[s].append((d, bid))
                    pairs.add((d, s))
            ppips = {}
            for k, v in self.db.ppips(ttype).items():
                ppips[k.split(".", 1)[1]] = v
            # A mux selection encoded with every bit clear is in effect
            # whenever none of the mux's other selections is (it sets no
            # bit, so the decoder cannot report it). Global buffer inputs
            # are the routing muxes that have them (BUFGCTRLn_Ix <- IMUX).
            zero = set()
            for name, bits in self.db.segbits(ttype).items():
                feat = name.split(".", 1)[1] if "." in name else name
                if bits and all(not b.value for b in bits) and feat.count(".") == 1:
                    d, sw = feat.split(".")
                    if (sw, d) in pairs:
                        zero.add(feat)
            spw = {}
            for i, s in enumerate(j.get("sites", [])):
                for pin, info in s["site_pins"].items():
                    if info and info.get("wire"):
                        spw[info["wire"]] = (i, pin)
            self._tt[ttype] = TT(set(j.get("wires", {})), pips_into, pairs, ppips, zero, spw, j.get("sites", []))
        return self._tt[ttype]

    def site_pin_dir(self, site_type: str, pin: str) -> str:
        if site_type not in self._site_dir:
            import json, os
            p = f"{self.db.root}/site_type_{site_type}.json"
            self._site_dir[site_type] = json.load(open(p))["site_pins"] if os.path.exists(p) else {}
        return self._site_dir[site_type].get(pin, {}).get("direction", "?")

    # ---- node construction (lazy BFS over tileconn) --------------------
    def _conn_index(self):
        if self._conn is None:
            idx = defaultdict(list)
            for e in self.db.tileconn:
                a, b = e["tile_types"]
                dx, dy = e["grid_deltas"]
                for wa, wb in e["wire_pairs"]:
                    idx[(a, wa)].append((dx, dy, b, wb))
                    idx[(b, wb)].append((-dx, -dy, a, wa))
            self._conn = idx
        return self._conn

    def node(self, tile: str, wire: str) -> int:
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
            g = tg[t]
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


# ---- sites ---------------------------------------------------------------

@dataclass
class Site:
    name: str           # absolute, e.g. SLICE_X112Y39
    stype: str          # SLICEL, IOB33M, ...
    tile: str
    index: int          # index into tile_type sites
    fprefix: str        # FASM feature prefix in the tile, e.g. SLICEM_X0
    features: set = field(default_factory=set)


def _rel_names(db: Db, tile: str, fab: Fabric) -> dict[int, tuple[str, str]]:
    """tile_type site index -> (absolute site name, fasm prefix)."""
    t = db.tilegrid[tile]
    tt = fab.tt(t["type"])
    by_prefix = defaultdict(list)
    for absname, stype in t.get("sites", {}).items():
        m = re.match(r"^(.*)_X(\d+)Y(\d+)$", absname)
        if m:
            by_prefix[m.group(1)].append((int(m.group(2)), int(m.group(3)), absname, stype))
    out = {}
    for i, s in enumerate(tt.sites):
        cands = by_prefix.get(s["prefix"], [])
        if not cands:
            continue
        xs = sorted({c[0] for c in cands})
        ys = sorted({c[1] for c in cands})
        m = re.match(r"X(\d+)Y(\d+)", s["name"])
        rx, ry = int(m.group(1)), int(m.group(2))
        hit = [c for c in cands if xs.index(c[0]) == rx and ys.index(c[1]) == ry]
        if not hit:
            continue
        absname = hit[0][2]
        st = s["type"]
        # X-Ray numbers I/O sites from the TOP of the tile: IOB_Y0 is the
        # upper site (higher Y), IOB_Y1 the lower one.
        fy = len(ys) - 1 - ry
        if st.startswith("SLICE"):
            pre = f"{st}_X{rx}"
        elif st.startswith("IOB"):
            pre = f"IOB_Y{fy}"
        elif st.startswith(("ILOGIC", "OLOGIC", "IDELAY", "ODELAY")):
            pre = f"{s['prefix']}_Y{fy}"
        else:
            # Clock buffers (BUFGCTRL, BUFHCE, ...) are named by their
            # position inside the tile, not by absolute site name.
            pre = f"{s['prefix']}.{s['prefix']}_X{rx}Y{ry}"
        out[i] = (absname, pre)
    return out


@dataclass
class Netlist:
    sites: dict[str, Site]
    # signal name -> verilog expression for site outputs, filled by models
    lines: list[str] = field(default_factory=list)
    ports_in: dict[str, str] = field(default_factory=dict)   # package pin -> signal
    ports_out: dict[str, str] = field(default_factory=dict)  # package pin -> expr
    regs: list[tuple[str, str]] = field(default_factory=list)  # (signal, description)
    errors: list[str] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)
    clock_sinks: list[tuple[str, str]] = field(default_factory=list)


def sig(site: str, pin: str) -> str:
    return re.sub(r"[^A-Za-z0-9_]", "_", f"s_{site}__{pin}")


class Extractor:
    def __init__(self, db: Db, dec: DesignDecode):
        self.db = db
        self.dec = dec
        self.fab = Fabric(db)
        self.active: dict[str, set[str]] = {t: set(td.features) for t, td in dec.tiles.items()}
        self.sites: dict[str, Site] = {}
        self.site_by_tile_index: dict[tuple[str, int], Site] = {}
        self.errors: list[str] = []
        self.warnings: list[str] = []
        self._driver_memo: dict[int, tuple] = {}
        self.used_sites: set[str] = set()
        self._index_sites()

    # ---- site discovery ---------------------------------------------------
    def _index_sites(self):
        for tile, feats in self.active.items():
            rel = _rel_names(self.db, tile, self.fab)
            ttype = self.db.tilegrid[tile]["type"]
            tt = self.fab.tt(ttype)
            for i, (absname, pre) in rel.items():
                s = Site(absname, tt.sites[i]["type"], tile, i, pre)
                for f in feats:
                    if f.startswith(pre + "."):
                        s.features.add(f[len(pre) + 1:])
                self.sites[absname] = s
                self.site_by_tile_index[(tile, i)] = s
                if s.features:
                    self.used_sites.add(absname)

    def site_at(self, tile: str, index: int) -> Site:
        key = (tile, index)
        if key not in self.site_by_tile_index:
            rel = _rel_names(self.db, tile, self.fab)
            ttype = self.db.tilegrid[tile]["type"]
            tt = self.fab.tt(ttype)
            absname, pre = rel.get(index, (f"{tile}_site{index}", f"site{index}"))
            s = Site(absname, tt.sites[index]["type"], tile, index, pre)
            for f in self.active.get(tile, ()):
                if f.startswith(pre + "."):
                    s.features.add(f[len(pre) + 1:])
            self.sites[absname] = s
            self.site_by_tile_index[key] = s
        return self.site_by_tile_index[key]

    # ---- driver resolution ------------------------------------------------
    def driver(self, nid: int, _stack=None) -> tuple:
        """Resolve a node to ('site', site, pin) | ('const', v) | ('undriven',)."""
        if nid in self._driver_memo:
            return self._driver_memo[nid]
        stack = _stack if _stack is not None else set()
        if nid in stack:
            return ("loop",)
        stack.add(nid)
        sources = []
        defaults = []
        for tile, wire in self.fab.nodes[nid]:
            ttype = self.db.tilegrid[tile]["type"]
            tt = self.fab.tt(ttype)
            act = self.active.get(tile, ())
            site_out = False
            if wire in tt.site_pin_wire:
                si, pin = tt.site_pin_wire[wire]
                stype = tt.sites[si]["type"]
                if self.fab.site_pin_dir(stype, pin) == "OUT":
                    site_out = True
                    s = self.site_at(tile, si)
                    if stype == "TIEOFF":
                        sources.append(("const", 0 if pin == "HARD0" else 1, f"{tile}/{pin}"))
                    else:
                        sources.append(("site", s.name, pin))
            if site_out:
                # A site output pin is driven by its site model only; the
                # DB's pseudo-pips into it are site route-throughs (OLOGIC
                # D1->OQ, BUFHCE I->O, ...) that the model already covers.
                continue
            for src, _bid in tt.pips_into.get(wire, ()):
                feat = f"{wire}.{src}"
                kind = tt.ppips.get(feat)
                if feat in act:
                    sources.append(("pip", tile, src))
                elif kind == "always":
                    sources.append(("always", tile, src))
                elif kind == "default" or feat in tt.zero_pips:
                    defaults.append(("default", tile, src))
        if not sources:
            sources = defaults
        result = ("undriven",)
        resolved = []
        for s in sources:
            if s[0] in ("site", "const"):
                resolved.append(s)
                continue
            r = self.driver(self.fab.node(s[1], s[2]), stack)
            if r[0] not in ("undriven", "loop"):
                resolved.append(r)
        uniq = {r[:3] if r[0] == "site" else ("const", r[1]) for r in resolved}
        if len(uniq) > 1:
            self.errors.append(f"multiply driven node {self.fab.nodes[nid][:2]}: {sorted(uniq)}")
        if resolved:
            r = resolved[0]
            result = r[:3] if r[0] == "site" else ("const", r[1])
        stack.discard(nid)
        if _stack is None or result[0] != "undriven":
            self._driver_memo[nid] = result
        return result

    def pin_driver(self, site: Site, pin: str) -> tuple:
        tt = self.fab.tt(self.db.tilegrid[site.tile]["type"])
        wire = (tt.sites[site.index]["site_pins"].get(pin) or {}).get("wire")
        if wire is None:
            return ("nopin",)
        d = self.driver(self.fab.node(site.tile, wire))
        if d[0] == "site":
            self.used_sites.add(d[1])
        return d

    def expr(self, d: tuple) -> str:
        if d[0] == "site":
            return sig(d[1], d[2])
        if d[0] == "const":
            return f"1'b{d[1]}"
        return "1'bx"

    def inp(self, site: Site, pin: str, what: str, matters: bool = True) -> str:
        d = self.pin_driver(site, pin)
        if d[0] not in ("site", "const") and matters:
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
                model = MODELS.get(_family(s.stype))
                if model is None:
                    if s.features:
                        self.errors.append(f"{s.name} ({s.stype}) is configured but has no model: {sorted(s.features)[:6]}")
                    else:
                        self.warnings.append(f"{s.name} ({s.stype}) drives logic but has no model")
                    continue
                model(self, s, nl)
        known = set()
        for s in self.sites.values():
            if s.features:
                known.update(f"{s.fprefix}.{f}" for f in s.features)
        for tile, feats in self.active.items():
            tt = self.fab.tt(self.db.tilegrid[tile]["type"])
            for f in feats:
                parts = f.split(".")
                if len(parts) == 2 and (parts[1], parts[0]) in tt.pip_pairs:
                    continue
                if f in known:
                    continue
                if not _is_clock_tile_feature(self.db.tilegrid[tile]["type"], f):
                    self.warnings.append(f"unclassified feature {tile}.{f}")
        nl.errors = self.errors
        nl.warnings = self.warnings
        return nl


def _family(stype: str) -> str:
    if stype in ("SLICEL", "SLICEM"):
        return "SLICE"
    if stype.startswith("IOB33"):
        return "IOB33"
    return stype


def _is_clock_tile_feature(ttype: str, f: str) -> bool:
    # Clock-network configuration (spine enables, buffer USED/ACTIVE bits) is
    # checked by clockcheck.py, not modelled as logic here.
    return ttype.startswith(("CLK_", "HCLK", "CFG_CENTER")) or f.endswith("_ACTIVE") or "ENABLE" in f


# ---- site models -----------------------------------------------------------

def _lut_init(s: Site, L: str) -> int:
    v = 0
    for f in s.features:
        m = re.match(rf"^{L}LUT\.INIT\[(\d+)\]$", f)
        if m:
            v |= 1 << int(m.group(1))
    return v


def _depends(init: int, k: int, width: int = 6) -> bool:
    for i in range(1 << width):
        if not (i >> k) & 1 and ((init >> i) & 1) != ((init >> (i | 1 << k)) & 1):
            return True
    return False


def model_slice(x: Extractor, s: Site, nl: Netlist):
    F = s.features
    n = lambda pin: sig(s.name, pin)
    L = nl.lines
    unsupported = [f for f in F if re.match(r"^[A-D]LUT\.(RAM|SRL|SMALL)$", f) or f in ("WA7USED", "WA8USED", "WEMUX.CE", "LATCH")]
    if unsupported:
        x.errors.append(f"{s.name}: LUTRAM/SRL/latch mode not modelled: {sorted(unsupported)}")
    L.append(f"// {s.name} ({s.stype}) @ {s.tile}")
    for i, c in enumerate(LETTERS):
        init = _lut_init(s, c)
        ins = []
        for k in range(6):
            dep6 = _depends(init, k)
            dep5 = k < 5 and _depends(init & 0xFFFFFFFF, k, 5)
            used = (init != 0) and (dep6 or dep5)
            ins.append(x.inp(s, f"{c}{k+1}", f"{c}LUT input", matters=used) if used else "1'b1")
        idx = "{" + ",".join(reversed(ins)) + "}"
        L.append(f"wire [63:0] {n(c+'_INIT')} = 64'h{init:016x};")
        L.append(f"wire {n(c)} = {n(c+'_INIT')}[{idx}];")
        L.append(f"wire {n(c+'_O5')} = {n(c+'_INIT')}[{{1'b0,{','.join(reversed(ins[:5]))}}}];")
    # bypass pins, only resolved when something uses them
    byp = {}

    def X(c):
        if c not in byp:
            byp[c] = x.inp(s, f"{c}X", f"{c}X bypass")
        return byp[c]

    # carry
    carry_used = any(f.startswith(("PRECYINIT", "CARRY4")) or f.endswith((".XOR", ".CY")) for f in F) \
        or any(f"{c}FFMUX.XOR" in F or f"{c}FFMUX.CY" in F or f"{c}OUTMUX.XOR" in F or f"{c}OUTMUX.CY" in F for c in LETTERS)
    if carry_used:
        if "PRECYINIT.AX" in F:
            ci = X("A")
        elif "PRECYINIT.C1" in F:
            ci = "1'b1"
        elif "PRECYINIT.CIN" in F:
            ci = x.inp(s, "CIN", "carry in")
        else:
            ci = "1'b0"  # PRECYINIT.C0 is the all-clear encoding
        for i, c in enumerate(LETTERS):
            # CY0 mux: the feature set selects the LUT's O5; the all-clear
            # default selects the bypass pin (AX..DX). Proven on the
            # hardware-blinking Vivado counter, whose head reads DI0 = AX
            # (an unrouted BYP reads VCC) to carry out of bit 0.
            di = n(c + "_O5") if f"CARRY4.{c}CY0" in F else X(c)
            L.append(f"wire {n(c+'_CI')} = {ci};")
            L.append(f"wire {n(c+'_XOR')} = {n(c)} ^ {n(c+'_CI')};")
            L.append(f"wire {n(c+'_CY')} = {n(c)} ? {n(c+'_CI')} : {di};")
            ci = n(c + "_CY")
        L.append(f"wire {n('COUT')} = {n('D_CY')};")
    # wide muxes (MUXF7: O = S ? I1 : I0; F7AMUX I1=A, I0=B; F7BMUX I1=C, I0=D; F8 I1=F7A, I0=F7B)
    if any(".F7" in f or ".F8" in f for f in F):
        L.append(f"wire {n('F7A')} = {X('A')} ? {n('A')} : {n('B')};")
        L.append(f"wire {n('F7B')} = {X('C')} ? {n('C')} : {n('D')};")
        L.append(f"wire {n('F8')} = {X('B')} ? {n('F7A')} : {n('F7B')};")
        x.warnings.append(f"{s.name}: F7/F8 mux used (polarity not yet validated against Vivado)")
    # flip-flops
    ffs = [c for c in LETTERS if any(f.startswith(f"{c}FF") for f in F)]
    ff5 = [c for c in LETTERS if any(f.startswith(f"{c}5FF") for f in F)]
    if ffs or ff5:
        clk = x.inp(s, "CLK", "clock")
        nl.clock_sinks.append((s.name, clk))
        ce = x.inp(s, "CE", "clock enable") if "CEUSEDMUX" in F else "1'b1"
        sr = x.inp(s, "SR", "set/reset") if "SRUSEDMUX" in F else "1'b0"
        edge = "negedge" if "CLKINV" in F else "posedge"
        sync = "FFSYNC" in F
        L.append(f"wire {n('CE_')} = {ce}; wire {n('SR_')} = {sr};")
        for c in ffs:
            sel = [f.split(".", 1)[1] for f in F if f.startswith(f"{c}FFMUX.")]
            src = sel[0] if sel else None
            d = {"O6": n(c), "O5": n(c + "_O5"), "XOR": n(c + "_XOR"), "CY": n(c + "_CY"),
                 "F7": n("F7A") if c == "A" else n("F7B"), "F8": n("F8")}.get(src)
            if src in ("AX", "BX", "CX", "DX"):
                d = X(c)
            if d is None:
                x.errors.append(f"{s.name}.{c}FF: no data mux selection ({sel})")
                d = "1'bx"
            _ff(nl, n(c + "Q"), d, clk, edge, n("CE_"), n("SR_"), sync,
                init=0 if f"{c}FF.ZINI" in F else 1, srval=0 if f"{c}FF.ZRST" in F else 1, name=f"{s.name}.{c}FF")
        for c in ff5:
            sel = [f.split(".", 1)[1] for f in F if f.startswith(f"{c}5FFMUX.")]
            d = {"IN_A": n(c + "_O5"), "IN_B": X(c)}.get(sel[0]) if sel else None
            if d is None:
                x.errors.append(f"{s.name}.{c}5FF: no data mux selection ({sel})")
                d = "1'bx"
            _ff(nl, n(c + "5Q"), d, clk, edge, n("CE_"), n("SR_"), sync,
                init=0 if f"{c}5FF.ZINI" in F else 1, srval=0 if f"{c}5FF.ZRST" in F else 1, name=f"{s.name}.{c}5FF")
    for c in ffs:
        L.append(f"wire {n(c+'Q_')} = {n(c+'Q')};")
    # output muxes
    for c in LETTERS:
        sel = [f.split(".", 1)[1] for f in F if f.startswith(f"{c}OUTMUX.")]
        if not sel:
            L.append(f"wire {n(c+'MUX')} = 1'bx;")
            continue
        m = sel[0]
        e = {"O6": n(c), "O5": n(c + "_O5"), "XOR": n(c + "_XOR"), "CY": n(c + "_CY"),
             "F7": n("F7A") if c == "A" else n("F7B"), "F8": n("F8"), f"{c}5Q": n(c + "5Q")}.get(m)
        if e is None:
            x.errors.append(f"{s.name}: {c}OUTMUX.{m} not modelled")
            e = "1'bx"
        L.append(f"wire {n(c+'MUX')} = {e};")
    for c in LETTERS:
        if c not in ffs:
            L.append(f"wire {n(c+'Q')} = 1'bx;")


def _ff(nl: Netlist, q: str, d: str, clk: str, edge: str, ce: str, sr: str, sync: bool,
        init: int, srval: int, name: str):
    L = nl.lines
    L.append(f"reg {q} = 1'b{init};")
    if sync:
        L.append(f"always @({edge} {clk}) if ({sr}) {q} <= 1'b{srval}; else if ({ce}) {q} <= {d};")
    else:
        L.append(f"always @({edge} {clk} or posedge {sr}) if ({sr}) {q} <= 1'b{srval}; else if ({ce}) {q} <= {d};")
    nl.regs.append((q, name))


def model_iob(x: Extractor, s: Site, nl: Netlist):
    F = s.features
    n = lambda pin: sig(s.name, pin)
    pin = _package_pin(x.db, s.name)
    is_in = any(f.endswith((".IN", ".IN_ONLY")) for f in F)
    is_out = any(".DRIVE." in f for f in F) and not any(f.endswith(".IN_ONLY") for f in F)
    pull = "UP" if "PULLTYPE.PULLUP" in F else "DOWN" if "PULLTYPE.PULLDOWN" in F else "KEEPER" if "PULLTYPE.KEEPER" in F else "NONE"
    nl.lines.append(f"// {s.name} ({s.stype}) pad {pin} in={is_in} out={is_out} pull={pull}")
    if is_in:
        nl.ports_in[pin] = n("PAD")
        nl.lines.append(f"wire {n('I')} = {n('PAD')};")
    else:
        nl.lines.append(f"wire {n('I')} = 1'bx;")
    if is_out:
        o = x.inp(s, "O", "output data")
        t = x.inp(s, "T", "output tristate", matters=False)
        nl.ports_out[pin] = f"(({t}) === 1'b0 ? ({o}) : 1'bz)"
        if t != "1'b0" and not t.startswith("s_"):
            x.warnings.append(f"{s.name} ({pin}): output tristate resolves to {t}")
    if not is_in and not is_out and F:
        x.warnings.append(f"{s.name} ({pin}): configured but neither input nor output: {sorted(F)}")


def model_ilogic(x: Extractor, s: Site, nl: Netlist):
    F = s.features
    n = lambda pin: sig(s.name, pin)
    if any(f.startswith(("IDDR", "ISERDES", "IFF.")) and "ZINV" not in f for f in F):
        x.errors.append(f"{s.name}: IDDR/ISERDES/IFF not modelled: {sorted(F)}")
    d = x.inp(s, "D", "input data")
    inv = "" if "ZINV_D" in F else "~"
    nl.lines.append(f"wire {n('O')} = {inv}{d};")


def model_ologic(x: Extractor, s: Site, nl: Netlist):
    F = s.features
    n = lambda pin: sig(s.name, pin)
    bad = [f for f in F if f.startswith(("OSERDES.IN_USE", "ODDR", "OSERDES.DATA_RATE_OQ", "TDDR"))]
    if bad:
        x.errors.append(f"{s.name}: ODDR/OSERDES not modelled: {sorted(bad)}")
    if "OMUX.D1" in F or "OQUSED" in F:
        d1 = x.inp(s, "D1", "output data")
        inv = "~" if "IS_D1_INVERTED" in F else ""
        nl.lines.append(f"wire {n('OQ')} = {inv}{d1};")
    else:
        nl.lines.append(f"wire {n('OQ')} = 1'bx;")
    if "OSERDES.DATA_RATE_TQ.BUF" in F:
        t1 = x.inp(s, "T1", "tristate")
        inv = "" if "ZINV_T1" in F else "~"
        nl.lines.append(f"wire {n('TQ')} = {inv}{t1};")
    else:
        nl.lines.append(f"wire {n('TQ')} = 1'b1;")


def model_bufg(x: Extractor, s: Site, nl: Netlist):
    F = s.features
    n = lambda pin: sig(s.name, pin)
    if "IN_USE" not in F:
        x.errors.append(f"{s.name}: BUFGCTRL drives logic but is not IN_USE")
    def pinv(p, zinv=None, isinv=None):
        v = x.inp(s, p, p, matters=False)
        inv = (zinv is not None and zinv not in F) or (isinv is not None and isinv in F)
        return f"(~{v})" if inv else v
    s0 = pinv("S0", zinv="ZINV_S0"); ce0 = pinv("CE0", zinv="ZINV_CE0")
    s1 = pinv("S1", zinv="ZINV_S1"); ce1 = pinv("CE1", zinv="ZINV_CE1")
    # An input this buffer selects must be driven: an undriven clock leaves
    # every flop on the buffer stuck.
    uses0 = "IN_USE" in F and ("ZINV_S0" in F or "ZINV_CE0" in F)
    uses1 = "IN_USE" in F and ("ZINV_S1" in F or "ZINV_CE1" in F)
    i0 = x.inp(s, "I0", "clock in 0", matters=uses0)
    i1 = x.inp(s, "I1", "clock in 1", matters=uses1)
    nl.lines.append(f"wire {n('O')} = (({s0}) & ({ce0})) === 1'b1 ? {i0} : ((({s1}) & ({ce1})) === 1'b1 ? {i1} : 1'b0);")


def model_bufhce(x: Extractor, s: Site, nl: Netlist):
    F = s.features
    n = lambda pin: sig(s.name, pin)
    if "IN_USE" not in F:
        x.errors.append(f"{s.name}: BUFHCE drives logic but is not IN_USE")
    i = x.inp(s, "I", "clock in")
    ce = x.inp(s, "CE", "CE", matters=False)
    if "ZINV_CE" not in F:
        ce = f"(~{ce})"
    nl.lines.append(f"wire {n('O')} = ({ce}) === 1'b0 ? 1'b0 : {i};")


MODELS = {
    "SLICE": model_slice,
    "IOB33": model_iob,
    "ILOGICE3": model_ilogic,
    "OLOGICE3": model_ologic,
    "BUFGCTRL": model_bufg,
    "BUFHCE": model_bufhce,
}


@lru_cache(maxsize=None)
def _pin_map(db: Db) -> dict[str, str]:
    return {r["site"]: p for p, r in db.package_pins.items()}


def _package_pin(db: Db, site: str) -> str:
    return _pin_map(db).get(site, site)


def extract(db: Db, dec: DesignDecode) -> Netlist:
    ex = Extractor(db, dec)
    return ex.build()


def to_verilog(nl: Netlist, module: str = "extracted") -> str:
    ports = []
    for p in sorted(nl.ports_in):
        ports.append(f"input wire {p}")
    for p in sorted(nl.ports_out):
        ports.append(f"output wire {p}")
    out = [f"// Netlist extracted from a bitstream by tools/x7v (bitstream LVS)",
           "`timescale 1ns/1ps", f"module {module}(" + ", ".join(ports) + ");"]
    decls, body = [], []
    for p in sorted(nl.ports_in):
        decls.append(f"wire {nl.ports_in[p]};")
        body.append(f"assign {nl.ports_in[p]} = {p};")
    # Hoist declarations: models reference each other's outputs in any order.
    for raw in nl.lines:
        if raw.lstrip().startswith(("//", "always")):
            body.append(raw)
            continue
        for stmt in (s.strip() for s in raw.split(";") if s.strip()):
            m = re.match(r"^wire\s+(\[[^\]]+\]\s+)?(\w+)\s*=\s*(.*)$", stmt, re.S)
            if m:
                decls.append(f"wire {m.group(1) or ''}{m.group(2)};")
                body.append(f"assign {m.group(2)} = {m.group(3)};")
            elif stmt.startswith("reg "):
                decls.append(stmt + ";")
            else:
                body.append(stmt + ";")
    out.extend(decls)
    out.extend(body)
    for p in sorted(nl.ports_out):
        out.append(f"assign {p} = {nl.ports_out[p]};")
    out.append("endmodule")
    return "\n".join(out) + "\n"
