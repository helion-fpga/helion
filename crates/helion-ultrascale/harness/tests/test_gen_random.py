# Unit tests for the US-07 random-design generator and loop driver.
#
# Run: python3 -m unittest discover -s tests -v
# (from the harness dir). Stdlib only. No emitter is needed: the
# tests resolve designs against the tier-1 reference only, which
# exercises every generator invariant without the slow bitstream
# round trip.
#
# No em dashes anywhere.

import hashlib
import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import gen_golden
from diff_engine import resolve_feature
from fasm import classify, parse_fasm
from gen_random import (
    GENERATED_DIR,
    PART,
    SEED_BASE,
    design_name,
    generate_design,
    probe_designs,
    register_design,
    validate_design,
    write_design,
)
from reference import MissingReference, Tier1Reference

HARNESS_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


class TestGeneratorDeterminism(unittest.TestCase):
    def test_same_seed_same_bytes(self):
        ref = Tier1Reference()
        first = generate_design(ref, 7)
        second = generate_design(ref, 7)
        self.assertEqual(first["fasm"], second["fasm"])
        self.assertEqual(first["record"], second["record"])
        self.assertEqual(first["ff_locations"], second["ff_locations"])

    def test_different_seeds_differ(self):
        ref = Tier1Reference()
        digests = set()
        for i in range(10):
            d = generate_design(ref, i)
            digests.add(hashlib.sha256(d["fasm"].encode()).hexdigest())
        self.assertEqual(len(digests), 10)

    def test_classes_alternate(self):
        ref = Tier1Reference()
        self.assertEqual(generate_design(ref, 0)["record"]["class"], "bram")
        self.assertEqual(generate_design(ref, 1)["record"]["class"], "core")
        self.assertEqual(generate_design(ref, 2)["record"]["class"], "bram")


class TestGeneratorValidity(unittest.TestCase):
    def test_all_features_resolve(self):
        # Reference resolution (no emitter): every generated feature
        # must have a public encoding, on several designs of both
        # classes and several slices.
        ref = Tier1Reference()
        for i in (0, 1, 2, 3, 42, 99):
            design = generate_design(ref, i)
            totals = validate_design(ref, design)
            self.assertGreater(totals.get("LUT_INIT", 0), 0)
            self.assertGreater(totals.get("INT_PIP", 0), 0)
            self.assertGreater(totals.get("IO_CONFIG", 0), 0)
            if design["record"]["class"] == "bram":
                self.assertGreater(totals.get("BRAM_CONTENT", 0), 0)
                self.assertGreater(totals.get("BRAM_PARITY", 0), 0)
            else:
                self.assertNotIn("BRAM_CONTENT", totals)

    def test_no_duplicate_bel_per_slice(self):
        ref = Tier1Reference()
        for i in range(10):
            design = generate_design(ref, i)
            seen = set()
            for feat in parse_fasm(design["fasm"]):
                if feat.tile.startswith("SLICE_"):
                    parts = feat.feature.split(".")
                    key = (feat.tile, parts[0], parts[1] if len(parts) > 1 else "")
                    self.assertNotIn(key, seen, design["name"])
                    seen.add(key)

    def test_pips_on_distinct_int_tiles(self):
        ref = Tier1Reference()
        for i in range(10):
            design = generate_design(ref, i)
            tiles = [
                f.tile for f in parse_fasm(design["fasm"])
                if classify(f) == "INT_PIP"
            ]
            self.assertEqual(len(tiles), len(set(tiles)))

    def test_int_tiles_are_y0_intersection(self):
        # The harness reference and the emitter agree on INT tile
        # coordinates only at Y=0 (see the gen_random module
        # docstring: reference reads Y as absolute row, emitter
        # reads it as the FAR row). Every generated INT tile must
        # sit in that intersection, or emit fails loud.
        ref = Tier1Reference()
        for i in range(10):
            design = generate_design(ref, i)
            for feat in parse_fasm(design["fasm"]):
                if classify(feat) == "INT_PIP":
                    self.assertTrue(
                        feat.tile.endswith("Y0"), design["name"]
                    )

    def test_placement_hash_recomputes(self):
        ref = Tier1Reference()
        design = generate_design(ref, 5)
        with tempfile.TemporaryDirectory() as tmp:
            _, _, phash = write_design(tmp, design)
            with open(os.path.join(
                tmp, "%s.%s.meta.json" % (design["name"], PART)
            )) as f:
                meta = json.load(f)
            record = {k: v for k, v in meta.items()
                      if k not in ("placement_hash", "fasm_sha256")}
            with open(os.path.join(
                tmp, "%s.%s.fasm" % (design["name"], PART)
            )) as f:
                fasm_text = f.read()
            self.assertEqual(
                gen_golden.placement_hash(record, fasm_text), phash
            )
            self.assertEqual(meta["placement_hash"], phash)

    def test_ff_locations_registered(self):
        ref = Tier1Reference()
        design = generate_design(ref, 4)
        register_design(design)
        try:
            locs = gen_golden.design_ff_locations(design["name"])
            self.assertEqual(locs, design["ff_locations"])
            for tile, bel in locs:
                self.assertTrue(tile.startswith("SLICE_X"))
        finally:
            # Registration mutates the shared DESIGNS dict; remove the
            # entry so other test modules still see only the frozen
            # goldens there.
            gen_golden.DESIGNS.pop(design["name"], None)

    def test_frozen_goldens_unaffected(self):
        # The ff_locations patch keeps the old single-slice behaviour
        # for entries without the key.
        self.assertEqual(
            gen_golden.design_ff_locations("blinky"),
            [("SLICE_X0Y0", "AFF")],
        )


class TestProbeDesigns(unittest.TestCase):
    def test_probe_blocks_name_the_encoding(self):
        # Each probe's block must fail reference resolution with the
        # exact missing data named: that is the behaviour the loop
        # relies on to report blocked, never green.
        ref = Tier1Reference()
        expected = {
            "probe_dsp": "DSP48E2",
            "probe_uram": "URAM288",
        }
        for design in probe_designs(ref):
            feats = parse_fasm(design["fasm"])
            if design["name"] in expected:
                block = expected[design["name"]]
                probe_feats = [
                    f for f in feats if classify(f) == block
                ]
                self.assertEqual(len(probe_feats), 1)
                _, _, _, missing = resolve_feature(
                    ref, PART, probe_feats[0]
                )
                self.assertIsNotNone(missing)
                self.assertIn(block, missing)
            else:
                self.assertEqual(design["name"], "probe_clk")
                probe_feats = [
                    f for f in feats if classify(f) == "CLK"
                ]
                self.assertEqual(len(probe_feats), 1)
                _, bits, coord, missing = resolve_feature(
                    ref, PART, probe_feats[0]
                )
                self.assertIsNone(missing)
                self.assertEqual(coord, "tile-relative")
                self.assertGreater(len(bits), 0)


if __name__ == "__main__":
    unittest.main()
