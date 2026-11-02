"""Block-by-block equivalence harness, in the style of Project X-Ray.

WHY THIS EXISTS
---------------
Every previous round diffed the WHOLE 4-megabit image and then hunted whichever
bits disagreed. That has two fatal properties:

  1. It gives no location. 70 scattered bits tell you nothing about which part
     of the flow is sound.
  2. It cannot distinguish "our router made a different but valid choice" from
     "our router is broken", because both look like a bit-level mismatch.

X-Ray solved the same problem for CHARACTERISATION by never looking at the whole
device: they programme a harness that routes through a small region of interest
(ROI) and fuzz one region at a time. See
work/prjxray-db/artix7/harness/README.md -- the ROI grid is `GRID_X_MIN..MAX` /
`GRID_Y_MIN..MAX` and every port records `wires_outside_roi` so the region can
be isolated.

There is no prebuilt harness for Zynq-7 in this DB (X-Ray only shipped Artix-7
board harnesses), but the METHOD needs none: we have the full tilegrid and the
segbits DB for xc7z020, so we can define a block ourselves and decode it.

WHAT THIS TOOL DOES
-------------------
Given two bitstreams and a rectangular block of tiles, it:
  * enumerates every tile inside the block, by type
  * decodes each tile's features from BOTH images, independently
  * reports per-tile: identical / differing / (and the feature delta)
  * aggregates a block verdict

The point is the INVARIANT, not the diff: a block that is identical in
isolation is a reference that must keep holding as the design grows. Blocks are
the unit of proven correctness; the whole-device diff is not.

USAGE
    python3 work/blockdiff.py <ours.bit> <ref.bit> [--x X0:X1] [--y Y0:Y1]
                             [--radius N] [--tiles "A,B,C"] [--json out.json]

With no block given it prints a per-block (one clock-region row chunk) summary,
so you can SEE which blocks are clean instead of guessing from a global count.
"""
import sys, os, json, argparse
from collections import defaultdict

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import x7decode as X


def tile_coord(name):
    """`INT_R_X71Y41` -> (71, 41). Returns None for tiles without a coord."""
    tail = name.rsplit("_", 1)[-1]
    if not tail.startswith("X") or "Y" not in tail:
        return None
    try:
        xs, ys = tail[1:].split("Y")
        return int(xs), int(ys)
    except ValueError:
        return None


def segbits_for(tile_type):
    """The segbits file that describes this tile type, or None."""
    t = tile_type.lower()
    base = f"{X.DB}/xc7z020/segbits_{t}.db"
    return base if os.path.exists(base) else None


class Image:
    def __init__(self, path, tilegrid):
        self.path = path
        self.frames = X.frames_by_address(X.full_addresses(), X.payload(path))
        self.tilegrid = tilegrid
        self._cache = {}

    def feats(self, tile):
        if tile in self._cache:
            return self._cache[tile]
        t = self.tilegrid.get(tile)
        if not t:
            self._cache[tile] = None
            return None
        seg = segbits_for(t["type"])
        if not seg:
            self._cache[tile] = None
            return None
        try:
            f = set(X.active_features(tile, seg, self.frames))
        except Exception:
            f = set()
        self._cache[tile] = f
        return f


def load_tilegrid():
    return json.load(open(X.TILEGRID))


def tiles_in_block(tg, x0, x1, y0, y1):
    out = []
    for name, t in tg.items():
        c = tile_coord(name)
        if not c:
            continue
        x, y = c
        if x0 <= x <= x1 and y0 <= y <= y1:
            if segbits_for(t["type"]):
                out.append(name)
    return sorted(out)


def compare_block(ours, ref, tiles):
    """Return (identical, differing, details) for one block."""
    identical, differing, details = [], [], []
    for tile in tiles:
        a, b = ours.feats(tile), ref.feats(tile)
        if a is None or b is None:
            continue
        if a == b:
            identical.append(tile)
        else:
            differing.append(tile)
            details.append(
                {
                    "tile": tile,
                    "missing": sorted(b - a),
                    "extra": sorted(a - b),
                }
            )
    return identical, differing, details


def coords_of(tiles):
    xs, ys = [], []
    for t in tiles:
        c = tile_coord(t)
        if c:
            xs.append(c[0])
            ys.append(c[1])
    return xs, ys


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("ours")
    ap.add_argument("ref")
    ap.add_argument("--x", help="X0:X1 inclusive")
    ap.add_argument("--y", help="Y0:Y1 inclusive")
    ap.add_argument("--radius", type=int, default=0,
                    help="expand the auto block around every differing tile by N")
    ap.add_argument("--y-chunk", type=int, default=8,
                    help="height of the auto summary blocks (default 8 rows)")
    ap.add_argument("--json", help="write the full report here")
    args = ap.parse_args()

    tg = load_tilegrid()
    ours = Image(args.ours, tg)
    ref = Image(args.ref, tg)

    # ---- explicit block -------------------------------------------------
    if args.x or args.y:
        allc = [tile_coord(n) for n in tg if tile_coord(n)]
        x0, x1 = (map(int, args.x.split(":")) if args.x
                  else (min(c[0] for c in allc), max(c[0] for c in allc)))
        y0, y1 = (map(int, args.y.split(":")) if args.y
                  else (min(c[1] for c in allc), max(c[1] for c in allc)))
        tiles = tiles_in_block(tg, x0, x1, y0, y1)
        ident, diff, details = compare_block(ours, ref, tiles)
        print(f"BLOCK X{x0}..{x1} Y{y0}..{y1}: {len(tiles)} decodable tiles")
        print(f"  identical: {len(ident)}   differing: {len(diff)}")
        for d in details[:40]:
            print(f"  != {d['tile']}")
            for m in d["missing"][:8]:
                print(f"       missing {m}")
            for e in d["extra"][:8]:
                print(f"       extra   {e}")
        rc = 0 if not diff else 1
        if args.json:
            json.dump({"block": [x0, x1, y0, y1], "identical": ident,
                       "differing": details}, open(args.json, "w"), indent=1)
        return rc

    # ---- auto summary: every block, so the clean ones are visible -------
    ys_all = [c[1] for c in (tile_coord(n) for n in tg) if c]
    xs_all = [c[0] for c in (tile_coord(n) for n in tg) if c]
    y0, y1 = min(ys_all), max(ys_all)
    x0, x1 = min(xs_all), max(xs_all)

    print(f"block summary  ours={os.path.basename(args.ours)}  ref={os.path.basename(args.ref)}")
    print(f"device X{x0}..{x1} Y{y0}..{y1}, blocks of {args.y_chunk} rows")
    print(f"{'Y block':>12}  {'tiles':>6} {'same':>6} {'diff':>5}  verdict")
    report = []
    clean_blocks = 0
    total_blocks = 0
    for yb in range(y0, y1 + 1, args.y_chunk):
        ye = min(yb + args.y_chunk - 1, y1)
        tiles = tiles_in_block(tg, x0, x1, yb, ye)
        if not tiles:
            continue
        ident, diff, details = compare_block(ours, ref, tiles)
        total_blocks += 1
        if not diff:
            clean_blocks += 1
        verdict = "CLEAN" if not diff else f"{len(diff)} tile(s) differ"
        print(f"{yb:5d}..{ye:<6d}  {len(tiles):6d} {len(ident):6d} {len(diff):5d}  {verdict}")
        report.append({"y": [yb, ye], "tiles": len(tiles),
                       "identical": ident, "differing": details})

    print()
    print(f"CLEAN BLOCKS: {clean_blocks} of {total_blocks}")
    if args.json:
        json.dump({"blocks": report}, open(args.json, "w"), indent=1)
        print(f"wrote {args.json}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
