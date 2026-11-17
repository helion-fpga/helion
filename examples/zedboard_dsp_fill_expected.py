#!/usr/bin/env python3
"""Expected signature for examples/zedboard_dsp_fill.sv (and _prims.sv), computed without them.

    python3 examples/zedboard_dsp_fill_expected.py [--lanes N] [--ncyc C]   # print the parameter
    python3 examples/zedboard_dsp_fill_expected.py --sim                    # check the inferred RTL
    python3 examples/zedboard_dsp_fill_expected.py --sim --prims DSP48E1.v  # also the primitive RTL

The lane seeds, LFSR, multiply-accumulate pipeline, fold and signature are
written here from the description in the design's header. --sim runs the
design in Verilator to completion twice, from power-on and again after a
BTNC press: with this value (LD7 and LD6 must light) and with EXP_SIG off
by one bit (LD7 and LD0 lit, LD6 dark). --prims also runs
zedboard_dsp_fill_prims.sv against a DSP48E1 simulation model you supply.
"""
import argparse
import os
import subprocess
import sys
import tempfile

M32 = 0xFFFFFFFF
M48 = (1 << 48) - 1
POLY = 0x04C11DB7


def seed(lane):
    h = ((lane + 1) * 0x9E3779B1) & M32
    h ^= h >> 16
    h = (h * 0x85EBCA6B) & M32
    h ^= h >> 13
    return h | 1


def step(s):
    return ((s << 1) & M32) ^ (POLY if s >> 31 else 0)


def sext(v, w):
    return v - (1 << w) if v >> (w - 1) else v


def rotl48(v, r):
    r %= 48
    return ((v << r) | (v >> (48 - r))) & M48


def expected(lanes, ncyc):
    # acc during cycle c (c = 0 is the first cycle after restart) is the sum
    # of the products of LFSR states S_0 .. S_{c-3}: a, m and acc are three
    # registers, all cleared by restart.
    prods = []
    for l in range(lanes):
        s, p = seed(l), []
        for _ in range(ncyc):
            p.append(sext(s & 0x1FFFFFF, 25) * sext(s >> 14, 18))  # a = S[24:0], b = S[31:14]
            s = step(s)
        prods.append(p)
    accs = [0] * lanes
    sig = 0
    for c in range(ncyc):
        x = 0
        for l in range(lanes):
            x ^= rotl48(accs[l], l)
        sig = ((sig << 1) & M32) ^ (POLY if sig >> 31 else 0) ^ (x & M32) ^ (x >> 32)
        if c >= 2:  # the product of S_{c-2} reaches acc at the end of cycle c
            for l in range(lanes):
                accs[l] = (accs[l] + prods[l][c - 2]) & M48
    return sig


TB = r"""
module tb;
  logic clk = 0, rst = 0;
  logic [7:0] led;
  %s #(%s) dut(.clk, .rst, .led);
  always #5 clk = ~clk;
  initial begin
    repeat (%d) @(posedge clk);
    $display("LED %%08b", led);
    rst = 1;  // BTNC: run the test again
    repeat (5) @(posedge clk);
    rst = 0;
    repeat (%d) @(posedge clk);
    $display("LED %%08b", led);
    $finish;
  end
endmodule
"""


def sim(top, srcs, params, cycles, work):
    tb = os.path.join(work, "tb.sv")
    with open(tb, "w") as f:
        f.write(TB % (top, ", ".join(f".{k}({v})" for k, v in params.items()), cycles, cycles))
    obj = os.path.join(work, "obj")
    r = subprocess.run(["verilator", "--binary", "-Wno-fatal", "--top-module", "tb", "-Mdir", obj, *srcs, tb],
                       capture_output=True, text=True)
    if r.returncode:
        sys.exit(r.stdout + r.stderr)
    out = subprocess.run([os.path.join(obj, "Vtb")], check=True, capture_output=True, text=True).stdout
    return [int(w.split()[0], 2) for w in out.split("LED ")[1:]]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--lanes", type=int, default=192)
    ap.add_argument("--ncyc", type=int, default=1024)
    ap.add_argument("--sim", action="store_true")
    ap.add_argument("--prims", metavar="DSP48E1_MODEL", help="with --sim: also run the primitive design on this model")
    args = ap.parse_args()
    assert rotl48(1 << 47, 1) == 1 and step(0x80000000) == POLY and sext(0x1FFFFFF, 25) == -1
    sig = expected(args.lanes, args.ncyc)
    print(f"LANES = {args.lanes}: {args.lanes} DSP48E1 ({100 * args.lanes / 220:.0f}% of 220), NCYC = {args.ncyc}")
    print(f"    parameter logic [31:0] EXP_SIG = 32'h{sig:08X}")
    if not args.sim:
        return
    here = os.path.dirname(os.path.abspath(__file__))
    designs = [("zedboard_dsp_fill", [os.path.join(here, "zedboard_dsp_fill.sv")])]
    if args.prims:
        designs.append(("zedboard_dsp_fill_prims", [os.path.join(here, "zedboard_dsp_fill_prims.sv"), args.prims]))
    default = args.lanes == 192 and args.ncyc == 1024
    base = {"LANES": args.lanes, "NCYC": args.ncyc}
    # At the default size the design's own parameter value is checked.
    cases = [("right value", {} if default else {**base, "EXP_SIG": f"32'h{sig:08X}"}, 0b11000000),
             ("EXP_SIG corrupted", {**base, "EXP_SIG": f"32'h{sig ^ 1:08X}"}, 0b10000001)]
    bad = 0
    for top, srcs in designs:
        for name, p, want in cases:
            with tempfile.TemporaryDirectory() as work:
                leds = sim(top, srcs, p, args.ncyc + 100, work)
            for run, led in zip(("power-on", "after BTNC"), leds):
                good = led == want  # the LD0 heartbeat is still dark this early
                bad += not good
                print(f"{top}, {name}, {run}: LED {led:08b} (LD7 done={led >> 7 & 1}, LD6 pass={led >> 6 & 1}) {'ok' if good else 'WRONG'}")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
