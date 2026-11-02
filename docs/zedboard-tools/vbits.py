"""Trustworthy feature differ: decode with x7decode, never the raw FASM text.

`scripts/bitdiff.py` reports phantom misses (notably ALUT.INIT[32..63]) because
it compares FASM lines textually. This decodes both bitstreams through the
segbits DB and compares the resulting feature sets, which is the same
instrument clkdiff uses.
"""
import sys, os, json
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import x7decode as X

def feats_for(bit, tiles):
    addrs = X.full_addresses()
    frames = X.frames_by_address(addrs, X.payload(bit))
    out = {}
    for tile, segname in tiles:
        p = f"{X.DB}/{segname}"
        if not os.path.exists(p):
            continue
        out[tile] = set(X.active_features(tile, p, frames))
    return out

def main():
    ours, ref = sys.argv[1], sys.argv[2]
    tiles = json.load(open(sys.argv[3])) if len(sys.argv) > 3 else []
    tiles = [(t, s) for t, s in tiles]
    if not tiles:
        # Comparing nothing prints "TOTAL DIFFS: 0", which reads as a pass. It
        # is not one: without a tile list this tool has looked at no silicon at
        # all. Refuse rather than mislead.
        print("vbits: no tile list given -- nothing would be compared.")
        print("       pass work/spine_tiles.json, or use clkdiff.py for a")
        print("       full-device scan.")
        return 2
    a = feats_for(ours, tiles); b = feats_for(ref, tiles)
    total = 0
    for tile, _ in tiles:
        fa, fb = a.get(tile, set()), b.get(tile, set())
        miss, extra = sorted(fb - fa), sorted(fa - fb)
        if not miss and not extra:
            print(f"== {tile}  IDENTICAL ({len(fb)} features)")
            continue
        print(f"== {tile}  ref {len(fb)} / ours {len(fa)}")
        for m in miss: print(f"     MISSING {m}"); total += 1
        for e in extra: print(f"     extra   {e}"); total += 1
    print(f"\nTOTAL DIFFS: {total}")
    return 0

if __name__ == "__main__":
    sys.exit(main())
