#!/usr/bin/env python3
"""Does Helion's Vivado export simulate equal to the RTL?

    python3 tools/vivado_export/simcheck.py OUT.v RTL.sv [more.sv ...] [--top T] [--cycles N] [--seed N] [--no-pulses]

OUT.v is the netlist `zedbit --vivado=OUT` writes. It is simulated with the
behavioural primitive models in prims.v (written here, not Xilinx's), side
by side with the RTL in Verilator, under the same random inputs. Every
output bit the netlist has is compared after every clock edge. Clocks are
the ports OUT.xdc puts `create_clock` on; inputs named like a reset are held
active for the first cycles and pulsed now and then, like svcheck does.

Exit status 0 only when no output bit ever differs.
"""
from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
W = 1024
RESET_RE = re.compile(r"(rst|reset|clr|clear)", re.I)


def ports_of(netlist: str, top: str):
    m = re.search(rf"module {re.escape(top)} \((.*?)\);", netlist, re.S)
    if not m:
        sys.exit(f"no module {top} in the netlist")
    out = []
    for line in m.group(1).split(","):
        f = re.match(r"\s*(input|output|inout) wire (?:\[(\d+):0\] )?(\w+)", line)
        if f:
            out.append((f.group(3), f.group(1), int(f.group(2) or 0) + 1))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("netlist")
    ap.add_argument("rtl", nargs="+")
    ap.add_argument("--top")
    ap.add_argument("--cycles", type=int, default=5000)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--work")
    ap.add_argument("--no-pulses", action="store_true", help="reset only at start-up, never pulsed again")
    a = ap.parse_args()
    text = open(a.netlist).read()
    top = a.top or re.search(r"^module (\w+)", text, re.M).group(1)
    work = a.work or os.path.splitext(a.netlist)[0] + "_simcheck"
    os.makedirs(work, exist_ok=True)
    ports = ports_of(text, top)
    xdc = os.path.splitext(a.netlist)[0] + ".xdc"
    clocks = set()
    for line in open(xdc):
        if line.startswith("create_clock"):
            clocks |= set(re.findall(r"get_ports \{?(\w+)", line))
    ins = [(p, w) for p, d, w in ports if d == "input"]
    outs = [(p, w) for p, d, w in ports if d == "output"]
    data = [p for p, _ in ins if p not in clocks]
    nv = os.path.join(work, "netlist.v")
    open(nv, "w").write(re.sub(rf"^module {re.escape(top)} \(", f"module {top}__vx (", text, flags=re.M))

    t = ["`timescale 1ns/1ps", "module vx_tb;", "integer k, j, bad = 0;"]
    for p, _ in ins:
        t.append(f"reg [{W - 1}:0] in_{p} = 0;")
    for p, w in outs:
        t.append(f"wire [{W - 1}:0] r_{p}; assign r_{p} = dut.\\{p} ;")
        t.append(f"wire [{w - 1}:0] n_{p};")
    conn = ", ".join(f".\\{p} (in_{p}[$bits(dut.\\{p} )-1:0])" for p, _ in ins)
    t.append(f"{top} dut({conn});")
    conn = ", ".join([f".{p}(in_{p}[{w - 1}:0])" for p, w in ins] + [f".{p}(n_{p})" for p, _ in outs])
    t.append(f"{top}__vx net({conn});")
    t.append("initial begin")
    t.append(f"  j = $urandom({a.seed});")
    for p in data:
        if RESET_RE.search(p):
            low = bool(re.search(r"(_n|_ni|n|_b|_l)$", p, re.I))
            t.append(f"  in_{p} = {0 if low else 1};")
    t.append("  #5;")
    t.append(f"  for (k = 0; k < {a.cycles}; k = k + 1) begin")
    for c in sorted(clocks):
        t.append(f"    in_{c} = 1;")
    t.append("    #1;")
    for p, w in outs:
        t.append(f'    if (r_{p}[{w - 1}:0] !== n_{p}) begin bad = bad + 1; if (bad <= 20) $display("DIFF %0d {p} rtl=%h net=%h", k, r_{p}[{w - 1}:0], n_{p}); end')
    t.append("    #4;")
    for c in sorted(clocks):
        t.append(f"    in_{c} = 0;")
    for p in data:
        if RESET_RE.search(p):
            low = bool(re.search(r"(_n|_ni|n|_b|_l)$", p, re.I))
            on, off = (0, 1) if low else (1, 0)
            again = "" if a.no_pulses else " || ($urandom % 61) == 0"
            t.append(f"    in_{p} = (k < 3{again}) ? {on} : {off};")
        else:
            t.append(f"    for (j = 0; j < {W}; j = j + 32) in_{p}[j +: 32] = $urandom;")
    t.append("    #5;")
    t.append("  end")
    t.append(f'  $display("DONE %0d cycles, %0d cycles with a differing output", {a.cycles}, bad);')
    for p, w in outs:
        t.append(f'  $display("FINAL {p} = %h", n_{p});')
    t.append("  $finish;")
    t.append("end")
    t.append("endmodule")
    tb = os.path.join(work, "tb.sv")
    open(tb, "w").write("\n".join(t) + "\n")
    obj = os.path.join(work, "obj")
    cmd = ["verilator", "--binary", "--timing", "-O1", "--x-initial", "0", "--x-assign", "0", "-Wno-fatal", "-Wno-lint",
           "-Wno-WIDTH", "-Wno-PINMISSING", "-Wno-MULTIDRIVEN", "-Wno-LATCH", "-Wno-UNOPTFLAT", "-Wno-MULTITOP",
           "+1364-2005ext+v", "--top-module", "vx_tb", "--Mdir", obj, "-o", "sim", os.path.join(HERE, "prims.v"), nv, tb] + a.rtl
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        print("\n".join([l for l in r.stderr.splitlines() if "%Error" in l][:20]) or r.stderr[-2000:])
        sys.exit("verilator failed")
    r = subprocess.run([os.path.join(obj, "sim")], capture_output=True, text=True)
    print(r.stdout.strip())
    m = re.search(r"DONE (\d+) cycles, (\d+) cycles", r.stdout)
    ok = m is not None and m.group(2) == "0"
    print(f"{'PASS' if ok else 'FAIL'}  {top}: {len(outs)} output ports, {sum(w for _, w in outs)} bits compared each cycle")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
