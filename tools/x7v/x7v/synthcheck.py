"""Synthesis-only check: Helion's netlist IR versus the RTL.

Runs Helion's own front end (examples/synth2v.rs), maps the RTL's port bits
onto the IR, wraps it with the board's package pins, and co-simulates it
against the RTL with the same harness the bitstream checks use. Placement,
routing and bitgen are not involved, so a failure here is a synthesis bug.

Port mapping follows Helion's conventions and refuses to guess:
  input  bit i of port p (width w): IR net `p` if w == 1 else `p_i`
  output bit i of port p          : the IobOut cell with attrs PORT=p, BIT=i
                                    (or, for a scalar port, the single IobOut
                                    whose PAD net is p)
"""
from __future__ import annotations

import json
import os
import re
import subprocess
from dataclasses import dataclass, field

from . import cosim

SYNTH2V = os.environ.get("X7V_SYNTH2V", os.path.expanduser("~/helion-zed-verify/target/release/examples/synth2v"))


@dataclass
class SynthReport:
    design: str
    ok: bool = False
    synth_error: str = ""
    cells: dict = field(default_factory=dict)
    problems: list[str] = field(default_factory=list)
    cosim: cosim.CosimResult | None = None

    def summary(self) -> str:
        lines = [f"{self.design}: {'PASS' if self.ok else 'FAIL'}  cells={self.cells}"]
        if self.synth_error:
            lines.append(f"  synthesis error: {self.synth_error[:300]}")
        for p in self.problems[:12]:
            lines.append(f"  - {p}")
        if len(self.problems) > 12:
            lines.append(f"  ... {len(self.problems) - 12} more")
        r = self.cosim
        if r is not None:
            lines.append(f"  cosim: {r.compared} samples, {len(r.mismatches)} mismatches, "
                         f"{r.net_unknown} X/Z from netlist, {r.rtl_unknown} RTL don't-care")
            for k, b, x, y in r.mismatches[:8]:
                lines.append(f"    cycle {k}: {b} rtl={x} helion={y}")
        return "\n".join(lines)


def _combined_source(rtl: list[str], workdir: str, top: str) -> str:
    if len(rtl) == 1:
        return rtl[0]
    ext = os.path.splitext(rtl[0])[1]
    out = os.path.join(workdir, f"{top}{ext}")
    with open(out, "w") as f:
        for p in rtl:
            f.write(open(p).read())
            f.write("\n")
    return out


def run(design: str, rtl: list[str], top: str, xdc: str, workdir: str,
        cycles: int = 3000, seed: int = 1) -> SynthReport:
    os.makedirs(workdir, exist_ok=True)
    rep = SynthReport(design)
    src = _combined_source(rtl, workdir, top)
    v = os.path.join(workdir, "helion_netlist.v")
    j = os.path.join(workdir, "helion_netlist.json")
    cmd = [SYNTH2V, src, v, j] + ([top] if top and not cosim.is_verilog(rtl) else [])
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=1800)
    if r.returncode != 0 or not os.path.exists(j):
        rep.synth_error = (r.stderr.strip().splitlines() or ["synth2v failed"])[-1]
        return rep
    info = json.load(open(j))
    if "error" in info:
        rep.synth_error = info["error"]
        return rep
    rep.cells = info.get("cells", {})
    for k in ("SIM_ONLY", "NO_BODY", "WIDE_CONE", "ASSIGN_NOT_LOWERED", "GENERATE_NOT_LOWERED",
              "WIDTH_OVERFLOW", "FLATTEN_CAP", "CLOCK_MUX", "CLOCK_GATE", "INOUT_ENABLE_NOT_LOWERED",
              "ASYNC_RESET_NOT_LOWERED"):
        if k in info.get("attrs", {}):
            rep.problems.append(f"design flagged {k}: synthesis did not lower part of the RTL")
    for s in info.get("unsupported", []):
        rep.problems.append(f"unsupported cell: {s}")
    for n, drv in info.get("multiply_driven", {}).items():
        rep.problems.append(f"net {n} has several drivers: {drv}")

    pin_of, clocks = cosim.parse_xdc(xdc)
    ports = cosim.sv_ports(rtl, top) if cosim.is_verilog(rtl) else cosim.vhdl_ports(rtl, top)
    undriven = {u["net"]: u["verilog"] for u in info.get("undriven", [])}
    used_inputs = set()
    wrapper_ports, conns = [], []
    for p in ports:
        if p.direction != "in":
            continue
        for i, b in enumerate(reversed(p.bits())):
            idx = (p.lo + i) if p.width else None
            net = p.name if p.width is None or p.width == 1 else f"{p.name}_{idx}"
            pin = pin_of.get(b)
            match = next((k for k in undriven if k.lower() == net.lower()), None)
            if match is None:
                continue  # the netlist never reads this input bit
            used_inputs.add(match)
            if pin is None:
                rep.problems.append(f"input {b} has no PACKAGE_PIN in the XDC")
                continue
            if f"input wire {pin}" not in wrapper_ports:
                wrapper_ports.append(f"input wire {pin}")
            conns.append(f".{undriven[match]}({pin})")
    for n, vname in undriven.items():
        if n not in used_inputs:
            rep.problems.append(f"internal net `{n}` is read but never driven")
            conns.append(f".{vname}(1'bx)")
    iobs = info.get("iobs_out", [])
    outs_by_bit: dict[str, list[dict]] = {}
    for io in iobs:
        a = io.get("attrs", {})
        if "PORT" in a:
            key = f"{a['PORT']}[{a['BIT']}]" if "BIT" in a else a["PORT"]
        else:
            key = io["pad"]
        outs_by_bit.setdefault(key, []).append(io)
    for p in ports:
        if p.direction != "out":
            continue
        for b in p.bits():
            key = b if p.width else p.name
            cands = outs_by_bit.get(key, [])
            if not cands and p.width is not None and p.width == 1:
                cands = outs_by_bit.get(p.name, [])
            if p.width and p.width > 1 and not cands and outs_by_bit.get(p.name):
                rep.problems.append(f"output {b}: IOBs exist for bus `{p.name}` but carry no bit index (PORT/BIT attrs)")
                continue
            if not cands:
                rep.problems.append(f"output {b}: no IOB drives it")
                continue
            if len(cands) > 1:
                rep.problems.append(f"output {b}: {len(cands)} IOBs claim it")
            pin = pin_of.get(b)
            if pin is None:
                rep.problems.append(f"output {b} has no PACKAGE_PIN in the XDC")
                continue
            wrapper_ports.append(f"output wire {pin}")
            conns.append(f".{cands[0]['verilog']}({pin})")
    wrapper = (f"module extracted({', '.join(wrapper_ports)});\n"
               f"helion_netlist u({', '.join(conns)});\nendmodule\n")
    full = os.path.join(workdir, "netlist.v")
    open(full, "w").write(open(v).read() + "\n" + wrapper)
    pins_in = {w.split()[-1] for w in wrapper_ports if w.startswith("input")}
    pins_out = {w.split()[-1] for w in wrapper_ports if w.startswith("output")}
    try:
        rep.cosim = cosim.run(os.path.join(workdir, "cosim"), rtl, top, xdc, full, pins_in, pins_out, cycles, seed)
    except Exception as e:
        rep.problems.append(f"simulation failed: {str(e)[:300]}")
        return rep
    # An input the design never reads legitimately has no pad in the netlist
    # (the RTL simulation still drives it, so a real dependency would show up
    # as output mismatches).
    rep.ok = not rep.problems and rep.cosim.ok and rep.cosim.net_unknown == 0
    return rep


def run_micro(path: str, top: str, workdir: str, cycles: int = 1000, seed: int = 1, translate_only: bool = False) -> SynthReport:
    """Check one small SV/VHDL file with auto-generated pin constraints (every
    port bit gets a unique pseudo pin; the first 1-bit input named clk/clock
    is the clock)."""
    os.makedirs(workdir, exist_ok=True)
    ports = cosim.sv_ports([path], top) if cosim.is_verilog([path]) else cosim.vhdl_ports([path], top)
    lines = []
    for p in ports:
        for b in p.bits():
            pin = "P_" + re.sub(r"[^A-Za-z0-9]", "_", b)
            lines.append(f"set_property PACKAGE_PIN {pin} [get_ports {{{b}}}]")
            if p.direction == "in" and p.width is None and re.fullmatch(r"(clk|clock)", p.name, re.I):
                lines.append(f"create_clock -period 10 [get_ports {p.name}]")
    xdc = os.path.join(workdir, "auto.xdc")
    open(xdc, "w").write("\n".join(lines) + "\n")
    if translate_only:
        return run_sv(os.path.basename(path), [path], top, xdc, workdir, cycles, seed)
    return run(os.path.basename(path), [path], top, xdc, workdir, cycles, seed)


VHDL2SV = os.environ.get("X7V_VHDL2SV", os.path.expanduser("~/helion-zed-verify/target/release/examples/vhdl2sv"))


def run_sv(design: str, rtl: list[str], top: str, xdc: str, workdir: str,
           cycles: int = 3000, seed: int = 1) -> SynthReport:
    """Translator-only check: co-simulate the SystemVerilog that Helion's
    VHDL front end emits against the VHDL itself (no synthesis). A failure
    here is a VHDL translation bug; a pass here with a synthcheck failure
    points at helion-sv."""
    os.makedirs(workdir, exist_ok=True)
    rep = SynthReport(design)
    src = _combined_source(rtl, workdir, top)
    r = subprocess.run([VHDL2SV, src, top], capture_output=True, text=True, timeout=600)
    if r.returncode != 0:
        rep.synth_error = (r.stderr.strip().splitlines() or ["vhdl2sv failed"])[-1]
        return rep
    sv = r.stdout
    renames = dict(re.findall(r"^// port-rename (\S+) (\S+)$", sv, re.M))
    m = re.search(r"^module\s+(\w+)", sv, re.M)
    mod = m.group(1)
    pin_of, _ = cosim.parse_xdc(xdc)
    ports = cosim.vhdl_ports(rtl, top)
    wrapper_ports, conns = [], []
    for p in ports:
        pins = []
        for b in p.bits():
            pin = pin_of.get(b)
            if pin is None:
                rep.problems.append(f"{b} has no PACKAGE_PIN in the XDC")
                pin = None
            pins.append(pin)
        if any(x is None for x in pins):
            continue
        for pin in pins:
            wrapper_ports.append(f"{'input' if p.direction == 'in' else 'output'} wire {pin}")
        conns.append(f".{renames.get(p.name, p.name)}({{{', '.join(pins)}}})")
    wrapper = (f"module extracted({', '.join(wrapper_ports)});\n"
               f"{mod} u({', '.join(conns)});\nendmodule\n")
    full = os.path.join(workdir, "translated.v")
    open(full, "w").write(sv + "\n" + wrapper)
    pins_in = {w.split()[-1] for w in wrapper_ports if w.startswith("input")}
    pins_out = {w.split()[-1] for w in wrapper_ports if w.startswith("output")}
    try:
        rep.cosim = cosim.run(os.path.join(workdir, "cosim"), rtl, top, xdc, full, pins_in, pins_out, cycles, seed)
    except Exception as e:
        rep.problems.append(f"simulation failed: {str(e)[:300]}")
        return rep
    rep.ok = not rep.problems and rep.cosim.ok and rep.cosim.net_unknown == 0
    return rep
