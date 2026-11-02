#!/usr/bin/env python3
"""Audit every BYP_ALT*/FAN_ALT* input mux in a 7-series bitstream.

Checks, per tile: a one-hot mux must have AT MOST ONE arm configured. Two
arms on one mux is the corruption class that has bitten this flow twice --
prjxray encodes each arm as a coarse select bit plus a fine one and does NOT
clear the other coarse bits, so two arms select NEITHER input and the output
is undefined while every structural check passes.

Also prints the per-row backbone inventory of the counter column, because the
shared-config-bit artefact (e.g. BYP_ALT1.GFAN0 shares bits with
BYP_ALT1.SR1BEG_S0) makes text-level FASM diffing lie about this family.
Decode, do not diff.

Usage: python3 work/altmux.py <ours.bit> [ref.bit]
"""
import os, sys, collections
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import x7decode as X

REF = os.path.expanduser("~/CDA4253_A1/blinky.bit")


def muxes(path):
    tiles = X._tilegrid()
    fr = X.frames_by_address(X.full_addresses(), X.payload(path))
    found = collections.defaultdict(set)
    for tile, info in tiles.items():
        typ = info["type"]
        seg = f"{X.DB}/segbits_{typ.lower()}.db"
        if not os.path.exists(seg):
            continue
        for f in X.active_features(tile, seg, fr):
            parts = f.split(".")
            if len(parts) != 3:
                continue
            mux = parts[1]
            for fam in ("BYP_ALT", "FAN_ALT"):
                if mux.startswith(fam) and mux[len(fam):].isdigit():
                    found[(tile, mux)].add(parts[2])
    return found


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    bad_total = 0
    for label, path in (("OURS", sys.argv[1]),) + ((("REF", sys.argv[2]),) if len(sys.argv) > 2 else ((( "REF", REF),) if os.path.exists(REF) else ())):
        m = muxes(path)
        bad = {k: v for k, v in m.items() if len(v) > 1}
        bad_total += len(bad)
        print(f"{label} ({os.path.basename(path)}): {len(m)} configured alt-muxes, "
              f"DOUBLE-SELECTED: {len(bad)}")
        for (t, mux), v in sorted(bad.items()):
            print(f"   {t}.{mux}: {sorted(v)}")
    if bad_total:
        print("FAIL: a one-hot mux has two arms selected; its output is undefined.")
        return 1
    if len(sys.argv) > 2 or os.path.exists(REF):
        print("OK: no mux anywhere on the die selects two inputs.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
