#!/usr/bin/env python3
"""Regenerate span_aliases.txt from Project IceStorm's icebox (HX8K).

Every span wire name (sp4_*, sp12_*, span4_*, span12_*) that the routing
databases next to this script use in a tile, `(x, y, name)` exactly as
spelled there, is grouped with every name IceStorm's `follow_net` says is
the same copper (transitively, through icebox's own name normalization),
and each group gets one wire id. Names icebox says a tile does not have
(`tile_has_net`, e.g. `span4_horz_N` in a top IO tile) are left out, so
the router never uses them. Output lines: `<wire id> <x> <y> <name>`.

    python3 gen_span_aliases.py /path/to/icestorm/python > span_aliases.txt
"""
import os
import re
import sys

sys.path.insert(0, sys.argv[1])
import icebox  # noqa: E402

here = os.path.dirname(os.path.abspath(__file__))
ic = icebox.iceconfig()
ic.setup_empty_8k()
span = re.compile(r"^(sp4_|sp12_|span4_|span12_)")


def raw_names(fname):
    names = set()
    for line in open(os.path.join(here, fname)):
        f = line.rstrip("\n").split("\t")
        if len(f) >= 4 and f[1] in ("buffer", "routing"):
            names.update(n for n in f[2:4] if span.match(n))
    return names


db = {k: raw_names(f) for k, f in [("l", "io_l.txt"), ("r", "io_r.txt"), ("t", "io_t.txt"), ("b", "io_b.txt"),
                                   ("logic", "logic_8k.txt"), ("ramb", "ramb_8k.txt"), ("ramt", "ramt_8k.txt")]}

parent = {}


def find(a):
    parent.setdefault(a, a)
    while parent[a] != a:
        parent[a] = parent[parent[a]]
        a = parent[a]
    return a


def union(a, b):
    ra, rb = find(a), find(b)
    if ra != rb:
        parent[max(ra, rb)] = min(ra, rb)


for x in range(ic.max_x + 1):
    for y in range(ic.max_y + 1):
        if (x, y) in ic.io_tiles:
            kind = ic.tile_pos(x, y)
        elif (x, y) in ic.logic_tiles:
            kind = "logic"
        elif (x, y) in ic.ramb_tiles:
            kind = "ramb"
        elif (x, y) in ic.ramt_tiles:
            kind = "ramt"
        else:
            continue
        for n in db[kind]:
            nn = icebox.netname_normalize(n)
            assert nn is not None, (x, y, n)
            if not ic.tile_has_net(x, y, nn):
                continue
            union((x, y, n), (x, y, nn))
            for m in ic.follow_net((x, y, nn)):
                if span.match(m[2]):
                    union((x, y, nn), m)

groups = {}
for k in parent:
    groups.setdefault(find(k), []).append(k)
for wid, root in enumerate(sorted(groups)):
    for x, y, n in sorted(groups[root]):
        print(wid, x, y, n)
