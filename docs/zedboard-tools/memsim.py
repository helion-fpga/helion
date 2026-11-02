"""Simulate the register-file READ path on the HNF netlist.

Checks the property the earlier round left untested: that `assign dout = mem[addr]`
computes the RIGHT WORD, not merely that it produces some driver.

Method: drive a synthetic table straight into the storage flops (`mem_w<k>_<b>`),
settle the combinational netlist to a fixpoint, and read `dout`. This isolates
the read mux -- the write side is exercised separately by the same flops.

Run: python3 work/memsim.py
"""
import os, subprocess, sys
from collections import defaultdict

import os as _os
HELION = _os.environ.get("HELION_REPO", ".")
DB = _os.environ.get("HELION_XILINX_DB", "work/prjxray-db")
sys.argv = ["memsim"]

SRC = """
module memled(input logic clk, input logic we, input logic [3:0] addr,
              input logic [7:0] din, output logic [7:0] led);
  logic [7:0] mem [0:15];
  always_ff @(posedge clk) if (we) mem[addr] <= din;
  assign led = mem[addr];
endmodule
"""


def hnf(sv):
    import tempfile
    with tempfile.NamedTemporaryFile("w", suffix=".sv", delete=False) as f:
        f.write(sv)
        path = f.name
    r = subprocess.run(
        ["./target/release/helion", "hnf", path],
        cwd=HELION, capture_output=True, text=True,
        env={**os.environ, "HELION_XILINX_DB": DB},
    )
    os.unlink(path)
    if r.returncode != 0:
        raise SystemExit("hnf failed: " + r.stderr[-2000:])
    return r.stdout


def parse(text):
    cells, conns = {}, defaultdict(list)
    for line in text.splitlines():
        p = line.split()
        if not p:
            continue
        if p[0] == "cell":
            init = int(p[3], 16) if len(p) > 3 and p[3].startswith("0x") else None
            cells[p[1]] = {"kind": p[2], "init": init}
        elif p[0] == "net":
            for c in p[2:]:
                cell, pin = c.split("/", 1)
                conns[cell].append((pin, p[1]))
    return cells, conns


def lut6(init, iv):
    idx = 0
    for i in range(6):
        if iv.get(f"I{i}", False):
            idx |= 1 << i
    return bool((init >> idx) & 1)


def drive(cells, conns, mem, addr, wraps=400):
    """Fixpoint-settle the combinational part with memory and address forced.

    NOTE the output of a gate is the net on its `O` pin, which is NOT named
    after the cell (`u_wshare_l41/O` drives `u_wshare_n41`). Matching outputs
    by cell name silently reads nothing.
    """
    val = {}
    for (w, b), v in mem.items():
        val[f"mem_w{w}_{b}"] = v
    for b in range(8):
        val[f"addr_{b}"] = bool((addr >> b) & 1)
    luts = [c for c in cells if cells[c]["kind"].startswith("Lut")]
    for _ in range(wraps):
        changed = False
        for l in luts:
            iv = {pin: val.get(net, False) for pin, net in conns[l] if pin != "O"}
            o = lut6(cells[l]["init"], iv)
            for pin, net in conns[l]:
                if pin == "O" and val.get(net) != o:
                    val[net] = o
                    changed = True
        if not changed:
            return val
    raise SystemExit("netlist did not settle to a fixpoint")


def dout(cells, conns, val, bit):
    """The combinational value reaching `led_<bit>`'s register.

    `led_<bit>` is driven by the output register's Q, which `drive()` never
    evaluates (it settles COMBINATIONAL cells only), so read the register's D
    input: that is the read-mux result, which is the property under test.
    """
    net = f"led_{bit}"
    for cell, pins in conns.items():
        for pin, n in pins:
            if n != net:
                continue
            if pin == "Q":
                d = [n2 for p2, n2 in conns[cell] if p2 == "D"]
                if not d:
                    return ("ff-nodriver", cell, None)
                return ("ff", cell, val.get(d[0]))
            if pin == "O":
                return ("lut", cell, val.get(net))
    return ("none", None, None)


def main():
    text = hnf(SRC)
    cells, conns = parse(text)
    print(f"cells={len(cells)} luts={sum(1 for c in cells if cells[c]['kind'].startswith('Lut'))} "
          f"ffs={sum(1 for c in cells if cells[c]['kind'] in ('Hff','HFF'))}")

    # A table where every word is a distinguishable function of its index, so a
    # mux wired to the wrong word or the wrong address bit cannot pass.
    table = {w: [(w ^ (w >> 2) ^ 0x5) & 0xFF for _ in range(1)][0] for w in range(16)}
    mem = {(w, b): bool((table[w] >> b) & 1) for w in range(16) for b in range(8)}

    failures = 0
    for addr in range(16):
        val = drive(cells, conns, mem, addr)
        got = 0
        kinds = set()
        for b in range(8):
            k, c, v = dout(cells, conns, val, b)
            kinds.add(k)
            if v:
                got |= 1 << b
        ok = got == table[addr]
        if not ok:
            failures += 1
        print(f"  addr={addr:2d} expect=0x{table[addr]:02x} got=0x{got:02x} {'' if ok else '<-- MISMATCH'}")

    # Negative control: the check must be able to FAIL. Corrupt one word and
    # confirm the same comparison reports a mismatch.
    bad = dict(mem)
    bad[(7, 0)] = not bad[(7, 0)]
    val = drive(cells, conns, bad, 7)
    got = 0
    for b in range(8):
        _, _, v = dout(cells, conns, val, b)
        if v:
            got |= 1 << b
    control = got != table[7]
    print(f"negative control: corrupting mem[7] bit0 is detected: {control}")

    if failures or not control:
        print(f"FAIL: {failures} wrong words, control={control}")
        return 1
    print("ALL 16 WORDS READ CORRECTLY")
    return 0


if __name__ == "__main__":
    sys.exit(main())
