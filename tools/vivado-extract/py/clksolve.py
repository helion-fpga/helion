#!/usr/bin/env python3
"""Solve clock-tile PIP bits from clkfuzz.tcl specimens (prjxray segmatch style).

  clksolve.py <specdir> <tilegrid.json> <xray-db-dir> [--types T1,T2] [--min-on N]
  clksolve.py selftest

One sample = one tile of a tracked type in one specimen: its set config bits
(tile-relative "FF_BBB", as in X-Ray segbits) and the PIPs Vivado used there.
For each PIP tag, candidate bits = bits set in every sample that uses it, minus
bits set in every sample of that type. A sample without the PIP that still has
the whole candidate set is a conflict. PIPs X-Ray already knows check the solver.
Prints TSV: type tag n_on status bits xray_bits agree.
"""
import glob
import json
import os
import re
import sys
from collections import defaultdict

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
TYPES = "HCLK_CMT,HCLK_CMT_L,CLK_HROW_TOP_R,CLK_HROW_BOT_R,CLK_BUFG_REBUF,CLK_BUFG_TOP_R,CLK_BUFG_BOT_R"
ECC_WORD, ECC_BITS = 50, 13  # 7-series frame ECC: word 50 bits 0-12


def pip_tag(name):
    """'HCLK_CMT_X8Y26/HCLK_CMT.SRC->>DST' -> ('HCLK_CMT', 'HCLK_CMT.DST.SRC')."""
    m = re.match(r"[^/]+/([^.]+)\.(\S+?)(<<->>|->>|->)(\S+)$", name)
    # shortcut: a bidirectional pip's name does not say which way the net used it, so it is
    # skipped (only CLK_BUFG_REBUF GCLK TOP/BOT on 7-series, which X-Ray already has); read
    # the direction from the net's ROUTE property if one ever needs solving.
    return (m[1], f"{m[1]}.{m[4]}.{m[2]}") if m and m[3] != "<<->>" else (None, None)


def tile_bits(frames, b):
    base, nfr, off, nw = int(b["baseaddr"], 16), b["frames"], b["offset"], b["words"]
    out = set()
    for minor in range(nfr):
        fr = frames.get(base + minor)
        if not fr:
            continue
        for w in range(off, off + nw):
            v = fr[w]
            while v:
                bit = (v & -v).bit_length() - 1
                v &= v - 1
                if not (w == ECC_WORD and bit < ECC_BITS):
                    out.add(f"{minor:02d}_{(w - off) * 32 + bit}")
    return out


def load_samples(specdir, grid, types):
    import bitframes
    import gzip
    tiles = {k: v for k, v in grid.items() if v["type"] in types and "CLB_IO_CLK" in v.get("bits", {})}
    samples = []  # (type, bits, tags)
    for pf in sorted(glob.glob(os.path.join(specdir, "spec_*.pips.tsv*"))):
        sid = os.path.basename(pf).split(".")[0]
        bf = next(iter(glob.glob(os.path.join(specdir, sid + ".bit*"))), None)
        if not bf:
            continue
        used = defaultdict(set)
        op = gzip.open if pf.endswith(".gz") else open
        for line in op(pf, "rt"):
            f = line.rstrip("\n").split("\t")
            if len(f) >= 4 and f[1] in tiles:
                used[f[1]].add(pip_tag(f[3])[1])
        _, frames = bitframes.frames(bf)
        for t, v in tiles.items():
            samples.append((v["type"], tile_bits(frames, v["bits"]["CLB_IO_CLK"]), used[t] - {None}))
    return samples


def solve(samples, min_on=1):
    by_type = defaultdict(list)
    for ty, bits, tags in samples:
        by_type[ty].append((bits, tags))
    res = []
    for ty, ss in sorted(by_type.items()):
        always = set.intersection(*(b for b, _ in ss))
        for tag in sorted(set().union(*(t for _, t in ss))):
            on = [b for b, t in ss if tag in t]
            if len(on) < min_on:
                continue
            cand = set.intersection(*on) - always
            off_hits = sum(1 for b, t in ss if tag not in t and cand <= b) if cand else 0
            # A pip used in every sample shares the "always" bits; it cannot be separated yet.
            status = "UNSEPARABLE" if len(on) == len(ss) else "NOBITS" if not cand else ("CONFLICT" if off_hits else "OK")
            res.append((ty, tag, len(on), status, cand))
    return res


def xray_segbits(dbdir, ty):
    p = os.path.join(dbdir, f"segbits_{ty.lower()}.db")
    out = {}
    if os.path.exists(p):
        for line in open(p):
            f = line.split()
            if f:
                out[f[0]] = {b for b in f[1:] if not b.startswith("!")}
    return out


def report(res, dbdir):
    db = {}
    print("type\ttag\tn_on\tstatus\tbits\txray_bits\tagree")
    for ty, tag, n, st, cand in res:
        if ty not in db:
            db[ty] = xray_segbits(dbdir, ty)
        # Bits X-Ray gives to non-PIP features (e.g. *_USED) ride along with PIPs; drop them.
        feat = set().union(*(v for k, v in db[ty].items() if k.count(".") != 2)) if db[ty] else set()
        bits = cand - feat
        xb = db[ty].get(tag)
        agree = "" if xb is None else ("yes" if xb == bits else "NO")
        print(f"{ty}\t{tag}\t{n}\t{st}\t{' '.join(sorted(bits))}\t{'' if xb is None else ' '.join(sorted(xb))}\t{agree}")


def selftest():
    import random
    rnd = random.Random(1)
    # Two-hot 4x4 mux in one tile type: pip (r, c) sets row bit r and column bit c.
    truth = {f"T.D.S{r}{c}": {f"00_{r}", f"01_{c}"} for r in range(4) for c in range(4)}
    samples = []
    for _ in range(300):
        tag = rnd.choice(sorted(truth)) if rnd.random() < 0.8 else None
        bits = {"05_5"} | (truth[tag] if tag else set()) | {f"09_{rnd.randrange(40)}"}
        samples.append(("T", bits, {tag} if tag else set()))
    got = {tag: cand for _, tag, _, st, cand in solve(samples, 3) if st == "OK"}
    bad = [t for t in got if got[t] != truth[t]]
    assert got and not bad, bad
    assert pip_tag("HCLK_CMT_X8Y26/HCLK_CMT.HCLK_CMT_CK_IN0->>HCLK_CMT_MUX_CLK_0") == (
        "HCLK_CMT", "HCLK_CMT.HCLK_CMT_MUX_CLK_0.HCLK_CMT_CK_IN0")
    assert pip_tag("T_X1Y1/T.A<<->>B") == (None, None)
    print(f"selftest ok: {len(got)} of {len(truth)} pips solved exactly")


def main():
    a = sys.argv[1:]
    if a[:1] == ["selftest"]:
        return selftest()
    types = set((a[a.index("--types") + 1] if "--types" in a else TYPES).split(","))
    min_on = int(a[a.index("--min-on") + 1]) if "--min-on" in a else 2
    specdir, grid, dbdir = a[0], json.load(open(a[1])), a[2]
    report(solve(load_samples(specdir, grid, types), min_on), dbdir)


if __name__ == "__main__":
    main()
