"""Co-simulate RTL against a netlist extracted from decoded FASM.

Adapted from x7v's cosim.py (7-series), ported to UltraScale+ CLB:
same stimulus model and the same two-samples-per-cycle comparison.
Differences from x7v:
- Verilog/SystemVerilog RTL only (no VHDL, no MMCM/BUFG shims:
  clock sites are out of scope for the v1 rebuild, see README).
- Two port-mapping modes: XDC mode (package pins, like x7v) and
  explicit port-map mode (RTL port bit -> slice pin, e.g.
  "SLICE_X0Y0.AMUX"), used by the hand-built fixtures.
- No verilator path: iverilog only.

Both simulators read the same stimulus file and write two output
vectors per cycle. Cycle k: inputs change and clock falls at
t=10k, outputs are sampled at t=10k+4, clock rises at t=10k+5,
outputs are sampled again at t=10k+9. RTL 'U'/'X' outputs (no reset
yet, no initialiser) are don't-care for that cycle and are counted
separately.

No em dashes anywhere.
"""
from __future__ import annotations

import os
import random
import re
import subprocess
from dataclasses import dataclass, field


def parse_xdc(path: str) -> tuple[dict[str, str], set[str]]:
    """Return ({port_bit: package_pin}, {clock ports})."""
    pins: dict[str, str] = {}
    clocks: set[str] = set()
    with open(path) as f:
        text = f.read()
    for raw in text.splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        names = []
        for m in re.finditer(r"get_ports\s+(\{([^}]*)\}|\"([^\"]*)\"|([^\s\]]+))", line):
            body = m.group(2) if m.group(2) is not None else (m.group(3) if m.group(3) is not None else m.group(4))
            names.extend(body.split())
        m = re.search(r"PACKAGE_PIN\s+(\w+)", line)
        if m and names:
            for n in names:
                pins[_norm(n)] = m.group(1)
        if line.startswith("create_clock") and names:
            clocks.update(_norm(n) for n in names)
    return pins, clocks


def _norm(port: str) -> str:
    return port.strip().strip('"')


@dataclass
class Port:
    name: str
    direction: str              # in / out
    width: int | None           # None = scalar
    hi: int = 0
    lo: int = 0

    def bits(self) -> list[str]:
        if self.width is None:
            return [self.name]
        step = -1 if self.hi >= self.lo else 1
        return [f"{self.name}[{i}]" for i in range(self.hi, self.lo + step, step)]


RESET_RATE = 0.01
RESET_PULSE = True
NO_RESET_SETTLE = 64
RESET_AT: tuple[int, int] | None = None


def make_stimulus(bits: list[str], cycles: int, seed: int, hold: dict[str, float] | None = None) -> list[str]:
    """Random walk: each input bit flips with a per-bit probability per cycle.
    The first 4 cycles hold all zeros, then any reset-like input is pulsed
    high for 3 cycles so designs start from a defined state."""
    rng = random.Random(seed)
    cur = {b: 0 for b in bits}
    out = []
    resets = [b for b in bits if re.search(r"(^|_)(rst|reset|btnc|clr)", b, re.I)]
    for k in range(cycles):
        if k >= 4:
            for b in bits:
                p = (hold or {}).get(b, 0.15)
                if b in resets:
                    p = RESET_RATE if RESET_PULSE and RESET_AT is None else 0.0
                if rng.random() < p:
                    cur[b] ^= 1
        if 4 <= k < 7 and RESET_PULSE:
            for b in resets:
                cur[b] = 1
        if k == 7:
            for b in resets:
                cur[b] = 0
        if RESET_AT is not None and k >= 7:
            for b in resets:
                cur[b] = int(RESET_AT[0] <= k < RESET_AT[0] + RESET_AT[1])
        out.append("".join(str(cur[b]) for b in bits) or "0")
    return out


def verilog_tb(pin_of: dict[str, str], netlist_ports_in: set[str], netlist_ports_out: set[str],
               clock_pin: str | list[str] | None, in_bits: list[str], out_bits: list[str], n_cycles: int,
               stim: str, trace: str) -> str:
    clock_pins = (
        [] if clock_pin is None
        else [clock_pin] if isinstance(clock_pin, str) else list(clock_pin)
    )
    decl = []
    conn = []
    for pin in sorted(netlist_ports_in):
        decl.append(f"reg r_{pin} = 1'b0;")
        conn.append(f".{pin}(r_{pin})")
    for pin in sorted(netlist_ports_out):
        decl.append(f"wire w_{pin};")
        conn.append(f".{pin}(w_{pin})")
    drive = []
    for i, b in enumerate(in_bits):
        pin = pin_of.get(b)
        if pin in netlist_ports_in:
            drive.append(f"    r_{pin} = vec[{len(in_bits) - 1 - i}];")
    sample = []
    for b in out_bits:
        pin = pin_of.get(b)
        if pin in netlist_ports_out:
            sample.append(f'    $fwrite(fo, "%s", bitc(w_{pin}));')
        else:
            sample.append(f'    $fwrite(fo, "-");')
    clk_lo = "\n".join(f"    r_{p} = 1'b0;" for p in clock_pins if p in netlist_ports_in)
    clk_hi = "\n".join(f"    r_{p} = 1'b1;" for p in clock_pins if p in netlist_ports_in)
    return f"""`timescale 1ns/1ps
module tb;
{chr(10).join(decl)}
extracted dut({', '.join(conn)});
integer fi, fo, k, rc;
reg [{max(1, len(in_bits))}-1:0] vec;
function [7:0] bitc(input v);
  bitc = (v === 1'b1) ? "1" : (v === 1'b0) ? "0" : (v === 1'bz) ? "z" : "x";
endfunction
integer j;
initial begin
  fi = $fopen("{stim}", "r");
  fo = $fopen("{trace}", "w");
  for (k = 0; k < {n_cycles}; k = k + 1) begin
    rc = $fscanf(fi, "%b", vec);
{clk_lo}
{chr(10).join(drive)}
    #4;
{chr(10).join(sample)}
    $fwrite(fo, " ");
    #1;
{clk_hi}
    #4;
{chr(10).join(sample)}
    $fwrite(fo, "\\n");
    #1;
  end
  $fclose(fo);
  $finish;
end
endmodule
"""


def rtl_verilog_tb(top: str, ports: list[Port], clock: str | list[str] | None, in_bits: list[str], out_bits: list[str],
                   n_cycles: int, stim: str, trace: str) -> str:
    clocks = [] if clock is None else [clock] if isinstance(clock, str) else list(clock)
    decl, conn, drive, sample = [], [], [], []
    for p in ports:
        w = "" if p.width is None else f"[{p.hi}:{p.lo}] "
        if p.direction == "in":
            decl.append(f"reg {w}{p.name} = 0;")
        else:
            decl.append(f"wire {w}{p.name};")
        conn.append(f".{p.name}({p.name})")
    n = max(1, len(in_bits))
    for i, b in enumerate(in_bits):
        drive.append(f"    {b} = vec[{n - 1 - i}];")
    for b in out_bits:
        sample.append(f'    $fwrite(fo, "%s", bitc({b}));')
    clk_lo = "\n".join(f"    {c} = 1'b0;" for c in clocks)
    clk_hi = "\n".join(f"    {c} = 1'b1;" for c in clocks)
    return f"""`timescale 1ns/1ps
module tb_rtl;
{chr(10).join(decl)}
{top} dut({', '.join(conn)});
integer fi, fo, k, rc;
reg [{n}-1:0] vec;
function [7:0] bitc(input v);
  bitc = (v === 1'b1) ? "1" : (v === 1'b0) ? "0" : (v === 1'bz) ? "z" : "x";
endfunction
initial begin
  fi = $fopen("{stim}", "r");
  fo = $fopen("{trace}", "w");
  for (k = 0; k < {n_cycles}; k = k + 1) begin
    rc = $fscanf(fi, "%b", vec);
{clk_lo}
{chr(10).join(drive)}
    #4;
{chr(10).join(sample)}
    $fwrite(fo, " ");
    #1;
{clk_hi}
    #4;
{chr(10).join(sample)}
    $fwrite(fo, "\\n");
    #1;
  end
  $fclose(fo);
  $finish;
end
endmodule
"""


def sv_ports(rtl: list[str], top: str) -> list[Port]:
    parts = []
    for f in rtl:
        with open(f) as fh:
            parts.append(re.sub(r"//[^\n]*", "", fh.read()))
    text = "\n".join(parts)
    m = re.search(rf"module\s+{top}\s*(?:#\s*\(.*?\)\s*)?\((.*?)\)\s*;", text, re.S)
    if not m:
        raise ValueError(f"module {top} not found")
    ports = []
    cur_dir, cur_rng = None, None
    for item in m.group(1).split(","):
        item = item.strip()
        mm = re.match(r"(input|output|inout)?\s*(?:wire|logic|reg)?\s*(?:signed\s*)?(\[\s*(\d+)\s*:\s*(\d+)\s*\])?\s*(\w+)$", item)
        if not mm:
            continue
        if mm.group(1):
            cur_dir = mm.group(1)
            cur_rng = (mm.group(3), mm.group(4)) if mm.group(2) else None
        elif mm.group(2):
            cur_rng = (mm.group(3), mm.group(4))
        d = "in" if cur_dir == "input" else "out"
        if cur_rng:
            hi, lo = int(cur_rng[0]), int(cur_rng[1])
            ports.append(Port(mm.group(5), d, abs(hi - lo) + 1, hi, lo))
        else:
            ports.append(Port(mm.group(5), d, None))
    return ports


@dataclass
class CosimResult:
    cycles: int
    compared: int
    mismatches: list[tuple[int, str, str, str]] = field(default_factory=list)  # (cycle, bit, rtl, net)
    rtl_unknown: int = 0
    net_unknown: int = 0
    unmapped_outputs: list[str] = field(default_factory=list)
    unmapped_inputs: list[str] = field(default_factory=list)
    log: str = ""

    @property
    def ok(self) -> bool:
        return not self.mismatches and not self.unmapped_outputs and self.compared > 0


def run(workdir: str, rtl_files: list[str], top: str, netlist_v: str,
        ports_in: set[str], ports_out: set[str],
        xdc: str | None = None, port_map: dict[str, str] | None = None,
        cycles: int = 2000, seed: int = 1) -> CosimResult:
    """Co-simulate RTL against the extracted netlist.

    Either `xdc` (XDC mode: RTL port bit -> package pin, netlist ports
    are package pins) or `port_map` (explicit mode: RTL port bit ->
    netlist port name, e.g. a slice pin) must be given.
    """
    if (xdc is None) == (port_map is None):
        raise ValueError("exactly one of xdc and port_map is required")
    os.makedirs(workdir, exist_ok=True)
    if xdc is not None:
        pin_of, clocks = parse_xdc(xdc)
    else:
        pin_of, clocks = dict(port_map), set()
    ports = sv_ports(rtl_files, top)
    clock_names = [
        p.name for p in ports
        if p.direction == "in" and p.width is None
        and (p.name in clocks or re.fullmatch(r"(clk|clock|gclk|sysclk|clk\d+)", p.name, re.I))
    ]
    in_bits = [b for p in ports if p.direction == "in" for b in p.bits() if b not in clock_names]
    out_bits = [b for p in ports if p.direction == "out" for b in p.bits()]
    res = CosimResult(cycles, 0)
    res.unmapped_inputs = [b for b in in_bits if pin_of.get(b) not in ports_in]
    res.unmapped_outputs = [b for b in out_bits if pin_of.get(b) not in ports_out]
    stim = os.path.join(workdir, "stim.txt")
    with open(stim, "w") as f:
        f.write("\n".join(make_stimulus(in_bits, cycles, seed)) + "\n")
    rtl_trace = os.path.join(workdir, "rtl.trace")
    net_trace = os.path.join(workdir, "net.trace")
    with open(os.path.join(workdir, "tb_rtl.v"), "w") as f:
        f.write(rtl_verilog_tb(top, ports, clock_names, in_bits, out_bits, cycles, stim, rtl_trace))
    with open(os.path.join(workdir, "tb.v"), "w") as f:
        f.write(verilog_tb(pin_of, ports_in, ports_out,
                           [pin_of[c] for c in clock_names if pin_of.get(c)],
                           in_bits, out_bits, cycles, stim, net_trace))
    logs = []
    r = subprocess.run(["iverilog", "-g2012", "-o", os.path.join(workdir, "rtl.vvp"),
                        os.path.join(workdir, "tb_rtl.v"), *rtl_files], capture_output=True, text=True)
    logs.append(r.stdout + r.stderr)
    if r.returncode == 0:
        r = subprocess.run(["vvp", "-n", os.path.join(workdir, "rtl.vvp")], capture_output=True, text=True, cwd=workdir)
        logs.append(r.stdout[-2000:] + r.stderr[-2000:])
    if r.returncode != 0:
        res.log = "\n".join(logs)
        raise RuntimeError(f"RTL iverilog failed:\n{res.log[-3000:]}")
    r = subprocess.run(["iverilog", "-g2012", "-o", os.path.join(workdir, "net.vvp"),
                        os.path.join(workdir, "tb.v"), netlist_v], capture_output=True, text=True)
    logs.append(r.stdout + r.stderr)
    if r.returncode != 0:
        res.log = "\n".join(logs)
        raise RuntimeError(f"iverilog failed:\n{res.log[-3000:]}")
    r = subprocess.run(["vvp", "-n", os.path.join(workdir, "net.vvp")], capture_output=True, text=True, cwd=workdir)
    logs.append(r.stdout[-2000:] + r.stderr[-2000:])
    res.log = "\n".join(logs)
    with open(rtl_trace) as f:
        a = f.read().split()
    with open(net_trace) as f:
        b = f.read().split()
    for t, (ra, nb) in enumerate(zip(a, b)):
        k = t // 2
        pre = t % 2 == 0
        for i, bitname0 in enumerate(out_bits):
            bitname = f"{bitname0} (before edge)" if pre else bitname0
            rv, nv = ra[i], nb[i]
            if nv == "-":
                continue
            if rv in "xu":
                res.rtl_unknown += 1
                if not RESET_PULSE and k >= NO_RESET_SETTLE:
                    res.mismatches.append((k, bitname, "x (no reset)", nv))
                continue
            if nv in "xz":
                res.net_unknown += 1
            res.compared += 1
            if rv != nv:
                res.mismatches.append((k, bitname, rv, nv))
    if len(a) != len(b):
        res.mismatches.append((-1, "trace-length", str(len(a)), str(len(b))))
    return res
