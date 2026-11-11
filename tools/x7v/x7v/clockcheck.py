"""Clock-network configuration rules (enables that are not logic).

The global clock spine, horizontal clock rows and leaf buffers carry
buffer-enable features (`GCLKk_ENABLE_ABOVE/BELOW`, `*_ACTIVE`,
`ENABLE_BUFFER.*`, `*_USED`) that the logic netlist cannot see. A clock path
missing one of them is dead on silicon while every netlist check passes.

Rules (X-Ray feature semantics; every rule holds on all clocked Vivado
designs in the reference corpus, across both spine topologies seen):

R1 spine segments: the vertical GCLK spine is cut into segments by
   CLK_BUFG_REBUF tiles. For every segment a global clock k occupies (from
   its BUFG to each HROW that taps it), the REBUF at the segment's top end
   has GCLKk_ENABLE_ABOVE and the REBUF at its bottom end GCLKk_ENABLE_BELOW.
R2 spine crossings: every REBUF the clock crosses has the BOT<->TOP pip in
   the direction of travel.
R3 HROW taps: a CLK_HROW_CK_MUX_OUT_* <- CLK_HROW_*_CK_GCLKk pip needs
   CLK_HROW_R_CK_GCLKk_ACTIVE; a pip from CLK_HROW_CK_IN_<side><n> needs
   CLK_HROW_CK_IN_<side><n>_ACTIVE.
R4 leaf buffers: an HCLK_L/R pip from HCLK_CK_BUFHCLKn needs
   ENABLE_BUFFER.HCLK_CK_BUFHCLKn, and the HCLK_CMT(_L) tile of the same
   clock row on the same side needs HCLK_CMT_CK_BUFHCLKn_USED.
"""
from __future__ import annotations

import re
from dataclasses import dataclass, field

from .db import Db
from .decode import DesignDecode


@dataclass
class ClockReport:
    errors: list[str] = field(default_factory=list)
    notes: list[str] = field(default_factory=list)
    extra: list[str] = field(default_factory=list)  # enables set with no reason


def check(db: Db, dec: DesignDecode) -> ClockReport:
    rep = ClockReport()
    tg = db.tilegrid
    feats = {t: set(d.features) for t, d in dec.tiles.items()}

    def has(t, f):
        return f in feats.get(t, ())

    rebufs = sorted((tg[t]["grid_y"], t) for t in tg if tg[t]["type"] == "CLK_BUFG_REBUF")
    clock_x = tg[rebufs[0][1]]["grid_x"] if rebufs else None

    # Sources: BUFG tiles driving GCLK k.
    src_y: dict[int, list[int]] = {}
    for t, fs in feats.items():
        if tg[t]["type"].startswith("CLK_BUFG_") and "REBUF" not in tg[t]["type"]:
            for f in fs:
                m = re.match(r"^CLK_BUFG_CK_GCLK(\d+)\.CLK_BUFG_BUFGCTRL\d+_O$", f)
                if m:
                    src_y.setdefault(int(m.group(1)), []).append(tg[t]["grid_y"])
    # Sinks: HROW taps of GCLK k.
    tap_y: dict[int, list[tuple[int, str]]] = {}
    for t, fs in feats.items():
        ty = tg[t]["type"]
        if not ty.startswith("CLK_HROW"):
            continue
        for f in fs:
            m = re.match(r"^CLK_HROW_CK_MUX_OUT_[LR]\d+\.CLK_HROW_[LR]_CK_GCLK(\d+)$", f)
            if m:
                k = int(m.group(1))
                tap_y.setdefault(k, []).append((tg[t]["grid_y"], t))
                if not has(t, f"CLK_HROW_R_CK_GCLK{k}_ACTIVE"):
                    rep.errors.append(f"R3 {t}: taps GCLK{k} but lacks CLK_HROW_R_CK_GCLK{k}_ACTIVE")
            m = re.match(r"^.*\.CLK_HROW_CK_IN_([LR])(\d+)$", f)
            if m and not has(t, f"CLK_HROW_CK_IN_{m.group(1)}{m.group(2)}_ACTIVE"):
                rep.errors.append(f"R3 {t}: uses CLK_HROW_CK_IN_{m.group(1)}{m.group(2)} but lacks its _ACTIVE")

    # R1/R2 over the spine. Segments are the gaps between consecutive REBUF
    # rows (plus the open ends); a path from BUFG row sy to HROW row hy
    # occupies every segment that overlaps [min, max].
    ys = [y for y, _ in rebufs]
    name_at = {y: t for y, t in rebufs}
    bounds = [None] + ys + [None]
    segments = list(zip(bounds[:-1], bounds[1:]))  # (top_rebuf_y, bottom_rebuf_y)

    needed_enables = set()
    for k in sorted(set(src_y) | set(tap_y)):
        if k not in src_y:
            rep.errors.append(f"R1 GCLK{k}: tapped by {[t for _, t in tap_y[k]]} but no BUFG drives it")
            continue
        if k not in tap_y:
            rep.notes.append(f"GCLK{k}: driven by a BUFG but no HROW taps it")
            continue
        for sy in src_y[k]:
            for hy, ht in tap_y[k]:
                lo, hi = sorted((sy, hy))
                for top, bot in segments:
                    if (top is None or top < hi) and (bot is None or bot > lo) or (top is not None and bot is not None and top < lo < bot) or (top is not None and bot is not None and top < hi < bot):
                        if top is not None:
                            needed_enables.add((name_at[top], f"GCLK{k}_ENABLE_ABOVE"))
                        if bot is not None:
                            needed_enables.add((name_at[bot], f"GCLK{k}_ENABLE_BELOW"))
                down = hy > sy
                for r in ys:
                    if lo < r < hi:
                        want = (f"CLK_BUFG_REBUF_R_CK_GCLK{k}_BOT.CLK_BUFG_REBUF_R_CK_GCLK{k}_TOP" if down
                                else f"CLK_BUFG_REBUF_R_CK_GCLK{k}_TOP.CLK_BUFG_REBUF_R_CK_GCLK{k}_BOT")
                        if not has(name_at[r], want):
                            rep.errors.append(f"R2 {name_at[r]}: GCLK{k} crosses {'down' if down else 'up'} but lacks pip {want}")
    for t, f in sorted(needed_enables):
        if not has(t, f):
            rep.errors.append(f"R1 {t}: lacks {f}")
    for y, t in rebufs:
        for f in feats.get(t, ()):
            if "ENABLE" in f and (t, f) not in needed_enables:
                rep.extra.append(f"{t}.{f}")

    # R4 leaf buffers and CMT USED.
    cmt_tiles = [c for c in tg if tg[c]["type"] in ("HCLK_CMT", "HCLK_CMT_L")]
    for t, fs in feats.items():
        ty = tg[t]["type"]
        if ty not in ("HCLK_L", "HCLK_R"):
            continue
        for f in fs:
            m = re.match(r"^[A-Z0-9_]+\.HCLK_CK_BUFHCLK(\d+)$", f)
            if not m:
                continue
            n = int(m.group(1))
            if not has(t, f"ENABLE_BUFFER.HCLK_CK_BUFHCLK{n}"):
                rep.errors.append(f"R4 {t}: uses HCLK_CK_BUFHCLK{n} but lacks ENABLE_BUFFER.HCLK_CK_BUFHCLK{n}")
            row = tg[t]["grid_y"]
            right = clock_x is not None and tg[t]["grid_x"] > clock_x
            cmts = [c for c in cmt_tiles if tg[c]["grid_y"] == row
                    and (clock_x is None or (tg[c]["grid_x"] > clock_x) == right)]
            for c in cmts:
                if not has(c, f"HCLK_CMT_CK_BUFHCLK{n}_USED"):
                    rep.errors.append(f"R4 {c}: row uses BUFHCLK{n} (via {t}) but lacks HCLK_CMT_CK_BUFHCLK{n}_USED")
    return rep
