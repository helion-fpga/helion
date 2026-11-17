#!/usr/bin/env python3
"""svcheck: does Helion's synthesized netlist behave like the RTL?

    python3 tools/svcheck/svcheck.py design.sv [more.sv ...] [--top NAME]
                                     [--cycles N] [--seed N] [--work DIR]
    python3 tools/svcheck/svcheck.py --corpus DIR     # every *.sv in DIR

The design is synthesized with Helion's own front end (the `hnf` example of
helion-cli), the netlist is written out as gate-level Verilog, and both are
simulated side by side in Verilator under the same random inputs. Outputs are
compared every clock cycle. Registers start at 0 on both sides, as they do on
the FPGA.

Nothing is guessed: an output bit Helion gives no driver for, a net that is
read but never driven, a cell this script cannot model, or a net with two
drivers is a failure, reported by name.

Needs `verilator` (5.x) on PATH and a release build of the `hnf` example:
    cargo build --release -p helion-cli --example hnf

Device primitives (block RAM Helion infers with HELION_V2_HARD=1, or ones the
RTL instantiates) are simulated from Verilog models found by module name in
`--models DIR` (repeatable; also SVCHECK_MODELS, colon separated), then in
tools/svcheck/models, which holds Helion's own models of a few primitives.
"""
from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
HNF = os.environ.get("SVCHECK_HNF", os.path.join(ROOT, "target/release/examples/hnf"))
W = 1024  # width of every port vector in the bench
MODELS = [os.path.join(os.path.dirname(os.path.abspath(__file__)), "models")]

# Device primitives a netlist may hold as BlackBox cells: port -> (output, width).
def _ports(ins, outs):
    d = {}
    for spec, out in ((ins, False), (outs, True)):
        for p in spec.split():
            name, _, w = p.partition(":")
            d[name] = (out, int(w or 1))
    return d


_RAMB18 = _ports("ADDRARDADDR:14 ADDRBWRADDR:14 CLKARDCLK CLKBWRCLK DIADI:16 DIBDI:16 DIPADIP:2 DIPBDIP:2 ENARDEN ENBWREN REGCEAREGCE REGCEB "
                 "RSTRAMARSTRAM RSTRAMB RSTREGARSTREG RSTREGB WEA:2 WEBWE:4", "DOADO:16 DOBDO:16 DOPADOP:2 DOPBDOP:2")
_RAMB36 = _ports("ADDRARDADDR:16 ADDRBWRADDR:16 CASCADEINA CASCADEINB CLKARDCLK CLKBWRCLK DIADI:32 DIBDI:32 DIPADIP:4 DIPBDIP:4 ENARDEN ENBWREN "
                 "INJECTDBITERR INJECTSBITERR REGCEAREGCE REGCEB RSTRAMARSTRAM RSTRAMB RSTREGARSTREG RSTREGB WEA:4 WEBWE:8",
                 "CASCADEOUTA CASCADEOUTB DBITERR DOADO:32 DOBDO:32 DOPADOP:4 DOPBDOP:4 ECCPARITY:8 RDADDRECC:9 SBITERR")
PRIMS = {
    "RAMB18E1": _RAMB18,
    "RAMB36E1": _RAMB36,
    "DSP48E1": _ports("A:30 ACIN:30 ALUMODE:4 B:18 BCIN:18 C:48 CARRYCASCIN CARRYIN CARRYINSEL:3 CEA1 CEA2 CEAD CEALUMODE CEB1 CEB2 CEC CECARRYIN "
                      "CECTRL CED CEINMODE CEM CEP CLK D:25 INMODE:5 MULTSIGNIN OPMODE:7 PCIN:48 RSTA RSTALLCARRYIN RSTALUMODE RSTB RSTC RSTCTRL RSTD "
                      "RSTINMODE RSTM RSTP", "ACOUT:30 BCOUT:18 CARRYCASCOUT CARRYOUT:4 MULTSIGNOUT OVERFLOW P:48 PATTERNBDETECT PATTERNDETECT PCOUT:48 UNDERFLOW"),
    "MMCME2_ADV": _ports("CLKFBIN CLKIN1 CLKIN2 CLKINSEL DADDR:7 DCLK DEN DI:16 DWE PSCLK PSEN PSINCDEC PWRDWN RST",
                         "CLKFBOUT CLKFBOUTB CLKFBSTOPPED CLKINSTOPPED CLKOUT0 CLKOUT0B CLKOUT1 CLKOUT1B CLKOUT2 CLKOUT2B CLKOUT3 CLKOUT3B CLKOUT4 "
                         "CLKOUT5 CLKOUT6 DO:16 DRDY LOCKED PSDONE"),
    "MMCME2_BASE": _ports("CLKFBIN CLKIN1 PWRDWN RST", "CLKFBOUT CLKFBOUTB CLKOUT0 CLKOUT0B CLKOUT1 CLKOUT1B CLKOUT2 CLKOUT2B CLKOUT3 CLKOUT3B "
                          "CLKOUT4 CLKOUT5 CLKOUT6 LOCKED"),
    "PLLE2_ADV": _ports("CLKFBIN CLKIN1 CLKIN2 CLKINSEL DADDR:7 DCLK DEN DI:16 DWE PWRDWN RST",
                        "CLKFBOUT CLKOUT0 CLKOUT1 CLKOUT2 CLKOUT3 CLKOUT4 CLKOUT5 DO:16 DRDY LOCKED"),
    "BUFG": _ports("I", "O"),
    "BUFGCE": _ports("I CE", "O"),
    "IDDR": _ports("C CE D R S", "Q1 Q2"),
    "ODDR": _ports("C CE D1 D2 R S", "Q"),
    "SRL16E": _ports("A0 A1 A2 A3 CE CLK D", "Q"),
    "SRLC32E": _ports("A:5 CE CLK D", "Q Q31"),
    "CARRY4": _ports("CI CYINIT DI:4 S:4", "CO:4 O:4"),
}
CLOCK_PINS = {"CLK", "C", "CLKARDCLK", "CLKBWRCLK"}


def prim_pin(module: str, pin: str):
    """(port, bit) of a primitive's pin `NAME` or `NAME<bit>`, or None."""
    ports = PRIMS.get(module, {})
    if pin in ports and ports[pin][1] == 1:
        return pin, 0
    m = re.match(r"^(.*?)(\d+)$", pin)
    if m and m.group(1) in ports and int(m.group(2)) < ports[m.group(1)][1]:
        return m.group(1), int(m.group(2))
    return None


def param(v: str) -> str:
    """A parameter value as Verilog: numbers as they are, anything else a string."""
    if re.fullmatch(r"-?\d+(\.\d+)?|\d+'[sS]?[hHbBdDoO][0-9a-fA-F_xXzZ]+", v):
        return v
    return '"' + v.replace('"', '\\"') + '"'


def vid(name: str) -> str:
    return "n_" + re.sub(r"[^A-Za-z0-9_]", "_", name)


class Netlist:
    def __init__(self, text: str):
        self.top = ""
        self.ports: list[tuple[str, str]] = []
        self.cells: dict[str, tuple[str, str]] = {}  # name -> (kind, arg)
        self.order: list[str] = []
        self.attrs: dict[str, dict[str, str]] = {}
        self.dattrs: dict[str, str] = {}
        self.pin_net: dict[tuple[str, str], str] = {}
        self.nets: dict[str, list[tuple[str, str]]] = {}
        for line in text.splitlines():
            f = line.split()
            if not f:
                continue
            if f[0] == "design":
                self.top = f[1]
            elif f[0] == "dattr":
                self.dattrs[f[1]] = " ".join(f[2:])
            elif f[0] == "port":
                self.ports.append((f[1], f[2]))
            elif f[0] == "cell":
                self.cells[f[1]] = (f[2], " ".join(f[3:]))
                self.order.append(f[1])
            elif f[0] == "cattr":
                self.attrs.setdefault(f[1], {})[f[2]] = " ".join(f[3:])
            elif f[0] == "net":
                eps = []
                for ep in f[2:]:
                    cell, _, pin = ep.rpartition("/")
                    eps.append((cell, pin))
                    self.pin_net[(cell, pin)] = f[1]
                self.nets.setdefault(f[1], []).extend(eps)


OUT_PINS = {"Lut6": {"O"}, "Hff": {"Q"}, "IobIn": {"O"}, "ClockBuf": {"O"}, "Carry4": {"O0", "O1", "O2", "O3", "CO"}}


def build(nl: Netlist, widths: dict[str, int]):
    """Return (verilog, problems, clock ports, output map)."""
    problems: list[str] = []
    in_ports = [p for p, d in nl.ports if d in ("in", "inout")]
    out_ports = [p for p, d in nl.ports if d == "out"]

    def port_bit(net: str):
        for p in in_ports:
            w = widths.get(p, 1)
            if net == p and w == 1:
                return p, 0
            if net.startswith(p + "_") and net[len(p) + 1:].isdigit() and int(net[len(p) + 1:]) < max(w, 1):
                return p, int(net[len(p) + 1:])
        return None

    drivers: dict[str, list[str]] = {}
    for net, eps in nl.nets.items():
        for cell, pin in eps:
            kind, arg = nl.cells.get(cell, ("?", ""))
            pp = prim_pin(arg, pin) if kind == "BlackBox" else None
            if pin in OUT_PINS.get(kind, ()) or (pp and PRIMS[arg][pp[0]][0]):
                drivers.setdefault(net, []).append(f"{cell}.{pin}")
    # An IobOut inside the hierarchy passes its I net on to its PAD net.
    for cell in nl.order:
        if nl.cells[cell][0] != "IobOut":
            continue
        i, pad = nl.pin_net.get((cell, "I")), nl.pin_net.get((cell, "PAD"))
        if i and pad and i != pad and pad not in out_ports:
            drivers.setdefault(pad, []).append(f"{cell}.PAD")
    for net, d in sorted(drivers.items()):
        if len(d) > 1:
            problems.append(f"net {net} has {len(d)} drivers: {', '.join(d[:4])}")

    v: list[str] = []
    body: list[str] = []
    used_inputs: set[tuple[str, int]] = set()
    declared: set[str] = set()

    def ref(net: str | None) -> str:
        if net is None:
            return "1'b0"
        pb = port_bit(net)
        if net not in drivers and pb:
            used_inputs.add(pb)
            return f"in_{pb[0]}[{pb[1]}]"
        if net not in drivers:
            if net not in declared:
                problems.append(f"net {net} is read but nothing drives it")
                declared.add(net)
                body.append(f"wire {vid(net)} = 1'b0;")
            return vid(net)
        return vid(net)

    clocks: set[str] = set()

    def clock_root(net: str | None, depth=0) -> str | None:
        if net is None or depth > 8:
            return None
        pb = port_bit(net)
        if net not in drivers and pb:
            return pb[0]
        for d in drivers.get(net, []):
            cell = d.split(".")[0]
            kind = nl.cells[cell][0]
            if kind in ("IobIn", "ClockBuf") or (kind == "BlackBox" and nl.cells[cell][1] in ("BUFG", "BUFGCE")):
                return clock_root(nl.pin_net.get((cell, "I")) or nl.pin_net.get((cell, "PAD")), depth + 1)
        return None

    for net in drivers:
        if net not in declared:
            declared.add(net)
            d = drivers[net][0]
            cell = d.split(".")[0]
            if nl.cells[cell][0] == "Hff":
                init = 1 if nl.attrs.get(cell, {}).get("INIT") in ("1", "1'b1") else 0
                body.append(f"reg {vid(net)} = 1'b{init};")
            else:
                body.append(f"wire {vid(net)};")
    logic: list[str] = []
    for cell in nl.order:
        kind, arg = nl.cells[cell]
        pin = lambda p: nl.pin_net.get((cell, p))
        if kind == "Lut6":
            o = pin("O")
            if o is None:
                continue
            ins = ", ".join(ref(pin(f"I{i}")) for i in range(5, -1, -1))
            logic.append(f"assign {vid(o)} = 64'h{int(arg, 16):016x} >> {{{ins}}};")
        elif kind == "Hff":
            q = pin("Q")
            if q is None:
                continue
            ck = pin("CLK") or pin("C")
            if ck is None:
                problems.append(f"flop {cell} has no clock")
                continue
            # A clock straight from a pin is toggled by the bench; a clock
            # made by logic is whatever the netlist computes.
            root = clock_root(ck)
            if root is not None:
                clocks.add(root)
            edge = "negedge" if nl.attrs.get(cell, {}).get("CLK_INV") == "1" else "posedge"
            known = {"D", "Q", "CLK", "C", "CE", "SR", "RST", "R", "S", "ARST"}
            extra = [p for (c, p) in nl.pin_net if c == cell and p not in known]
            if extra:
                problems.append(f"flop {cell}: pins {extra} are not modelled")
            d = ref(pin("D"))
            stmt = f"{vid(q)} <= {d};"
            if pin("CE"):
                stmt = f"if ({ref(pin('CE'))}) {stmt}"
            rst = pin("SR") or pin("RST") or pin("R")
            if rst:
                stmt = f"if ({ref(rst)}) {vid(q)} <= 1'b0; else {stmt}"
            if pin("S"):
                stmt = f"if ({ref(pin('S'))}) {vid(q)} <= 1'b1; else {stmt}"
            if pin("ARST"):
                # Asynchronous set/reset: level ARST_LEVEL forces ARST_VAL.
                at = nl.attrs.get(cell, {})
                level = at.get("ARST_LEVEL", "1")
                aedge = "posedge" if level == "1" else "negedge"
                a = ref(pin("ARST"))
                logic.append(f"always @({edge} {ref(ck)} or {aedge} {a}) if ({a} == 1'b{level}) {vid(q)} <= 1'b{at.get('ARST_VAL', '0')}; else begin {stmt} end")
            else:
                logic.append(f"always @({edge} {ref(ck)}) {stmt}")
        elif kind == "IobOut":
            i, pad = pin("I"), pin("PAD")
            if i and pad and i != pad and pad not in out_ports:
                logic.append(f"assign {vid(pad)} = {ref(i)};")
        elif kind in ("IobIn", "ClockBuf"):
            o = pin("O")
            src = pin("I") or pin("PAD")
            if o and src and o != src:
                logic.append(f"assign {vid(o)} = {ref(src)};")
        elif kind == "BlackBox" and arg in PRIMS:
            # An instance of the primitive's model, bit for bit.
            buses: dict[str, dict[int, str]] = {}
            for (c, p), net in nl.pin_net.items():
                if c != cell:
                    continue
                pp = prim_pin(arg, p)
                if pp is None:
                    problems.append(f"cell {cell} ({arg}): pin {p} is not a port of the primitive")
                    continue
                buses.setdefault(pp[0], {})[pp[1]] = net
            conns = []
            for port, bits in sorted(buses.items()):
                out, w = PRIMS[arg][port]
                if port in CLOCK_PINS and not out:
                    root = clock_root(bits.get(0))
                    if root is not None:
                        clocks.add(root)
                parts = []
                for i in range(w - 1, -1, -1):
                    net = bits.get(i)
                    if out:
                        if net is None:
                            net = f"{cell}__{port}_{i}"
                            body.append(f"wire {vid(net)};")
                        parts.append(vid(net))
                    else:
                        parts.append(ref(net) if net is not None else "1'b0")
                conns.append(f".{port}({{{', '.join(parts)}}})")
            params = ", ".join(f".{k}({param(v)})" for k, v in sorted(nl.attrs.get(cell, {}).items()))
            logic.append(f"{arg} {'#(' + params + ') ' if params else ''}{vid(cell)} ({', '.join(conns)});")
        elif kind == "Carry4":
            # O = S ^ carry; carry out = S ? carry : DI. Carry in from CI or CYINIT.
            ci = pin("CI")
            c = ref(ci) if ci else f"1'b{1 if nl.attrs.get(cell, {}).get('CYINIT') in ('1', 'true', 'TRUE') else 0}"
            for j in range(4):
                s, o = pin(f"S{j}"), pin(f"O{j}")
                if s is None:
                    break
                if o:
                    logic.append(f"assign {vid(o)} = {ref(s)} ^ {c};")
                nxt = f"{vid(cell)}_c{j}"
                body.append(f"wire {nxt};")
                logic.append(f"assign {nxt} = {ref(s)} ? {c} : {ref(pin(f'DI{j}'))};")
                c = nxt
            if pin("CO"):
                logic.append(f"assign {vid(pin('CO'))} = {c};")
        else:
            problems.append(f"cell {cell} of kind {kind} is not modelled")

    # Output bits. A bit is the net named for it when that net is driven;
    # otherwise the port's pads are taken in order when there is exactly
    # one per bit.
    outs: dict[str, list[str | None]] = {}
    for p in out_ports:
        w = max(widths.get(p, 1), 1)
        pads = [nl.pin_net.get((c, "I")) for c in nl.order if nl.cells[c][0] == "IobOut" and nl.pin_net.get((c, "PAD")) == p]
        by_attr = {int(nl.attrs[c]["BIT"]): nl.pin_net.get((c, "I")) for c in nl.order
                   if nl.cells[c][0] == "IobOut" and nl.attrs.get(c, {}).get("PORT") == p and nl.attrs[c].get("BIT", "").isdigit()}
        bits: list[str | None] = []
        for i in range(w):
            name = p if w == 1 else f"{p}_{i}"
            if by_attr.get(i) is not None:
                bits.append(by_attr[i])
            elif name in drivers:
                bits.append(name)
            elif len(pads) == w and pads[i] is not None:
                bits.append(pads[i])
            else:
                bits.append(None)
        missing = [i for i, b in enumerate(bits) if b is None]
        if missing:
            problems.append(f"output {p}: no driver for bit(s) {missing[:16]}{' ...' if len(missing) > 16 else ''} ({len(pads)} pads for {w} bits)")
        outs[p] = bits
    for p in out_ports:
        for i, b in enumerate(outs[p]):
            if b is not None:
                logic.append(f"assign o_{p}[{i}] = {ref(b)};")
                logic.append(f"assign k_{p}[{i}] = 1'b1;")
    ports = [f"input wire [{W - 1}:0] in_{p}" for p in in_ports]
    ports += [f"output wire [{W - 1}:0] o_{p}" for p in out_ports]
    ports += [f"output wire [{W - 1}:0] k_{p}" for p in out_ports]
    v.append("module helion_netlist(" + ", ".join(ports) + ");")
    v += body + logic
    for p in out_ports:
        n = len(outs[p])
        v.append(f"assign o_{p}[{W - 1}:{n}] = 0; assign k_{p}[{W - 1}:{n}] = 0;")
        for i, b in enumerate(outs[p]):
            if b is None:
                v.append(f"assign o_{p}[{i}] = 1'b0; assign k_{p}[{i}] = 1'b0;")
    v.append("endmodule")
    return "\n".join(v) + "\n", problems, clocks


RESET_RE = re.compile(r"(rst|reset|clr|clear)", re.I)


def bench(nl: Netlist, clocks: set[str], cycles: int, seed: int, with_netlist: bool, regs=None, pulses=True) -> str:
    ins = [p for p, d in nl.ports if d in ("in", "inout")]
    outs = [p for p, d in nl.ports if d == "out"]
    for p in ins:
        if not clocks and re.search(r"(^|_)(clk|clock|ck)", p, re.I):
            clocks = clocks | {p}
    data = [p for p in ins if p not in clocks]
    t = ["`timescale 1ns/1ps", "module svcheck_tb;", "integer k, j;"]
    if regs:
        t.append(f"reg [{len(regs) - 1}:0] seen = 0;")
    for p in ins:
        t.append(f"reg [{W - 1}:0] in_{p} = 0;")
    for p in outs:
        t.append(f"wire [{W - 1}:0] r_{p};")
        t.append(f"assign r_{p} = dut.\\{p} ;")
        if with_netlist:
            t.append(f"wire [{W - 1}:0] o_{p}; wire [{W - 1}:0] k_{p};")
    # Escaped identifiers: a Verilog port may be named with an SV keyword.
    conn = ", ".join(f".\\{p} (in_{p}[$bits(dut.\\{p} )-1:0])" for p in ins)
    t.append(f"{nl.top} dut({conn});")
    if with_netlist:
        conn = ", ".join([f".in_{p}(in_{p})" for p in ins] + [f".o_{p}(o_{p})" for p in outs] + [f".k_{p}(k_{p})" for p in outs])
        t.append(f"helion_netlist net({conn});")
    t.append("initial begin")
    for p in ins + outs:
        t.append(f'  $display("W {p} %0d", $bits(dut.\\{p} ));')
    if with_netlist:
        t.append(f"  j = $urandom({seed});")
        for p in data:
            if RESET_RE.search(p):
                low = bool(re.search(r"(_n|_ni|n|_b|_l)$", p, re.I))
                t.append(f"  in_{p} = {0 if low else 1};")
        # Let the reset values settle before the first clock edge.
        t.append("  #5;")
        t.append(f"  for (k = 0; k < {cycles}; k = k + 1) begin")
        for c in sorted(clocks):
            t.append(f"    in_{c} = 1;")
        t.append("    #1;")
        fmt = " ".join(["%0d"] + ["%h %h %h"] * len(outs))
        args = ", ".join(["k"] + [f"r_{p}, o_{p}, k_{p}" for p in outs])
        t.append(f'    $display("S {fmt}", {args});')
        for i, (net, hier, idx, inv) in enumerate(regs or []):
            ref = f"dut.{hier}" if idx == "s" else f"dut.{hier}[{idx}]"
            t.append(f'    if (!seen[{i}] && ({ref} !== (net.{vid(net)} ^ 1\'b{inv}))) begin seen[{i}] = 1; $display("R %0d {hier}{"" if idx == "s" else "[" + idx + "]"}", k); end')
        t.append("    #4;")
        for c in sorted(clocks):
            t.append(f"    in_{c} = 0;")
        for p in data:
            if RESET_RE.search(p):
                low = bool(re.search(r"(_n|_ni|n|_b|_l)$", p, re.I))
                on, off = (0, 1) if low else (1, 0)
                again = " || ($urandom % 61) == 0" if pulses else ""
                t.append(f"    in_{p} = (k < 3{again}) ? {on} : {off};")
            else:
                t.append(f"    for (j = 0; j < {W}; j = j + 32) in_{p}[j +: 32] = $urandom;")
        t.append("    #5;")
        t.append("  end")
    t.append("  $finish;")
    t.append("end")
    t.append("endmodule")
    return "\n".join(t) + "\n"


def verilate(work: str, files: list[str], tag: str):
    obj = os.path.join(work, "obj_" + tag)
    shutil.rmtree(obj, ignore_errors=True)
    cmd = ["verilator", "--binary", "--timing", "-O0", "--x-initial", "0", "--x-assign", "0", "-Wno-fatal", "-Wno-lint",
           "-Wno-WIDTH", "-Wno-PINMISSING", "-Wno-MULTIDRIVEN", "-Wno-LATCH", "-Wno-UNOPTFLAT", "+1364-2005ext+v", "--top-module", "svcheck_tb", "--Mdir", obj, "-o", "sim"]
    for d in MODELS:
        cmd += ["-y", d]
    cmd += ["+libext+.v+.sv"] + files
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=1800)
    if r.returncode != 0:
        err = [l for l in r.stderr.splitlines() if "%Error" in l]
        return None, (err or r.stderr.splitlines() or ["verilator failed"])[0]
    r = subprocess.run([os.path.join(obj, "sim")], capture_output=True, text=True, timeout=1800)
    return r.stdout, ""


def check(files: list[str], top: str | None, cycles: int, seed: int, work: str, regs: bool = False) -> tuple[bool, list[str]]:
    os.makedirs(work, exist_ok=True)
    src = files[0]
    if len(files) > 1:
        src = os.path.join(work, "all.sv")
        with open(src, "w") as f:
            for p in files:
                f.write(open(p).read() + "\n")
    regmap = os.path.join(work, "regmap.txt")
    r = subprocess.run([HNF] + (['--legacy'] if os.environ.get('SVCHECK_LEGACY') else []) + [src] + (['--top', top] if top else [])
                       + (['--regmap', regmap] if regs else []), capture_output=True, text=True, timeout=1800)
    if r.returncode != 0:
        tail = [l for l in r.stderr.splitlines() if l.strip()][-1:] or ["synthesis failed"]
        return False, [f"synthesis: {tail[0][:300]}"]
    nl = Netlist(r.stdout)
    notes = []
    # Asynchronous resets are built as synchronous ones (the fabric's flops
    # have no asynchronous input). The two agree when reset is applied at
    # start-up, which is what is checked; a reset pulsed between clock edges
    # of a running design reaches non-reset registers one edge apart.
    async_as_sync = "is built as synchronous" in r.stderr and not os.environ.get("SVCHECK_PULSES")
    soft = [k for k in ("SIM_ONLY", "NO_BODY", "WIDE_CONE", "ASSIGN_NOT_LOWERED", "GENERATE_NOT_LOWERED", "FLATTEN_CAP",
                        "WIDTH_OVERFLOW", "WORD_PIPELINE_CAP", "GATE_PRIMITIVE") if nl.dattrs.get(k) == "1"]
    if soft:
        notes.append(f"synthesis says part of the design was not lowered: {', '.join(soft)}")
    # Pass 1: port widths from the RTL itself.
    tb0 = os.path.join(work, "tb_widths.sv")
    open(tb0, "w").write(bench(nl, set(), 0, seed, False))
    out, err = verilate(work, [src, tb0], "w")
    if out is None:
        return False, notes + [f"reference simulator rejects the RTL: {err[:300]}"]
    widths = {m.group(1): int(m.group(2)) for m in re.finditer(r"^W (\S+) (\d+)$", out, re.M)}
    vtext, problems, clocks = build(nl, widths)
    notes += problems
    nv = os.path.join(work, "helion_netlist.v")
    open(nv, "w").write(vtext)
    tb = os.path.join(work, "tb.sv")
    reg_list = None
    if regs and os.path.exists(regmap):
        # Flops whose source register is a scalar or a bit of a plain vector.
        reg_list = [tuple(l.split()) for l in open(regmap) if l.split()[2] != "-"]
    open(tb, "w").write(bench(nl, clocks, cycles, seed, True, reg_list, pulses=not async_as_sync))
    out, err = verilate(work, [src, nv, tb], "c")
    if out is None:
        return False, notes + [f"co-simulation did not build: {err[:300]}"]
    outs = [p for p, d in nl.ports if d == "out"]
    bad: dict[str, tuple[int, int]] = {}
    compared = 0
    for line in out.splitlines():
        if not line.startswith("S "):
            continue
        f = line.split()
        k = int(f[1])
        for i, p in enumerate(outs):
            r_, o_, m_ = (int(x, 16) for x in f[2 + 3 * i: 5 + 3 * i])
            mask = m_ & ((1 << widths.get(p, 1)) - 1)
            diff = (r_ ^ o_) & mask
            compared += bin(mask).count("1")
            b = 0
            while diff:
                if diff & 1:
                    key = f"{p}[{b}]"
                    first, n = bad.get(key, (k, 0))
                    bad[key] = (first, n + 1)
                diff >>= 1
                b += 1
    if reg_list is not None:
        firsts = sorted((int(l.split()[1]), l.split()[2]) for l in out.splitlines() if l.startswith("R "))
        for cyc, name in firsts[:20]:
            notes.append(f"register {name} first differs from the RTL at cycle {cyc}")
        if len(firsts) > 20:
            notes.append(f"... and {len(firsts) - 20} more register bits differ")
    for key, (first, n) in sorted(bad.items(), key=lambda kv: kv[1][0])[:12]:
        notes.append(f"{key} differs from the RTL in {n} of {cycles} cycles (first at cycle {first})")
    if len(bad) > 12:
        notes.append(f"... and {len(bad) - 12} more output bits differ")
    if compared == 0 and outs:
        notes.append("no output bit could be compared")
    ok = not notes
    tail = " (reset held at start-up only: asynchronous resets are built as synchronous)" if async_as_sync else ""
    return ok, notes + ([] if not ok else [f"{compared} output samples equal over {cycles} cycles{tail}"])


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("files", nargs="*")
    ap.add_argument("--top")
    ap.add_argument("--corpus")
    ap.add_argument("--cycles", type=int, default=400)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--work", default=os.path.join(ROOT, "target", "svcheck"))
    ap.add_argument("-j", type=int, default=6)
    ap.add_argument("--regs", action="store_true", help="also compare every flop with its source register, each cycle")
    ap.add_argument("--models", action="append", default=[], help="directory of Verilog models of device primitives (module.v), searched first")
    a = ap.parse_args()
    MODELS[:0] = a.models + [d for d in os.environ.get("SVCHECK_MODELS", "").split(":") if d]
    if not os.path.exists(HNF):
        sys.exit(f"missing {HNF}: cargo build --release -p helion-cli --example hnf")
    jobs = []
    if a.corpus:
        for f in sorted(os.listdir(a.corpus)):
            if f.endswith(".sv") or f.endswith(".v"):
                jobs.append(([os.path.join(a.corpus, f)], None))
    if a.files:
        jobs.append((a.files, a.top))
    if not jobs:
        ap.error("no design given")

    def run(job):
        files, top = job
        name = os.path.splitext(os.path.basename(files[0]))[0]
        try:
            return name, check(files, top, a.cycles, a.seed, os.path.join(a.work, name), a.regs)
        except subprocess.TimeoutExpired:
            return name, (False, ["timed out"])

    fails = 0
    with ThreadPoolExecutor(max_workers=a.j) as ex:
        for name, (ok, notes) in ex.map(run, jobs):
            print(f"{'PASS' if ok else 'FAIL'}  {name}")
            for n in notes:
                print(f"      {n}")
            fails += not ok
    print(f"{len(jobs) - fails} of {len(jobs)} designs equal their RTL")
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
