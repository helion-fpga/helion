#!/usr/bin/env python3
"""Decode fuzz.tcl bitstreams into X-Ray segbits fields and list what X-Ray cannot explain.

  xraydecode.py <fuzzdir> <tilegrid.json> <xray-db-dir> [--types CMT_TOP_R_LOWER_B,...]
  xraydecode.py selftest

For every <id>.bit.gz next to base.bit.gz (ids and props from fuzz.log), prints
  id  props  changed fields (FIELD=base->var, multi-bit fields as integers)  unexplained bits
where unexplained = tile-relative bits that differ from base, appear in no segbits entry and
sit in no word another tile (e.g. the CMT's INT_R) also covers; those are only counted.
"""
import json
import os
import re
import sys
from collections import defaultdict

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from clksolve import tile_bits  # noqa: E402

TYPES = "CMT_TOP_R_LOWER_B,CMT_TOP_L_LOWER_B,CMT_TOP_R_UPPER_T,CMT_TOP_L_UPPER_T"


def load_db(dbdir, ty):
    """-> (fields {name: {index: (pos bits, neg bits)}}, all bits X-Ray mentions)."""
    fields, known = defaultdict(dict), set()
    p = os.path.join(dbdir, f"segbits_{ty.lower()}.db")
    for line in open(p) if os.path.exists(p) else []:
        f = line.split()
        if not f:
            continue
        m = re.match(r"(.*)\[(\d+)\]$", f[0])
        name, idx = (m[1], int(m[2])) if m else (f[0], None)
        pos = {b for b in f[1:] if not b.startswith("!")}
        neg = {b[1:] for b in f[1:] if b.startswith("!")}
        fields[name][idx] = (pos, neg)
        known |= pos | neg
    return fields, known


def decode(bits, fields):
    out = {}
    for name, ent in fields.items():
        if None in ent:
            pos, neg = ent[None]
            out[name] = int(pos <= bits and not (neg & bits))
        else:
            out[name] = sum(1 << i for i, (pos, neg) in ent.items() if pos <= bits and not (neg & bits))
    return out


def main():
    a = sys.argv[1:]
    if a[:1] == ["selftest"]:
        f = {"T.F": {0: ({"00_1"}, set()), 2: ({"00_3"}, set())}, "T.E": {None: ({"01_0"}, {"01_1"})}}
        assert decode({"00_1", "00_3", "01_0"}, f) == {"T.F": 5, "T.E": 1}
        assert decode({"01_0", "01_1"}, f) == {"T.F": 0, "T.E": 0}
        return print("selftest ok")
    import bitframes
    d, grid, dbdir = a[0], json.load(open(a[1])), a[2]
    types = set((a[a.index("--types") + 1] if "--types" in a else TYPES).split(","))
    tiles = {k: v for k, v in grid.items() if v["type"] in types and "CLB_IO_CLK" in v.get("bits", {})}
    dbs = {ty: load_db(dbdir, ty) for ty in types}
    # Words a tracked tile shares with another tile (e.g. CMT and its INT_R): their bits
    # belong to that tile's own segbits, so they are counted apart, not as unexplained.
    segs = [(t, int(x["baseaddr"], 16), x["frames"], x["offset"], x["words"]) for t, tv in grid.items() for x in tv.get("bits", {}).values()]
    shared = {}
    for t, v in tiles.items():
        b = v["bits"]["CLB_IO_CLK"]
        base, off = int(b["baseaddr"], 16), b["offset"]
        shared[t] = {(m, w - off) for m in range(b["frames"]) for w in range(off, off + b["words"])
                     if any(o != t and ba <= base + m < ba + n and oo <= w < oo + nw for o, ba, n, oo, nw in segs)}

    def tiles_of(path):
        _, fr = bitframes.frames(path)
        return {t: tile_bits(fr, v["bits"]["CLB_IO_CLK"]) for t, v in tiles.items()}

    base = tiles_of(os.path.join(d, "base.bit.gz"))
    print("id\tprops\tchanged\tunexplained\tbits in words shared with other tiles")
    for line in open(os.path.join(d, "fuzz.log")):
        f = line.rstrip("\n").split("\t")
        bit = os.path.join(d, f"{f[0]}.bit.gz")
        if len(f) < 2 or f[1] != "OK" or not os.path.exists(bit):
            continue
        var, changed, unexpl, nshared = tiles_of(bit), [], [], []
        for t, vb in var.items():
            bb = base[t]
            if vb == bb:
                continue
            fields, known = dbs[grid[t]["type"]]
            db, dv = decode(bb, fields), decode(vb, fields)
            changed += [f"{t}:{n.split('.', 1)[1]}={db[n]}->{dv[n]}" for n in sorted(dv) if dv[n] != db[n]]
            for b in sorted((vb ^ bb) - known):
                m, bi = map(int, b.split("_"))
                (nshared.append if (m, bi // 32) in shared[t] else unexpl.append)(f"{t}:{b}")
        print(f"{f[0]}\t{' '.join(f[2:])}\t{' '.join(changed)}\t{' '.join(unexpl)}\t{len(nshared)}")


if __name__ == "__main__":
    main()
