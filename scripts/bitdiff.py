#!/usr/bin/env python3
"""Reverse-engineer a reference bitstream and report exactly where Helion
diverges from it, tile by tile and feature by feature.

This is the general form of `clkdiff.py`. Rather than hand-checking one
subsystem, it decodes BOTH bitstreams over the whole die and reports:

  * every tile whose configuration differs, grouped by tile type
  * within a tile, the exact features present in one and absent in the other
  * a summary ranked by "how much of the reference we are missing"

Usage:
    bitdiff.py <ours.bit> <ref.bit> [--type INT_R] [--tile INT_R_X73Y46]
               [--limit N] [--quiet]

The intended workflow: build the same design with Vivado and with Helion,
then run this. It prints the divergence in the same vocabulary the FASM
database uses, so each line maps straight to a feature to emit.

Ground truth used throughout this project:
    ~/CDA4253_A1/blinky.bit   real Vivado 2023.2, xc7z020clg484-1, LD0 on T22
"""
import argparse
import collections
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', '..', 'work'))

try:
    import gt_decode as G
except ImportError:  # allow running from the repo root
    sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), 'work'))
    import gt_decode as G


def load(db, path):
    return G.frames_by_address(db.full_addresses(), G.payload(path))


def seg_path(db, tile):
    """segbits path for a tile, or None when the DB ships none."""
    ty = db._tg[tile]['type']
    p = '%s/segbits_%s.db' % (db.root, ty.lower())
    return p if os.path.exists(p) else None


def feats(db, tile, frames):
    p = seg_path(db, tile)
    if p is None:
        return set()
    try:
        return set(G.active_features(db, tile, p, frames))
    except Exception:
        return set()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('ours')
    ap.add_argument('ref')
    ap.add_argument('--family', default='zynq7')
    ap.add_argument('--device', default='xc7z020')
    ap.add_argument('--part', default='xc7z020clg484-1')
    ap.add_argument('--grid', default='xc7z020')
    ap.add_argument('--type', default=None,
                    help='only report tiles whose type starts with this')
    ap.add_argument('--tile', default=None, help='only report this tile')
    ap.add_argument('--limit', type=int, default=25,
                    help='max differing tiles to print (0 = all)')
    ap.add_argument('--quiet', action='store_true',
                    help='summary only')
    a = ap.parse_args()

    db = G.DB(a.family, a.device, a.part, a.grid)
    ref = load(db, a.ref)
    ours = load(db, a.ours)

    by_type = collections.Counter()
    diff_tiles = []
    missing_total = 0
    extra_total = 0

    for tile in sorted(db._tg):
        ty = db._tg[tile]['type']
        if a.type and not ty.startswith(a.type):
            continue
        if a.tile and tile != a.tile:
            continue
        f_ref = feats(db, tile, ref)
        f_our = feats(db, tile, ours)
        if f_ref == f_our:
            continue
        missing = sorted(f_ref - f_our)
        extra = sorted(f_our - f_ref)
        missing_total += len(missing)
        extra_total += len(extra)
        by_type[ty] += len(missing)
        diff_tiles.append((len(missing), tile, ty, missing, extra))

    diff_tiles.sort(key=lambda x: (-x[0], x[1]))

    print('ref : %s' % a.ref)
    print('ours: %s' % a.ours)
    print()
    print('%d tiles differ; %d features missing, %d extra'
          % (len(diff_tiles), missing_total, extra_total))
    print()

    if by_type:
        print('missing features by tile type (highest first):')
        for ty, n in by_type.most_common():
            print('   %-24s %6d' % (ty, n))
        print()

    if a.quiet:
        return 0

    shown = diff_tiles if a.limit == 0 else diff_tiles[:a.limit]
    for n, tile, ty, missing, extra in shown:
        print('== %s (%s)  missing %d, extra %d' % (tile, ty, len(missing), len(extra)))
        for x in missing[:20]:
            print('     MISSING %s' % x)
        if len(missing) > 20:
            print('     ... %d more missing' % (len(missing) - 20))
        for x in extra[:10]:
            print('     extra   %s' % x)
        if len(extra) > 10:
            print('     ... %d more extra' % (len(extra) - 10))
    if len(diff_tiles) > len(shown):
        print('\n... %d more differing tiles (use --limit 0 for all)'
              % (len(diff_tiles) - len(shown)))

    return 1 if missing_total else 0


if __name__ == '__main__':
    sys.exit(main())
