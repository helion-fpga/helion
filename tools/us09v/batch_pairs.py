"""Batch-verify the 14 public xczu3eg pairs: decode each with the
decoder binary, then run place/route/extract on the decoded FASM
with one shared Db/Fabric (loading the DB per pair would dominate
the runtime). Writes a per-pair summary to stdout and a JSON
summary to the output path.

Usage: python3 batch_pairs.py [pairs_dir] [out_json]
"""
import gc
import json
import os
import re
import sys
import time
from collections import Counter

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from us09v.db import Db  # noqa: E402
from us09v.extract import Extractor  # noqa: E402
from us09v.fabric import Fabric  # noqa: E402
from us09v.fasmio import parse_fasm_file  # noqa: E402
from us09v.placecheck import check_place_fasm  # noqa: E402
from us09v.routecheck import check_route_fasm  # noqa: E402
from us09v.verify import decode_bitstream  # noqa: E402

BITS_DIR = os.path.expanduser(
    "~/workspace/ultra-bitgen-crew/decoder-validation/bits"
)
DB_ROOT = os.path.expanduser("~/workspace/prjuray-db-ref/zynqusp")


def kind_counts(violations, patterns):
    counts = Counter()
    for v in violations:
        for name, pat in patterns:
            if pat in v:
                counts[name] += 1
                break
        else:
            counts["other: " + v[:50]] += 1
    return dict(counts)


def main():
    bits_dir = sys.argv[1] if len(sys.argv) > 1 else BITS_DIR
    out_json = sys.argv[2] if len(sys.argv) > 2 else None
    bits = sorted(
        f for f in os.listdir(bits_dir) if f.endswith(".bit")
    )
    print(f"{len(bits)} pairs", flush=True)
    t0 = time.time()
    db = Db(DB_ROOT)
    fab = Fabric(db)
    print(f"db+fabric loaded in {time.time() - t0:.1f}s", flush=True)
    # Resumable: results already in the JSON are skipped, and the
    # JSON is rewritten after each pair, so a restart just means
    # relaunching. The decoded FASM is written next to the JSON.
    results = []
    done = set()
    if out_json and os.path.exists(out_json):
        try:
            with open(out_json) as f:
                results = json.load(f)
            done = {r["design"] for r in results}
            print(f"resuming: {len(done)} pairs already done", flush=True)
        except (ValueError, OSError):
            results, done = [], set()
    out_dir = os.path.dirname(out_json) if out_json else None
    for name in bits:
        design = name[: -len(".bit")]
        if design in done:
            continue
        path = os.path.join(bits_dir, name)
        t1 = time.time()
        fasm_lines, exit_code, stderr = decode_bitstream(path, DB_ROOT)
        unexplained = None
        m = re.search(r"(\d+) set bits no feature explains", stderr or "")
        if m:
            unexplained = int(m.group(1))
        fasm_path = (
            os.path.join(out_dir, design + ".decoded.fasm")
            if out_dir else None
        )
        if fasm_path:
            with open(fasm_path, "w") as f:
                f.write("\n".join(fasm_lines) + "\n")
            tiles = parse_fasm_file(fasm_path)
        else:
            import tempfile

            fd, tmp = tempfile.mkstemp(suffix=".fasm")
            with os.fdopen(fd, "w") as f:
                f.write("\n".join(fasm_lines) + "\n")
            tiles = parse_fasm_file(tmp)
            os.unlink(tmp)
        n_tiles = len(tiles)
        n_feats = sum(len(v) for v in tiles.values())
        pr = check_place_fasm(db, tiles, fab)
        place_kinds = kind_counts(
            pr.violations,
            [
                ("CLK1 undriven", "CLK1 is undriven"),
                ("CLK2 undriven", "CLK2 is undriven"),
                ("CIN undriven", "CIN is undriven"),
                ("chain gap", "chain gap"),
                ("chain position", "not the slice directly below"),
                ("capacity", "capacity"),
                ("control set", "control set"),
                ("site type", "site type"),
            ],
        )
        rr = check_route_fasm(db, tiles, fab)
        route_kinds = kind_counts(
            rr.violations,
            [
                ("undriven sink", "undriven sink"),
                ("PIP-only net", "driven only by a PIP"),
                ("multi driver", "drivers"),
                ("PIP missing", "does not exist"),
                ("unknown tile", "unknown tile"),
            ],
        )
        ex = Extractor(db, tiles, {}, {})
        nl = ex.build()
        err_kinds = kind_counts(
            ex.errors,
            [
                ("undriven LUT input", "LUT input) is undriven"),
                ("undriven FF", "FF) is undriven"),
                ("undriven CIN", "carry in"),
                ("unmodelled site", "has no model"),
                ("clock site", "clock site not modelled"),
                ("multiply driven", "multiply driven"),
            ],
        )
        rec = {
            "design": design,
            "decode_exit": exit_code,
            "unexplained_bits": unexplained,
            "fasm_tiles": n_tiles,
            "fasm_features": n_feats,
            "place_slices": pr.slices,
            "place_carry": pr.carry_chains,
            "place_violations": len(pr.violations),
            "place_kinds": place_kinds,
            "route_pips": rr.pips,
            "route_nets": rr.nets,
            "route_violations": len(rr.violations),
            "route_kinds": route_kinds,
            "extract_sites": len(nl.sites),
            "extract_errors": len(ex.errors),
            "extract_error_kinds": err_kinds,
            "extract_warnings": len(ex.warnings),
            "extract_first_errors": ex.errors[:5],
            "seconds": round(time.time() - t1, 1),
        }
        results.append(rec)
        if out_json:
            with open(out_json, "w") as f:
                json.dump(results, f, indent=2)
        print(
            f"{design}: decode={exit_code} unexplained={unexplained} "
            f"tiles={n_tiles} feats={n_feats} "
            f"place={len(pr.violations)} route={len(rr.violations)} "
            f"extract_err={len(ex.errors)} ({time.time() - t1:.0f}s)",
            flush=True,
        )
        # Release the pair's large structures (the FASM dict, the
        # violation/error lists) before the next pair: without this
        # the process grows across pairs and is OOM-killed on the
        # largest designs.
        del tiles, pr, rr, ex, nl
        gc.collect()
    print(f"total {time.time() - t0:.0f}s", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
