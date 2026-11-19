#!/usr/bin/env python3
"""EXP_LUT for examples/zedboard_fill.sv, from its formula and not from the RTL.

    python3 examples/zedboard_fill_expected.py [LANES [NCYC]]     # default 1040 1024
"""
import sys

M = 0xFFFFFFFF


def seed(l):
    h = ((l + 1) * 0x9E3779B1) & M
    h ^= h >> 16
    h = (h * 0x85EBCA6B) & M
    h ^= h >> 13
    return h | 1


def rotl(v, r):
    r %= 32
    return ((v << r) | (v >> (32 - r))) & M if r else v


def step(s):
    b = lambda k: (s >> (k % 32)) & 1
    n = 0
    for j in range(32):
        n |= (b(j + 31) ^ (b(j + 3) & b(j + 7)) ^ b(j + 12)) << j
    return n


def expected(lanes, ncyc):
    sig, moving = 0, 0
    for l in range(lanes):
        s = seed(l)
        seen = {s}
        for _ in range(ncyc):
            s = step(s)
            seen.add(s)
        moving += len(seen) > ncyc // 2
        sig ^= rotl(s, l % 32)
    return sig, moving


if __name__ == "__main__":
    lanes = int(sys.argv[1]) if len(sys.argv) > 1 else 1040
    ncyc = int(sys.argv[2]) if len(sys.argv) > 2 else 1024
    sig, moving = expected(lanes, ncyc)
    print(f"LANES={lanes} NCYC={ncyc} EXP_LUT=32'h{sig:08X}  ({moving} of {lanes} lanes visit more than {ncyc // 2} distinct states)")
