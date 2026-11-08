#!/usr/bin/env python3
# Differential harness: UltraScale+ bitgen, block by block, greedy queue.
#
# One command runs the full diff on blinky and counter for xcku3p and
# xcvu3p, prints the blocks ordered by diff size descending (that
# ordering IS the greedy work queue), and writes JSONL records plus a
# run manifest recording the seed and placement hash with every run.
#
#   python3 diff.py [--design blinky|counter|all] [--part xcku3p|xcvu3p|all]
#                   [--reference-bit PATH] [--emitter-cmd CMD] [--out DIR]
#
# Tier 1 (default): diff against the public segbits DBs plus BitFiltrator
# tables. Tier 2 (--reference-bit PATH): also record a Vivado reference
# bitstream for the tier-2 diff; with no reference bit present the run
# reports tier 1 only and says so. The harness never invents a reference.
#
# A run whose placement hash differs from the golden meta is rejected,
# not compared (exit 3). A harness that reports green on a comparison
# it did not run is a FAIL: until an emitter lands, every block that
# needs one reports blocked with the missing piece named, never zero.
#
# Exit codes: 0 run completed (blocked records are a result, not a
# harness failure); 1 harness error; 2 missing --reference-bit file;
# 3 placement hash mismatch (golden tampered or stale).
#
# No em dashes anywhere.

import argparse
import datetime
import hashlib
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from diff_engine import diff_design, format_queue, write_jsonl
from emitter import find_emitter
from gen_golden import GOLDEN_DIR, placement_hash
from reference import Tier1Reference

DESIGNS = ("blinky", "counter")
PARTS = ("xcku3p", "xcvu3p")


def load_meta(design, part):
    meta_path = os.path.join(GOLDEN_DIR, "%s.%s.meta.json" % (design, part))
    with open(meta_path) as f:
        return json.load(f)


def check_placement(meta, fasm_path):
    """Recompute the placement hash from the meta record and the FASM file.

    Returns (recomputed, ok). A mismatch means the golden was tampered
    with or is stale; the run must be rejected, not compared.
    """
    record = {k: v for k, v in meta.items()
              if k not in ("placement_hash", "fasm_sha256")}
    with open(fasm_path) as f:
        fasm_text = f.read()
    recomputed = placement_hash(record, fasm_text)
    return recomputed, recomputed == meta["placement_hash"]


def main():
    ap = argparse.ArgumentParser(description="UltraScale+ bitgen diff harness")
    ap.add_argument("--design", choices=list(DESIGNS) + ["all"], default="all")
    ap.add_argument("--part", choices=list(PARTS) + ["all"], default="all")
    ap.add_argument("--reference-bit", default=None,
                    help="Vivado reference bitstream for the tier-2 slot")
    ap.add_argument("--emitter-cmd", default=None,
                    help="bitgen-core emitter executable (see emitter.py)")
    ap.add_argument("--out", default=None,
                    help="run output dir (default runs/<UTC timestamp>)")
    ap.add_argument("--golden", default=GOLDEN_DIR,
                    help="golden FASM dir (default golden/)")
    args = ap.parse_args()

    designs = DESIGNS if args.design == "all" else (args.design,)
    parts = PARTS if args.part == "all" else (args.part,)

    if args.reference_bit is not None and not os.path.exists(args.reference_bit):
        print("error: --reference-bit file not found: %s" % args.reference_bit,
              file=sys.stderr)
        return 2
    tier = "tier1+tier2" if args.reference_bit else "tier1"
    if args.reference_bit:
        print("tier 2 slot: reference bit %s recorded; tier-2 diff is "
              "blocked until the emitter can parse a Vivado bitstream "
              "(see manifest)" % args.reference_bit)

    out_dir = args.out
    if out_dir is None:
        stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        out_dir = os.path.join(os.path.dirname(os.path.abspath(__file__)), "runs", stamp)
    os.makedirs(out_dir, exist_ok=True)

    ref = Tier1Reference()
    emitter = find_emitter(args.emitter_cmd)
    if emitter is None:
        print("no emitter configured: blocks that need one report blocked")
    else:
        print("emitter: %s" % emitter.describe())

    # Cross-source consistency check, every run: BitFiltrator vs prjuray
    # must agree on the LUT and FF mappings for the golden slice.
    cross = {}
    for bel in ("A6LUT", "B6LUT", "C6LUT", "D6LUT"):
        checked, mismatches = ref.cross_check_lut("xcku3p", 0, 0, bel)
        cross["lut-%s" % bel] = {"checked": checked, "mismatches": mismatches}
    for bel in ("AFF", "BFF", "CFF", "DFF"):
        cross["ff-%s" % bel] = {"match": ref.cross_check_ff("xcku3p", 0, 0, bel)}
    cross_ok = all(
        (v.get("mismatches") == [] if "mismatches" in v else v.get("match") is True)
        for v in cross.values()
    )
    print("reference cross-check (BitFiltrator vs prjuray, SLICE_X0Y0): %s"
          % ("PASS" if cross_ok else "FAIL"))
    if not cross_ok:
        print(json.dumps(cross, indent=2))
        return 1

    all_records = []
    run_manifest = {
        "timestamp_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "tier": tier,
        "reference_bit": args.reference_bit,
        "emitter": emitter.describe() if emitter else None,
        "cross_check": cross,
        "combos": [],
    }
    exit_code = 0
    for design in designs:
        for part in parts:
            meta = load_meta(design, part)
            fasm_path = os.path.join(args.golden, "%s.%s.fasm" % (design, part))
            recomputed, ok = check_placement(meta, fasm_path)
            if not ok:
                print("REJECTED %s/%s: placement hash mismatch: meta %s, "
                      "recomputed %s" % (design, part, meta["placement_hash"],
                                         recomputed), file=sys.stderr)
                return 3
            workdir = os.path.join(out_dir, "%s-%s" % (design, part))
            os.makedirs(workdir, exist_ok=True)
            records = diff_design(
                ref, emitter, design, part, fasm_path,
                meta["seed"], meta["placement_hash"], tier, workdir,
                reference_bit=args.reference_bit,
            )
            all_records.extend(records)
            run_manifest["combos"].append({
                "design": design,
                "part": part,
                "seed": meta["seed"],
                "placement_hash": meta["placement_hash"],
                "provisional_tiles": meta.get("provisional_tiles", []),
            })
            print()
            print(format_queue(records, design, part))

    jsonl_path = os.path.join(out_dir, "diff.jsonl")
    write_jsonl(all_records, jsonl_path)
    if args.reference_bit:
        run_manifest["tier2"] = {
            "status": "blocked",
            "reference_bit": args.reference_bit,
            "missing": (
                "tier-2 diff needs the emitter to parse a Vivado "
                "bitstream (bitgen-core parse_bitstream on a Vivado .bit); "
                "no emitter is configured, so no tier-2 comparison ran"
                if emitter is None else
                "tier-2 diff needs the emitter to parse a Vivado "
                "bitstream; emitter present but Vivado-bit parsing is not "
                "implemented in the harness yet"
            ),
        }
    manifest_path = os.path.join(out_dir, "manifest.json")
    run_manifest["records"] = len(all_records)
    run_manifest["jsonl"] = jsonl_path
    with open(manifest_path, "w") as f:
        json.dump(run_manifest, f, indent=2, sort_keys=True)
        f.write("\n")
    print()
    print("wrote %d records to %s" % (len(all_records), jsonl_path))
    print("manifest: %s" % manifest_path)
    print("tier: %s" % tier)
    return exit_code


if __name__ == "__main__":
    sys.exit(main())
