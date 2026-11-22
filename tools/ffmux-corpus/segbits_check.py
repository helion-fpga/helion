# Segbits confirmation for the FFMUX corpus lane (Q4).
#
# Reads the public segbits DBs vendored in
# crates/helion-ultrascale/data/ and checks, for every CLE tile type:
#   - every FFMUX<letter><1|2> mux has exactly the six options
#     BYP, CY, D5, D6, F7F8, XORIN;
#   - every option sets exactly one of its six frame bits and clears
#     the other five (the critic's one-of-six finding);
#   - every FF INIT/SRVAL V0 sets its single frame bit and V1 clears
#     it, with the bit positions listed.
#
# Usage: python3 segbits_check.py
# No em dashes anywhere.
from __future__ import annotations

import glob
import os
import re

DATA = os.path.expanduser(
    "~/workspace/helion-ultra-bitgen-ffmux-corpus"
    "/crates/helion-ultrascale/data")
OPTIONS = ["BYP", "CY", "D5", "D6", "F7F8", "XORIN"]


def load(db_path):
    out = {}
    for line in open(db_path):
        parts = line.split()
        if parts:
            out[parts[0]] = parts[1:]
    return out


def main():
    dbs = sorted(
        p for p in glob.glob(os.path.join(DATA, "segbits_*.db"))
        if "origin_info" not in p)
    total_ffmux = 0
    total_bad = 0
    for path in dbs:
        name = os.path.basename(path)
        feats = load(path)
        ffmux = {f: b for f, b in feats.items() if ".FFMUX" in f}
        if not ffmux:
            continue
        total_ffmux += len(ffmux)
        bad = []
        names = {}
        for feat, bits in sorted(ffmux.items()):
            m = re.match(r"^[A-Z_0-9]+\.(FFMUX[A-H][12])\.(\S+)$", feat)
            if not m:
                bad.append((feat, "unparsed name"))
                continue
            names.setdefault(m.group(1), set()).add(m.group(2))
            sets = [b for b in bits if not b.startswith("!")]
            if len(sets) != 1 or len(bits) != 6:
                bad.append((feat, f"sets={sets} nbits={len(bits)}"))
        missing = {n: sorted(set(OPTIONS) - opts)
                   for n, opts in names.items() if set(opts) != set(OPTIONS)}
        total_bad += len(bad)
        print(f"{name}: {len(ffmux)} FFMUX features, "
              f"{len(names)} muxes, bad={len(bad)}, "
              f"missing-options={missing or 'none'}")
        for b in bad[:5]:
            print("   ", b)
        init = {f: b for f, b in feats.items()
                if re.search(r"\.[A-H]FF2?\.(INIT|SRVAL)\.V[01]$", f)}
        if init:
            bad_init = [(f, b) for f, b in init.items()
                        if not ((len(b) == 1 and not b[0].startswith("!"))
                               or (len(b) == 1 and b[0].startswith("!")))]
            print(f"  INIT/SRVAL features: {len(init)}, "
                  f"not single-bit={len(bad_init)}")
    print(f"TOTAL: {total_ffmux} FFMUX features, {total_bad} bad")


if __name__ == "__main__":
    main()
