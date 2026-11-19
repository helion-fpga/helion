#!/usr/bin/env python3
# Seeded random-design generator for the UltraScale+ bitgen 100-design
# loop (US-07).
#
# Produces random valid FASM designs on xcku3p, each with a placement
# meta file, reproducible from its seed: the same seed always produces
# the same design bytes. Two classes:
#   core: random LUT INITs, FF INIT/SRVAL, INT PIPs from real DB edges,
#         plus the frozen IO lines.
#   bram: core plus one RAMB18 with random INIT/INITP contents.
# Class alternates by index (even = bram, odd = core), so 50 of 100
# designs exercise BRAM_CONTENT/BRAM_PARITY.
#
# Coverage rules, enforced here, never fudged:
# - Every LUT/FF/BRAM/INT feature is resolved against the tier-1
#   reference before the design is written (validate_design); a
#   feature with no public encoding never lands in a generated design.
# - INT PIPs come only from prjuray-db segbits_int.db (3778 entries),
#   one PIP per distinct INT tile, so no two PIPs can fight over the
#   same tile-relative bit.
# - DSP48E2, URAM288, and CLK (BUFGCE/MMCM/RCLK/CMT site config)
#   have no public encoding, so the generator never emits them in the
#   100-design loop. They are covered instead by the blocked-encoding
#   probe designs (probe_dsp, probe_uram, probe_clk), which exist only
#   to prove the harness reports them blocked with the exact missing
#   data named, never green. The probe designs are NOT counted in the
#   100.
# - IO lines stay the frozen pair on HPIO_RIGHT_X0Y0 (the one tile
#   the emitter bridges provisionally); IO_CONFIG stays blocked on
#   the missing xcku3p tilegrid, as in the goldens.
# - xcvu3p is not generated: no xcvu3p device file exists in any
#   public source, so every absolute block would report blocked.
#   The loop runs xcku3p only until the device-file gap closes.
#
# Placement hash: the same scheme as gen_golden.placement_hash, so
# diff.py-style verification (recompute and compare with the meta)
# rejects a tampered generated design the same way it rejects a
# tampered golden.
#
# Multi-slice note: generated designs place logic on several slices,
# so their FF locations are recorded per tile in the design entry
# (gen_golden.DESIGNS[name]["ff_locations"]). The harness round-trip
# stage completes FF INIT/SRVAL defaults from those locations; the
# frozen goldens keep the old single-slice behaviour (see the
# design_ff_locations patch in gen_golden.py).
#
# Usage:
#   python3 gen_random.py [--out DIR] [--count N] [--seed-base N]
#   python3 gen_random.py --probe [--out DIR]
#
# No em dashes anywhere.

import hashlib
import json
import os
import random
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from gen_golden import CLK_IO, LED_IO, placement_hash
from reference import Tier1Reference

SEED_BASE = 20261119
N_DESIGNS = 100
PART = "xcku3p"
GENERATED_DIR = os.path.join(
    os.path.dirname(os.path.abspath(__file__)), "generated"
)

LUT_LETTERS = "ABCDEFGH"
FF_BELS = ["%sFF" % c for c in "ABCDEFGH"]
NUM_CLB_PER_COLUMN = 60
NUM_BRAM18_PER_COLUMN = 24

# Valid coordinate ranges on xcku3p (BitFiltrator device file):
# 4 clock-region rows, 60 CLB rows each, 113 CLB columns per row,
# 10 BRAM columns per row, 24 BRAM rows per column.
MAX_SLICE_Y = 4 * NUM_CLB_PER_COLUMN - 1  # 239
MAX_BRAM_Y = 4 * NUM_BRAM18_PER_COLUMN - 1  # 95


def design_name(index):
    return "rand%03d" % index


def clb_columns(ref, row_major):
    """Sorted CLB column indices for one clock-region row."""
    dev = ref.device(PART)
    row = dev["slrs"]["SLR0"]["rowMajors"][str(row_major)]
    return sorted(int(k) for k in row["clb_colMajors"])


def bram_columns(ref, row_major):
    """Sorted BRAM column indices for one clock-region row."""
    dev = ref.device(PART)
    row = dev["slrs"]["SLR0"]["rowMajors"][str(row_major)]
    return sorted(int(k) for k in row["bram_content_colMajors"])


def int_pip_features(ref):
    """Sorted INT PIP feature strings (without the INT. prefix)."""
    db = ref.segbits("int")
    return sorted(k[4:] for k in db if k.startswith("INT."))


def _random_slice(rng, ref, used):
    """One unused (slice_x, slice_y) pair."""
    while True:
        y = rng.randrange(0, MAX_SLICE_Y + 1)
        x = rng.choice(clb_columns(ref, y // NUM_CLB_PER_COLUMN))
        if (x, y) not in used:
            used.add((x, y))
            return x, y


def _random_int_tile(rng, ref, used):
    """One unused (int_col, int_y) pair (logical CLB column index)."""
    while True:
        y = rng.randrange(0, MAX_SLICE_Y + 1)
        col = rng.choice(clb_columns(ref, y // NUM_CLB_PER_COLUMN))
        if (col, y) not in used:
            used.add((col, y))
            return col, y


def generate_design(ref, index, seed_base=SEED_BASE):
    """Build one random design. Returns a dict with name, fasm, meta
    (without placement hash), ff_locations, and per-block feature counts.
    Deterministic in (seed_base, index): the RNG is seeded from them
    and nothing else (no time, no dict ordering, no global state).
    """
    rng = random.Random(seed_base + index)
    name = design_name(index)
    seed = seed_base + index
    klass = "bram" if index % 2 == 0 else "core"

    used_slices = set()
    n_slices = rng.choice([1, 1, 2, 2, 3])
    slices = []
    for _ in range(n_slices):
        x, y = _random_slice(rng, ref, used_slices)
        lut_bels = rng.sample(LUT_LETTERS, rng.randint(1, 8))
        ff_bels = rng.sample(FF_BELS, rng.randint(0, 8))
        luts = [(bel, rng.getrandbits(64)) for bel in lut_bels]
        ffs = [(bel, rng.randint(0, 1), rng.randint(0, 1)) for bel in ff_bels]
        slices.append({"x": x, "y": y, "luts": luts, "ffs": ffs})

    used_int = set()
    pips = []
    for _ in range(rng.randint(1, 3)):
        col, y = _random_int_tile(rng, ref, used_int)
        feat = rng.choice(int_pip_features(ref))
        pips.append({"col": col, "y": y, "feature": feat})

    bram = None
    if klass == "bram":
        row_major = rng.randrange(0, 4)
        bx = rng.choice(bram_columns(ref, row_major))
        by = row_major * NUM_BRAM18_PER_COLUMN + rng.randrange(
            0, NUM_BRAM18_PER_COLUMN
        )
        bram = {
            "x": bx,
            "y": by,
            "init": rng.getrandbits(16384),
            "initp": rng.getrandbits(2048),
        }

    lines = []
    lines.append("# random FASM: %s on %s (class %s)" % (name, PART, klass))
    lines.append("# seed: %d" % seed)
    lines.append(
        "# source: generated by gen_random.py (US-07); LUT INITs, FF "
        "INIT/SRVAL, and BRAM contents are random values drawn from "
        "this seed, never hand-typed."
    )
    lines.append(
        "# placement: logic on %d slice(s), chosen at random from the "
        "xcku3p device file (113 CLB columns, 4 clock-region rows)."
        % n_slices
    )
    lines.append(
        "# PROVISIONAL: INT and HPIO tile coordinates below use the "
        "logical CLB column index as the tile X, NOT a verified "
        "Vivado tile X. No xcku3p tilegrid exists in any public DB "
        "yet. The PIP and IO feature strings are verified to exist "
        "in prjuray-db segbits_int.db and segbits_hpio_right.db."
    )
    sites = []
    ff_locations = []
    for s in slices:
        tile = "SLICE_X%dY%d" % (s["x"], s["y"])
        for bel, init in s["luts"]:
            lines.append(
                "%s.%s6LUT.INIT[63:0] = 0x%016x" % (tile, bel, init)
            )
            sites.append({"tile": tile, "bel": "%s6LUT" % bel, "role": "lut"})
        for bel, init, srval in s["ffs"]:
            lines.append("%s.%s.INIT = %d" % (tile, bel, init))
            lines.append("%s.%s.SRVAL = %d" % (tile, bel, srval))
            sites.append({"tile": tile, "bel": bel, "role": "ff"})
            ff_locations.append((tile, bel))
    if bram is not None:
        tile = "RAMB18_X%dY%d" % (bram["x"], bram["y"])
        lines.append(
            "# BRAM: one %s; random INIT/INITP contents from this seed."
            % tile
        )
        lines.append(
            "%s.INIT[16383:0] = 0x%04096x" % (tile, bram["init"])
        )
        lines.append(
            "%s.INITP[2047:0] = 0x%0512x" % (tile, bram["initp"])
        )
        sites.append({"tile": tile, "bel": "RAMB18E2", "role": "bram"})
    for pip in pips:
        lines.append(
            "INT_X%dY%d.%s" % (pip["col"], pip["y"], pip["feature"])
        )
    lines.append("HPIO_RIGHT_X0Y0.%s" % CLK_IO)
    lines.append("HPIO_RIGHT_X0Y0.%s" % LED_IO)
    fasm = "\n".join(lines) + "\n"

    int_tiles = sorted(
        {"INT_X%dY%d" % (p["col"], p["y"]) for p in pips}
    )
    record = {
        "design": name,
        "part": PART,
        "seed": seed,
        "class": klass,
        "sites": sites,
        "provisional_tiles": int_tiles + ["HPIO_RIGHT_X0Y0"],
        "notes": (
            "INT/HPIO tile X is the logical CLB column index, not a "
            "verified Vivado tile X (no xcku3p tilegrid in any public DB)."
        ),
    }
    counts = {
        "LUT_INIT": sum(len(s["luts"]) for s in slices),
        "FF_INIT": sum(2 * len(s["ffs"]) for s in slices),
        "BRAM_CONTENT": 1 if bram is not None else 0,
        "BRAM_PARITY": 1 if bram is not None else 0,
        "INT_PIP": len(pips),
        "IO_CONFIG": 2,
    }
    return {
        "name": name,
        "fasm": fasm,
        "record": record,
        "ff_locations": ff_locations,
        "counts": counts,
    }


def write_design(out_dir, design):
    """Write one design's FASM plus meta (with placement hash)."""
    name = design["name"]
    phash = placement_hash(design["record"], design["fasm"])
    meta = dict(design["record"])
    meta["placement_hash"] = phash
    meta["fasm_sha256"] = hashlib.sha256(
        design["fasm"].encode()
    ).hexdigest()
    fasm_path = os.path.join(out_dir, "%s.%s.fasm" % (name, PART))
    meta_path = os.path.join(out_dir, "%s.%s.meta.json" % (name, PART))
    with open(fasm_path, "w") as f:
        f.write(design["fasm"])
    with open(meta_path, "w") as f:
        json.dump(meta, f, indent=2, sort_keys=True)
        f.write("\n")
    return fasm_path, meta_path, phash


def register_design(design):
    """Register a generated design in gen_golden.DESIGNS so the
    harness round-trip stage can complete its FF defaults.

    Only the FF list (and per-tile locations) is needed there; the
    LUT list is empty because the round-trip stage never reads it.
    """
    import gen_golden

    gen_golden.DESIGNS[design["name"]] = {
        "luts": [],
        "ffs": [bel for _, bel in design["ff_locations"]],
        "ff_locations": list(design["ff_locations"]),
    }


def validate_design(ref, design):
    """Resolve every feature of a generated design against the
    tier-1 reference. Returns {block: total_bits}.

    Raises MissingReference (via resolve_feature's missing field) as
    an assertion if any feature has no public encoding: a design
    that cannot be resolved must never be written to the loop.
    IO_CONFIG resolves tile-relative (expected: the tilegrid gap);
    that still counts as resolved here, because the harness reports
    it blocked with the tilegrid named, not as a generator failure.
    """
    from diff_engine import resolve_feature
    from fasm import parse_fasm

    totals = {}
    for feat in parse_fasm(design["fasm"]):
        block, bits, _, missing = resolve_feature(ref, PART, feat)
        if missing:
            raise AssertionError(
                "generated design %s: feature %s has no public "
                "encoding: %s" % (design["name"], feat, missing)
            )
        totals[block] = totals.get(block, 0) + len(bits)
    return totals


# ---- blocked-encoding probe designs (not counted in the 100) ----

def _probe_base(name, seed):
    lines = [
        "# probe FASM: %s on %s" % (name, PART),
        "# seed: %d" % seed,
        "# source: generated by gen_random.py --probe (US-07). This "
        "design exists only to prove the harness reports its block "
        "blocked with the exact missing data named, never green.",
        "# placement: blinky logic in SLICE_X0Y0 plus one feature in "
        "the probed block.",
        "# PROVISIONAL: INT and HPIO tile coordinates use the logical "
        "CLB column index as the tile X, NOT a verified Vivado tile X.",
        "SLICE_X0Y0.A6LUT.INIT[63:0] = 0x5555555555555555",
        "SLICE_X0Y0.AFF.INIT = 0",
        "SLICE_X0Y0.AFF.SRVAL = 0",
        "INT_X0Y0.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1",
        "HPIO_RIGHT_X0Y0.%s" % CLK_IO,
        "HPIO_RIGHT_X0Y0.%s" % LED_IO,
    ]
    return lines


def probe_designs(ref):
    """Three probe designs, one per blocked encoding.

    probe_dsp: one DSP48E2 feature. probe_uram: one URAM288 feature.
    probe_clk: one real RCLK BUFCE_LEAF feature from the vendored
    segbits_rclk_int_l.db (the feature string is read from the DB,
    never hand-typed). Each probe is expected to report its probed
    block blocked: DSP/URAM have no public encoding at all, CLK
    resolves tile-relative only (absolute FARs need the tilegrid)
    and the emitter has no RCLK/CMT tile mapping, so emit fails.
    """
    probes = []
    dsp_lines = _probe_base("probe_dsp", SEED_BASE + 1000)
    dsp_lines.insert(9, "DSP48E2_X0Y0.AREG.0")
    probes.append(("probe_dsp", dsp_lines))
    uram_lines = _probe_base("probe_uram", SEED_BASE + 1001)
    uram_lines.insert(9, "URAM288_X0Y0.MEM.0")
    probes.append(("probe_uram", uram_lines))
    rclk_db = ref.segbits("rclk_int_l")
    rclk_feat = sorted(
        k[len("RCLK_INT_L."):] for k in rclk_db
        if k.startswith("RCLK_INT_L.BUFCE_LEAF_")
    )[0]
    clk_lines = _probe_base("probe_clk", SEED_BASE + 1002)
    clk_lines.insert(9, "RCLK_INT_L_X0Y0.%s" % rclk_feat)
    probes.append(("probe_clk", clk_lines))
    out = []
    for name, lines in probes:
        fasm = "\n".join(lines) + "\n"
        record = {
            "design": name,
            "part": PART,
            "seed": SEED_BASE + 1000 + len(out),
            "class": "probe",
            "sites": [
                {"tile": "SLICE_X0Y0", "bel": "A6LUT", "role": "lut"},
                {"tile": "SLICE_X0Y0", "bel": "AFF", "role": "ff"},
            ],
            "provisional_tiles": ["INT_X0Y0", "HPIO_RIGHT_X0Y0"],
            "notes": (
                "Probe design: its probed block has no public "
                "encoding (DSP48E2/URAM288) or no emitter tile "
                "mapping (CLK/RCLK); the harness must report it "
                "blocked, never green."
            ),
        }
        out.append({
            "name": name,
            "fasm": fasm,
            "record": record,
            "ff_locations": [("SLICE_X0Y0", "AFF")],
            "counts": {},
        })
    return out


def main():
    import argparse

    ap = argparse.ArgumentParser(description="US-07 random-design generator")
    ap.add_argument("--out", default=GENERATED_DIR)
    ap.add_argument("--count", type=int, default=N_DESIGNS)
    ap.add_argument("--seed-base", type=int, default=SEED_BASE)
    ap.add_argument("--probe", action="store_true",
                    help="generate the 3 blocked-encoding probe designs")
    args = ap.parse_args()
    ref = Tier1Reference()
    os.makedirs(args.out, exist_ok=True)
    if args.probe:
        for design in probe_designs(ref):
            register_design(design)
            _, _, phash = write_design(args.out, design)
            print("%s: placement_hash %s" % (design["name"], phash))
        return 0
    seen = set()
    for index in range(args.count):
        design = generate_design(ref, index, args.seed_base)
        totals = validate_design(ref, design)
        digest = hashlib.sha256(design["fasm"].encode()).hexdigest()
        if digest in seen:
            raise SystemExit(
                "duplicate design at index %d: refusing to pad the "
                "count with a duplicate" % index
            )
        seen.add(digest)
        register_design(design)
        _, _, phash = write_design(args.out, design)
        print(
            "%s class=%s luts=%d ff_lines=%d pips=%d bram=%d "
            "ref_bits=%d placement_hash %s"
            % (
                design["name"],
                design["record"]["class"],
                design["counts"]["LUT_INIT"],
                design["counts"]["FF_INIT"],
                design["counts"]["INT_PIP"],
                design["counts"]["BRAM_CONTENT"],
                sum(totals.values()),
                phash,
            )
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
