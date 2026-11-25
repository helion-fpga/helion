#!/usr/bin/env python3
"""consistency.py - US-01 validation (W-VALIDATE).

For one decoded FASM pair, check every PIP line against the static
public DBs (tile_types + tileconn), and check every BEL-pin connection
implied by the decoded FASM against the tileconn stitch for that pin.

Checks per PIP line (tile, dst, src):
  C1  dst wire exists in the tile type's wire list.
  C2  src wire exists in the tile type's wire list.
  C3  the (dst, src) edge exists in the tile type's PIP list.
      Bidirectional PIPs are written with a .FWD/.REV suffix on the
      source wire in FASM (prjuray's own vocabulary for the <<->>
      edges in tile_type_INT, is_directional 0); the suffix is
      stripped before the check and counted separately as
      bidir_suffix, never as a contradiction.
  C4  for INT-tile PIPs whose dst or src is a BEL-facing wire
      (IMUX_*, LOGIC_OUTS_*, CTRL_*), tileconn must stitch that wire
      to a CLE site-pin wire (CLE_CLE_[LM]_SITE_0_<pin>); otherwise
      the decoded connection names a pin/wire pair tileconn says
      cannot exist.
  C5  (informational only) whether a PIP wire appears in tileconn's
      (tile type, wire) universe. tileconn lists only inter-tile
      stitch wires; most INT-internal wires live in tile_types only,
      so C5 is reported as a scope observation, never a contradiction.
  Site PIPs (tile field is a SLICE_X..Y.. site name, not a tile) are
  resolved to their tile through tilegrid before the C1-C3 checks.

A contradiction is a decoded PIP failing C3 or C4, i.e. a PIP edge or
a pin connection the static DBs say cannot exist. Distinct failing
(dst, src) edges are listed individually with occurrence counts and
one example tile. Zero contradictions is a result and is stated so.

Usage: python3 consistency.py <decoded-fasm> <out-json>
"""
import json
import re
import sys
from collections import Counter, defaultdict

DB = "/home/hatch/workspace/prjuray-db-ref/zynqusp"
PART = "xczu3eg-sbva484-1-e"

PIP_RE = re.compile(r"^(.+?)\.PIP\.([^.]+)\.(.+)$")
BEL_WIRE = re.compile(r"^(IMUX_[EW]\d+|LOGIC_OUTS_[EW]\d+|CTRL_[EW]\d+)$")
SITE_WIRE = re.compile(r"^CLE_CLE_[LM]_SITE_0_")

_tt_cache = {}


def tile_type_data(ttype):
    if ttype in _tt_cache:
        return _tt_cache[ttype]
    try:
        d = json.load(open(f"{DB}/tile_types/tile_type_{ttype}.json"))
    except FileNotFoundError:
        _tt_cache[ttype] = None
        return None
    wires = set(d.get("wires") or [])
    pips = set()
    for info in (d.get("pips") or {}).values():
        pips.add((info.get("dst_wire"), info.get("src_wire")))
    out = (wires, pips)
    _tt_cache[ttype] = out
    return out


def main():
    fasm, out_path = sys.argv[1:3]
    tg = json.load(open(f"{DB}/{PART}/tilegrid.json"))
    tile_type = {n: t["type"] for n, t in tg.items()}
    site_to_tile = {}
    for tname, t in tg.items():
        for sname in (t.get("sites") or {}):
            site_to_tile[sname] = tname
    tc = json.load(open(f"{DB}/{PART}/tileconn.json"))
    conn = defaultdict(list)
    tc_wires = set()
    for e in tc:
        a, b = e["tile_types"]
        dx, dy = e["grid_deltas"]
        for wa, wb in e["wire_pairs"]:
            conn[(a, wa)].append((b, wb, dx, dy))
            conn[(b, wb)].append((a, wa, -dx, -dy))
            tc_wires.add((a, wa))
            tc_wires.add((b, wb))

    counts = Counter()
    bad_edge = Counter()
    bad_edge_example = {}
    bad_bel = Counter()
    bad_bel_example = {}
    bad_wire = Counter()
    no_ttype = Counter()
    with open(fasm) as f:
        for line in f:
            line = line.strip()
            if ".PIP." not in line:
                continue
            m = PIP_RE.match(line)
            if not m:
                counts["unparsed_pip_line"] += 1
                continue
            tile, dst, src = m.group(1), m.group(2), m.group(3)
            counts["pips_checked"] += 1
            if tile.startswith("SLICE_"):
                tile = site_to_tile.get(tile, tile)
            # Bidirectional PIPs carry a .FWD/.REV suffix on the
            # source wire in FASM; strip it for the static-DB checks.
            for suf in (".FWD", ".REV"):
                if src.endswith(suf):
                    src = src[: -len(suf)]
                    counts["bidir_suffix"] += 1
                    break
            for suf in (".FWD", ".REV"):
                if dst.endswith(suf):
                    dst = dst[: -len(suf)]
                    counts["bidir_suffix"] += 1
                    break
            ttype = tile_type.get(tile)
            if ttype is None:
                counts["unknown_tile"] += 1
                bad_wire[(tile, dst, src)] += 1
                continue
            data = tile_type_data(ttype)
            if data is None:
                counts["no_tiletype_file"] += 1
                no_ttype[ttype] += 1
                continue
            wires, pips = data
            ok = True
            if dst not in wires:
                counts["c1_dst_wire_missing"] += 1
                bad_wire[(ttype, dst, "dst")] += 1
                ok = False
            if src not in wires:
                counts["c2_src_wire_missing"] += 1
                bad_wire[(ttype, src, "src")] += 1
                ok = False
            if (dst, src) not in pips:
                counts["c3_edge_missing"] += 1
                bad_edge[(ttype, dst, src)] += 1
                if (ttype, dst, src) not in bad_edge_example:
                    bad_edge_example[(ttype, dst, src)] = tile
                ok = False
            if ttype.startswith("INT"):
                for w, role in ((dst, "dst"), (src, "src")):
                    if BEL_WIRE.match(w):
                        partners = conn.get((ttype, w), [])
                        if not any(
                            p[0].startswith("CLE")
                            and SITE_WIRE.match(p[1])
                            for p in partners
                        ):
                            counts["c4_bel_no_site_partner"] += 1
                            key = (ttype, w, role)
                            bad_bel[key] += 1
                            if key not in bad_bel_example:
                                bad_bel_example[key] = (
                                    tile,
                                    sorted(
                                        (p[0], p[1]) for p in partners
                                    )[:4],
                                )
                        else:
                            counts["c4_bel_agree"] += 1
            for w in (dst, src):
                if (ttype, w) not in tc_wires:
                    counts["info_wire_not_in_tileconn"] += 1
                    break
            if ok:
                counts["pips_fully_agree"] += 1
    contradictions = (
        counts["c3_edge_missing"] + counts["c4_bel_no_site_partner"]
    )
    result = {
        "fasm": fasm,
        "counts": dict(counts),
        "contradictions": contradictions,
        "bad_edges": [
            {
                "tile_type": k[0],
                "dst": k[1],
                "src": k[2],
                "occurrences": v,
                "example_tile": bad_edge_example[k],
            }
            for k, v in sorted(bad_edge.items())
        ],
        "bad_bel_wires": [
            {
                "tile_type": k[0],
                "wire": k[1],
                "role": k[2],
                "occurrences": v,
                "example_tile": bad_bel_example[k][0],
                "tileconn_partners": bad_bel_example[k][1],
            }
            for k, v in sorted(bad_bel.items())
        ],
        "missing_dst_src_wires": [
            {"tile_type": k[0], "wire": k[1], "role": k[2], "occurrences": v}
            for k, v in sorted(bad_wire.items())
        ][:50],
        "n_missing_dst_src_wire_kinds": len(bad_wire),
        "tile_types_without_file": dict(no_ttype),
    }
    with open(out_path, "w") as f:
        json.dump(result, f, indent=1)
    print(json.dumps({
        "fasm": fasm,
        "pips_checked": counts["pips_checked"],
        "fully_agree": counts["pips_fully_agree"],
        "contradictions": contradictions,
        "c1": counts["c1_dst_wire_missing"],
        "c2": counts["c2_src_wire_missing"],
        "c3": counts["c3_edge_missing"],
        "c4": counts["c4_bel_no_site_partner"],
        "c4_agree": counts["c4_bel_agree"],
        "info_wire_not_in_tileconn": counts["info_wire_not_in_tileconn"],
        "bidir_suffix": counts["bidir_suffix"],
        "no_ttype": counts["no_tiletype_file"],
        "distinct_bad_edges": len(bad_edge),
        "distinct_bad_bel": len(bad_bel),
    }, indent=1))


if __name__ == "__main__":
    main()
