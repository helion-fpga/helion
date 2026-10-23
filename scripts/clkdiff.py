#!/usr/bin/env python3
"""Feature-for-feature diff of the clock spine between a ground-truth
Vivado bitstream and a Helion bitstream.

Usage: clkdiff.py <ours.bit> [<ref.bit>]
"""
import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gt_decode as G

REF = os.path.expanduser('~/CDA4253_A1/blinky.bit')
# The full clock spine of the ZedBoard 100 MHz GCLK (Y9), in signal order.
PATH = ['LIOB33_X0Y25','LIOI3_X0Y25','HCLK_IOI3_X1Y26','HCLK_CMT_X8Y26',
        'HCLK_L_X174Y26','HCLK_CLB_X73Y26','CLK_HROW_BOT_R_X87Y26',
        'CLK_BUFG_REBUF_X87Y38','CLK_BUFG_BOT_R_X87Y48']

def feats(db, tile, frames):
    ty = db._tg[tile]['type']
    seg = '%s/segbits_%s.db' % (db.root, ty.lower())
    if not os.path.exists(seg):
        return None
    return set(G.active_features(db, tile, seg, frames))

def main():
    ours_path = sys.argv[1]
    ref_path = sys.argv[2] if len(sys.argv) > 2 else REF
    db = G.DB('zynq7', 'xc7z020', 'xc7z020clg484-1', 'xc7z020')
    addrs = db.full_addresses()
    ref = G.frames_by_address(addrs, G.payload(ref_path))
    ours = G.frames_by_address(addrs, G.payload(ours_path))
    print(f"ref : {ref_path}")
    print(f"ours: {ours_path}\n")
    total_missing = 0
    for t in PATH:
        if t not in db._tg:
            print(f"== {t}: NOT IN TILEGRID"); continue
        a = feats(db, t, ref); b = feats(db, t, ours)
        if a is None:
            print(f"== {t}: no segbits"); continue
        missing = sorted(a - b); extra = sorted(b - a)
        total_missing += len(missing)
        status = "IDENTICAL" if not missing and not extra else ""
        print(f"== {t}  (ref {len(a)} / ours {len(b)}) {status}")
        for x in missing: print(f"     MISSING {x}")
        for x in extra:   print(f"     extra   {x}")
    print(f"\nTOTAL MISSING vs Vivado: {total_missing}")
    return 1 if total_missing else 0

if __name__ == '__main__':
    sys.exit(main())
