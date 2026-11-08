import sys
sys.path.insert(0, '/Users/saksham/Documents/zedboard-helion-debug/work')
import x7decode as X
ref = X.payload('/Users/saksham/Documents/zedboard-helion-debug/work/ref/GT-blinky.bit')
tiles = X._tilegrid()
frames = X.frames_by_address(X.full_addresses(), ref, check=False)
DB = '/Users/saksham/Documents/zedboard-helion-debug/work/prjxray-db/zynq7'
import glob
seg = {}
for fn in glob.glob(DB + '/segbits_clb*.db'):
    if 'origin' in fn: continue
    for line in open(fn):
        p = line.split()
        if len(p) < 2 or 'PRECYINIT' not in p[0]: continue
        terms = []
        for b in p[1:]:
            neg = b.startswith('!')
            fo, bp = b.lstrip('!').split('_')
            terms.append((neg, int(fo), int(bp)))
        seg.setdefault(p[0], []).append(terms)
def selected(feat, base, off, nw):
    for terms in seg.get(feat, []):
        ok = True
        for neg, fo, bp in terms:
            wi = off + bp // 32
            if wi < off or wi >= off + nw: continue
            w = frames[base + fo][wi]
            bit = (w >> (bp % 32)) & 1
            if bit == (1 if neg else 0):
                ok = False; break
        if ok: return True
    return False
found = []
for t, v in tiles.items():
    if 'bits' not in v or 'CLB_IO_CLK' not in v['bits'] or 'CLB' not in v['type']: continue
    g = v['bits']['CLB_IO_CLK']; base = int(g['baseaddr'], 16); off = g['offset']; nw = g['words']
    for half in ('SLICEM_X0', 'SLICEL_X1'):
        for m in ('AX', 'CIN', 'C1'):
            if selected('%s.%s.PRECYINIT.%s' % (v['type'], half, m), base, off, nw):
                found.append((t, half, m))
for f in sorted(found): print(f)
print('non-C0 halves:', len(found))
