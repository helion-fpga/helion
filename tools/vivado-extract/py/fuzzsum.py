#!/usr/bin/env python3
"""Summarize a fuzz.tcl output directory.

  fuzzsum.py <fuzz-out-dir> [--sweep sweep.tsv]

For every <id>.bit.gz next to base.bit.gz: writes <id>.diff.tsv (bitframes.py
diff) and prints one row per variant: property change, number of flipped bits,
the frame columns/rows they fall in, and each bit as row:col:minor:frame_bit
(frame_bit = word*32 + bit) when there are few enough to list. Variants with
no flipped bits are listed as such: that is a result (the value has no config
bit on its own), not a failure. Rows from fuzz.log that failed are repeated.
Writes summary.tsv in the same directory.
"""
import argparse
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import bitframes  # noqa: E402


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("dir")
    ap.add_argument("--max-list", type=int, default=24)
    a = ap.parse_args()
    d = a.dir
    log = {}
    for line in open(os.path.join(d, "fuzz.log")):
        f = line.rstrip("\n").split("\t")
        if len(f) >= 2:
            log[f[0]] = f[1:]
    base = os.path.join(d, "base.bit.gz")
    _, base_frames = bitframes.frames(base)
    us = None
    rows = []
    for vid, info in log.items():
        status = info[0]
        bit = os.path.join(d, f"{vid}.bit.gz")
        if status != "OK" or not os.path.exists(bit):
            rows.append((vid, status, " ".join(info[1:]), "", "", ""))
            continue
        idc, fr = bitframes.frames(bit)
        us = bitframes.is_ultrascale(idc)
        diff = []
        for far in sorted(set(fr) | set(base_frames)):
            wa, wb = base_frames.get(far, ()), fr.get(far, ())
            for w in range(max(len(wa), len(wb))):
                x = (wa[w] if w < len(wa) else 0) ^ (wb[w] if w < len(wb) else 0)
                while x:
                    b = (x & -x).bit_length() - 1
                    x &= x - 1
                    up = (wb[w] if w < len(wb) else 0) >> b & 1
                    diff.append((far, *bitframes.far_fields(far, us), w, b, "0->1" if up else "1->0"))
        with open(os.path.join(d, f"{vid}.diff.tsv"), "w") as f:
            f.write("FAR\tblock\trow\tcol\tminor\tword\tbit\tchange\n")
            for r in diff:
                f.write(f"{r[0]:#010x}\t" + "\t".join(map(str, r[1:])) + "\n")
        cols = sorted({(r[1], r[2], r[3]) for r in diff})
        bits = ",".join(f"{r[2]}:{r[3]}:{r[4]}:{r[5] * 32 + r[6]}{'+' if r[7] == '0->1' else '-'}"
                        for r in diff[: a.max_list])
        if len(diff) > a.max_list:
            bits += f",...(+{len(diff) - a.max_list})"
        rows.append((vid, "OK", " ".join(info[1:]), str(len(diff)),
                     " ".join(f"blk{b}/r{r}/c{c}" for b, r, c in cols[:8]), bits))
    with open(os.path.join(d, "summary.tsv"), "w") as f:
        f.write("id\tstatus\tchange\tbits\tcolumns\tbits(row:col:minor:framebit, +=set -=cleared)\n")
        for r in rows:
            f.write("\t".join(r) + "\n")
    for r in rows:
        print("\t".join(r)[:300])


if __name__ == "__main__":
    main()
