import sys
sys.path.insert(0, '/Users/saksham/Documents/zedboard-helion-debug/work')
import x7decode as X
from collections import defaultdict
ref = X.payload('/Users/saksham/Documents/zedboard-helion-debug/work/ref/GT-blinky.bit')
tiles = X._tilegrid()
frames = X.frames_by_address(X.full_addresses(), ref, check=False)
DB = '/Users/saksham/Documents/zedboard-helion-debug/work/prjxray-db/zynq7'
seg = {}
for line in open(DB + '/segbits_clblm_r.db'):
    p = line.split()
    if len(p) < 2: continue
    for b in p[1:]:
        seg.setdefault(b.lstrip('!'), []).append(('!' if b.startswith('!') else '') + p[0])
for y in range(39, 46):
    t = 'CLBLM_R_X71Y%d' % y
    g = tiles[t]['bits']['CLB_IO_CLK']
    base = int(g['baseaddr'], 16); off = g['offset']
    feats = defaultdict(list)
    for fo in range(g['frames']):
        fr = frames[base + fo]
        for wi in range(len(fr)):
            word = fr[wi]
            if not word: continue
            for bit in range(32):
                if word >> bit & 1:
                    bp = (wi - off) * 32 + bit
                    key = '%02d_%02d' % (fo, bp)
                    if key in seg:
                        for f in seg[key]:
                            feats[f].append((fo, bit))
    print('===', t, base == 0 and '' or '')
    interesting = [f for f in sorted(feats) if any(k in f for k in ('INIT', 'PRECY', 'CARRY4', 'DFFMUX', 'FFMUX', 'ZINI', 'ZRST', 'CEUSE', 'SRUSE', 'SYNC'))]
    for f in interesting:
        print('  ', f)
    if not interesting:
        print('   (no decoded features; n set words checked)')
