"""Corrected HNF simulator: settle to FIXPOINT, then sample, then clock."""
import subprocess, sys, os
from collections import defaultdict

def get_hnf(sv):
    r=subprocess.run(["./target/release/helion","hnf",sv],
      cwd="/Users/saksham/Documents/zedboard-helion-debug/helion",
      capture_output=True,text=True,
      env={**os.environ,"HELION_XILINX_DB":"/Users/saksham/Documents/zedboard-helion-debug/work/prjxray-db"})
    return r.stdout

def parse(text):
    cells={}; conns=defaultdict(list)
    for line in text.splitlines():
        p=line.split()
        if not p: continue
        if p[0]=="cell":
            init=int(p[3],16) if len(p)>3 and p[3].startswith("0x") else None
            cells[p[1]]={"kind":p[2],"init":init,"attrs":{}}
        elif p[0]=="cattr":
            # cattr <cell> <k> <v>
            if p[1] in cells: cells[p[1]]["attrs"][p[2]]=" ".join(p[3:])
        elif p[0]=="net":
            for c in p[2:]:
                cell,pin=c.split("/",1); conns[cell].append((pin,p[1]))
    return cells,conns

def lut6(init,iv):
    idx=0
    for i in range(6):
        if iv.get(f"I{i}",False): idx|=(1<<i)
    return bool((init>>idx)&1)

def run(sv, cycles):
    text=get_hnf(sv); cells,conns=parse(text)
    luts=[c for c in cells if cells[c]["kind"].startswith("Lut")]
    ffs =[c for c in cells if cells[c]["kind"] in ("Hff","HFF")]
    # CARRY4 elements, grouped by their chain attr and ordered by carry_index
    # (LSB first). An untagged Carry4 is its own single-element chain.
    carries=[c for c in cells if cells[c]["kind"]=="Carry4"]
    _chains=defaultdict(list)
    for c in carries:
        a=cells[c]["attrs"]
        _chains[a.get("chain",c)].append((int(a.get("carry_index",0)),c))
    carries_by_chain=[[c for _,c in sorted(v)] for v in _chains.values()]
    # FFs whose D net is driven by a CARRY4 sum output.
    _carry_sums={net for c in carries for pin,net in conns[c] if pin.startswith("O")}
    carry_ffs={f for f in ffs if any(pin=="D" and net in _carry_sums for pin,net in conns[f])}
    q={f:False for f in ffs}
    outs=[]
    for cyc in range(cycles):
        val={'clk':True}
        carry_next={}
        for f in ffs:
            for pin,net in conns[f]:
                if pin=="Q": val[net]=q[f]
        # settle to fixpoint
        for _ in range(20):
            changed=False
            # Re-pin the register outputs each pass. In HNF an FF's D and Q are
            # the same net, and a carry writes its sum there, so without this a
            # carry output would feed the very LUTs that produce it.
            for f in ffs:
                for pin,net in conns[f]:
                    if pin=="Q": val[net]=q[f]
            for l in luts:
                iv={pin:val.get(net,False) for pin,net in conns[l] if pin!="O"}
                o=lut6(cells[l]["init"],iv)
                for pin,net in conns[l]:
                    if pin=="O" and val.get(net)!=o: val[net]=o; changed=True
            for f in ffs:
                # HNF ties an FF's D and Q to one net. When a carry drives that
                # net, the carry result IS the next value -- do not overwrite it
                # with the (stale) Q we just read in.
                if f in carry_ffs: continue
                iv={pin:val.get(net,False) for pin,net in conns[f] if pin!="Q"}
                val[f+"/D"]=iv.get("D",False)
            # CARRY4: a 4-lane fast carry chain in one slice. Per lane
            #   O<n> = S ^ cin      (S comes from the {A,B,C,D}S pin)
            #   cout = DI | (S & cin)
            # The chain entry is the element's CIN net, or the PRECYINIT
            # constant on the chain's first element. Evaluating these by the
            # recorded pin names (not by re-deriving the adder) is what makes
            # this an independent check of the emitted netlist.
            for c in carries_by_chain:
                for name in c:
                    cell=cells[name]
                    # Chain entry: an explicit CIN net, else the CYINIT const.
                    cin=None
                    for pin,net in conns[name]:
                        if pin=="CIN": cin=val.get(net,False)
                    if cin is None:
                        ci=cell["attrs"].get("cyinit")
                        cin = (ci=="1") if ci is not None else False
                    for lane,letter in enumerate("ABCD"):
                        snet=None; dinet=None; onet=None
                        for pin,net in conns[name]:
                            if pin==f"{letter}S": snet=net
                            elif pin==f"{letter}DI": dinet=net
                            elif pin==f"O{lane}": onet=net
                        if onet is None: continue
                        S=val.get(snet,False) if snet else False
                        DI=val.get(dinet,False) if dinet else False
                        o = S ^ cin
                        cout = DI or (S and cin)
                        # `onet` is the FF's Q net (HNF ties D and Q to one
                        # net). Writing it directly would clobber the value the
                        # other lanes are still reading this pass, so record it
                        # as the target FF's next D and let the clock edge apply
                        # it -- exactly as a real flop does.
                        # Record the lane result unconditionally: a `False`
                        # is a real result (that bit clears), and skipping it
                        # would leave the FF holding its previous value.
                        if carry_next.get(onet)!=o:
                            carry_next[onet]=o
                            changed=True
                        cin = cout
                    # Publish the carry out on its NET, which is what the next
                    # element's CIN pin is wired to. Keying it by cell name
                    # (`<cell>/COUT`) would never be read, silently dropping the
                    # carry between groups.
                    for pin,net in conns[name]:
                        if pin=="COUT": val[net]=cout
            if not changed: break
        # Publish the sampled state. A carry-driven net was deliberately NOT
        # written into `val` (that would feed back into its own LUTs), so fold
        # the carry results in here for the observer.
        for net,v in carry_next.items(): val[net]=v
        # `val` is reused every cycle, so store a snapshot: appending the dict
        # itself would leave every sample aliasing the final cycle.
        outs.append(dict(val))
        for f in ffs:
            if f in carry_ffs:
                # A carry-driven sum bit: the carry recorded the next value.
                for pin,net in conns[f]:
                    if pin=="D" and net in carry_next:
                        q[f]=carry_next[net]
            else:
                q[f]=val.get(f+"/D",False)
    return outs, ffs

if __name__=="__main__":
    sv=sys.argv[1]; n=int(sys.argv[2]) if len(sys.argv)>2 else 200
    outs,ffs=run(sv,n)
    for bit in [0,5,6,7,12,24]:
        key=f"count_{bit}"
        tr=[1 if o.get(key) else 0 for o in outs]
        ch=sum(1 for a,b in zip(tr,tr[1:]) if a!=b)
        print(f"count_{bit:2}: transitions={ch:4}  first40={''.join(map(str,tr[:40]))}")
    tr=[1 if o.get('LD0') or o.get('u_iob') else 0 for o in outs]
    # output pin is the IOB's I net
    # The design under test is `q <= q + 1` from a power-up 0, so after N clock
    # edges the register must read exactly N. Check every cycle: a carry chain
    # that drops or misroutes a carry fails this at the group boundary (bit 4,
    # 8, 12, ...) rather than at bit 0.
    vals=[]
    for o in outs:
        vals.append(sum((1<<b) for b in range(25) if o.get(f"count_{b}")))
    print()
    print("FULL COUNT SEQUENCE (25-bit int, first 8 cycles):", vals[:8])
    print("  every 64th cycle:", vals[::64][:6])
    expected=list(range(1,len(vals)+1))
    print("  counts 1,2,3,... with no gaps or repeats:", vals==expected)
    if vals!=expected:
        for i,(a,b) in enumerate(zip(vals,expected)):
            if a!=b:
                print(f"  first divergence at cycle {i}: got {a}, want {b}")
                break
