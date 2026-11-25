#!/usr/bin/env python3
"""Vivado's per-arc delays (reference.tcl output) next to Helion's 7-series constants.

  compare_timing.py [--timing-rs crates/helion-xilinx7/src/timing.rs] <ref-dir>...

Reads cell_arcs.tsv, paths.tsv, summary.tsv and net_delays.tsv from every
<ref-dir>, pools them, and prints a Markdown table: for each delay Helion
models as one constant, Vivado's slow-corner min / median / max over the arcs
seen, and the error of Helion's constant against Vivado's max.
Constants are read from the given timing.rs (`pub const NAME_PS: u32 = N;`).
"""
import argparse
import csv
import os
import re
import statistics
import sys
from collections import defaultdict


def tsv(path):
    if not os.path.exists(path):
        return []
    with open(path, newline="") as f:
        return list(csv.DictReader(f, delimiter="\t", quoting=csv.QUOTE_NONE))


def ps(v):
    try:
        return round(float(v) * 1000)
    except (TypeError, ValueError):
        return None


def slow_max(r):
    vals = [ps(r[k]) for k in ("DELAY_SLOW_MAX_RISE", "DELAY_SLOW_MAX_FALL")]
    vals = [v for v in vals if v is not None]
    return max(vals) if vals else None


# (label, Helion constant, predicate on an arc row)
CLASSES = [
    ("LUT input -> O (any pin)", "LUT_PS",
     lambda r: r["REF_NAME"].startswith("LUT") and r["TYPE"] == "combinational"),
    ("FF C -> Q", "FF_CKQ_PS",
     lambda r: r["REF_NAME"].startswith("FD") and r["TO"] == "Q" and "Clk to Q" in r["TYPE"]),
    ("FF D setup", "FF_SETUP_PS",
     lambda r: r["REF_NAME"].startswith("FD") and r["TO"] == "D" and r["TYPE"] == "setup"),
    ("FF CE setup", None,
     lambda r: r["REF_NAME"].startswith("FD") and r["TO"] == "CE" and r["TYPE"] == "setup"),
    ("FF D hold", None,
     lambda r: r["REF_NAME"].startswith("FD") and r["TO"] == "D" and r["TYPE"] == "hold"),
    ("CARRY4 S -> O", "CARRY_S_TO_O_PS",
     lambda r: r["REF_NAME"] == "CARRY4" and r["FROM"].startswith("S") and r["TO"].startswith("O")),
    ("CARRY4 CI -> CO", "CARRY_CI_TO_CO_PS",
     lambda r: r["REF_NAME"] == "CARRY4" and r["FROM"] in ("CI", "CYINIT") and r["TO"].startswith("CO")),
    ("RAMB CLK -> DO (no DO_REG)", "BRAM_CKO_PS",
     lambda r: r["REF_NAME"].startswith("RAMB") and "CLK" in r["FROM"] and r["TO"].startswith("DO")
     and "Clk to Q" in r["TYPE"]),
    ("RAMB input setup", "BRAM_SETUP_PS",
     lambda r: r["REF_NAME"].startswith("RAMB") and r["TYPE"] == "setup"),
    ("DSP48E1 CLK -> P", None,
     lambda r: r["REF_NAME"] == "DSP48E1" and r["TO"].startswith("P") and "Clk to Q" in r["TYPE"]),
    ("BUFG I -> O", None, lambda r: r["REF_NAME"].startswith("BUFG") and r["TYPE"] == "combinational"),
    ("IBUF I -> O", None, lambda r: r["REF_NAME"] == "IBUF" and r["TYPE"] == "combinational"),
    ("OBUF I -> O", None, lambda r: r["REF_NAME"] == "OBUF" and r["TYPE"] == "combinational"),
]


def site_xy(s):
    m = re.search(r"_X(\d+)Y(\d+)$", s or "")
    return (int(m.group(1)), int(m.group(2))) if m else None


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--timing-rs")
    ap.add_argument("refs", nargs="+")
    a = ap.parse_args()
    consts = {}
    if a.timing_rs:
        for m in re.finditer(r"pub const (\w+_PS): u32 = (\d+);", open(a.timing_rs).read()):
            consts[m.group(1)] = int(m.group(2))

    arcs, paths, nets = [], [], []
    print("# Vivado reference timing vs Helion constants\n")
    print("| design | clock | period ns | WNS ns | WHS ns | LUT | FF | CARRY | RAMB18 | RAMB36 | DSP |")
    print("|---|---|---|---|---|---|---|---|---|---|---|")
    for d in a.refs:
        arcs += tsv(os.path.join(d, "cell_arcs.tsv"))
        paths += tsv(os.path.join(d, "paths.tsv"))
        nets += tsv(os.path.join(d, "net_delays.tsv"))
        s = {r["key"]: r["value"] for r in tsv(os.path.join(d, "summary.tsv"))}
        clocks = sorted({k.split(".")[1] for k in s if k.startswith("clock.")})
        for c in clocks:
            print(f"| {os.path.basename(d.rstrip('/'))} | {c} | {s.get(f'clock.{c}.period_ns')} | "
                  f"{s.get(f'clock.{c}.wns_ns')} | {s.get(f'clock.{c}.whs_ns')} | {s.get('cells.lut')} | "
                  f"{s.get('cells.ff')} | {s.get('cells.carry')} | {s.get('cells.ramb18')} | "
                  f"{s.get('cells.ramb36')} | {s.get('cells.dsp')} |")

    print("\n## Cell arcs (slow corner, max of rise/fall, ps)\n")
    print("| arc | n | Vivado min | median | max | Helion constant | Helion - Vivado max |")
    print("|---|---|---|---|---|---|---|")
    for label, const, pred in CLASSES:
        vals = [v for r in arcs if pred(r) for v in [slow_max(r)] if v is not None]
        if not vals:
            continue
        hv = consts.get(const) if const else None
        err = f"{hv - max(vals):+d}" if hv is not None else ""
        print(f"| {label} | {len(vals)} | {min(vals)} | {round(statistics.median(vals))} | {max(vals)} | "
              f"{f'{const}={hv}' if hv is not None else '-'} | {err} |")

    lut_pin = defaultdict(list)
    for r in arcs:
        m = re.search(r"\((A\d)\)", r["FROM"])
        if r["REF_NAME"].startswith("LUT") and m and r["TYPE"] == "combinational":
            lut_pin[m.group(1)].append(slow_max(r))
    if lut_pin:
        print("\nLUT delay by physical pin (slow max, ps): " +
              ", ".join(f"{p} {min(v)}-{max(v)}" for p, v in sorted(lut_pin.items())))

    if paths:
        print("\n## Clock terms on the worst paths (ns)\n")
        for k in ("UNCERTAINTY", "CLOCK_PESSIMISM", "SKEW"):
            v = [float(p[k]) for p in paths if p.get(k) not in (None, "")]
            if v:
                print(f"- {k}: min {min(v):.3f}, median {statistics.median(v):.3f}, max {max(v):.3f} over {len(v)} paths")
        if "MARGIN_PS" in consts:
            print(f"- Helion folds skew + uncertainty + model error into MARGIN_PS = {consts['MARGIN_PS']}")

    if nets:
        by_d = defaultdict(list)
        for n in nets:
            a_, b_ = site_xy(n["DRIVER_SITE"]), site_xy(n["LOAD_SITE"])
            if a_ and b_ and n["DRIVER_SITE"].startswith("SLICE") and n["LOAD_SITE"].startswith("SLICE"):
                d = abs(a_[0] - b_[0]) + abs(a_[1] - b_[1])
                by_d[min(d, 40) // 5 * 5].append(int(n["SLOW_MAX"]))
        if by_d:
            print("\n## Slice-to-slice net delay vs Manhattan site distance (slow max, ps)\n")
            print("| distance | loads | median | p90 |")
            print("|---|---|---|---|")
            for d in sorted(by_d):
                v = sorted(by_d[d])
                print(f"| {d}-{d + 4}{'+' if d == 40 else ''} | {len(v)} | {v[len(v) // 2]} | {v[int(len(v) * 0.9)]} |")


if __name__ == "__main__":
    sys.exit(main())
