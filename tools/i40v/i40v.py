#!/usr/bin/env python3
"""i40v: check an iCE40-HX8K bitstream against its RTL, independently of
the tool that made it.

i40v shares no code with helion-ice40. Everything it knows about the chip
comes from Project IceStorm (icepack, iceunpack, icebox_vlog, icetime and
the icebox Python module) and everything it knows about the design comes
from the RTL, read by yosys and simulated by Icarus Verilog.

  i40v.py check   --rtl a.sv [--rtl b.sv] --top T --pcf p.pcf --asc x.asc [options]
  i40v.py golden  --rtl ... --top T --pcf p.pcf --work DIR [--compare-io x.asc] [check options]
  i40v.py mutate  --asc in.asc --out bad.asc --kind lutbit|cen|negclk|asyncsr [--seed N]

`check` runs, on the given .asc:
  roundtrip  icepack -> iceunpack: every set bit survives, and the unpacked
             text is a pack/unpack fixed point.
  extract    icebox_vlog -p PCF -c rebuilds a netlist from the config bits
             (with IceStorm's exactly-one-driver check, -D).
  cosim      the netlist and the RTL, side by side in iverilog under random
             stimulus for --cycles clock cycles, compared twice a cycle. Once
             with every --reset input held inactive (power-on state: all
             flip-flops 0, as on the chip; RTL registers without an initial
             value start at 0) and once with the resets pulsed at random.
  formal     yosys miter of RTL and netlist, multi-clock exact (clk2fflogic),
             proved for all reachable states with ABC's pdr from the
             all-zero power-on state (skipped with --formal off, or when the
             design has block RAM).
  timing     icetime -d hx8k -P ct256: Fmax must be at least --fmax MHz.

`golden` builds the same RTL with yosys synth_ice40 + nextpnr-ice40 +
icepack and runs `check` on that bitstream: a check of the checker. With
--compare-io it also compares the IO tile configuration (PINTYPE, input
enable) of every constrained pin between the two bitstreams.

`mutate` corrupts a good .asc (flip one LUT truth-table bit of a used logic
cell, drop a tile's clock enable, invert a tile's clock, make an async reset
sync) so a test can show that `check` fails on it.

Exit status: 0 when every check passed, 1 when one failed, 2 on usage or
tool errors. Missing tools are errors, never skips.
"""

import argparse
import json
import os
import random
import re
import shutil
import subprocess
import sys
import tempfile

# ---------------------------------------------------------------- tools


def tool(name):
    env = "I40V_" + re.sub(r"[^A-Z0-9]", "_", name.upper())
    p = os.environ.get(env) or shutil.which(name)
    if not p:
        raise SystemExit(f"i40v: {name} not found on PATH (or {env}); install Project IceStorm / yosys / iverilog / nextpnr-ice40")
    return p


def run(cmd, cwd=None, timeout=3600, stdout_path=None):
    """Run a command; return (rc, stdout, stderr)."""
    try:
        if stdout_path:
            with open(stdout_path, "w") as f:
                p = subprocess.run(cmd, cwd=cwd, stdout=f, stderr=subprocess.PIPE, text=True, timeout=timeout)
            return p.returncode, "", p.stderr
        p = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, timeout=timeout)
        return p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired:
        return 124, "", f"timeout after {timeout}s: {' '.join(cmd)}"


def icebox_module():
    try:
        import icebox  # noqa: F401
    except ImportError:
        base = os.path.dirname(os.path.realpath(tool("icebox_vlog")))
        for cand in (os.path.join(base, "..", "share", "icestorm", "python"), base):
            if os.path.exists(os.path.join(cand, "icebox.py")):
                sys.path.insert(0, os.path.abspath(cand))
                break
    import icebox
    return icebox


# ---------------------------------------------------------------- results


class Report:
    def __init__(self, label):
        self.label = label
        self.rows = []

    def add(self, name, ok, detail):
        self.rows.append((name, ok, detail))
        state = {True: "PASS", False: "FAIL", None: "SKIP"}[ok]
        print(f"[{self.label}] {name:<10} {state}  {detail}", flush=True)

    @property
    def ok(self):
        return all(r[1] is not False for r in self.rows) and any(r[1] for r in self.rows)


# ---------------------------------------------------------------- .asc


def asc_bits(text):
    """Set bits of an .asc: tile bits, non-zero RAM digits, extra bits."""
    bits = set()
    kind, x, y, row = None, 0, 0, 0
    for line in text.splitlines():
        if line.startswith("."):
            f = line.split()
            kind = f[0]
            if kind == ".extra_bit":
                bits.add(tuple(f))
                kind = None
                continue
            x, y = (f[1], f[2]) if len(f) >= 3 else (0, 0)
            row = 0
        elif kind and kind.endswith("_tile") and line and set(line) <= {"0", "1"}:
            for i, c in enumerate(line):
                if c == "1":
                    bits.add((kind, x, y, row, i))
            row += 1
        elif kind == ".ram_data" and line.strip():
            for i, c in enumerate(line.strip()):
                if c != "0":
                    bits.add((kind, x, y, row, i, c))
            row += 1
    return bits


def check_roundtrip(rep, asc, work):
    pack, unpack = tool("icepack"), tool("iceunpack")
    b1, a2, b2, a3 = (os.path.join(work, n) for n in ("rt.bin", "rt.asc", "rt2.bin", "rt2.asc"))
    for cmd in ([pack, asc, b1], [unpack, b1, a2], [pack, a2, b2], [unpack, b2, a3]):
        rc, _, err = run(cmd)
        if rc:
            rep.add("roundtrip", False, f"{os.path.basename(cmd[0])} failed: {err.strip()[:300]}")
            return
    orig, back = asc_bits(open(asc).read()), asc_bits(open(a2).read())
    if orig != back:
        lost, gained = len(orig - back), len(back - orig)
        rep.add("roundtrip", False, f"{lost} bit(s) lost, {gained} gained, e.g. {sorted(orig ^ back)[:3]}")
        return
    if open(a2).read() != open(a3).read():
        rep.add("roundtrip", False, "iceunpack output is not a pack/unpack fixed point")
        return
    rep.add("roundtrip", True, f"{len(orig)} set bits identical after icepack/iceunpack ({os.path.getsize(b1)} byte .bin)")


# ---------------------------------------------------------------- RTL / netlist


def yosys(script, work, name):
    path = os.path.join(work, name + ".ys")
    with open(path, "w") as f:
        f.write(script)
    rc, out, err = run([tool("yosys"), "-q", "-l", os.path.join(work, name + ".log"), "-s", path], cwd=work)
    return rc, out + err


def read_rtl_cmds(rtl):
    return "\n".join(f"read_verilog -sv -defer {os.path.abspath(f)}" for f in rtl)


def build_reference(rtl, top, work):
    """The RTL as yosys elaborates it, as plain Verilog for iverilog, with
    power-on state 0 for every register and memory without an initial value.
    Returns (ref.v path, ports {name: (dir, width)}, has_memory)."""
    ref_v, ref_json = os.path.join(work, "ref.v"), os.path.join(work, "ref.json")
    rc, log = yosys(
        f"""{read_rtl_cmds(rtl)}
hierarchy -check -top {top}
proc
flatten
opt_clean
setundef -zero -init -params
rename {top} ref_top
write_verilog -noattr {ref_v}
write_json {ref_json}
""",
        work,
        "ref",
    )
    if rc:
        raise SystemExit(f"i40v: yosys could not read the RTL:\n{log[-2000:]}")
    mod = json.load(open(ref_json))["modules"]["ref_top"]
    ports = {n: (p["direction"], len(p["bits"])) for n, p in mod["ports"].items()}
    has_mem = any(c["type"].startswith("$mem") for c in mod["cells"].values())
    return ref_v, ports, has_mem


def netlist_ports(net_v, work):
    js = os.path.join(work, "net.json")
    # proc: the netlist has always blocks, which write_json refuses.
    rc, log = yosys(f"read_verilog {net_v}\nhierarchy -top chip\nproc\nwrite_json {js}\n", work, "netports")
    if rc:
        raise SystemExit(f"i40v: yosys could not read the extracted netlist:\n{log[-2000:]}")
    mod = json.load(open(js))["modules"]["chip"]
    out = {}
    for n, p in mod["ports"].items():
        off = p.get("offset", 0)
        out[n] = (p["direction"], len(p["bits"]), off, bool(p.get("upto")))
    return out


def extract(rep, asc, pcf, work):
    net_v = os.path.join(work, "net.v")
    rc, _, err = run([tool("icebox_vlog"), "-p", pcf, "-d", "ct256", "-c", "-s", "-D", asc], stdout_path=net_v)
    if rc or not os.path.getsize(net_v):
        rep.add("extract", False, f"icebox_vlog failed: {err.strip()[-400:]}")
        return None
    text = open(net_v).read()
    nffs = len(re.findall(r"always @", text))
    nluts = len(re.findall(r"^assign .*\?", text, re.M))
    warn = err.strip()
    rep.add("extract", not warn, f"icebox_vlog netlist: {nffs} flip-flops, about {nluts} LUT expressions" + (f"; icebox_vlog: {warn[:300]}" if warn else ""))
    return net_v


def gate_wrapper(rtl_ports, net_ports, work):
    """`gate_top`: the netlist behind the RTL's exact port list. RTL inputs the
    netlist does not have are left unread; RTL output bits it does not drive
    read x (and so never match a known RTL value)."""
    lines = ["module gate_top("]
    decls, conns, assigns = [], [], []
    for name, (d, w) in rtl_ports.items():
        rng = f"[{w - 1}:0] " if w > 1 else ""
        decls.append(f"  {'input' if d == 'input' else 'output'} wire {rng}{esc(name)}")
    lines.append(",\n".join(decls))
    lines.append(");")
    for name, (d, w, off, upto) in net_ports.items():
        if name not in rtl_ports:
            if d == "input":
                conns.append(f".{esc(name)}(1'b0)")
            continue
        rd, rw = rtl_ports[name]
        if (d == "input") != (rd == "input"):
            raise SystemExit(f"i40v: port {name} is {rd} in the RTL but {d} in the netlist")
        lo, hi = off, off + w - 1
        if hi >= rw:
            raise SystemExit(f"i40v: netlist port {name}[{hi}:{lo}] is wider than the RTL's {rw} bits")
        if d == "input":
            sl = f"{esc(name)}" if rw == 1 else f"{esc(name)}[{hi}:{lo}]"
            conns.append(f".{esc(name)}({sl})")
        else:
            wire = f"g_{len(assigns)}"
            assigns.append((name, lo, hi, wire, w))
            conns.append(f".{esc(name)}({wire})")
    body = []
    covered = {}
    for name, lo, hi, wire, w in assigns:
        body.append(f"  wire [{w - 1}:0] {wire};")
        for k in range(lo, hi + 1):
            covered[(name, k)] = f"{wire}[{k - lo}]"
    for name, (d, w) in rtl_ports.items():
        if d == "input":
            continue
        for k in range(w):
            src = covered.get((name, k), "1'bx")
            dst = esc(name) if w == 1 else f"{esc(name)}[{k}]"
            body.append(f"  assign {dst} = {src};")
    lines += body
    lines.append(f"  chip u_chip({', '.join(conns)});")
    lines.append("endmodule")
    path = os.path.join(work, "gate_top.v")
    with open(path, "w") as f:
        f.write("\n".join(lines) + "\n")
    missing = [f"{n}[{k}]" for n, (d, w) in rtl_ports.items() if d != "input" for k in range(w) if (n, k) not in covered]
    return path, missing


def esc(name):
    return name if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_$]*", name) else f"\\{name} "


# ---------------------------------------------------------------- co-simulation


def testbench(rtl_ports, clock, resets, cycles, seed, pulse_resets, work, tag):
    ins = [(n, w) for n, (d, w) in rtl_ports.items() if d == "input"]
    outs = [(n, w) for n, (d, w) in rtl_ports.items() if d != "input"]
    reset_names = {n for n, _ in resets}
    data = [(n, w) for n, w in ins if n != clock and n not in reset_names]
    L = ["`timescale 1ns/1ps", "module tb;"]
    for n, w in ins:
        L.append(f"  reg [{w - 1}:0] i_{n} = 0;")
    for n, w in outs:
        L.append(f"  wire [{w - 1}:0] r_{n}, g_{n};")
    conn = lambda p: ", ".join([f".{esc(n)}(i_{n})" for n, _ in ins] + [f".{esc(n)}({p}_{n})" for n, _ in outs])
    L.append(f"  ref_top u_ref({conn('r')});")
    L.append(f"  gate_top u_gate({conn('g')});")
    L.append("  integer seed, cyc, mism, xbits, k, checks;")
    L.append("  task compare(input integer phase); begin")
    L.append("    checks = checks + 1;")
    for n, w in outs:
        L.append(f"    for (k = 0; k < {w}; k = k + 1)")
        L.append(f"      if (r_{n}[k] === 1'bx || r_{n}[k] === 1'bz) xbits = xbits + 1;")
        L.append(f"      else if (g_{n}[k] !== r_{n}[k]) begin mism = mism + 1;")
        L.append(f"        if (mism <= 10) $display(\"MISMATCH cycle %0d phase %0d {n}[%0d] rtl=%b netlist=%b\", cyc, phase, k, r_{n}[k], g_{n}[k]); end")
    L.append("  end endtask")
    L.append("  task stim; begin")
    for n, w in data:
        L.append(f"    i_{n} = {{" + ", ".join(["$random(seed)"] * ((w + 31) // 32)) + "};")
    L.append("  end endtask")
    L.append("  task resets(input integer start); begin")
    for n, lvl in resets:
        act, inact = (1, 0) if lvl else (0, 1)
        if pulse_resets:
            L.append(f"    if (start) i_{n} = {act}; else if (($random(seed) & 31) == 0) i_{n} = {act}; else if (($random(seed) & 3) == 0) i_{n} = {inact};")
        else:
            L.append(f"    i_{n} = {inact};")
    L.append("  end endtask")
    L.append("  initial begin")
    L.append(f"    seed = {seed}; mism = 0; xbits = 0; checks = 0;")
    for n, lvl in resets:
        L.append(f"    i_{n} = {0 if lvl else 1};")
    L.append("    #1;")
    L.append(f"    for (cyc = 0; cyc < {cycles}; cyc = cyc + 1) begin")
    L.append("      stim; resets(cyc < 3); #4 compare(0);")
    if clock:
        L.append(f"      #5 i_{clock} = 1;")
    else:
        L.append("      #5;")
    L.append("      #2 stim; resets(cyc < 3); #3 compare(1);")
    if clock:
        L.append(f"      #5 i_{clock} = 0;")
    else:
        L.append("      #5;")
    L.append("      #1;")
    L.append("    end")
    L.append(f"    $display(\"RESULT cycles=%0d checks=%0d mismatches=%0d xbits=%0d\", {cycles}, checks, mism, xbits);")
    L.append("    $finish;")
    L.append("  end")
    L.append("endmodule")
    path = os.path.join(work, f"tb_{tag}.v")
    with open(path, "w") as f:
        f.write("\n".join(L) + "\n")
    return path


def cosim(rep, ref_v, gate_v, net_v, rtl_ports, clock, resets, cycles, seed, work):
    modes = [("noreset", False)] + ([("reset", True)] if resets else [])
    for tag, pulse in modes:
        tb = testbench(rtl_ports, clock, resets, cycles, seed + (7 if pulse else 0), pulse, work, tag)
        exe = os.path.join(work, f"sim_{tag}.vvp")
        rc, out, err = run([tool("iverilog"), "-g2012", "-o", exe, tb, ref_v, gate_v, net_v])
        if rc:
            rep.add(f"cosim-{tag}", False, f"iverilog failed: {(out + err).strip()[-600:]}")
            continue
        rc, out, err = run([tool("vvp"), "-n", exe])
        m = re.search(r"RESULT cycles=(\d+) checks=(\d+) mismatches=(\d+) xbits=(\d+)", out)
        if rc or not m:
            rep.add(f"cosim-{tag}", False, f"simulation did not finish: {(out + err).strip()[-600:]}")
            continue
        cyc, checks, mism, xb = map(int, m.groups())
        first = [l for l in out.splitlines() if l.startswith("MISMATCH")][:3]
        what = f"{cyc} cycles, {checks} comparisons, {mism} mismatching output bits"
        if xb:
            what += f", {xb} RTL output bits x (not compared)"
        if resets:
            what += ", resets " + ("pulsed at random" if pulse else "held inactive")
        rep.add(f"cosim-{tag}", mism == 0 and cyc >= 1, what + ("; " + "; ".join(first) if first else ""))


# ---------------------------------------------------------------- formal


def formal(rep, rtl, top, gate_v, net_v, work, timeout):
    aig = os.path.join(work, "miter.aig")
    rc, log = yosys(
        f"""{read_rtl_cmds(rtl)}
hierarchy -check -top {top}
proc
flatten
setundef -zero -init -params
rename {top} gold
design -stash gold
read_verilog {net_v} {gate_v}
hierarchy -top gate_top
proc
flatten
rename gate_top gate
design -stash gate
design -copy-from gold -as gold gold
design -copy-from gate -as gate gate
miter -equiv -flatten -make_outputs -ignore_gold_x gold gate miter
hierarchy -top miter
memory -nomap
memory_map
opt_clean
clk2fflogic
setundef -zero -init
opt -fast
techmap
opt -fast
aigmap
opt_clean
write_aiger -zinit -map {os.path.join(work, 'miter.aim')} {aig}
""",
        work,
        "formal",
    )
    if rc:
        rep.add("formal", False, f"yosys miter failed: {log.strip()[-600:]}")
        return
    rc, out, err = run([tool("yosys-abc"), "-c", f"read_aiger {aig}; strash; print_stats; pdr -T {timeout}"], timeout=timeout + 60)
    text = out + err
    stats = re.search(r"lat\s*=\s*(\d+).*?and\s*=\s*(\d+)", text)
    size = f" ({stats.group(1)} latches, {stats.group(2)} ANDs)" if stats else ""
    if re.search(r"Property proved|UNSAT", text):
        rep.add("formal", True, f"RTL == netlist for every reachable state from power-on (yosys miter + clk2fflogic, ABC pdr){size}")
    elif re.search(r"Output \d+ of miter .* was asserted in frame (\d+)", text):
        fr = re.search(r"asserted in frame (\d+)", text).group(1)
        rep.add("formal", False, f"counterexample: RTL and netlist outputs differ {fr} global-clock steps after power-on{size}")
    else:
        rep.add("formal", None, f"undecided within {timeout}s{size}: {text.strip()[-300:]}")


# ---------------------------------------------------------------- timing


def timing(rep, asc, pcf, fmax_min, work):
    rpt = os.path.join(work, "icetime.txt")
    # Homebrew's icetime does not find its own chipdb; look next to the binary.
    exe = os.path.realpath(tool("icetime"))
    db = os.path.join(os.path.dirname(os.path.dirname(exe)), "share", "icestorm", "chipdb", "chipdb-8k.txt")
    chipdb = ["-C", db] if os.path.isfile(db) else []
    rc, out, err = run([tool("icetime"), "-d", "hx8k", "-P", "ct256", *chipdb, "-p", pcf, "-t", "-r", rpt, asc])
    text = (open(rpt).read() if os.path.exists(rpt) else "") + out + err
    m = re.search(r"Total path delay:\s*([\d.]+)\s*ns\s*\(([\d.]+)\s*MHz\)", text)
    if rc or not m:
        rep.add("timing", False, f"icetime failed: {text.strip()[-400:]}")
        return
    ns, mhz = float(m.group(1)), float(m.group(2))
    rep.add("timing", mhz >= fmax_min, f"icetime: critical path {ns:.2f} ns, Fmax {mhz:.1f} MHz (need {fmax_min:g} MHz)")


# ---------------------------------------------------------------- check


def check(args, asc, label):
    work = args.work
    os.makedirs(work, exist_ok=True)
    rep = Report(label)
    check_roundtrip(rep, asc, work)
    ref_v, rtl_ports, has_mem = build_reference(args.rtl, args.top, work)
    clock = args.clock if args.clock else ("clk" if "clk" in rtl_ports else None)
    if clock and clock not in rtl_ports:
        raise SystemExit(f"i40v: --clock {clock} is not a port of {args.top}")
    resets = []
    for r in args.reset or []:
        name, _, lvl = r.partition("=")
        if name not in rtl_ports:
            raise SystemExit(f"i40v: --reset {name} is not a port of {args.top}")
        resets.append((name, int(lvl or "1")))
    net_v = extract(rep, asc, args.pcf, work)
    if net_v:
        gate_v, missing = gate_wrapper(rtl_ports, netlist_ports(net_v, work), work)
        if missing:
            rep.add("ports", False, f"netlist drives no pad for RTL output bit(s) {', '.join(missing[:8])}")
        cosim(rep, ref_v, gate_v, net_v, rtl_ports, clock, resets, args.cycles, args.seed, work)
        if args.formal == "off":
            rep.add("formal", None, "not run (--formal off)")
        elif has_mem and args.formal == "auto":
            rep.add("formal", None, "not run: the design has memories (block RAM is checked by co-simulation only)")
        else:
            formal(rep, args.rtl, args.top, gate_v, net_v, work, args.formal_timeout)
    timing(rep, asc, args.pcf, args.fmax, work)
    verdict = "PASS" if rep.ok else "FAIL"
    print(f"[{label}] VERDICT {verdict}", flush=True)
    return rep


# ---------------------------------------------------------------- golden


def golden_build(args):
    work = os.path.join(args.work, "golden")
    os.makedirs(work, exist_ok=True)
    js, asc = os.path.join(work, "golden.json"), os.path.join(work, "golden.asc")
    rc, log = yosys(f"{read_rtl_cmds(args.rtl)}\nsynth_ice40 -top {args.top} -json {js}\n", work, "synth")
    if rc:
        raise SystemExit(f"i40v: yosys synth_ice40 failed:\n{log[-1500:]}")
    rc, out, err = run([tool("nextpnr-ice40"), "--hx8k", "--package", "ct256", "--pcf", args.pcf, "--json", js, "--asc", asc, "--seed", "1", "-q",
                        "--log", os.path.join(work, "nextpnr.log")])
    if rc:
        raise SystemExit(f"i40v: nextpnr-ice40 failed:\n{(out + err)[-1500:]}")
    return asc


def io_config(asc, pcf):
    """PINTYPE and input-enable state of each constrained pin."""
    icebox = icebox_module()
    ic = icebox.iceconfig()
    ic.read_file(asc)
    pins = {p: (x, y, z) for p, x, y, z in icebox.pinloc_db["8k-ct256"]}
    cons = []
    for line in open(pcf):
        f = line.split("#")[0].split()
        if f and f[0] == "set_io":
            args = [a for a in f[1:] if not a.startswith("-")]
            cons.append((args[-2], args[-1]))
    ieren = {(r[3], r[4], r[5]): (r[0], r[1], r[2]) for r in ic.ieren_db()}
    out = {}
    for port, pin in cons:
        x, y, z = pins[pin]
        tc = icebox.tileconfig(ic.io_tiles[(x, y)])
        pt = ["0"] * 6
        for e in ic.tile_db(x, y):
            if e[1] == f"IOB_{z}" and e[2].startswith("PINTYPE_") and tc.match(e[0]):
                pt[int(e[2][8:])] = "1"
        ie = ""
        if (x, y, z) in ieren:
            ix, iy, iz = ieren[(x, y, z)]
            itc = icebox.tileconfig(ic.io_tiles[(ix, iy)])
            for e in ic.tile_db(ix, iy):
                if e[1] == "IoCtrl" and e[2] in (f"IE_{iz}", f"REN_{iz}"):
                    ie += f"{e[2][:-2]}={'1' if itc.match(e[0]) else '0'} "
        out[port] = (pin, "".join(pt), ie.strip())
    return out


def compare_io(rep, ours, golden, pcf, clock):
    a, b = io_config(ours, pcf), io_config(golden, pcf)
    bad, clk_note = [], ""
    for port, (pin, pt, ie) in a.items():
        gpin, gpt, gie = b[port]
        if (pt, ie) != (gpt, gie):
            if port == clock:
                clk_note = f"; clock pin {pin} differs as expected (Helion takes it through fabric to a global buffer, nextpnr uses the pad's direct global input): ours PINTYPE {pt} {ie}, nextpnr {gpt} {gie}"
            else:
                bad.append(f"{port}@{pin}: ours PINTYPE {pt} {ie}, nextpnr {gpt} {gie}")
    n = len(a) - (1 if clk_note else 0)
    rep.add("io-bits", not bad, (f"{n} pin(s) configured as nextpnr configures them" if not bad else "; ".join(bad[:6])) + clk_note)


# ---------------------------------------------------------------- mutate


def mutate(asc_in, asc_out, kind, seed):
    """Corrupt one feature of a good bitstream; returns a description."""
    icebox = icebox_module()
    ic = icebox.iceconfig()
    ic.read_file(asc_in)
    rnd = random.Random(seed)
    tiles = sorted(ic.logic_tiles)

    def lc_bits(tile, lc):
        # LC_i: 10 bits each in rows 2i and 2i+1, columns 36..45.
        return [(2 * lc + (j // 10), 36 + (j % 10)) for j in range(20)]

    def setbit(tile, r, c, v):
        row = tile[r]
        tile[r] = row[:c] + v + row[c + 1:]

    cands = []
    for t in tiles:
        tile = ic.logic_tiles[t]
        for lc in range(8):
            b = lc_bits(tile, lc)
            lut = [tile[r][c] for (r, c) in [b[i] for i in (4, 14, 15, 5, 6, 16, 17, 7, 3, 13, 12, 2, 1, 11, 10, 0)]]
            dff = tile[b[9][0]][b[9][1]] == "1"
            cands.append((t, lc, b, lut, dff))
    used = [c for c in cands if "1" in c[3] and "0" in c[3]]
    if kind == "lutbit":
        t, lc, b, lut, _ = rnd.choice(used)
        r, c = b[rnd.choice([4, 14, 15, 5, 6, 16, 17, 7, 3, 13, 12, 2, 1, 11, 10, 0])]
        tile = ic.logic_tiles[t]
        setbit(tile, r, c, "0" if tile[r][c] == "1" else "1")
        what = f"flipped one LUT truth-table bit of logic cell ({t[0]},{t[1]}) LC{lc}"
    elif kind in ("cen", "negclk", "asyncsr"):
        hit = None
        for t in tiles:
            tile = ic.logic_tiles[t]
            tc = icebox.tileconfig(tile)
            ffs = [c for c in cands if c[0] == t and c[4]]
            if not ffs:
                continue
            if kind == "cen":
                ent = [e for e in ic.tile_db(t[0], t[1]) if e[1] == "buffer" and e[3] == "lutff_global/cen" and tc.match(e[0])]
                if ent:
                    for bitname in ent[0][0]:
                        m = re.fullmatch(r"!?B(\d+)\[(\d+)\]", bitname)
                        if not bitname.startswith("!"):
                            setbit(tile, int(m.group(1)), int(m.group(2)), "0")
                    hit = f"dropped the clock enable of logic tile ({t[0]},{t[1]}) (its {len(ffs)} flip-flops now always load)"
            elif kind == "negclk":
                setbit(tile, 0, 0, "0" if tile[0][0] == "1" else "1")
                hit = f"inverted the clock polarity (NegClk) of logic tile ({t[0]},{t[1]})"
            elif kind == "asyncsr":
                for (_, lc, b, _, _) in ffs:
                    r, c = b[19]
                    if tile[r][c] == "1":
                        setbit(tile, r, c, "0")
                        hit = f"made the asynchronous reset of ({t[0]},{t[1]}) LC{lc} synchronous"
                        break
            if hit:
                break
        if not hit:
            raise SystemExit(f"i40v mutate: the bitstream has no {kind} feature to corrupt")
        what = hit
    else:
        raise SystemExit(f"i40v mutate: unknown kind {kind}")
    write_asc(ic, asc_in, asc_out)
    return what


def write_asc(ic, asc_in, asc_out):
    """Rewrite asc_in with the (modified) logic tiles of ic."""
    out, kind, key, row = [], None, None, 0
    for line in open(asc_in).read().splitlines():
        if line.startswith("."):
            f = line.split()
            kind = f[0]
            key = (int(f[1]), int(f[2])) if kind == ".logic_tile" else None
            row = 0
            out.append(line)
        elif kind == ".logic_tile" and key is not None and line and set(line) <= {"0", "1"}:
            out.append(ic.logic_tiles[key][row])
            row += 1
        else:
            out.append(line)
    with open(asc_out, "w") as f:
        f.write("\n".join(out) + "\n")


# ---------------------------------------------------------------- main


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)

    def common(p):
        p.add_argument("--rtl", action="append", required=True, help="RTL source (repeat)")
        p.add_argument("--top", required=True)
        p.add_argument("--pcf", required=True)
        p.add_argument("--work", default=None, help="scratch directory (default: a temporary one, removed)")
        p.add_argument("--clock", default=None, help="clock input (default clk if present)")
        p.add_argument("--reset", action="append", help="reset input and its active level, e.g. arst_n=0 (repeat)")
        p.add_argument("--cycles", type=int, default=10000)
        p.add_argument("--seed", type=int, default=1)
        p.add_argument("--formal", choices=["auto", "on", "off"], default="auto")
        p.add_argument("--formal-timeout", type=int, default=300)
        p.add_argument("--fmax", type=float, default=12.0, help="minimum Fmax in MHz (HX8K breakout clock: 12)")

    c = sub.add_parser("check")
    common(c)
    c.add_argument("--asc", required=True)
    g = sub.add_parser("golden")
    common(g)
    g.add_argument("--compare-io", default=None, help="another .asc whose IO configuration must match the golden one")
    m = sub.add_parser("mutate")
    m.add_argument("--asc", required=True)
    m.add_argument("--out", required=True)
    m.add_argument("--kind", required=True, choices=["lutbit", "cen", "negclk", "asyncsr"])
    m.add_argument("--seed", type=int, default=1)
    args = ap.parse_args()

    if args.cmd == "mutate":
        print("i40v mutate:", mutate(args.asc, args.out, args.kind, args.seed))
        return 0

    tmp = None
    if not args.work:
        tmp = tempfile.mkdtemp(prefix="i40v-")
        args.work = tmp
    args.pcf = os.path.abspath(args.pcf)
    try:
        if args.cmd == "check":
            rep = check(args, os.path.abspath(args.asc), "i40v")
        else:
            asc = golden_build(args)
            base = args.work
            args.work = os.path.join(base, "golden")
            rep = check(args, asc, "golden")
            if args.compare_io:
                compare_io(rep, os.path.abspath(args.compare_io), asc, args.pcf, args.clock or "clk")
                print(f"[golden] VERDICT {'PASS' if rep.ok else 'FAIL'}", flush=True)
        return 0 if rep.ok else 1
    finally:
        if tmp:
            shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
