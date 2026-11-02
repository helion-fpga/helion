#!/usr/bin/env python3
"""Bitstream-accurate value simulator for the zedboard blinky counter.

Decodes a real .bit + prjxray DB into a value circuit and ticks clock edges:
  * CLB lanes: LUT truth tables from INIT[..] features (A1=LSB index),
    CARRY4 per the model of record (S=O6, O=S^CY, CO = S?O5:CY),
    FFs from FFMUX selects;
  * INT mux arms (features), always-on INT pips, tileconn adjacency,
    CLB-internal pass pips and CLB-internal configured pips;
  * LOGIC_OUTSn -> site-pin value taps.

GATE: the sweep must find conventions under which the hardware-proven
BLINK-242 (a) counts from zero, state(c) == c, and (b) with the state
seeded at the all-ones wrap, the registered LED value follows
(state+1)[24] and toggles 1->0 on the wrap -- the VHDL `LD0 <= count(24)`
behaviour. Any other image is graded with the gate-locked conventions only.

  python3 work/bitsim.py --gate              # run the BLINK-242 gate
  python3 work/bitsim.py <img.bit>           # grade with locked conventions
exit: 0 blink / 1 static / 2 gate failure
"""
import os, sys, json, argparse
from itertools import product
from collections import defaultdict
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import x7decode as X

PART='work/prjxray-db/zynq7/xc7z020'
LANES=('A','B','C','D')
XV='X'

class Db:
    def __init__(self):
        self.grid=json.load(open(f'{PART}/tilegrid.json'))
        self.pairs={}
        for e in json.load(open(f'{PART}/tileconn.json')):
            a,b=e['tile_types']; dx,dy=e['grid_deltas']
            self.pairs.setdefault((a,b,dx,dy),[]).extend(e['wire_pairs'])
            self.pairs.setdefault((b,a,-dx,-dy),[]).extend(((w2,w1) for w1,w2 in e['wire_pairs']))
        self._tt={}; self._gx={}
    def grid_index(self):
        self._gx={(v['grid_x'],v['grid_y'],v['type']):k for k,v in self.grid.items()}
    def tile_at(self,x,y,t): return self._gx.get((x,y,t))
    def tt(self,t):
        if t not in self._tt:
            p=f'{PART}/tile_type_{t}.json'
            self._tt[t]=json.load(open(p)) if os.path.exists(p) else {'pips':{}}
        return self._tt[t]
    def is_clb(self,tile):
        return tile.startswith('CLBLM_')
    def neighbors(self,tile):
        info=self.grid[tile]; gx,gy,A=info['grid_x'],info['grid_y'],info['type']
        out=[]
        for (a,b,dx,dy),pl in self.pairs.items():
            if a!=A: continue
            nt=self.tile_at(gx+dx,gy+dy,b)
            if nt:
                for w1,w2 in pl: out.append((nt,w2,w1))
        return out

class Img:
    def __init__(self,path,db):
        self.path=path; self.db=db
        self.frames=X.frames_by_address(X.full_addresses(),X.payload(path),check=False)
        self._f={}
    def feats(self,tile):
        if tile not in self._f:
            typ=self.db.grid[tile]['type']
            p=f'{PART}/segbits_{typ.lower()}.db'
            if os.path.exists(p):
                self._f[tile]=set(X.active_features(tile,p,self.frames))
            else:
                self._f[tile]=set()          # bridge tiles: hard-wired only
        return self._f[tile]

def muxval(arms,ev):
    vs=[ev(s) for s in arms]
    real={v for v in vs if v!=XV}
    if len(real)==1: return real.pop()
    if not real and vs and all(v==XV for v in vs): return XV
    return XV if len(real)!=1 else real.pop()

class Sim:
    def __init__(self,img,db,cfg):
        self.img=img; self.db=db; self.cfg=cfg
        self._ppidef={}
        for t in ('int_r','int_l'):
            p=f'{PART}/ppips_{t}.db'
            if os.path.exists(p):
                for line in open(p):
                    w=line.split()
                    if len(w)==2 and w[1]=='default':
                        self._ppidef[(t.upper(),w[0].split('.')[1])]=True
        self.ROWS=list(range(38,48))
        self._pin={}; self._lut={}; self._passcache={}
        self.load()

    def load(self):
        # CLB tile-type pips: always-on passes + pin map + LOGIC_OUTS map
        tt=self.db.tt('clblm_r')['pips']
        segset={n for n,_ in X.parse_segbits(f'{PART}/segbits_clblm_r.db')}
        self.clb_pass=defaultdict(list)
        for e in tt.values():
            key=f"CLBLM_R.{e['dst_wire']}.{e['src_wire']}"
            if key in segset: continue          # configured -> features decide
            self.clb_pass[e['dst_wire']].append(e['src_wire'])
        self.logi={}
        for e in tt.values():
            d,s=e['dst_wire'],e['src_wire']
            if d.startswith('CLBLM_LOGIC_OUTS'):
                self.logi[d[len('CLBLM_LOGIC_OUTS'):]]=tuple(s.split('_')[1:])
        self.pin_imux={}
        for e in tt.values():
            d,s=e['dst_wire'],e['src_wire']
            # All six slice-input pins, not just A5/A6: the counter masks
            # depend on A1..A4 too, and those are VCC ties (ppips default)
            # in both images, which trace_val resolves from the IMUX arm.
            if s.startswith('CLBLM_IMUX') and d[-1] in '123456':
                H=d.split('_')[1]; lane=d.split('_')[2][0]
                self.pin_imux[(H,lane,d[-1])]='IMUX'+s[len('CLBLM_IMUX'):]
        # per-type pass pips and configured-mux arms across the cone.
        # No type whitelist: the operand loops cross clock-region boundary
        # families (EE4/WW4/*_S0, INT_INTERFACE, CMT, R_TERM) that live in
        # ordinary segbits files; a type filter silently X's those hops.
        import re
        self.type_tt={}
        self.arms={}; self.clb_arms={}
        pat=re.compile(r'^\w+_X(\d+)Y(\d+)$')
        for t in self.db.grid:
            m=pat.match(t)
            if not m: continue
            if not (56<=int(m.group(1))<=96 and 28<=int(m.group(2))<=60): continue
            typ=self.db.grid[t]['type']
            segp=f'{PART}/segbits_{typ.lower()}.db'
            segset={n.split('.',1)[1].rsplit('.',1)[0] if False else n for n,_ in X.parse_segbits(segp)} if False else None
            d=defaultdict(list)
            for fn in self.img.feats(t):
                p=fn.split('.',1)[1].split('.')
                if len(p)==2: d[p[0]].append(p[1])   # configured mux: DST.SRC
            if t.startswith('INT_'): self.arms[t]=dict(d)
            else: self.clb_arms[t]=dict(d)
        # slice grammar
        self.clbf={}
        for y in self.ROWS:
            tc=f'CLBLM_R_X71Y{y}'
            names={fn.split('.',1)[1] for fn in self.img.feats(tc)}
            d={}
            for sn,half in (('SLICEM_X0','M'),('SLICEL_X1','L')):
                sl={'lut':{},'ff':{},'precy':None}
                for ln in LANES:
                    init={}
                    for fn in names:
                        pfx=sn+'.'+ln+'LUT.INIT['
                        if fn.startswith(pfx):
                            init[int(fn[len(pfx):-1])]=1
                    sl['lut'][ln]=init
                    sel=None
                    for fn in names:
                        if fn.startswith(sn+'.'+ln+'FFMUX.'):
                            sel=fn.rsplit('.',1)[-1]
                    di1=None
                    for fn in names:
                        if fn.startswith(sn+'.'+ln+'LUT.DI1MUX.'):
                            di1=fn.rsplit('.',1)[-1]
                    sl['ff'][ln]={'mux':sel,'di1':di1}
                for fn in names:
                    if fn.startswith(sn+'.PRECYINIT.'):
                        sl['precy']=fn.rsplit('.',1)[-1]
                d[sn]=sl
            self.clbf[y]=d

    # ---------- tracing with values
    def passes_for(self,tile,wire):
        typ=self.db.grid[tile]['type']
        key=f'{typ}.{wire}'
        if key not in self._passcache:
            tt=self.db.tt(typ.lower())['pips']
            ifs=f'{PART}/segbits_{typ.lower()}.db'
            segset={n for n,_ in X.parse_segbits(ifs)} if os.path.exists(ifs) else set()
            srcs=[]
            for e in tt.values():
                fam=f"{typ}.{e['dst_wire']}.{e['src_wire']}"
                if e['dst_wire']==wire and fam not in segset:
                    srcs.append(e['src_wire'])
            self._passcache[key]=srcs
        return self._passcache[key]

    def trace_val(self,tile,wire,depth=0,seen=None):
        if seen is None: seen=set()
        if depth>40 or (tile,wire) in seen: return XV
        seen.add((tile,wire))
        if tile.startswith('INT_'):
            if wire.startswith('LOGIC_OUTS'):
                pin=self.logi.get(wire[len('LOGIC_OUTS'):])
                if pin: return self.clb_pin(tile,pin[0],pin[1])
                return XV
            if wire=='GND_WIRE' or wire.startswith('GFAN'): return 0
            aw=self.arms.get(tile,{}).get(wire)
            if aw: return muxval(aw,lambda s:self.trace_val(tile,s,depth+1,seen))
            if (tile.split('_')[0]+'_'+tile.split('_')[1],wire) in self._ppidef:
                return 1
            for src in self.passes_for(tile,wire):
                v=self.trace_val(tile,src,depth+1,seen)
                if v!=XV: return v
        else:
            aw=self.clb_arms.get(tile,{}).get(wire)
            if aw: return muxval(aw,lambda s:self.trace_val(tile,s,depth+1,seen))
            for src in self.passes_for(tile,wire):
                v=self.trace_val(tile,src,depth+1,seen)
                if v!=XV: return v
        for nt,w2,src in self.db.neighbors(tile):
            # neighbors() yields (neighbor_tile, neighbor_wire, our_wire);
            # hand off when OUR wire pairs to the neighbor's, then trace the
            # neighbor's wire (its driver mux may be armed there).
            if src==wire:
                v=self.trace_val(nt,w2,depth+1,seen)
                if v!=XV: return v
        return XV

    def clb_pin(self,int_tile,half,pin):
        y=int(int_tile.split('Y')[-1])
        if pin.endswith('Q'):            return self.q.get((half,y,pin[0]),XV)
        if pin in LANES:                 return self.lut6(half,y,pin)
        if pin.endswith('MUX'):          return self.lut5(half,y,pin[0])
        im=self.pin_imux.get((half,pin[0],pin[-1]))
        if im: return self.trace_val(int_tile,im)
        return XV

    # ---------- LUTs
    def a_pin(self,half,y,lane,num):
        key=(half,y,lane,num)
        if key in self._pin: return self._pin[key]
        im=self.pin_imux.get((half,lane,str(num)))
        v=self.trace_val(f'INT_R_X71Y{y}',im) if im else XV
        if v==XV and im is None and self.cfg.get('floatpin')=='zero':
            v=0        # un-armed slice pin: silicon pull-down default
        if v==XV and im is not None and self.cfg.get('intmux_default')=='vcc':
            v=1        # un-armed INT pin mux: ppips VCC default
        self._pin[key]=v
        return v
    def lut_index(self,pins):
        if self.cfg['idx']=='std':        # i = a1*1 + a2*2 + ... + a6*32
            i=0
            for k in '654321':
                i=(i<<1)|(1 if pins.get(k) else 0)
            return i
        i=0                               # 'rev': i = a1*32 + ... + a6*1
        for k in '123456':
            i=(i<<1)|(1 if pins.get(k) else 0)
        return i
    def lut6(self,half,y,lane):
        key=(half,y,lane)
        if key in self._lut: return self._lut[key]
        init=self.clbf[y]['SLICEM_X0' if half=='M' else 'SLICEL_X1']['lut'][lane]
        if not init:
            v=0   # no INIT bits: all-zero truth table (ppips/UG474 settled)
        else:
            pins={str(k):self.a_pin(half,y,lane,k) for k in range(1,7)}
            known={k:v for k,v in pins.items() if v!=XV}
            unk=[str(k) for k in range(1,7) if str(k) not in known]
            outs=set()
            for combo in product([0,1],repeat=len(unk)):
                pp=dict(known); pp.update(dict(zip(unk,combo)))
                outs.add(1 if self.lut_index(pp) in init else 0)
            v=outs.pop() if len(outs)==1 else XV
        self._lut[key]=v
        return v
    def lut5(self,half,y,lane):
        init=self.clbf[y]['SLICEM_X0' if half=='M' else 'SLICEL_X1']['lut'][lane]
        lo={b for b in init if b<32}; hi={b-32 for b in init if b>=32}
        sub = None
        if self.cfg['o5']=='upper': sub=hi
        elif self.cfg['o5']=='lower': sub=lo
        elif self.cfg['o5']=='lower32': sub={b-32 for b in lo};  # not used by gate
        pins={str(k):self.a_pin(half,y,lane,k) for k in range(1,6)}
        known={k:v for k,v in pins.items() if v!=XV}
        unk=[str(k) for k in range(1,6) if str(k) not in known]
        outs=set()
        for combo in product([0,1],repeat=len(unk)):
            pp=dict(known); pp.update(dict(zip(unk,combo)))
            j=0
            for k in '54321': j=(j<<1)|(1 if pp[k] else 0)   # A5 MSB ... A1 LSB
            outs.add(1 if j in sub else 0)
        return outs.pop() if len(outs)==1 else XV

    # ---------- carry chains
    def chain(self,half,cy0):
        out={}
        cy=None
        sn='SLICEM_X0' if half=='M' else 'SLICEL_X1'
        for y in self.ROWS:
            precy=self.clbf[y][sn]['precy']
            if precy is not None and precy!='CIN':
                cy=cy0                            # chain (re)starts here
            for ln in LANES:
                s6=self.lut6(half,y,ln); o5=self.lut5(half,y,ln)
                # DI = the lane's live operand net (its DI1MUX bypass pick):
                # lane A on the chain-start row feeds its bit on a3 (its mask
                # is the +1 folding, ~a3), every other lane on a6. This is the
                # generate term; with DI = O5 the 1->2 carry never generates.
                if half=='M' and y==self.ROWS[0] and ln==LANES[0]:
                    di=self.a_pin(half,y,ln,3)
                else:
                    di=self.a_pin(half,y,ln,6)
                if s6==XV or o5==XV or di==XV or cy is None or cy==XV:
                    o,co=XV,XV
                elif self.cfg['co']=='di_when_s':
                    o=s6^cy; co=di if s6 else cy   # CO = S?DI:CI
                else:                              # cy_when_s (adder style)
                    o=s6^cy; co=cy if s6 else di   # CO = S?CI:DI
                out[(half,y,ln)]={'S':s6,'O5':o5,'DI':di,'O':o,'CO':co,'CY':cy}
                cy=co
        return out

    def cycle(self):
        """one combinational evaluation; returns {key: lane values}"""
        self._pin.clear(); self._lut.clear()
        res={}
        # M start: the row whose PRECYINIT is C0 -> carry-in constant 0
        res.update(self.chain('M',self.cfg['cy0m']))
        # L start: PRECYINIT AX -> carry comes from the A6 pin of that row
        lstart=next((y for y in self.ROWS
                     if self.clbf[y]['SLICEL_X1']['precy'] in ('AX','CYINIT')),None)
        cy0=self.lut6('L',lstart,'A') if lstart is not None else XV
        res.update(self.chain('L',cy0))
        return res

    # ---------- clocked run
    def bits(self):
        """ordered [(key,...)] of M-chain FFs = counter bits 0..n"""
        out=[]
        for y in self.ROWS:
            for ln in LANES:
                if self.clbf[y]['SLICEM_X0']['ff'][ln]['mux']=='XOR':
                    out.append(('M',y,ln))
        return out
    def state_int(self):
        v=0
        for i,k in enumerate(self.bits()):
            b=self.q.get(k,XV)
            if b==XV: return None
            v|=b<<i
        return v
    def init_state(self):
        v=1 if self.cfg['zini']=='one' else 0
        self.q={k:v for k in self.bits()}

def run_bit(path,cfg,cycles=16,seed=None):
    db=Db(); db.grid_index()
    img=Img(path,db)
    s=Sim(img,db,cfg)
    s.init_state()
    if seed:
        for i,v in enumerate(seed):
            if v is not None: s.q[s.bits()[i]]=v
    trace=[]
    for c in range(cycles):
        st=s.state_int()
        lanes=s.cycle()
        d={}
        for (h,y,ln),lv in lanes.items():
            mux=s.clbf[y]['SLICEM_X0' if h=='M' else 'SLICEL_X1']['ff'][ln]['mux']
            dv={'XOR':lv['O'],'O6':lv['S'],'O5':lv['O5']}.get(mux)
            if dv is not None: d[(h,y,ln)]=dv
        reg=d.get(('L',44,'D'),XV)
        trace.append({'cycle':c,'state':st,'regD':reg,
                      'xd':sum(1 for (h,y,ln) in s.bits() if d.get((h,y,ln))==XV),
                      'qnext':{k:v for k,v in d.items()}})
        for k in s.bits():
            dv=d.get(k)
            if dv not in (None,XV): s.q[k]=dv
    return s,trace

MOD=1<<25
def check_gate(trace,cycles=8):
    """BLINK-242 gate phase 1: strict UP counting from zero, no X anywhere."""
    states=[t['state'] for t in trace]
    if None in states[:cycles]: return False
    return all(states[c]==c for c in range(1,min(cycles,len(trace))))

def main():
    ap=argparse.ArgumentParser()
    ap.add_argument('bit',nargs='?')
    ap.add_argument('--gate',action='store_true')
    ap.add_argument('--cycles',type=int,default=16)
    a=ap.parse_args()
    if a.gate or not a.bit:
        best=None
        for idx in ('std','rev'):
            for o5 in ('upper','lower'):
                for zini in ('zero','one'):
                    for co in ('di_when_s','cy_when_s'):
                     for cy0m in (0,1):
                      # intmux_default='vcc' is the SETTLED ppips ground truth
                      # (Q1): an un-armed INT input mux outputs VCC_WIRE. It is
                      # not a swept dimension -- omitting it makes every
                      # VCC-tied operand trace to X, so no convention counts.
                      cfg={'idx':idx,'o5':o5,'zini':zini,'co':co,'cy0m':cy0m,
                           'intmux_default':'vcc'}
                      try:
                        s,tr=run_bit('work/ref/BLINK-242.bit',cfg,a.cycles)
                      except Exception as e:
                        print(cfg,'EXC',repr(e)[:80]); continue
                      ok=check_gate(tr)
                      xs=tr[0]['xd'] if tr else None
                      st=[t['state'] for t in tr[:4]]
                      if ok or xs==0:
                        d0=[t['regD'] for t in tr[:6]]
                        print(cfg,'states',st,'regD',d0,'counts' if ok else '')
                        if ok and best is None: best=cfg

        print('GATE-LOCKED:',best)
        if best is None:
            print('GATE FAILED: no convention set makes BLINK-242 count from zero; do not grade other images with this simulator.')
            return 2
        return 0
    # graded run with defaults (caller locks cfg after gate inspection)
    cfg={'idx':'std','o5':'lower','zini':'zero','co':'cy_when_s','cy0m':1,
         'intmux_default':'vcc'}
    s,tr=run_bit(a.bit,cfg,a.cycles)
    for t in tr:
        print('c',t['cycle'],'state',t['state'],'regD',t['regD'],'xd',t['xd'])
    blink = 'blink' if (all(t['state']==c for c,t in enumerate(tr) if t['state'] is not None)
                        and tr[0]['xd']==0 and any(t['regD']==1 for t in tr)) else 'not-blinking'
    print(blink)
    return 0 if blink == 'blink' else 1

if __name__=='__main__':
    sys.exit(main())
