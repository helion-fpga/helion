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
    terms = []
    for b in p[1:]:
        neg = b.startswith('!')
        fo, bp = b.lstrip('!').split('_')
        terms.append((neg, int(fo), int(bp)))
    seg.setdefault(p[0], []).append(terms)
def selected(feat, base, off, nwords):
    for terms in seg.get(feat, []):
        ok = True
        for neg, fo, bp in terms:
            wi = off + bp // 32
            if wi < off or wi >= off + nwords:
                continue  # term outside this tile's words: not decisive here
            w = frames[base + fo][wi]
            bit = (w >> (bp % 32)) & 1
            if bit == (1 if neg else 0):
                ok = False; break
        if ok:
            return True
    return False
nff = ncy = 0
for y in range(38, 47):
    t = 'CLBLM_R_X71Y%d' % y
    if t not in tiles or 'bits' not in tiles[t] or 'CLB_IO_CLK' not in tiles[t]['bits']:
        print('===', t, '(absent)'); continue
    g = tiles[t]['bits']['CLB_IO_CLK']
    base = int(g['baseaddr'], 16); off = g['offset']; nw = g['words']
    print('===', t, 'words', off, '..', off + nw - 1)
    inits = defaultdict(int)
    others = []
    for feat in seg:
        if '.INIT[' in feat:
            pre, idx = feat.rsplit('[', 1)
            if selected(feat, base, off, nw):
                inits[pre] |= 1 << int(idx[:-1])
        elif any(k in feat for k in ('ZINI','ZRST','PRECYINIT','FFMUX','CEUSE','SRUSE','FFSYNC')) and 'CARRY4' not in feat and '.AC' not in feat and '.BC' not in feat and '.CC' not in feat and '.DC' not in feat:
            if selected(feat, base, off, nw):
                others.append(feat)
        elif '.CARRY4.' in feat or '.ACY' in feat or '.BCY' in feat or '.CCY' in feat or '.DCY' in feat:
            if selected(feat, base, off, nw):
                others.append(feat)
    for l in sorted(inits):
        if inits[l]:
            print('  ', l, '= 0x%016x' % inits[l])
    for f in sorted(others):
        print('  ', f)
