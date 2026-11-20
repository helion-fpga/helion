#!/usr/bin/env python3
# 100-design harness loop for US-07.
#
# Runs the existing diff.py machinery (diff_engine.diff_design) over
# generated random designs from gen_random.py, in batches, and records
# honest runtimes per design and per batch. It does not change any
# comparison logic: the same emitter, the same reference, the same
# per-block records as a diff.py run, only over generated designs.
#
# Each design's placement hash is recomputed from its meta plus FASM
# before the diff runs; a mismatch rejects the batch (exit 3), the
# same rule diff.py applies to the frozen goldens. Generated designs
# are registered in gen_golden.DESIGNS from their meta sites so the
# round-trip stage completes FF defaults on the design's real tiles.
#
# Usage:
#   python3 gen_random.py                       # generate the 100 first
#   python3 run_100.py --start 0 --count 10    # batch of 10
#   python3 run_100.py --designs rand000,rand001
#   HELION_BITGEN_EMITTER=/path/to/helion-bitgen python3 run_100.py
#
# Output: per-batch JSONL plus manifest under harness/runs/us07-<stamp>/
# (the harness run layout), and a human-readable log under
# ultra-bitgen-crew/logs/us07-loop-<stamp>.log. Batch manifests record
# wall-clock seconds per design and for the batch; a 100-design total
# is only claimed from batches that actually ran.
#
# Exit codes: 0 run completed (blocked records are a result, not a
# failure); 1 harness error; 3 placement hash mismatch.
#
# No em dashes anywhere.

import argparse
import datetime
import hashlib
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import gen_golden
from diff_engine import diff_design, write_jsonl
from emitter import find_emitter
from gen_random import GENERATED_DIR, N_DESIGNS, PART, SEED_BASE, design_name
from reference import Tier1Reference

HARNESS_DIR = os.path.dirname(os.path.abspath(__file__))
DEFAULT_LOG_DIR = os.path.join(
    os.path.dirname(HARNESS_DIR), "logs"
)


def load_meta(generated_dir, name):
    meta_path = os.path.join(
        generated_dir, "%s.%s.meta.json" % (name, PART)
    )
    with open(meta_path) as f:
        return json.load(f)


def check_placement(meta, fasm_path):
    """Recompute the placement hash; return (recomputed, ok, sha_ok)."""
    record = {k: v for k, v in meta.items()
              if k not in ("placement_hash", "fasm_sha256")}
    with open(fasm_path) as f:
        fasm_text = f.read()
    recomputed = gen_golden.placement_hash(record, fasm_text)
    sha_ok = (
        hashlib.sha256(fasm_text.encode()).hexdigest()
        == meta.get("fasm_sha256")
    )
    return recomputed, recomputed == meta["placement_hash"], sha_ok


def register_from_meta(name, meta):
    """Register a generated design for the round-trip stage, from its
    meta sites (no dependence on gen_random's in-memory dict)."""
    ff_locations = [
        (s["tile"], s["bel"]) for s in meta.get("sites", [])
        if s.get("role") == "ff"
    ]
    gen_golden.DESIGNS[name] = {
        "luts": [],
        "ffs": [bel for _, bel in ff_locations],
        "ff_locations": ff_locations,
    }


def summarize(records):
    """Per-block {green, red, blocked} counts over records."""
    totals = {}
    for rec in records:
        slot = totals.setdefault(
            rec["block"], {"green": 0, "red": 0, "blocked": 0}
        )
        slot[rec["status"]] += 1
    return totals


def format_summary(totals, n_designs):
    lines = []
    lines.append(
        "aggregate over %d design(s): green/red/blocked per block"
        % n_designs
    )
    lines.append("%-13s %6s %6s %8s" % ("block", "green", "red", "blocked"))
    for block in sorted(totals):
        slot = totals[block]
        lines.append(
            "%-13s %6d %6d %8d"
            % (block, slot["green"], slot["red"], slot["blocked"])
        )
    return "\n".join(lines)


def main():
    ap = argparse.ArgumentParser(description="US-07 100-design loop")
    ap.add_argument("--generated-dir", default=GENERATED_DIR)
    ap.add_argument("--emitter-cmd", default=None)
    ap.add_argument("--out", default=None,
                    help="batch output dir (default runs/us07-<UTC stamp>)")
    ap.add_argument("--log-dir", default=DEFAULT_LOG_DIR)
    ap.add_argument("--start", type=int, default=0)
    ap.add_argument("--count", type=int, default=N_DESIGNS)
    ap.add_argument("--designs", default=None,
                    help="comma-separated design names (overrides start/count)")
    ap.add_argument("--seed-base", type=int, default=SEED_BASE)
    args = ap.parse_args()

    if args.designs:
        names = [n.strip() for n in args.designs.split(",") if n.strip()]
    else:
        names = [
            design_name(i)
            for i in range(args.start, args.start + args.count)
        ]
    if not names:
        print("error: no designs selected", file=sys.stderr)
        return 1

    stamp = datetime.datetime.now(datetime.timezone.utc).strftime(
        "%Y%m%dT%H%M%SZ"
    )
    out_dir = args.out or os.path.join(HARNESS_DIR, "runs", "us07-" + stamp)
    os.makedirs(out_dir, exist_ok=True)
    log_path = None
    if args.log_dir:
        os.makedirs(args.log_dir, exist_ok=True)
        log_path = os.path.join(
            args.log_dir, "us07-loop-%s.log" % stamp
        )

    ref = Tier1Reference()
    emitter = find_emitter(args.emitter_cmd)
    if emitter is None:
        print("no emitter configured: blocks that need one report blocked")
    else:
        print("emitter: %s" % emitter.describe())

    lines = []
    lines.append("US-07 loop batch %s part=%s designs=%d" % (stamp, PART, len(names)))
    lines.append("emitter: %s" % (emitter.describe() if emitter else None))

    all_records = []
    per_design = []
    batch_start = time.perf_counter()
    for name in names:
        fasm_path = os.path.join(
            args.generated_dir, "%s.%s.fasm" % (name, PART)
        )
        if not os.path.exists(fasm_path):
            print(
                "error: generated design missing: %s (run "
                "gen_random.py first)" % fasm_path, file=sys.stderr
            )
            return 1
        meta = load_meta(args.generated_dir, name)
        recomputed, ok, sha_ok = check_placement(meta, fasm_path)
        if not ok or not sha_ok:
            print(
                "REJECTED %s: placement hash mismatch: meta %s, "
                "recomputed %s, fasm sha ok=%s"
                % (name, meta["placement_hash"], recomputed, sha_ok),
                file=sys.stderr,
            )
            return 3
        register_from_meta(name, meta)
        workdir = os.path.join(out_dir, "%s-%s" % (name, PART))
        os.makedirs(workdir, exist_ok=True)
        t0 = time.perf_counter()
        records = diff_design(
            ref, emitter, name, PART, fasm_path,
            meta["seed"], meta["placement_hash"], "tier1", workdir,
        )
        seconds = time.perf_counter() - t0
        all_records.extend(records)
        status = {}
        for rec in records:
            status[rec["block"]] = rec["status"]
        green = sum(1 for s in status.values() if s == "green")
        red = sum(1 for s in status.values() if s == "red")
        blocked = sum(1 for s in status.values() if s == "blocked")
        per_design.append({
            "design": name,
            "part": PART,
            "seed": meta["seed"],
            "class": meta.get("class"),
            "placement_hash": meta["placement_hash"],
            "seconds": round(seconds, 1),
            "green": green,
            "red": red,
            "blocked": blocked,
        })
        line = (
            "%s class=%s %.1fs green=%d red=%d blocked=%d"
            % (name, meta.get("class"), seconds, green, red, blocked)
        )
        print(line)
        lines.append(line)

    batch_seconds = time.perf_counter() - batch_start
    totals = summarize(all_records)
    summary = format_summary(totals, len(names))
    print()
    print(summary)
    print()
    print("batch runtime: %.1f seconds for %d design(s) (mean %.1f s/design)"
          % (batch_seconds, len(names), batch_seconds / len(names)))
    lines.append("")
    lines.append(summary)
    lines.append("")
    lines.append(
        "batch runtime: %.1f seconds for %d design(s) (mean %.1f s/design)"
        % (batch_seconds, len(names), batch_seconds / len(names))
    )

    jsonl_path = os.path.join(out_dir, "diff.jsonl")
    write_jsonl(all_records, jsonl_path)
    manifest = {
        "timestamp_utc": datetime.datetime.now(
            datetime.timezone.utc
        ).isoformat(),
        "batch": stamp,
        "tier": "tier1",
        "part": PART,
        "seed_base": args.seed_base,
        "emitter": emitter.describe() if emitter else None,
        "designs": per_design,
        "block_totals": totals,
        "records": len(all_records),
        "runtime_seconds": round(batch_seconds, 1),
        "mean_seconds_per_design": round(
            batch_seconds / len(names), 1
        ),
        "jsonl": jsonl_path,
    }
    manifest_path = os.path.join(out_dir, "manifest.json")
    with open(manifest_path, "w") as f:
        json.dump(manifest, f, indent=2, sort_keys=True)
        f.write("\n")
    print("wrote %d records to %s" % (len(all_records), jsonl_path))
    print("manifest: %s" % manifest_path)
    lines.append("jsonl: %s" % jsonl_path)
    lines.append("manifest: %s" % manifest_path)
    if log_path:
        with open(log_path, "w") as f:
            f.write("\n".join(lines) + "\n")
        print("log: %s" % log_path)
    return 0


if __name__ == "__main__":
    sys.exit(main())
