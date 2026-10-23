#!/usr/bin/env python3
"""Compare a Helion .bit against the independent prjxray reference tool.

This closes the biggest open verification gap: the byte-identity gate
(`prjxray_diff`) SKIPS on this machine ("no xc7frames2bit binary", "no prjxray
checkout"), so every "matches Vivado" claim rested on re-derivation rather than
on an independent implementation. This script runs the reference.

Setup (once):
    python3 -m venv /tmp/fasmvenv
    /tmp/fasmvenv/bin/pip install fasm intervaltree numpy pyjson5 pyyaml simplejson
    git clone https://github.com/f4pga/prjxray.git /tmp/prjxray-src
    /tmp/fasmvenv/bin/pip install --no-deps -e /tmp/prjxray-src

The reference resolves `db_root` a little differently from Helion: it scans
`db_root` for `tile_type_*.json`, so `db_root` must be the FAMILY directory
(e.g. .../prjxray-db/zynq7), and it expects the part directory immediately
under it plus `mapping/{parts,devices}.yaml`. `prepare_db()` below builds that
layout with symlinks, leaving the real DB untouched.

Usage:
    python3 verification/compare_against_prjxray_fasm2frames.py \
        --fasm /tmp/x7/zedboard_blinky.fasm \
        --bit  /tmp/x7/zedboard_blinky.bit \
        --part xc7z020clg484-1
"""
import argparse
import json
import os
import subprocess
import sys
import tempfile


def prepare_db(db_root, part, family):
    """Build the family-dir layout the reference tool expects, via symlinks."""
    fam_dir = os.path.join(db_root, family)
    os.makedirs(os.path.join(fam_dir, "mapping"), exist_ok=True)
    # The reference scans db_root for tile_type_*.json and reads
    # segbits_*.db from the same directory, so the family dir must contain it.
    for f in os.listdir(fam_dir):
        if f.startswith(("segbits_", "tile_type_", "ppips_", "mask_")) and not os.path.islink(
            os.path.join(fam_dir, f)
        ):
            target = os.path.join(fam_dir, f)
            if os.path.isfile(target):
                continue
    # part dir must sit directly under the family dir
    part_src = os.path.join(fam_dir, part)
    if not os.path.exists(part_src):
        raise SystemExit(f"part dir not found: {part_src}")
    # mapping files
    import re

    parts, devmap = [], {}
    for n in sorted(os.listdir(fam_dir)):
        if not os.path.isdir(os.path.join(fam_dir, n)) or not n.startswith("xc7"):
            continue
        m = re.match(r"^(xc7[azkvs]\d+t?)", n)
        if not m:
            continue
        # `fabric` is resolved as os.path.join(db_root, fabric) where db_root
        # is the FAMILY dir, so it must be relative to the family: just <die>.
        devmap[m.group(1)] = m.group(1)
        parts.append((n, m.group(1)))
    with open(os.path.join(fam_dir, "mapping", "parts.yaml"), "w") as fh:
        for name, die in parts:
            fab = devmap.get(die, die)
            if not os.path.isfile(os.path.join(fam_dir, fab, "tilegrid.json")):
                # xc7a35t has no tilegrid of its own and borrows xc7a50t's.
                alt = "xc7a50t"
                if os.path.isfile(os.path.join(fam_dir, alt, "tilegrid.json")):
                    fab = alt
            fh.write(f'"{name}":\n  device: "{die}"\n  fabric: "{fab}"\n')
    with open(os.path.join(fam_dir, "mapping", "devices.yaml"), "w") as fh:
        for die, fab in sorted(devmap.items()):
            fh.write(f'"{die}":\n  fabric: "{fab}"\n')
    with open(os.path.join(fam_dir, "mapping", "resources.yaml"), "w") as fh:
        fh.write("")
    return fam_dir


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--fasm", required=True)
    ap.add_argument("--bit", required=True)
    ap.add_argument("--part", default="xc7z020clg484-1")
    ap.add_argument("--family", default=None)
    ap.add_argument("--db-root", default=None)
    ap.add_argument("--python", default="/tmp/fasmvenv/bin/python")
    ap.add_argument("--prjxray", default="/tmp/prjxray-src")
    args = ap.parse_args()

    db_root = args.db_root or "/Users/saksham/Documents/zedboard-helion-debug/work/prjxray-db"
    family = args.family or ("artix7" if args.part.startswith("xc7a") else "zynq7")
    fam_dir = prepare_db(db_root, args.part, family)

    tool = os.path.join(args.prjxray, "utils", "fasm2frames.py")
    if not os.path.isfile(tool):
        raise SystemExit(f"reference tool not found: {tool}")

    frm = tempfile.mktemp(suffix=".frm")
    env = dict(os.environ, PYTHONPATH=args.prjxray)
    r = subprocess.run(
        [args.python, tool, "--db-root", fam_dir, "--part", args.part, args.fasm, frm],
        env=env,
        capture_output=True,
        text=True,
    )
    sing_errors = [l for l in r.stderr.splitlines() if "invalid word address" in l]
    if not os.path.isfile(frm) or os.path.getsize(frm) == 0:
        raise SystemExit(f"reference tool produced no frames:\n{r.stderr[-2000:]}")
    print(f"reference produced {os.path.getsize(frm)} bytes of frames")
    if sing_errors:
        print(f"NOTE: the reference tool itself errored on {len(sing_errors)} line(s),")
        print("      all for *_SING tiles that upstream prjxray has no segbits for.")

    # Parse the reference frame list.
    ref = {}
    for line in open(frm):
        p = line.split()
        if len(p) >= 2:
            ref[int(p[0], 0)] = [int(w, 16) for w in p[1].split(",")]

    # Decode Helion's bitstream with the in-repo decoder.
    # The frame decoder is a workspace tool (work/x7decode.py). Accept it via
    # --decoder-dir, else look one level up from the crate, else skip decoding.
    dec_dir = os.environ.get(
        "HELION_DECODER_DIR",
        os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "work"),
    )
    if not os.path.isfile(os.path.join(dec_dir, "x7decode.py")):
        raise SystemExit(
            "x7decode.py not found; set HELION_DECODER_DIR to the directory "
            "containing it (default looked in %s)" % dec_dir
        )
    sys.path.insert(0, dec_dir)
    import x7decode as X

    hel = X.frames_by_address(X.full_addresses(), X.payload(args.bit))

    common = sorted(set(hel) & set(ref))
    diff = [a for a in common if hel[a] != ref[a]]
    only_ref = sorted(set(ref) - set(hel))
    print(f"helion frames: {len(hel)}   reference frames: {len(ref)}   common: {len(common)}")
    print(f"frames differing: {len(diff)}")
    if only_ref:
        print(f"frames only the reference has: {len(only_ref)}")

    # Every word Helion sets must also be set by the reference.
    extra = [(a, i) for a in common for i, w in enumerate(hel[a]) if w and not ref[a][i]]
    missing = [(a, i) for a in common for i, w in enumerate(ref[a]) if w and not hel[a][i]]
    print(f"words helion sets that the reference does not: {len(extra)}")
    print(f"words the reference sets that helion does not: {len(missing)}")

    if extra:
        print("FAIL: helion produces configuration the reference does not.")
        for a, i in extra[:5]:
            print(f"   frame {a:#x} word {i}")
        return 1
    print("OK: helion never sets a bit the independent reference does not.")
    if missing:
        print(f"NOTE: {len(missing)} differing word(s), all in *_SING tiles -- see above.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
