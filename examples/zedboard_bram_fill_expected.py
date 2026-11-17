#!/usr/bin/env python3
"""Expected signatures for examples/zedboard_bram_fill.sv, computed without it.

    python3 examples/zedboard_bram_fill_expected.py [--nmem N]   # print parameters
    python3 examples/zedboard_bram_fill_expected.py --sim        # check the RTL

The content formula, fold and signature are written here from the
description in the design's header. --sim runs the design in Verilator to
completion three times, from power-on and again after a BTNC press: with
these values (LD7 and LD6 must light), and with EXP_INIT or EXP_SWAP off by
one bit (LD7 lit, LD6 dark, LD0..LD2 the scans that failed).
"""
import argparse
import os
import subprocess
import sys
import tempfile

M32 = 0xFFFFFFFF


def word0(m, a):
    h = (m << 10) | a
    h = (h * 0x9E3779B1) & M32
    h ^= h >> 16
    h = (h * 0x85EBCA6B) & M32
    h ^= h >> 13
    return h & 0xFFFF


def rotl16(v, r):
    r %= 16
    return ((v << r) | (v >> (16 - r))) & 0xFFFF


def swap(v):
    return ((v & 0xFF) << 8) | (v >> 8)


def signature(nmem, word):
    sig = 0
    for a in range(1024):
        x = 0
        for m in range(nmem):
            x ^= rotl16(word(m, a), m)
        sig = (((sig << 1) & M32) ^ (0x04C11DB7 if sig >> 31 else 0)) ^ x
    return sig


def expected(nmem):
    return signature(nmem, word0), signature(nmem, lambda m, a: swap(word0(m, a)))


TB = r"""
module tb;
  logic clk = 0, rst = 0;
  logic [7:0] led;
  zedboard_bram_fill #(%s) dut(.clk, .rst, .led);
  always #5 clk = ~clk;
  initial begin
    repeat (4000) @(posedge clk);
    $display("LED %%08b", led);
    rst = 1;  // BTNC: run the test again
    repeat (5) @(posedge clk);
    rst = 0;
    repeat (4000) @(posedge clk);
    $display("LED %%08b", led);
    $finish;
  end
endmodule
"""


def sim(src, params, work):
    tb = os.path.join(work, "tb.sv")
    with open(tb, "w") as f:
        f.write(TB % ", ".join(f".{k}({v})" for k, v in params.items()))
    obj = os.path.join(work, "obj")
    r = subprocess.run(["verilator", "--binary", "-Wno-fatal", "--top-module", "tb", "-Mdir", obj, src, tb],
                       capture_output=True, text=True)
    if r.returncode:
        sys.exit(r.stdout + r.stderr)
    out = subprocess.run([os.path.join(obj, "Vtb")], check=True, capture_output=True, text=True).stdout
    return [int(w.split()[0], 2) for w in out.split("LED ")[1:]]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--nmem", type=int, default=240)
    ap.add_argument("--sim", action="store_true")
    args = ap.parse_args()
    assert rotl16(0x8001, 1) == 0x0003 and rotl16(0x1234, 0) == 0x1234 and swap(0x1234) == 0x3412
    ei, es = expected(args.nmem)
    print(f"NMEM = {args.nmem}: {args.nmem} RAMB18 ({args.nmem / 2:g} RAMB36 equivalents)")
    print(f"    parameter logic [31:0] EXP_INIT = 32'h{ei:08X},")
    print(f"    parameter logic [31:0] EXP_SWAP = 32'h{es:08X}")
    if not args.sim:
        return
    src = os.path.join(os.path.dirname(os.path.abspath(__file__)), "zedboard_bram_fill.sv")
    def params(a, b):
        return {"NMEM": args.nmem, "EXP_INIT": f"32'h{a:08X}", "EXP_SWAP": f"32'h{b:08X}"}
    # At the default size the design's own parameter values are checked.
    cases = [("right values", {"NMEM": 240} if args.nmem == 240 else params(ei, es), 0b11000000),
             ("EXP_INIT corrupted", params(ei ^ 1, es), 0b10000101),
             ("EXP_SWAP corrupted", params(ei, es ^ 1), 0b10000010)]
    bad = 0
    for name, p, want in cases:
        with tempfile.TemporaryDirectory() as work:
            leds = sim(src, p, work)
        for run, led in zip(("power-on", "after BTNC"), leds):
            good = led == want  # the LD0 heartbeat is still dark this early
            bad += not good
            print(f"{name}, {run}: LED {led:08b} (LD7 done={led >> 7 & 1}, LD6 pass={led >> 6 & 1}) {'ok' if good else 'WRONG'}")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
