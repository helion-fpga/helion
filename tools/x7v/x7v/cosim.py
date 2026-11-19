"""Co-simulate RTL against a netlist extracted from a bitstream.

Both simulators read the same stimulus file and write two output vectors per
cycle. Cycle k: inputs change and clock falls at t=10k, outputs are sampled
at t=10k+4, clock rises at t=10k+5, outputs are sampled again at t=10k+9. RTL 'U'/'X' outputs (no reset yet,
no initialiser) are don't-care for that cycle and are counted separately.
"""
from __future__ import annotations

import os
import random
import re
import subprocess
from dataclasses import dataclass, field


# ---- constraints -----------------------------------------------------------

def parse_xdc(path: str) -> tuple[dict[str, str], set[str]]:
    """Return ({port_bit: package_pin}, {clock ports})."""
    pins: dict[str, str] = {}
    clocks: set[str] = set()
    text = open(path).read()
    for raw in text.splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        names = []
        # [get_ports {a[0] b[1]}] or [get_ports name] or [get_ports "name"]
        for m in re.finditer(r"get_ports\s+(\{([^}]*)\}|\"([^\"]*)\"|([^\s\]]+))", line):
            body = m.group(2) if m.group(2) is not None else (m.group(3) if m.group(3) is not None else m.group(4))
            names.extend(body.split())
        m = re.search(r"PACKAGE_PIN\s+(\w+)", line)
        if m and names:
            for n in names:
                pins[_norm(n)] = m.group(1)
        if line.startswith("create_clock") and names:
            clocks.update(_norm(n) for n in names)
        if line.startswith("create_clock"):
            # A clock on an instance pin (the PS7's FCLK): "ps7/FCLKCLK[0]" -> "ps7.FCLKCLK[0]".
            for m in re.finditer(r"get_pins\s+(?:\{([^}]*)\}|(\S+)\])", line):
                clocks.update(n.replace("/", ".") for n in (m.group(1) or m.group(2)).split())
    return pins, clocks


def _norm(port: str) -> str:
    return port.strip().strip('"')


# ---- RTL interface -----------------------------------------------------------

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


def vhdl_ports(files: list[str], top: str) -> list[Port]:
    for f in files:
        text = re.sub(r"--[^\n]*", "", open(f).read())
        m = re.search(rf"entity\s+{top}\s+is\s+(?:generic\s*\(.*?\)\s*;\s*)?port\s*\((.*?)\)\s*;\s*end", text, re.S | re.I)
        if not m:
            continue
        ports = []
        for decl in m.group(1).split(";"):
            decl = decl.strip()
            if not decl:
                continue
            names, rest = decl.split(":", 1)
            mm = re.match(r"\s*(in|out|inout|buffer)\s+(.*)$", rest, re.S | re.I)
            d, typ = mm.group(1).lower(), mm.group(2).strip()
            vm = re.search(r"\(\s*(\d+)\s+(downto|to)\s+(\d+)\s*\)", typ, re.I)
            for n in names.split(","):
                n = n.strip()
                if vm:
                    hi, lo = int(vm.group(1)), int(vm.group(3))
                    ports.append(Port(n, d, abs(hi - lo) + 1, hi, lo))
                else:
                    ports.append(Port(n, d, None))
        return ports
    raise ValueError(f"entity {top} not found in {files}")


# ---- stimulus ------------------------------------------------------------------

# Simulator for Verilog/SystemVerilog RTL: `iverilog` (four-state), or
# `verilator` for sources Icarus cannot read (`X7V_RTL_SIM`; macros for the
# design in `X7V_DEFINES`, comma separated).
RTL_SIM = os.environ.get("X7V_RTL_SIM", "iverilog")

# Probability per cycle that a reset-like input flips after start-up
# (`verify --reset-rate`). Lower values give long runs between resets, which
# a processor needs to get anywhere in its program.
RESET_RATE = 0.01

# Pulse reset-like inputs at start-up (`verify --no-reset` turns this off and
# holds them at 0 for the whole run: a design must then start by itself).
RESET_PULSE = True
# Cycles a design gets to define its outputs when no reset is applied.
NO_RESET_SETTLE = 64
# (start, length): instead of random resets, pulse reset-like inputs once
# at that cycle (`verify --reset-at`), so a long test is seen recovering.
RESET_AT: tuple[int, int] | None = None


def make_stimulus(bits: list[str], cycles: int, seed: int, hold: dict[str, float] | None = None,
                  active_low: list[str] = ()) -> list[str]:
    """Random walk: each input bit flips with a per-bit probability per cycle.
    The first 4 cycles hold all zeros, then any reset-like input is pulsed
    high for 3 cycles so designs start from a defined state. `active_low`
    bits (the PS7's FCLKRESETN) all follow one reset, inverted."""
    rng = random.Random(seed)
    cur = {b: 0 for b in bits}
    out = []
    resets = [b for b in bits if re.search(r"(^|_)(rst|reset|btnc|clr)", b, re.I)] + list(active_low[:1])
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
        # A design with no data inputs still gets one placeholder column so
        # every stimulus line is non-empty (VHDL READ rejects empty lines).
        low = {b: 1 - cur[active_low[0]] for b in active_low}
        out.append("".join(str(low.get(b, cur[b])) for b in bits) or "0")
    return out


# ---- testbenches -----------------------------------------------------------------

def _vhdl_sig(p: Port) -> str:
    return "std_logic" if p.width is None else f"std_logic_vector({p.hi} {'downto' if p.hi >= p.lo else 'to'} {p.lo})"


def vhdl_tb(top: str, ports: list[Port], clock: str | None, in_bits: list[str], out_bits: list[str],
            stim: str, trace: str) -> str:
    sigs = "\n".join(f"  signal {p.name} : {_vhdl_sig(p)}{' := ' + repr('0') if p.direction == 'in' and p.width is None else ''};" for p in ports)
    sigs = sigs.replace("'0'", "'0'")
    pmap = ", ".join(f"{p.name} => {p.name}" for p in ports)
    assign_in = []
    for i, b in enumerate(in_bits):
        nm = b.replace("[", "(").replace("]", ")")
        assign_in.append(f"      {nm} <= to_sl(l({i + 1}));")
    outs = []
    for b in out_bits:
        nm = b.replace("[", "(").replace("]", ")")
        outs.append(f"      write(o, to_ch({nm}));")
    clk_low = f"      {clock} <= '0';" if clock else ""
    clk_high = f"      {clock} <= '1';" if clock else ""
    return f"""library ieee;
use ieee.std_logic_1164.all;
use std.textio.all;
entity tb is end entity;
architecture sim of tb is
{sigs}
  function to_sl(c : character) return std_logic is
  begin
    if c = '1' then return '1'; else return '0'; end if;
  end function;
  function to_ch(s : std_logic) return character is
  begin
    case s is
      when '0' | 'L' => return '0';
      when '1' | 'H' => return '1';
      when 'Z' => return 'z';
      when others => return 'x';
    end case;
  end function;
begin
  dut: entity work.{top} port map ({pmap});
  process
    file fi : text open read_mode is "{stim}";
    file fo : text open write_mode is "{trace}";
    variable li, o : line;
    variable l : string(1 to {max(1, len(in_bits))});
  begin
    while not endfile(fi) loop
      readline(fi, li);
      read(li, l);
{clk_low}
{chr(10).join(assign_in)}
      wait for 4 ns;
{chr(10).join(outs)}
      write(o, character'(' '));
      wait for 1 ns;
{clk_high}
      wait for 4 ns;
{chr(10).join(outs)}
      writeline(fo, o);
      wait for 1 ns;
    end loop;
    std.env.finish;
  end process;
end architecture;
"""


def verilog_tb(pin_of: dict[str, str], netlist_ports_in: set[str], netlist_ports_out: set[str],
               clock_pin: str | None, in_bits: list[str], out_bits: list[str], n_cycles: int,
               stim: str, trace: str) -> str:
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
    clk_lo = f"    r_{clock_pin} = 1'b0;" if clock_pin and clock_pin in netlist_ports_in else ""
    clk_hi = f"    r_{clock_pin} = 1'b1;" if clock_pin and clock_pin in netlist_ports_in else ""
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


def rtl_verilog_tb(top: str, ports: list["Port"], clock: str | None, in_bits: list[str], out_bits: list[str],
                   n_cycles: int, stim: str, trace: str) -> str:
    decl, conn, drive, sample = [], [], [], []
    for p in ports:
        w = "" if p.width is None else f"[{p.hi}:{p.lo}] "
        if p.direction == "in":
            decl.append(f"reg {w}{p.name} = 0;")
        else:
            decl.append(f"wire {w}{p.name};")
        conn.append(f".{p.name}({p.name})")
    n = max(1, len(in_bits))
    # A pin of an instance (the PS7 stub's "ps7.FCLKCLK[0]") is reached through the top.
    ref = lambda b: f"dut.{b}" if "." in b else b
    for i, b in enumerate(in_bits):
        drive.append(f"    {ref(b)} = vec[{n - 1 - i}];")
    for b in out_bits:
        sample.append(f'    $fwrite(fo, "%s", bitc({ref(b)}));')
    clk_lo = f"    {ref(clock)} = 1'b0;" if clock else ""
    clk_hi = f"    {ref(clock)} = 1'b1;" if clock else ""
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


def sv_ports(rtl: list[str], top: str) -> list["Port"]:
    text = "\n".join(re.sub(r"//[^\n]*", "", open(f).read()) for f in rtl)
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


def is_verilog(rtl: list[str]) -> bool:
    return rtl[0].endswith((".v", ".sv"))


# ---- run -------------------------------------------------------------------------

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
    axi: list[str] = field(default_factory=list)          # AXI master summary (PS7 designs)
    axi_errors: list[str] = field(default_factory=list)   # its readback and protocol errors

    @property
    def ok(self) -> bool:
        return not self.mismatches and not self.unmapped_outputs and self.compared > 0


# Registers the PS7's AXI masters must read back as written, {address: bit
# mask} (`verify --axi-rw`).
AXI_RW: dict[int, int] = {}


def run(workdir: str, rtl_files: list[str], top: str, xdc: str, netlist_v: str,
        ports_in: set[str], ports_out: set[str], cycles: int = 2000, seed: int = 1,
        ps7_ports: list | None = None) -> CosimResult:
    from . import ps7
    os.makedirs(workdir, exist_ok=True)
    pin_of, clocks = parse_xdc(xdc)
    ports = sv_ports(rtl_files, top) if is_verilog(rtl_files) else vhdl_ports(rtl_files, top)
    # A PS7 in the RTL is the design's edge: its outputs are inputs here, its inputs outputs.
    ps = ps7.boundary(rtl_files, top, ps7_ports) if ps7_ports and is_verilog(rtl_files) else None
    if ps and RTL_SIM == "verilator":
        # shortcut: unconnected PS7 inputs are told by Icarus's z; port to Verilator when a PS7 design needs it.
        raise ValueError("a PS7 design co-simulates on Icarus only (unset X7V_RTL_SIM)")
    clock = next((p.name for p in ports if p.name in clocks), None) or \
        next((b for b in (ps.out_bits if ps else ()) if b in clocks), None)
    if clock is None:
        clock = next((p.name for p in ports if p.direction == "in" and p.width is None and re.fullmatch(r"(clk|clock|gclk|sysclk)", p.name, re.I)), None)
    if clock is None and ps:
        clock = f"{ps.inst}.FCLKCLK[0]"
    in_bits = [b for p in ports if p.direction == "in" for b in p.bits() if b != clock]
    out_bits = [b for p in ports if p.direction == "out" for b in p.bits()]
    if ps:
        pin_of = {**pin_of, **ps.pin_of}
        in_bits += [b for b in ps.out_bits if b != clock]
        out_bits += ps.in_bits
    res = CosimResult(cycles, 0)
    res.unmapped_inputs = [b for b in in_bits if pin_of.get(b) not in ports_in]
    res.unmapped_outputs = [b for b in out_bits if pin_of.get(b) not in ports_out]
    if is_verilog(rtl_files):
        # Clocking primitives the RTL instantiates (MMCM/PLL/BUFG) and does
        # not define: x7v's shims onto its own clock model (mmcm.py).
        from . import mmcm
        text = "\n".join(open(f).read() for f in rtl_files)
        if any(re.search(rf"\b{p}\b", text) and not re.search(rf"\bmodule\s+{p}\b", text) for p in mmcm.PRIM_NAMES):
            prims = os.path.join(workdir, "x7v_prims.v")
            open(prims, "w").write(mmcm.RTL_PRIMS)
            rtl_files = [prims, *rtl_files]
    if ps:
        stub = os.path.join(workdir, "x7v_ps7.v")
        open(stub, "w").write(ps7.stub(ps7_ports))
        rtl_files = [stub, *rtl_files]
    stim = os.path.join(workdir, "stim.txt")
    lines = make_stimulus(in_bits, cycles, seed, active_low=ps.resets if ps else ())
    if ps and ps.masters:
        # The AXI masters react to the RTL's responses: run them against the
        # RTL once; what they drove becomes the stimulus both sides replay.
        base = os.path.join(workdir, "stim_random.txt")
        open(base, "w").write("\n".join(lines) + "\n")
        res.axi, res.axi_errors = ps7.run_axi(workdir, rtl_files, ps7.axi_tb(
            top, ports, ps, in_bits, clock, cycles, base, stim, os.path.join(workdir, "axi.log"), seed, AXI_RW))
    else:
        open(stim, "w").write("\n".join(lines) + "\n")
    rtl_trace = os.path.join(workdir, "rtl.trace")
    net_trace = os.path.join(workdir, "net.trace")
    if is_verilog(rtl_files):
        open(os.path.join(workdir, "tb_rtl.v"), "w").write(
            rtl_verilog_tb(top, ports, clock, in_bits, out_bits, cycles, stim, rtl_trace))
    else:
        open(os.path.join(workdir, "tb.vhd"), "w").write(
            vhdl_tb(top, ports, clock, in_bits, out_bits, stim, rtl_trace))
    open(os.path.join(workdir, "tb.v"), "w").write(
        verilog_tb(pin_of, ports_in, ports_out, pin_of.get(clock) if clock else None,
                   in_bits, out_bits, cycles, stim, net_trace))
    logs = []
    if is_verilog(rtl_files) and RTL_SIM == "verilator":
        # SystemVerilog that Icarus cannot read (packages, interfaces,
        # structs). Verilator is two-state: a register the RTL never
        # initialises starts at 0 instead of X, so nothing is don't-care.
        obj = os.path.join(workdir, "vobj")
        defs = [f"+define+{d}" for d in os.environ.get("X7V_DEFINES", "").split(",") if d]
        r = subprocess.run(["verilator", "--binary", "--timing", "-O1", "--x-initial", "0", "--x-assign", "0",
                            "-Wno-fatal", "-Wno-lint", "-Wno-WIDTH", "-Wno-PINMISSING", "-Wno-MULTIDRIVEN",
                            "-Wno-LATCH", "-Wno-UNOPTFLAT", "+1364-2005ext+v", "--top-module", "tb_rtl",
                            "--Mdir", obj, "-o", "sim", *defs, os.path.join(workdir, "tb_rtl.v"), *rtl_files],
                           capture_output=True, text=True)
        logs.append(r.stdout[-2000:] + r.stderr[-4000:])
        if r.returncode == 0:
            r = subprocess.run([os.path.join(obj, "sim")], capture_output=True, text=True, cwd=workdir)
            logs.append(r.stdout[-2000:] + r.stderr[-2000:])
        if r.returncode != 0:
            res.log = "\n".join(logs)
            raise RuntimeError(f"RTL verilator failed:\n{res.log[-3000:]}")
    elif is_verilog(rtl_files):
        r = subprocess.run(["iverilog", "-g2012", "-o", os.path.join(workdir, "rtl.vvp"),
                            os.path.join(workdir, "tb_rtl.v"), *rtl_files], capture_output=True, text=True)
        logs.append(r.stdout + r.stderr)
        if r.returncode == 0:
            r = subprocess.run(["vvp", "-n", os.path.join(workdir, "rtl.vvp")], capture_output=True, text=True, cwd=workdir)
            logs.append(r.stdout[-2000:] + r.stderr[-2000:])
        if r.returncode != 0:
            res.log = "\n".join(logs)
            raise RuntimeError(f"RTL iverilog failed:\n{res.log[-3000:]}")
    else:
        r = subprocess.run(["nvc", "--std=2008", "-L", workdir, "--work", f"work:{workdir}/work", "-a", *rtl_files,
                            os.path.join(workdir, "tb.vhd"), "-e", "tb", "-r"],
                           capture_output=True, text=True, cwd=workdir)
        logs.append(r.stdout + r.stderr)
        if r.returncode != 0:
            res.log = "\n".join(logs)
            raise RuntimeError(f"nvc failed:\n{res.log[-3000:]}")
    r = subprocess.run(["iverilog", "-g2012", "-o", os.path.join(workdir, "net.vvp"),
                        os.path.join(workdir, "tb.v"), netlist_v], capture_output=True, text=True)
    logs.append(r.stdout + r.stderr)
    if r.returncode != 0:
        res.log = "\n".join(logs)
        raise RuntimeError(f"iverilog failed:\n{res.log[-3000:]}")
    r = subprocess.run(["vvp", "-n", os.path.join(workdir, "net.vvp")], capture_output=True, text=True, cwd=workdir)
    logs.append(r.stdout[-2000:] + r.stderr[-2000:])
    res.log = "\n".join(logs)
    a = open(rtl_trace).read().split()
    b = open(net_trace).read().split()
    ps_in = set(ps.in_bits) if ps else set()
    # Two samples per cycle: just before the rising edge (inputs settled;
    # catches combinational paths and asynchronous set/reset) and after it.
    for t, (ra, nb) in enumerate(zip(a, b)):
        k = t // 2
        pre = t % 2 == 0
        for i, bitname0 in enumerate(out_bits):
            bitname = f"{bitname0} (before edge)" if pre else bitname0
            rv, nv = ra[i], nb[i]
            if nv == "-":
                continue
            if rv == "z" and bitname0 in ps_in:
                # A PS7 input the RTL leaves unconnected must read 0 (tied)
                # or float (an unused port's clock, left unrouted): never 1.
                res.compared += 1
                if nv not in "0z":
                    res.mismatches.append((k, bitname, "unconnected (0)", nv))
                continue
            if rv in "xu":
                res.rtl_unknown += 1
                # Without a reset an output the RTL still leaves undefined
                # after start-up is a failure, not a don't-care: otherwise a
                # design that never leaves power-up would pass vacuously.
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
