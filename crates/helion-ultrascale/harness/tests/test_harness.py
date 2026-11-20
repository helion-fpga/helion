# Unit tests for the UltraScale+ diff harness.
#
# Run: python3 -m unittest discover -s tests -v
# (from the harness dir). Stdlib only.
#
# The synthetic emitter (tests/fake_emitter.py) is test-only: it
# exercises the green/red paths of the diff engine. It never runs in
# a real diff.py invocation.
#
# No em dashes anywhere.

import json
import os
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from diff_engine import compute_diff, diff_design, greedy_order, resolve_feature
from emitter import Emitter, NO_EMITTER_MISSING
from fasm import FasmError, Feature, canonical_fasm, classify, parse_fasm, parse_init_value
from gen_golden import DESIGNS, GOLDEN_DIR, build_fasm, lut_init_64
from reference import MissingReference, Tier1Reference, int_col_offset_from_clb
from roundtrip import (
    canonical_roundtrip_fasm,
    complement_signatures,
    drop_feature_keys,
    is_unobservable_feature,
    roundtrip_feature_keys,
    scope_fasm_to_feature_keys,
    unobservable_int_pip_keys,
    zero_signature,
)

HARNESS_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def golden_fasm(design, part):
    with open(os.path.join(GOLDEN_DIR, "%s.%s.fasm" % (design, part))) as f:
        return f.read()


def golden_meta(design, part):
    with open(os.path.join(GOLDEN_DIR, "%s.%s.meta.json" % (design, part))) as f:
        return json.load(f)


def _fake_emitter_shim(tmp):
    """Executable shim wrapping tests/fake_emitter.py. Returns its path."""
    fake = os.path.join(HARNESS_DIR, "tests", "fake_emitter.py")
    shim = os.path.join(tmp, "emitter.sh")
    with open(shim, "w") as f:
        f.write("#!/bin/sh\nexec %s %s \"$@\"\n" % (sys.executable, fake))
    os.chmod(shim, 0o755)
    return shim


class TestFasm(unittest.TestCase):
    def test_parse_serialize_roundtrip(self):
        for design in ("blinky", "counter"):
            for part in ("xcku3p", "xcvu3p"):
                text = golden_fasm(design, part)
                once = canonical_fasm(text)
                twice = canonical_fasm(once)
                self.assertEqual(once, twice, "%s/%s canonical form unstable" % (design, part))
                self.assertEqual(len(parse_fasm(text)), len(parse_fasm(once)))

    def test_parse_rejects_garbage(self):
        with self.assertRaises(FasmError):
            parse_fasm("this is not fasm\n")
        with self.assertRaises(FasmError):
            parse_fasm("SLICE_X0Y0.A6LUT.INIT[63:0] =\n")
        with self.assertRaises(FasmError):
            parse_fasm("BOGUS_TILE.A6LUT.INIT[63:0] = 0x0\n")

    def test_classify_golden_lines(self):
        expected = {
            "blinky": {"LUT_INIT": 1, "FF_INIT": 2, "INT_PIP": 2, "IO_CONFIG": 2},
            "counter": {"LUT_INIT": 4, "FF_INIT": 8, "INT_PIP": 3, "IO_CONFIG": 2},
        }
        for design, counts in expected.items():
            feats = parse_fasm(golden_fasm(design, "xcku3p"))
            got = {}
            for feat in feats:
                got[classify(feat)] = got.get(classify(feat), 0) + 1
            self.assertEqual(got, counts, design)

    def test_classify_clk_lines(self):
        # CLK golden lines (tests/fixtures/clk_blinky.fasm, clk branch):
        # CMT WIRE, BUFCE_LEAF, and RCLK PIP lines are all CLK. The
        # RCLK PIP must NOT classify as INT_PIP, and an INT PIP still
        # does. Feature objects, not golden files: the fixture lives
        # on the clk branch, not in this harness tree.
        from fasm import Feature
        cases = [
            ("CMT_RIGHT_X0Y0", "WIRE.CLK_HDISTR_0_0.USED.V1", "CLK"),
            ("CMT_RIGHT_X0Y0", "WIRE.CLK_HROUTE_1_0.USED.V1", "CLK"),
            ("CMT_RIGHT_X0Y0", "PIP.CLK_HROUTE_1_0.CLK_HDISTR_0_0", "CLK"),
            ("CMT_L_X0Y0", "BUFGCE_X0Y0.BUFGCE.IINV.V0", "CLK"),
            ("RCLK_INT_L_X0Y0", "BUFCE_LEAF_X0Y0.BUFCE_LEAF.CE_TYPE.SYNC", "CLK"),
            ("RCLK_INT_L_X0Y0", "BUFCE_LEAF_X0Y0.BUFCE_LEAF.IN_USE.V1", "CLK"),
            ("RCLK_INT_L_X0Y0", "PIP.CLK_LEAF_SITES_0_CLK_IN.CLK_HDISTR_FT0_0", "CLK"),
            ("INT_X0Y0", "PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1", "INT_PIP"),
        ]
        for tile, feat, want in cases:
            self.assertEqual(
                classify(Feature(tile, feat)), want, "%s.%s" % (tile, feat)
            )

    def test_every_ff_has_exactly_one_srval_zero(self):
        # SRVAL golden completion (orchestrator session 13): every FF
        # in every generated golden has exactly one SRVAL line with
        # value 0, immediately after that FF's INIT line. Uses
        # build_fasm output, not hand-typed text, and checks the
        # on-disk golden matches the generator output.
        for design, spec in DESIGNS.items():
            for part in ("xcku3p", "xcvu3p"):
                text = build_fasm(design, part)
                self.assertEqual(text, golden_fasm(design, part),
                                 "%s/%s golden differs from build_fasm" % (design, part))
                lines = [l for l in text.splitlines()
                         if l and not l.startswith("#")]
                srval = [l for l in lines if ".SRVAL" in l]
                self.assertEqual(len(srval), len(spec["ffs"]),
                                 "%s/%s SRVAL line count" % (design, part))
                for bel in spec["ffs"]:
                    init_line = "SLICE_X0Y0.%s.INIT = 0" % bel
                    srval_line = "SLICE_X0Y0.%s.SRVAL = 0" % bel
                    self.assertEqual(
                        lines.count(srval_line), 1,
                        "%s/%s %s" % (design, part, srval_line))
                    self.assertEqual(
                        lines.index(srval_line),
                        lines.index(init_line) + 1,
                        "%s/%s SRVAL must follow INIT for %s"
                        % (design, part, bel))

    def test_init_values_recomputed_from_truth_tables(self):
        # Independent recomputation: enumerate the truth table directly.
        def truth(fn):
            init = 0
            for i in range(64):
                if fn([(i >> k) & 1 for k in range(6)]):
                    init |= 1 << i
            return init

        cases = {
            "blinky": {"A6LUT": truth(lambda b: 1 - b[0])},
            "counter": {
                "A6LUT": truth(lambda b: 1 - b[0]),
                "B6LUT": truth(lambda b: b[1] ^ b[0]),
                "C6LUT": truth(lambda b: b[2] ^ (b[1] & b[0])),
                "D6LUT": truth(lambda b: b[3] ^ (b[2] & b[1] & b[0])),
            },
        }
        for design, luts in cases.items():
            feats = parse_fasm(golden_fasm(design, "xcku3p"))
            for feat in feats:
                if classify(feat) != "LUT_INIT":
                    continue
                bel = feat.feature.split(".")[0]
                self.assertEqual(parse_init_value(feat.value), luts[bel],
                                 "%s %s INIT mismatch" % (design, bel))
        # And the generator's own function agrees.
        self.assertEqual(lut_init_64(lambda b: 1 - b[0]), 0x5555555555555555)



    def test_parse_bram_value_spellings(self):
        from fasm import parse_bram_value
        bits = parse_bram_value("0x1", 16384)
        self.assertEqual(len(bits), 16384)
        self.assertEqual(bits[0], 1)
        self.assertEqual(bits[1:], [0] * 16383)
        # Verilog spelling (emitter parse-back) and decimal agree.
        self.assertEqual(parse_bram_value("16384'h1", 16384), bits)
        self.assertEqual(parse_bram_value("1", 16384), bits)
        # One bit over the width fails loud instead of truncating.
        with self.assertRaises(FasmError):
            parse_bram_value("0x1" + "0" * 4096, 16384)
        with self.assertRaises(FasmError):
            parse_bram_value("16384'hZZ", 16384)
        with self.assertRaises(FasmError):
            parse_bram_value(None, 16384)


class TestCarry8(unittest.TestCase):
    # CARRY8 as its own harness block (CTO ruling 2026-11-08, US-05).
    # The vocabulary is the 24 prjuray segbits features per CLB tile
    # type: CARRY_TYPE.DUAL_CY4/SINGLE_CY8, CI.AX/CI.V0/CI.V1,
    # CI_TOP.EX/CI_TOP.V0/CI_TOP.V1, DI0.AX/DI0.DI0 through
    # DI7.HX/DI7.DI7. The list below is cross-checked against the DB
    # itself, not trusted on its own.
    CARRY8_SUFFIXES = (
        "CARRY_TYPE.DUAL_CY4", "CARRY_TYPE.SINGLE_CY8",
        "CI.AX", "CI.V0", "CI.V1",
        "CI_TOP.EX", "CI_TOP.V0", "CI_TOP.V1",
        "DI0.AX", "DI0.DI0", "DI1.BX", "DI1.DI1",
        "DI2.CX", "DI2.DI2", "DI3.DX", "DI3.DI3",
        "DI4.EX", "DI4.DI4", "DI5.FX", "DI5.DI5",
        "DI6.GX", "DI6.DI6", "DI7.HX", "DI7.DI7",
    )

    def test_classify_all_24_carry8_suffixes(self):
        db = Tier1Reference().segbits("clel_l")
        db_suffixes = sorted(
            k.split("CARRY8.", 1)[1]
            for k in db if ".CARRY8." in k
        )
        self.assertEqual(db_suffixes, sorted(self.CARRY8_SUFFIXES))
        for suffix in self.CARRY8_SUFFIXES:
            feat = parse_fasm("SLICE_X0Y0.CARRY8.%s\n" % suffix)[0]
            self.assertEqual(classify(feat), "CARRY8", suffix)
            self.assertNotEqual(classify(feat), "LUT_INIT", suffix)
            self.assertNotEqual(classify(feat), "FF_INIT", suffix)

    def test_classify_carry8_off_slice_fails_loud(self):
        feat = parse_fasm("INT_X0Y0.CARRY8.DI0.DI0\n")[0]
        with self.assertRaises(FasmError):
            classify(feat)

    def test_resolve_carry8_hand_checked_bits(self):
        # SLICE_X0Y0 is CLEL_R, column major 5: FAR base 0x500.
        # Positions read by hand from segbits_clel_r.db.
        ref = Tier1Reference()
        cases = {
            "CARRY8.CARRY_TYPE.DUAL_CY4": {(0x50C, 45): 1},
            "CARRY8.CARRY_TYPE.SINGLE_CY8": {(0x50C, 45): 0},
            "CARRY8.DI0.DI0": {(0x50C, 3): 1},
            "CARRY8.DI0.AX": {(0x50C, 3): 0},
            "CARRY8.CI.V1": {(0x50A, 18): 1, (0x50E, 1): 0},
            "CARRY8.CI.V0": {(0x50A, 18): 0, (0x50E, 1): 0},
            "CARRY8.CI_TOP.V1": {(0x50B, 31): 1, (0x50C, 27): 0},
            "CARRY8.CI_TOP.EX": {(0x50B, 31): 0, (0x50C, 27): 1},
            "CARRY8.DI7.DI7": {(0x50F, 47): 1},
        }
        for feature, want in cases.items():
            feat = parse_fasm("SLICE_X0Y0.%s\n" % feature)[0]
            block, bits, coord, miss = resolve_feature(ref, "xcku3p", feat)
            self.assertIsNone(miss, feature)
            self.assertEqual(block, "CARRY8", feature)
            self.assertEqual(coord, "absolute", feature)
            self.assertEqual(bits, want, feature)

    def test_resolve_carry8_unknown_feature_fails_loud(self):
        feat = parse_fasm("SLICE_X0Y0.CARRY8.NOPE\n")[0]
        self.assertEqual(classify(feat), "CARRY8")
        _, _, _, miss = resolve_feature(Tier1Reference(), "xcku3p", feat)
        self.assertIsNotNone(miss)
        self.assertIn("not in prjuray-db", miss)

    def test_resolve_carry8_on_all_clb_tile_types(self):
        # First column of each CLB tile type in xcku3p row 0:
        # CLEL_R x=0 major 5, CLEM x=1 major 6, CLEL_L x=7 major 18,
        # CLEM_R x=103 major 208. DUAL_CY4 is frame 12 bit 45 val 1
        # on every tile type (same DB line on all four).
        ref = Tier1Reference()
        for x, major in ((0, 5), (1, 6), (7, 18), (103, 208)):
            feat = parse_fasm(
                "SLICE_X%dY0.CARRY8.CARRY_TYPE.DUAL_CY4\n" % x)[0]
            block, bits, coord, miss = resolve_feature(ref, "xcku3p", feat)
            self.assertIsNone(miss, x)
            self.assertEqual(block, "CARRY8", x)
            self.assertEqual(bits, {((major << 8) | 12, 45): 1}, x)

    def _carry8_fasm(self, tmp):
        text = golden_fasm("blinky", "xcku3p")
        text += (
            "SLICE_X0Y0.CARRY8.CARRY_TYPE.DUAL_CY4\n"
            "SLICE_X0Y0.CARRY8.DI0.DI0\n"
            "SLICE_X0Y0.CARRY8.CI.V1\n"
        )
        path = os.path.join(tmp, "carry8.fasm")
        with open(path, "w") as f:
            f.write(text)
        return path

    def test_design_without_carry8_is_blocked(self):
        with tempfile.TemporaryDirectory() as tmp:
            emitter = Emitter(_fake_emitter_shim(tmp))
            recs = diff_design(
                Tier1Reference(), emitter, "blinky", "xcku3p",
                os.path.join(GOLDEN_DIR, "blinky.xcku3p.fasm"),
                20261107, "hash", "tier1", tmp,
            )
            by_block = {r["block"]: r for r in recs}
            carry8 = by_block["CARRY8"]
            # CTO ruling 2026-11-19: zero bits exercised is not passed;
            # an unexercised block reports blocked, like every other
            # block (FLAG-22-1).
            self.assertEqual(carry8["status"], "blocked")
            self.assertIsNone(carry8["diff_bits"])
            self.assertEqual(carry8["total_bits"], 0)

    def test_design_with_carry8_reports_carry8_line(self):
        with tempfile.TemporaryDirectory() as tmp:
            emitter = Emitter(_fake_emitter_shim(tmp))
            recs = diff_design(
                Tier1Reference(), emitter, "blinky", "xcku3p",
                self._carry8_fasm(tmp), 20261107, "hash", "tier1", tmp,
            )
            by_block = {r["block"]: r for r in recs}
            carry8 = by_block["CARRY8"]
            self.assertEqual(carry8["status"], "green")
            self.assertEqual(carry8["diff_bits"], 0)
            # DUAL_CY4 1 bit + DI0.DI0 1 bit + CI.V1 2 bits.
            self.assertEqual(carry8["total_bits"], 4)
            self.assertEqual(by_block["LUT_INIT"]["status"], "green")
            self.assertEqual(by_block["FF_INIT"]["status"], "green")
            self.assertEqual(by_block["INT_PIP"]["status"], "green")
            self.assertEqual(by_block["FRAMING"]["status"], "green")

    def test_carry8_flip_is_red(self):
        with tempfile.TemporaryDirectory() as tmp:
            emitter = Emitter(_fake_emitter_shim(tmp))
            os.environ["FAKE_FLIP_COUNT"] = "1"
            os.environ["FAKE_FLIP_BLOCK"] = "CARRY8"
            try:
                recs = diff_design(
                    Tier1Reference(), emitter, "blinky", "xcku3p",
                    self._carry8_fasm(tmp), 20261107, "hash", "tier1", tmp,
                )
            finally:
                del os.environ["FAKE_FLIP_COUNT"]
                del os.environ["FAKE_FLIP_BLOCK"]
            by_block = {r["block"]: r for r in recs}
            self.assertEqual(by_block["CARRY8"]["status"], "red")
            self.assertEqual(by_block["CARRY8"]["diff_bits"], 1)
            self.assertEqual(by_block["LUT_INIT"]["status"], "green")
            self.assertEqual(by_block["FF_INIT"]["status"], "green")

    def test_extra_carry8_bit_on_design_without_carry8_is_red(self):
        with tempfile.TemporaryDirectory() as tmp:
            emitter = Emitter(_fake_emitter_shim(tmp))
            os.environ["FAKE_EXTRA_BIT_BLOCK"] = "CARRY8"
            try:
                recs = diff_design(
                    Tier1Reference(), emitter, "blinky", "xcku3p",
                    os.path.join(GOLDEN_DIR, "blinky.xcku3p.fasm"),
                    20261107, "hash", "tier1", tmp,
                )
            finally:
                del os.environ["FAKE_EXTRA_BIT_BLOCK"]
            by_block = {r["block"]: r for r in recs}
            self.assertEqual(by_block["CARRY8"]["status"], "red")
            self.assertEqual(by_block["CARRY8"]["diff_bits"], 1)

    def test_no_emitter_carry8_is_blocked_not_green(self):
        with tempfile.TemporaryDirectory() as tmp:
            recs = diff_design(
                Tier1Reference(), None, "blinky", "xcku3p",
                os.path.join(GOLDEN_DIR, "blinky.xcku3p.fasm"),
                20261107, "hash", "tier1", tmp,
            )
            by_block = {r["block"]: r for r in recs}
            self.assertEqual(by_block["CARRY8"]["status"], "blocked")
            self.assertIsNone(by_block["CARRY8"]["diff_bits"])
            self.assertIn("CARRY8", by_block["CARRY8"]["missing"])


class TestRoundtrip(unittest.TestCase):
    def test_completes_missing_ff_init_with_v0(self):
        # Diff-harness session 3: a partial FASM completes to the
        # full form. Counter minus the DFF.INIT line gains
        # DFF.INIT.V0 (Xilinx default INIT is 0); completion is
        # idempotent, so the partial and the full golden canonicalize
        # identically.
        text = golden_fasm("counter", "xcku3p")
        partial = "\n".join(
            l for l in text.splitlines() if "DFF.INIT" not in l) + "\n"
        canon = canonical_roundtrip_fasm(partial, "counter")
        self.assertIn("SLICE_X0Y0.DFF.INIT.V0\n", canon)
        self.assertNotIn("DFF.INIT =", canon)
        full = canonical_roundtrip_fasm(text, "counter")
        self.assertEqual(canon, full)
        for bel in ("AFF", "BFF", "CFF", "DFF"):
            self.assertIn("SLICE_X0Y0.%s.INIT.V0\n" % bel, full)
        self.assertNotIn(".INIT =", full)

    def test_completes_missing_ff_srval_with_v0(self):
        # Diff-harness session 4: SRVAL completes like INIT. A
        # golden with no SRVAL line gains one .SRVAL.V0 per design
        # FF (Xilinx default SRVAL is 0); a valued SRVAL normalizes
        # to .V0/.V1; completion is idempotent.
        text = golden_fasm("counter", "xcku3p")
        canon = canonical_roundtrip_fasm(text, "counter")
        for bel in ("AFF", "BFF", "CFF", "DFF"):
            self.assertIn("SLICE_X0Y0.%s.SRVAL.V0\n" % bel, canon)
        self.assertNotIn("SRVAL =", canon)
        self.assertEqual(canonical_roundtrip_fasm(canon, "counter"), canon)
        valued = (
            "SLICE_X0Y0.AFF.SRVAL = 1\n"
            "SLICE_X0Y0.BFF.SRVAL = 1'b0\n"
        )
        canon2 = canonical_roundtrip_fasm(valued, "blinky")
        self.assertIn("SLICE_X0Y0.AFF.SRVAL.V1\n", canon2)
        self.assertIn("SLICE_X0Y0.BFF.SRVAL.V0\n", canon2)
        self.assertIn("SLICE_X0Y0.AFF.INIT.V0\n", canon2)

    def test_normalizes_valued_ff_init_and_verilog_lut_value(self):
        # Valued FF INIT/SRVAL spellings become .V0/.V1 (the V suffix
        # names the value); the real emitter's Verilog 64'h LUT value
        # becomes the golden's 0x spelling; line order does not
        # matter, the comparison is on the set of lines.
        text = (
            "SLICE_X0Y0.A6LUT.INIT[63:0] = 64'h8000000000000001\n"
            "SLICE_X0Y0.AFF.INIT = 1'b0\n"
            "SLICE_X0Y0.BFF.INIT = 1\n"
            "SLICE_X0Y0.AFF.SRVAL = 1\n"
        )
        canon = canonical_roundtrip_fasm(text, "blinky")
        self.assertIn(
            "SLICE_X0Y0.A6LUT.INIT[63:0] = 0x8000000000000001\n", canon)
        self.assertIn("SLICE_X0Y0.AFF.INIT.V0\n", canon)
        self.assertIn("SLICE_X0Y0.BFF.INIT.V1\n", canon)
        self.assertIn("SLICE_X0Y0.AFF.SRVAL.V1\n", canon)
        shuffled = "\n".join(reversed(text.strip().split("\n"))) + "\n"
        self.assertEqual(
            canonical_roundtrip_fasm(shuffled, "blinky"), canon)

    def test_real_emitter_parseback_spelling_matches_complete_golden(self):
        # The real emitter renders .V0/.V1 (never valued spelling) and
        # Verilog 64'h LUT values. A complete golden and its
        # parse-back spelling canonicalize identically. A partial
        # golden against a .V1-rendering parse-back differs on the
        # value (.V0 completed vs .V1 rendered), not the structure:
        # the emitted bitstream genuinely encodes INIT=1 where the
        # design default is INIT=0, so the check fails precisely.
        complete = (
            "SLICE_X0Y0.A6LUT.INIT[63:0] = 0x5555555555555555\n"
            "SLICE_X0Y0.AFF.INIT = 0\n"
        )
        parseback = (
            "SLICE_X0Y0.A6LUT.INIT[63:0] = 64'h5555555555555555\n"
            "SLICE_X0Y0.AFF.INIT.V0\n"
        )
        self.assertEqual(
            canonical_roundtrip_fasm(complete, "blinky"),
            canonical_roundtrip_fasm(parseback, "blinky"))
        partial = "SLICE_X0Y0.A6LUT.INIT[63:0] = 0x5555555555555555\n"
        parseback_partial = (
            "SLICE_X0Y0.A6LUT.INIT[63:0] = 64'h5555555555555555\n"
            "SLICE_X0Y0.AFF.INIT.V1\n"
        )
        self.assertNotEqual(
            canonical_roundtrip_fasm(partial, "blinky"),
            canonical_roundtrip_fasm(parseback_partial, "blinky"))
        self.assertIn(
            "SLICE_X0Y0.AFF.INIT.V0\n",
            canonical_roundtrip_fasm(partial, "blinky"))

    def test_rejects_bare_ff_init_and_unknown_design(self):
        # Bare AFF.INIT carries no value to select V0 or V1: fail
        # loud, never guess. A .V0/.V1 line takes no value, and an
        # FF value outside 0/1 is not a value at all.
        with self.assertRaises(FasmError):
            canonical_roundtrip_fasm("SLICE_X0Y0.AFF.INIT\n", "blinky")
        with self.assertRaises(FasmError):
            canonical_roundtrip_fasm(
                "SLICE_X0Y0.AFF.INIT.V0 = 0\n", "blinky")
        with self.assertRaises(FasmError):
            canonical_roundtrip_fasm("SLICE_X0Y0.AFF.INIT = 2\n", "blinky")
        with self.assertRaises(FasmError):
            canonical_roundtrip_fasm(
                golden_fasm("blinky", "xcku3p"), "nope")

    def test_feature_scoped_comparison_drops_own_tile_noise(self):
        # Diff-harness session 5: the FRAMING comparison is scoped
        # to the design's feature set. Parse-back lines for unused
        # BELs and attribute pairs on the design's own tile (unused
        # FF INIT.V1/SRVAL.V1, ABCDFF/EFGHFF CEUSED/CLKINV/SRINV/
        # SRUSED/MODE/SYNC, CARRY_TYPE, DI0-DI7) and on other tiles
        # are dropped after canonicalization, so the scoped
        # parse-back equals the completed golden. Scoping the
        # golden to its own keys is a no-op by construction. A
        # wrong value on a design feature survives the scope: V1
        # for AFF.INIT shares the (tile, AFF.INIT) key with the
        # golden's V0, so the mismatch is still visible.
        text = golden_fasm("blinky", "xcku3p")
        golden = canonical_roundtrip_fasm(text, "blinky")
        keys = roundtrip_feature_keys(golden)
        design_tiles = {f.tile for f in parse_fasm(text)}
        noise = (
            "SLICE_X0Y0.BFF.INIT.V1\n"
            "SLICE_X0Y0.BFF.SRVAL.V1\n"
            "SLICE_X0Y0.ABCDFF.CEUSED.V0\n"
            "SLICE_X0Y0.ABCDFF.CLKINV.V0\n"
            "SLICE_X0Y0.ABCDFF.SRINV.V0\n"
            "SLICE_X0Y0.ABCDFF.SRUSED.V0\n"
            "SLICE_X0Y0.ABCDFF.MODE.FF\n"
            "SLICE_X0Y0.ABCDFF.SYNC.ASYNC\n"
            "SLICE_X0Y0.CARRY8.CARRY_TYPE.SINGLE_CY8\n"
            "SLICE_X0Y0.CARRY8.DI0.AX\n"
            "SLICE_X9Y9.AFF.INIT.V1\n"
            "SLICE_X9Y9.AFF.SRVAL.V1\n"
        )
        parseback = text + noise
        scoped = scope_fasm_to_feature_keys(
            canonical_roundtrip_fasm(parseback, "blinky",
                                     tiles=design_tiles),
            keys)
        self.assertEqual(scoped, golden)
        self.assertEqual(scope_fasm_to_feature_keys(golden, keys), golden)
        wrong = text.replace("SLICE_X0Y0.AFF.INIT = 0",
                             "SLICE_X0Y0.AFF.INIT = 1") + noise
        scoped_wrong = scope_fasm_to_feature_keys(
            canonical_roundtrip_fasm(wrong, "blinky",
                                     tiles=design_tiles),
            keys)
        self.assertNotEqual(scoped_wrong, golden)
        self.assertIn("SLICE_X0Y0.AFF.INIT.V1\n", scoped_wrong)


class TestZeroBitPip(unittest.TestCase):
    def setUp(self):
        self.ref = Tier1Reference()

    # --- the observability rule, on synthetic DB entries ---

    def test_all_bang_without_complement_is_unobservable(self):
        bits = [(1, 11, 0), (3, 10, 0), (3, 11, 0)]
        self.assertTrue(is_unobservable_feature(bits, set()))

    def test_all_bang_with_exact_complement_is_observable(self):
        bits = [(12, 2, 0)]
        comp = {frozenset([(12, 2)])}
        self.assertFalse(is_unobservable_feature(bits, comp))

    def test_feature_with_a_set_bit_is_observable(self):
        bits = [(21, 35, 1), (36, 32, 0)]
        self.assertFalse(is_unobservable_feature(bits, set()))

    def test_empty_entry_is_not_unobservable(self):
        self.assertFalse(is_unobservable_feature([], set()))

    def test_drop_feature_keys_removes_only_named_lines(self):
        text = (
            "INT_X0Y0.PIP.IMUX_W23.VCC_WIRE\n"
            "INT_X0Y0.PIP.IMUX_E9.INT_NODE_IMUX_19_INT_OUT1\n"
        )
        keys = {("INT_X0Y0", "PIP.IMUX_W23.VCC_WIRE")}
        dropped = drop_feature_keys(text, keys)
        self.assertNotIn("VCC_WIRE", dropped)
        self.assertIn(
            "INT_X0Y0.PIP.IMUX_E9.INT_NODE_IMUX_19_INT_OUT1\n", dropped)
        # Dropping an empty key set keeps every line (sorted, like
        # the scope helper: canonical form is sorted).
        self.assertEqual(
            set(drop_feature_keys(text, set()).splitlines()),
            set(text.splitlines()))

    # --- the rule against the real segbits_int.db ---

    def test_five_failing_vcc_wire_features_are_unobservable(self):
        # The five US-07 FRAMING reds (F4): each feature is 6
        # must-be-zero bits, 0 set bits, no exact complement.
        cases = {
            "rand057": "INT.PIP.IMUX_W23.VCC_WIRE",
            "rand073": "INT.PIP.IMUX_E23.VCC_WIRE",
            "rand079": "INT.PIP.IMUX_E16.VCC_WIRE",
            "rand085": "INT.PIP.IMUX_E24.VCC_WIRE",
            "rand097": "INT.PIP.BYPASS_E10.VCC_WIRE",
        }
        db = self.ref.segbits("int")
        comp = complement_signatures(db)
        self.assertEqual(comp, set())
        for design, key in cases.items():
            bits = db[key]
            self.assertEqual(len(bits), 6, design)
            self.assertEqual(sum(v for (_, _, v) in bits), 0, design)
            self.assertTrue(is_unobservable_feature(bits, comp), design)

    def test_int_db_all_bang_count_is_145_with_no_complement(self):
        # Structural fact: every INT PIP lists at least one `!` bit,
        # so the INT complement set is empty and all 145 all-`!`
        # features (all VCC_WIRE) are unobservable.
        db = self.ref.segbits("int")
        all_bang = [k for k, b in db.items()
                    if all(v == 0 for (_, _, v) in b)]
        self.assertEqual(len(all_bang), 145)
        self.assertTrue(all(k.endswith(".VCC_WIRE") for k in all_bang))
        pure_set = [k for k, b in db.items()
                    if b and all(v == 1 for (_, _, v) in b)]
        self.assertEqual(pure_set, [])

    def test_ctrl_w5_vcc_wire_is_observable(self):
        # The one VCC_WIRE with a set bit (the normalization report's
        # DB exception): it sets 21_35, so parse-back renders it and
        # the contract must NOT drop it.
        db = self.ref.segbits("int")
        bits = db["INT.PIP.CTRL_W5.VCC_WIRE"]
        self.assertEqual(sum(v for (_, _, v) in bits), 1)
        self.assertIn((21, 35, 1), bits)
        self.assertFalse(
            is_unobservable_feature(bits, complement_signatures(db)))

    def test_ff_v1_has_complement_and_is_observable(self):
        # The guard against over-dropping: AFF.INIT.V1 is all-`!`
        # but its zero signature is AFF.INIT.V0's set-bit signature.
        db = self.ref.segbits("clel_l")
        comp = complement_signatures(db)
        v1 = db["CLEL_L.AFF.INIT.V1"]
        self.assertEqual(v1, [(12, 2, 0)])
        self.assertIn(zero_signature(v1), comp)
        self.assertFalse(is_unobservable_feature(v1, comp))

    def test_unobservable_keys_from_design_features(self):
        # Feature objects, not design files: the five failing
        # VCC_WIRE lines plus one observable PIP and the CTRL_W5
        # exception. Only the five come back.
        feats = [
            Feature("INT_X87Y0", "PIP.IMUX_W23.VCC_WIRE"),
            Feature("INT_X86Y0", "PIP.IMUX_E23.VCC_WIRE"),
            Feature("INT_X62Y0", "PIP.IMUX_E16.VCC_WIRE"),
            Feature("INT_X45Y0", "PIP.IMUX_E24.VCC_WIRE"),
            Feature("INT_X98Y0", "PIP.BYPASS_E10.VCC_WIRE"),
            Feature("INT_X0Y0", "PIP.IMUX_E9.INT_NODE_IMUX_19_INT_OUT1"),
            Feature("INT_X0Y0", "PIP.CTRL_W5.VCC_WIRE"),
        ]
        keys = unobservable_int_pip_keys(self.ref, feats)
        self.assertEqual(len(keys), 5)
        for tile, feat in keys:
            self.assertTrue(feat.endswith(".VCC_WIRE"))
            self.assertNotIn("CTRL_W5", feat)

    # --- end to end: FRAMING through the fake emitter ---

    def _diff_with_dropped_parse(self, text, drop, design="blinky"):
        # Runs diff_design with the fake emitter dropping `drop`
        # (comma-separated TILE.FEATURE) from its parse output,
        # mimicking the real emitter's parse-back dropping a feature
        # it cannot render. Returns {block: record}.
        with tempfile.TemporaryDirectory() as tmp:
            shim = _fake_emitter_shim(tmp)
            emitter = Emitter(shim)
            fasm_path = os.path.join(tmp, "design.fasm")
            with open(fasm_path, "w") as f:
                f.write(text)
            os.environ["FAKE_PARSE_DROP_FEATURES"] = drop
            try:
                recs = diff_design(
                    self.ref, emitter, design, "xcku3p",
                    fasm_path, 20261107, "hash", "tier1", tmp,
                )
            finally:
                del os.environ["FAKE_PARSE_DROP_FEATURES"]
            return {r["block"]: r for r in recs}

    def test_zero_bit_vcc_wire_roundtrip_green(self):
        # F4 regression: blinky plus rand057's VCC_WIRE, with the
        # fake parse dropping it like the real emitter's parse-back
        # does. Before the fix this is FRAMING red; after the fix
        # it is green, the exclusion named in checks_skipped, and
        # INT_PIP green (the covering bit-level check).
        pip = "INT_X0Y0.PIP.IMUX_W23.VCC_WIRE"
        text = golden_fasm("blinky", "xcku3p") + pip + "\n"
        by_block = self._diff_with_dropped_parse(text, pip)
        framing = by_block["FRAMING"]
        self.assertEqual(framing["status"], "green")
        self.assertEqual(framing["diff_bits"], 0)
        self.assertIn(
            "roundtrip-unobservable:" + pip,
            framing.get("checks_skipped", []))
        self.assertIn("1 skipped", framing["detail"])
        self.assertEqual(by_block["INT_PIP"]["status"], "green")
        self.assertEqual(by_block["INT_PIP"]["diff_bits"], 0)

    def test_observable_pip_dropped_still_red(self):
        # The fix must not blanket-drop PIPs: a normal PIP (one set
        # bit) dropped by parse-back is still a FRAMING red.
        pip = "INT_X0Y0.PIP.IMUX_E9.INT_NODE_IMUX_19_INT_OUT1"
        text = golden_fasm("blinky", "xcku3p") + pip + "\n"
        by_block = self._diff_with_dropped_parse(text, pip)
        self.assertEqual(by_block["FRAMING"]["status"], "red")
        self.assertEqual(by_block["FRAMING"]["diff_bits"], 1)

    def test_ctrl_w5_vcc_wire_dropped_still_red(self):
        # Same for the one observable VCC_WIRE: it sets 21_35, so a
        # parse-back that drops it is a real defect, still red.
        pip = "INT_X0Y0.PIP.CTRL_W5.VCC_WIRE"
        text = golden_fasm("blinky", "xcku3p") + pip + "\n"
        by_block = self._diff_with_dropped_parse(text, pip)
        self.assertEqual(by_block["FRAMING"]["status"], "red")
        self.assertEqual(by_block["FRAMING"]["diff_bits"], 1)

    def test_vcc_wire_only_design_roundtrips_green(self):
        # A design whose only non-default feature is a zero-bit PIP:
        # FRAMING green with the exclusion recorded, INT_PIP green
        # over exactly its 6 must-be-zero bits.
        pip = "INT_X0Y0.PIP.IMUX_W23.VCC_WIRE"
        by_block = self._diff_with_dropped_parse(pip + "\n", pip)
        self.assertEqual(by_block["FRAMING"]["status"], "green")
        self.assertEqual(by_block["INT_PIP"]["status"], "green")
        self.assertEqual(by_block["INT_PIP"]["total_bits"], 6)
        self.assertEqual(by_block["INT_PIP"]["diff_bits"], 0)


class TestReference(unittest.TestCase):
    def setUp(self):
        self.ref = Tier1Reference()

    def test_blinky_lut_spot_values(self):
        # SLICE_X0Y0: row 0, CLB col major 5, tile CLEL_R.
        # A6LUT pair[0] = (minor 11, offset 15); pair[1] = (minor 10, 15).
        bits = self.ref.resolve_lut("xcku3p", 0, 0, "A6LUT", 0x5555555555555555)
        self.assertEqual(len(bits), 64)
        self.assertEqual(len(set(bits)), 64, "LUT bits must be distinct positions")
        far0 = (0 << 24) | (0 << 18) | (5 << 8) | 11
        far1 = (0 << 24) | (0 << 18) | (5 << 8) | 10
        self.assertEqual(bits[(far0, 15)], 1, "INIT bit 0 is 1")
        self.assertEqual(bits[(far1, 15)], 0, "INIT bit 1 is 0")

    def test_srval_resolves_to_prjuray_db_positions(self):
        # SRVAL resolver gap (orchestrator session 13): resolve_srval
        # returns the prjuray .SRVAL.V0 position for the golden FFs
        # (AFF/BFF/CFF/DFF on CLEL_R) with bit = NOT(value), the same
        # inverted polarity as INIT. Positions are read from the DB,
        # not hand-typed: the delta rule (same frame as the INIT
        # RegLoc entry, offset -1 for A/C/E/G, +1 for B/D/F/H) must
        # agree with the DB for every FF. And a golden carrying
        # SRVAL lines resolves through resolve_feature with no
        # MissingReference.
        db = self.ref.segbits("clel_r")
        far_base = (0 << 24) | (0 << 18) | (5 << 8) | 12
        for bel in ("AFF", "BFF", "CFF", "DFF"):
            frame, bit, val = db["CLEL_R.%s.SRVAL.V0" % bel][0]
            self.assertEqual((frame, val), (12, 1), bel)
            self.assertEqual(
                self.ref.resolve_srval("xcku3p", 0, 0, bel, 0),
                {(far_base, bit): 1}, bel)
            self.assertEqual(
                self.ref.resolve_srval("xcku3p", 0, 0, bel, 1),
                {(far_base, bit): 0}, bel)
            self.assertTrue(
                self.ref.cross_check_srval("xcku3p", 0, 0, bel), bel)
        for bel in ("AFF", "BFF", "CFF", "DFF", "EFF", "FFF", "GFF", "HFF",
                    "AFF2", "BFF2", "CFF2", "DFF2", "EFF2", "FFF2", "GFF2", "HFF2"):
            self.assertTrue(
                self.ref.cross_check_srval("xcku3p", 0, 0, bel), bel)
        for design in ("blinky", "counter"):
            for feat in parse_fasm(golden_fasm(design, "xcku3p")):
                if ".SRVAL" not in feat.feature:
                    continue
                block, bits, coord, miss = resolve_feature(
                    self.ref, "xcku3p", feat)
                self.assertIsNone(miss, feat)
                self.assertEqual(block, "FF_INIT")
                self.assertEqual(coord, "absolute")
                self.assertEqual(len(bits), 1)

    def test_ff_init_inverted_polarity(self):
        # XAPP1230: CLB register frame bit is inverted vs INIT.
        # AFF INIT=0 stores frame bit 1 at (minor 12, offset 2).
        bits = self.ref.resolve_ff("xcku3p", 0, 0, "AFF", 0)
        far = (0 << 24) | (0 << 18) | (5 << 8) | 12
        self.assertEqual(bits, {(far, 2): 1})
        bits1 = self.ref.resolve_ff("xcku3p", 0, 0, "AFF", 1)
        self.assertEqual(bits1, {(far, 2): 0})

    def test_cross_checks_all_bels(self):
        for bel in ("A6LUT", "B6LUT", "C6LUT", "D6LUT", "E6LUT", "F6LUT", "G6LUT", "H6LUT"):
            checked, mismatches = self.ref.cross_check_lut("xcku3p", 0, 0, bel)
            self.assertEqual((checked, mismatches), (64, []), bel)
        for bel in ("AFF", "BFF", "CFF", "DFF", "EFF", "FFF", "GFF", "HFF",
                    "AFF2", "BFF2", "CFF2", "DFF2", "EFF2", "FFF2", "GFF2", "HFF2"):
            self.assertTrue(self.ref.cross_check_ff("xcku3p", 0, 0, bel), bel)

    def test_xcvu3p_device_file_missing(self):
        with self.assertRaises(MissingReference) as cm:
            self.ref.device("xcvu3p")
        self.assertIn("xcvu3p device file", str(cm.exception))

    def test_int_pip_resolves(self):
        bits = self.ref.resolve_int_pip(
            "xcku3p", 0, 0, "PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1")
        # 6 bits: 64_00 !66_00 !68_00 !72_00 !72_01 !74_00, INT col = 5-1 = 4.
        self.assertEqual(len(bits), 6)
        far64 = (0 << 24) | (0 << 18) | (4 << 8) | 64
        self.assertEqual(bits[(far64, 0)], 1)
        far66 = (0 << 24) | (0 << 18) | (4 << 8) | 66
        self.assertEqual(bits[(far66, 0)], 0)

    def test_int_col_offset_is_tile_type_dependent(self):
        # Two independent public sources agree on the split: the
        # xczu3eg tilegrid geometry (prjuray-db) and the xcku3p
        # device-file minors table (52 left, all CLEL_R; 61 right,
        # all other CLB types; 0 overlap, 0 neither).
        self.assertEqual(int_col_offset_from_clb("CLEL_R"), -1)
        for tile_type in ("CLEL_L", "CLEM", "CLEM_R"):
            self.assertEqual(int_col_offset_from_clb(tile_type), +1)

    def test_int_pip_resolves_on_clem_column(self):
        # CLB column 1 is CLEM, major 6, so its INT tile is major 7
        # (+1 branch): the BOUNCE bits land at column 7, not 5.
        bits = self.ref.resolve_int_pip(
            "xcku3p", 1, 0, "PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1")
        self.assertEqual(len(bits), 6)
        far64 = (0 << 24) | (0 << 18) | (7 << 8) | 64
        self.assertEqual(bits[(far64, 0)], 1)
        far66 = (0 << 24) | (0 << 18) | (7 << 8) | 66
        self.assertEqual(bits[(far66, 0)], 0)

    def test_int_pip_y_is_absolute_row(self):
        # F3: INT tile Y is an absolute tile-row coordinate, not
        # the FAR row. FAR row is Y // 60 (60 CLB rows per
        # clock-region row). Grounded in the xczu3eg tilegrid:
        # INT_X11Y60 baseaddr 0x00042300 decodes to block 0,
        # row 1, col 35, minor 0, and INT_X11Y0..Y59 all share
        # the row-0 baseaddr 0x00002300. The emitter used to
        # read Y as the FAR row itself (bitgen_device.rs
        # parse_int_tile); this test pins the harness side so a
        # future "alignment" with the emitter cannot silently
        # flip the convention back. CLB column 11 is CLEL_R,
        # major 26, in every xcku3p row, so its INT column is
        # major 25 in every row; only the row changes with Y.
        for y, row in ((0, 0), (1, 0), (59, 0), (60, 1),
                       (119, 1), (120, 2), (179, 2), (239, 3)):
            bits = self.ref.resolve_int_pip(
                "xcku3p", 11, y, "PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1")
            self.assertEqual(len(bits), 6, "Y=%d" % y)
            rows = set((far >> 18) & 0x3F for far, _ in bits)
            self.assertEqual(rows, set([row]), "Y=%d" % y)
            cols = set((far >> 8) & 0x3FF for far, _ in bits)
            self.assertEqual(cols, set([25]), "Y=%d" % y)
            far64 = (0 << 24) | (row << 18) | (25 << 8) | 64
            self.assertEqual(bits[(far64, 0)], 1, "Y=%d" % y)

    def test_unknown_pip_missing(self):
        with self.assertRaises(MissingReference):
            self.ref.resolve_int_pip("xcku3p", 0, 0, "PIP.NOPE.NOPE")

    def test_rclk_pip_resolves_tile_relative(self):
        bits = self.ref.resolve_rclk(
            "RCLK_INT_L_X0Y0", "PIP.CLK_LEAF_SITES_0_CLK_IN.CLK_HDISTR_FT0_0"
        )
        # 11 bits (clk.rs): 00_15, 01_15, 02_17 set; 8 must-be-zero.
        self.assertEqual(len(bits), 11)
        self.assertEqual(bits[("RCLK_INT_L_X0Y0", 0, 15)], 1)
        self.assertEqual(bits[("RCLK_INT_L_X0Y0", 1, 15)], 1)
        self.assertEqual(bits[("RCLK_INT_L_X0Y0", 2, 17)], 1)
        self.assertEqual(bits[("RCLK_INT_L_X0Y0", 0, 16)], 0)
        self.assertEqual(sum(bits.values()), 3)

    def test_rclk_bufce_leaf_resolves_tile_relative(self):
        bits = self.ref.resolve_rclk(
            "RCLK_INT_L_X0Y0", "BUFCE_LEAF_X0Y0.BUFCE_LEAF.CE_TYPE.SYNC"
        )
        # CE_TYPE.SYNC is !01_12: one must-be-zero bit, no set bit.
        self.assertEqual(bits, {("RCLK_INT_L_X0Y0", 1, 12): 0})
        bits = self.ref.resolve_rclk(
            "RCLK_INT_L_X0Y0", "BUFCE_LEAF_X0Y0.BUFCE_LEAF.IN_USE.V1"
        )
        self.assertEqual(bits, {("RCLK_INT_L_X0Y0", 0, 15): 1})
        bits = self.ref.resolve_rclk(
            "RCLK_INT_L_X0Y0", "BUFCE_LEAF_X0Y0.BUFCE_LEAF.DELAY_TAP.V0"
        )
        self.assertEqual(len(bits), 5)
        self.assertEqual(bits[("RCLK_INT_L_X0Y0", 0, 13)], 1)

    def test_rclk_unknown_feature_and_tile_fail_loud(self):
        with self.assertRaises(MissingReference):
            self.ref.resolve_rclk("RCLK_INT_L_X0Y0", "PIP.NOPE.NOPE")
        with self.assertRaises(MissingReference):
            self.ref.resolve_rclk(
                "INT_X0Y0", "PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1"
            )

    def test_cmt_wire_resolves_tile_relative(self):
        bits = self.ref.resolve_cmt_pip(
            "CMT_RIGHT_X0Y0", "WIRE.CLK_HDISTR_0_0.USED.V1"
        )
        self.assertEqual(bits, {("CMT_RIGHT_X0Y0", 7, 33): 1})
        bits = self.ref.resolve_cmt_pip(
            "CMT_RIGHT_X0Y0", "WIRE.CLK_HROUTE_1_0.USED.V1"
        )
        self.assertEqual(bits, {("CMT_RIGHT_X0Y0", 7, 149): 1})

    def test_cmt_left_has_no_segbits_file(self):
        with self.assertRaises(MissingReference) as ctx:
            self.ref.resolve_cmt_pip(
                "CMT_L_X0Y0", "WIRE.CLK_HDISTR_0_0.USED.V1"
            )
        self.assertIn("CMT_L", str(ctx.exception))

    def test_dsp_uram_have_no_encoding(self):
        feats = parse_fasm(
            "DSP_X0Y0.DSP48E2.AREG = 1\n"
            "URAM_X0Y0.URAM288.INIT = 0\n"
        )
        for feat in feats:
            _, _, _, miss = resolve_feature(self.ref, "xcku3p", feat)
            self.assertIsNotNone(miss, feat)
            self.assertIn("absent", miss)

    def test_bram_resolves_whole_array_and_spot_bits(self):
        from fasm import Feature
        from gen_golden import bram_content_value, bram_parity_value
        content = bram_content_value()
        parity = bram_parity_value()
        feat = Feature("RAMB18_X0Y0", "INIT[16383:0]", "0x%04096x" % content)
        block, bits, coord, miss = resolve_feature(self.ref, "xcku3p", feat)
        self.assertIsNone(miss)
        self.assertEqual(block, "BRAM_CONTENT")
        self.assertEqual(coord, "absolute")
        self.assertEqual(len(bits), 16384)
        # Content bit 0 (word 0 all-ones) is (far 0x01000000, ofst 0) = 1.
        self.assertEqual(bits[(0x01000000, 0)], 1)
        # Content bit 256 (word 1 all-zero) is (far 0x01000004, ofst 0) = 0.
        self.assertEqual(bits[(0x01000004, 0)], 0)
        # 8192 set bits: word 0 (256) + words 2..63 alternating (62*128).
        self.assertEqual(sum(bits.values()), 8192)
        feat = Feature("RAMB18_X0Y0", "INITP[2047:0]", "0x%0512x" % parity)
        block, bits, coord, miss = resolve_feature(self.ref, "xcku3p", feat)
        self.assertIsNone(miss)
        self.assertEqual(block, "BRAM_PARITY")
        self.assertEqual(len(bits), 2048)
        self.assertEqual(bits[(0x01000000, 48)], 1, "parity bit 0")
        self.assertEqual(bits[(0x0100001F, 59)], 1, "parity bit 255")
        self.assertEqual(sum(bits.values()), 1024)
        # Verilog spelling (emitter parse-back) resolves identically.
        feat = Feature(
            "RAMB18_X0Y0", "INIT[16383:0]", "16384'h%04096X" % content
        )
        block, bits, coord, miss = resolve_feature(self.ref, "xcku3p", feat)
        self.assertIsNone(miss)
        self.assertEqual(len(bits), 16384)
        self.assertEqual(sum(bits.values()), 8192)

    def test_bram36_reports_missing_not_green(self):
        from fasm import Feature
        feat = Feature("RAMB36_X0Y0", "INIT[32767:0]", "0x1")
        block, bits, coord, miss = resolve_feature(self.ref, "xcku3p", feat)
        self.assertEqual(block, "BRAM_CONTENT")
        self.assertEqual(bits, {})
        self.assertIsNotNone(miss)
        self.assertIn("36K", miss)


class TestBramResolver(unittest.TestCase):
    """BRAM reference resolver, ported from the live harness
    (FLAG-25-2): whole-array INIT/INITP resolve through
    parse_bram_value plus ref.resolve_bram/resolve_bram_parity.

    The live port is whole-array only: per-word INIT_XX spellings are
    not a separate path here (the emitter accepts them, the harness
    resolver does not split them; see the lane report follow-ups).
    """
    TILE = "RAMB18_X7Y85"
    # Deterministic patterns with both 0 and 1 bits across the array:
    # content bit i is 1 when i % 3 == 0, parity bit i when i % 5 == 0.
    CONTENT_VAL = sum(1 << i for i in range(0, 16384, 3))
    PARITY_VAL = sum(1 << i for i in range(0, 2048, 5))

    def setUp(self):
        self.ref = Tier1Reference()

    def _resolve(self, feature, value):
        return resolve_feature(
            self.ref, "xcku3p",
            Feature(self.TILE, feature, value))

    def test_whole_array_content_resolves(self):
        block, bits, coord, miss = self._resolve(
            "INIT[16383:0]", "0x%x" % self.CONTENT_VAL)
        self.assertIsNone(miss)
        self.assertEqual(block, "BRAM_CONTENT")
        self.assertEqual(coord, "absolute")
        self.assertEqual(len(bits), 16384)
        self.assertEqual(sum(bits.values()), 5462)

    def test_whole_array_parity_resolves(self):
        block, bits, coord, miss = self._resolve(
            "INITP[2047:0]", "0x%x" % self.PARITY_VAL)
        self.assertIsNone(miss)
        self.assertEqual(block, "BRAM_PARITY")
        self.assertEqual(coord, "absolute")
        self.assertEqual(len(bits), 2048)
        self.assertEqual(sum(bits.values()), 410)

    def test_no_bram_design_blocked_with_generic_wording(self):
        # rand001 carries no BRAM features: both BRAM blocks report
        # blocked. BRAM left NOT_EXERCISED in the port, so the
        # wording is the generic no-features one, not a BRAM string.
        harness_dir = os.path.dirname(
            os.path.dirname(os.path.abspath(__file__)))
        with open(os.path.join(
                harness_dir, "generated", "rand001.xcku3p.meta.json")) as f:
            meta = json.load(f)
        DESIGNS["rand001"] = {"luts": [], "ffs": [], "ff_locations": []}
        workdir = tempfile.mkdtemp(prefix="bram-nobram-")
        try:
            records = diff_design(
                self.ref, None, "rand001", "xcku3p",
                os.path.join(harness_dir, "generated", "rand001.xcku3p.fasm"),
                meta["seed"], meta["placement_hash"], "tier1", workdir)
        finally:
            # DESIGNS is shared module state: restore it so later
            # tests that iterate the golden designs never see rand001.
            del DESIGNS["rand001"]
        for block in ("BRAM_CONTENT", "BRAM_PARITY"):
            rec = [r for r in records if r["block"] == block][0]
            self.assertEqual(rec["status"], "blocked", block)
            self.assertEqual(
                rec.get("missing"),
                "no %s features in golden FASM for rand001/xcku3p; "
                "block not exercised by this design" % block,
                block)


class TestUram(unittest.TestCase):
    def setUp(self):
        self.ref = Tier1Reference()

    def test_classify_uram_lines(self):
        feats = parse_fasm(golden_fasm("uram", "xcku3p"))
        got = {}
        for feat in feats:
            got[classify(feat)] = got.get(classify(feat), 0) + 1
        self.assertEqual(got, {"URAM288": 6})

    def test_uram_golden_matches_generator(self):
        # The on-disk golden is exactly what build_fasm produces.
        for part in ("xcku3p", "xcvu3p"):
            self.assertEqual(
                build_fasm("uram", part), golden_fasm("uram", part),
                "uram/%s golden differs from build_fasm" % part)

    def test_uram_golden_lines_match_ug974(self):
        # The 6 non-comment lines must be exactly the six UG974
        # URAM288 attributes at their defaults (see
        # data/URAM288_SEARCH.md section 5). The spellings are the
        # ones the Rust is_uram288_entry detector matches.
        lines = [l for l in golden_fasm("uram", "xcku3p").splitlines()
                 if l and not l.startswith("#")]
        self.assertEqual(lines, [
            'URAM_X0Y0.URAM288.CASCADE_ORDER_A = "NONE"',
            'URAM_X0Y0.URAM288.CASCADE_ORDER_B = "NONE"',
            'URAM_X0Y0.URAM288.OREG_A = "FALSE"',
            'URAM_X0Y0.URAM288.OREG_B = "FALSE"',
            'URAM_X0Y0.URAM288.EN_ECC_RD_A = "FALSE"',
            'URAM_X0Y0.URAM288.EN_ECC_RD_B = "FALSE"',
        ])

    def test_uram_golden_has_no_init_line(self):
        # URAM288 has no INIT attributes in UG974. No feature line
        # in the golden may invent one. (The header comments discuss
        # INIT to explain its absence; only feature lines are
        # checked here.)
        lines = [l for l in golden_fasm("uram", "xcku3p").splitlines()
                 if l and not l.startswith("#")]
        for line in lines:
            self.assertNotIn("INIT", line)

    def test_uram_resolves_blocked_naming_absence(self):
        for feat in parse_fasm(golden_fasm("uram", "xcku3p")):
            _, _, _, miss = resolve_feature(self.ref, "xcku3p", feat)
            self.assertIsNotNone(miss, feat)
            self.assertIn("absent", miss)
            self.assertIn("URAM288", miss)

    def test_diff_uram_blocked_with_fake_emitter(self):
        with tempfile.TemporaryDirectory() as tmp:
            shim = _fake_emitter_shim(tmp)
            emitter = Emitter(shim)
            meta = golden_meta("uram", "xcku3p")
            recs = diff_design(
                self.ref, emitter, "uram", "xcku3p",
                os.path.join(GOLDEN_DIR, "uram.xcku3p.fasm"),
                meta["seed"], meta["placement_hash"], "tier1", tmp,
            )
            self.assertEqual(len(recs), 12)
            by_block = {r["block"]: r for r in recs}
            rec = by_block["URAM288"]
            self.assertEqual(rec["status"], "blocked")
            self.assertIsNone(rec["diff_bits"])
            self.assertIn("URAM288", rec["missing"])
            self.assertIn("absent", rec["missing"])


class TestLutram(unittest.TestCase):
    def setUp(self):
        self.ref = Tier1Reference()

    def test_classify_lutram_lines(self):
        feats = parse_fasm(golden_fasm("lutram", "xcku3p"))
        got = {}
        for feat in feats:
            got[classify(feat)] = got.get(classify(feat), 0) + 1
        self.assertEqual(got, {"LUT_INIT": 1, "LUTRAM": 6})

    def test_lutram_golden_matches_generator(self):
        # The on-disk golden is exactly what build_fasm produces.
        for part in ("xcku3p", "xcvu3p"):
            self.assertEqual(
                build_fasm("lutram", part), golden_fasm("lutram", part),
                "lutram/%s golden differs from build_fasm" % part)

    def test_lutram_golden_lines_match_rust_spelling(self):
        # The 7 non-comment lines of the golden. The mode lines match
        # the Rust lutram_fasm / lutram_slice_fasm output exactly;
        # the INIT line matches after roundtrip normalization only
        # (Rust renders Verilog 64'h..., the golden uses 0x...),
        # pinned below by canonicalizing the Rust spelling.
        lines = [l for l in golden_fasm("lutram", "xcku3p").splitlines()
                 if l and not l.startswith("#")]
        self.assertEqual(lines, [
            "SLICE_X1Y0.A6LUT.INIT[63:0] = 0x6996966996696996",
            "SLICE_X1Y0.ALUT.MODE.RAM",
            "SLICE_X1Y0.ALUT.DI1.DI1",
            "SLICE_X1Y0.ALUT.SIZE.LARGE",
            "SLICE_X1Y0.WA7USED.V0",
            "SLICE_X1Y0.WA8USED.V0",
            "SLICE_X1Y0.WA9USED.V0",
        ])
        rust_spelling = (
            "SLICE_X1Y0.A6LUT.INIT[63:0] = 64'h6996966996696996\n"
            "SLICE_X1Y0.ALUT.MODE.RAM\n"
            "SLICE_X1Y0.ALUT.DI1.DI1\n"
            "SLICE_X1Y0.ALUT.SIZE.LARGE\n"
            "SLICE_X1Y0.WA7USED.V0\n"
            "SLICE_X1Y0.WA8USED.V0\n"
            "SLICE_X1Y0.WA9USED.V0\n"
        )
        self.assertEqual(
            canonical_roundtrip_fasm(rust_spelling, "lutram"),
            canonical_roundtrip_fasm(
                golden_fasm("lutram", "xcku3p"), "lutram"),
        )

    def test_resolve_lutram_mode_exact_bits(self):
        # SLICE_X1Y0: row 0, CLB col major 6, tile CLEM. Absolute FARs
        # are base 0x600 plus the segbits frame (the tile minor).
        # MODE.RAM sets (9, 19) and clears (11, 18); DI1.DI1 sets
        # (10, 19); SIZE.LARGE, WA7/8/9USED.V0 clear one bit each.
        far = lambda minor: (0 << 24) | (0 << 18) | (6 << 8) | minor
        cases = {
            "ALUT.MODE.RAM": {(far(9), 19): 1, (far(11), 18): 0},
            "ALUT.DI1.DI1": {(far(10), 19): 1},
            "ALUT.SIZE.LARGE": {(far(8), 18): 0},
            "WA7USED.V0": {(far(8), 23): 0},
            "WA8USED.V0": {(far(11), 22): 0},
            "WA9USED.V0": {(far(9), 22): 0},
        }
        for feat_str, expected in cases.items():
            bits = self.ref.resolve_lutram_mode("xcku3p", 1, 0, feat_str)
            self.assertEqual(bits, expected, feat_str)
            feat = parse_fasm(
                "SLICE_X1Y0.%s\n" % feat_str)[0]
            block, rbits, coord, miss = resolve_feature(
                self.ref, "xcku3p", feat)
            self.assertIsNone(miss, feat_str)
            self.assertEqual(block, "LUTRAM")
            self.assertEqual(coord, "absolute")
            self.assertEqual(rbits, expected, feat_str)

    def test_lutram_mode_on_slicel_is_blocked(self):
        # SLICE_X0Y0 is CLEL_R: a LUTRAM there is unrepresentable and
        # resolves to a blocked record naming the tile type, never to
        # guessed bits.
        feat = parse_fasm("SLICE_X0Y0.ALUT.MODE.RAM\n")[0]
        _, _, _, miss = resolve_feature(self.ref, "xcku3p", feat)
        self.assertIsNotNone(miss)
        self.assertIn("CLEL_R", miss)
        self.assertIn("SLICEM", miss)

    def test_clem_cross_check(self):
        # The LUTRAM-extracted CLEM table agrees with prjuray too:
        # 64 checked, 0 mismatches on SLICE_X1Y0 A6LUT.
        self.assertEqual(
            self.ref.cross_check_lut("xcku3p", 1, 0, "A6LUT"), (64, []))

    def test_diff_lutram_green_with_fake_emitter(self):
        with tempfile.TemporaryDirectory() as tmp:
            shim = _fake_emitter_shim(tmp)
            emitter = Emitter(shim)
            meta = golden_meta("lutram", "xcku3p")
            recs = diff_design(
                self.ref, emitter, "lutram", "xcku3p",
                os.path.join(GOLDEN_DIR, "lutram.xcku3p.fasm"),
                meta["seed"], meta["placement_hash"], "tier1", tmp,
            )
            by_block = {r["block"]: r for r in recs}
            self.assertEqual(by_block["LUTRAM"]["status"], "green")
            self.assertEqual(by_block["LUTRAM"]["diff_bits"], 0)
            self.assertEqual(by_block["LUTRAM"]["total_bits"], 7)
            self.assertEqual(by_block["LUT_INIT"]["status"], "green")
            self.assertEqual(by_block["LUT_INIT"]["diff_bits"], 0)
            self.assertEqual(by_block["LUT_INIT"]["total_bits"], 64)
            self.assertEqual(by_block["FRAMING"]["status"], "green")

    def test_diff_lutram_xcvu3p_blocked(self):
        # No xcvu3p device file: both LUTRAM and LUT_INIT report
        # blocked with the missing data named, never zero.
        with tempfile.TemporaryDirectory() as tmp:
            shim = _fake_emitter_shim(tmp)
            emitter = Emitter(shim)
            meta = golden_meta("lutram", "xcvu3p")
            recs = diff_design(
                self.ref, emitter, "lutram", "xcvu3p",
                os.path.join(GOLDEN_DIR, "lutram.xcvu3p.fasm"),
                meta["seed"], meta["placement_hash"], "tier1", tmp,
            )
            by_block = {r["block"]: r for r in recs}
            for block in ("LUTRAM", "LUT_INIT"):
                self.assertEqual(by_block[block]["status"], "blocked")
                self.assertIsNone(by_block[block]["diff_bits"])
                self.assertIn("xcvu3p", by_block[block]["missing"])



class TestDiffEngine(unittest.TestCase):
    def setUp(self):
        self.ref = Tier1Reference()

    def test_compute_diff_counts_both_directions(self):
        ref_bits = {(0x50B, 15): 1, (0x50A, 15): 0, (0x50C, 2): 1}
        emitted = {(0x50B, 15): 0, (0x50A, 15): 0, (0x50C, 2): 1, (0x999, 1): 1}
        # One flipped required bit + one extra set bit = 2.
        self.assertEqual(compute_diff(ref_bits, emitted), 2)
        self.assertEqual(compute_diff(ref_bits, dict(ref_bits)), 0)

    def test_clk_golden_splits_resolved_from_blocked(self):
        # The 9 CLK lines of tests/fixtures/clk_blinky.fasm (clk
        # branch): 8 resolve tile-relative from public segbits DBs, 1
        # is blocked (CMT BUFGCE site config, no public encoding).
        # Feature objects, not the fixture file: the fixture lives on
        # the clk branch, not in this harness tree.
        from fasm import Feature
        lines = [
            ("CMT_L_X0Y0", "BUFGCE_X0Y0.BUFGCE.IINV.V0"),
            ("CMT_RIGHT_X0Y0", "WIRE.CLK_HDISTR_0_0.USED.V1"),
            ("CMT_RIGHT_X0Y0", "WIRE.CLK_HROUTE_1_0.USED.V1"),
            ("RCLK_INT_L_X0Y0", "BUFCE_LEAF_X0Y0.BUFCE_LEAF.CE_TYPE.SYNC"),
            ("RCLK_INT_L_X0Y0", "BUFCE_LEAF_X0Y0.BUFCE_LEAF.CEINV.V0"),
            ("RCLK_INT_L_X0Y0", "BUFCE_LEAF_X0Y0.BUFCE_LEAF.DELAY_TAP.V0"),
            ("RCLK_INT_L_X0Y0", "BUFCE_LEAF_X0Y0.BUFCE_LEAF.IINV.V0"),
            ("RCLK_INT_L_X0Y0", "BUFCE_LEAF_X0Y0.BUFCE_LEAF.IN_USE.V1"),
            ("RCLK_INT_L_X0Y0", "PIP.CLK_LEAF_SITES_0_CLK_IN.CLK_HDISTR_FT0_0"),
        ]
        resolved = []
        missing = []
        for tile, feat in lines:
            block, bits, coord, miss = resolve_feature(
                self.ref, "xcku3p", Feature(tile, feat)
            )
            self.assertEqual(block, "CLK", "%s.%s" % (tile, feat))
            if miss:
                missing.append((tile, feat, miss))
            else:
                self.assertEqual(coord, "tile-relative", "%s.%s" % (tile, feat))
                resolved.append((tile, feat, bits))
        self.assertEqual(len(resolved), 8)
        self.assertEqual(len(missing), 1)
        self.assertEqual(missing[0][0], "CMT_L_X0Y0")
        self.assertIn("BUFGCE", missing[0][1])
        self.assertIn("CMT_LEFT", missing[0][2])
        # Spot bit counts: RCLK PIP 11 bits (3 set), HDISTR USED 1 bit.
        pip = [b for _, f, b in resolved if "CLK_LEAF_SITES_0" in f][0]
        self.assertEqual(len(pip), 11)
        self.assertEqual(sum(pip.values()), 3)

    def test_blocked_without_emitter_never_zero(self):
        with tempfile.TemporaryDirectory() as tmp:
            recs = diff_design(
                self.ref, None, "blinky", "xcku3p",
                os.path.join(GOLDEN_DIR, "blinky.xcku3p.fasm"),
                20261107, "hash", "tier1", tmp,
            )
            self.assertEqual(len(recs), 12)
            for rec in recs:
                self.assertEqual(rec["status"], "blocked")
                self.assertIsNone(rec["diff_bits"])
                self.assertIn("missing", rec)
                self.assertTrue(rec["missing"], rec["block"])
            by_block = {r["block"]: r for r in recs}
            self.assertEqual(by_block["LUT_INIT"]["total_bits"], 64)
            # FF_INIT counts INIT plus SRVAL, one bit per FF each.
            self.assertEqual(by_block["FF_INIT"]["total_bits"], 2)
            self.assertEqual(by_block["INT_PIP"]["total_bits"], 11)
            self.assertEqual(by_block["IO_CONFIG"]["total_bits"], 55)
            # CARRY8 is blocked without an emitter, like every block:
            # no comparison ran, so no zero-bit green.
            self.assertEqual(by_block["CARRY8"]["total_bits"], 0)

    def test_greedy_order_largest_first(self):
        recs = [
            {"block": "A", "diff_bits": 0, "total_bits": 10, "status": "green"},
            {"block": "B", "diff_bits": None, "total_bits": 64, "status": "blocked"},
            {"block": "C", "diff_bits": 3, "total_bits": 64, "status": "red"},
        ]
        self.assertEqual([r["block"] for r in greedy_order(recs)], ["B", "C", "A"])

    def test_fake_emitter_green_and_red(self):
        fake = os.path.join(HARNESS_DIR, "tests", "fake_emitter.py")
        with tempfile.TemporaryDirectory() as tmp:
            shim = os.path.join(tmp, "emitter.sh")
            with open(shim, "w") as f:
                f.write("#!/bin/sh\nexec %s %s \"$@\"\n" % (sys.executable, fake))
            os.chmod(shim, 0o755)
            emitter = Emitter(shim)
            env = dict(os.environ)
            # Green: no flips.
            recs = diff_design(
                self.ref, emitter, "blinky", "xcku3p",
                os.path.join(GOLDEN_DIR, "blinky.xcku3p.fasm"),
                20261107, "hash", "tier1", tmp,
            )
            by_block = {r["block"]: r for r in recs}
            self.assertEqual(by_block["LUT_INIT"]["status"], "green")
            self.assertEqual(by_block["LUT_INIT"]["diff_bits"], 0)
            self.assertEqual(by_block["FF_INIT"]["status"], "green")
            self.assertEqual(by_block["INT_PIP"]["status"], "green")
            self.assertEqual(by_block["FRAMING"]["status"], "green")
            # IO stays blocked: tile-relative reference cannot be compared.
            self.assertEqual(by_block["IO_CONFIG"]["status"], "blocked")
            # Red: flip 3 LUT bits.
            env["FAKE_FLIP_COUNT"] = "3"
            env["FAKE_FLIP_BLOCK"] = "LUT_INIT"
            old_env = dict(os.environ)
            os.environ.update(env)
            try:
                recs = diff_design(
                    self.ref, emitter, "blinky", "xcku3p",
                    os.path.join(GOLDEN_DIR, "blinky.xcku3p.fasm"),
                    20261107, "hash", "tier1", tmp,
                )
            finally:
                os.environ.clear()
                os.environ.update(old_env)
            by_block = {r["block"]: r for r in recs}
            self.assertEqual(by_block["LUT_INIT"]["status"], "red")
            self.assertEqual(by_block["LUT_INIT"]["diff_bits"], 3)
            self.assertEqual(by_block["FF_INIT"]["status"], "green")
            # Corrupt parse: round-trip mismatch is a FRAMING red.
            os.environ["FAKE_PARSE_CORRUPT"] = "1"
            try:
                recs = diff_design(
                    self.ref, emitter, "blinky", "xcku3p",
                    os.path.join(GOLDEN_DIR, "blinky.xcku3p.fasm"),
                    20261107, "hash", "tier1", tmp,
                )
            finally:
                del os.environ["FAKE_PARSE_CORRUPT"]
            by_block = {r["block"]: r for r in recs}
            self.assertEqual(by_block["FRAMING"]["status"], "red")
            # Extra bit in a block the design does not use: red, not green.
            os.environ["FAKE_EXTRA_BIT_BLOCK"] = "DSP48E2"
            try:
                recs = diff_design(
                    self.ref, emitter, "blinky", "xcku3p",
                    os.path.join(GOLDEN_DIR, "blinky.xcku3p.fasm"),
                    20261107, "hash", "tier1", tmp,
                )
            finally:
                del os.environ["FAKE_EXTRA_BIT_BLOCK"]
            by_block = {r["block"]: r for r in recs}
            self.assertEqual(by_block["DSP48E2"]["status"], "red")
            self.assertEqual(by_block["DSP48E2"]["diff_bits"], 1)

    def test_partial_ff_init_roundtrips_clean_after_completion(self):
        # Diff-harness session 3: a partial FASM (three of the
        # counter's four FF INITs specified, DFF unspecified)
        # round-trips clean after canonicalization completes the
        # missing FF to the .V0 default. The fake emitter echoes the
        # partial input, so both sides complete identically.
        text = golden_fasm("counter", "xcku3p")
        partial = "\n".join(
            l for l in text.splitlines() if "DFF.INIT" not in l) + "\n"
        with tempfile.TemporaryDirectory() as tmp:
            shim = _fake_emitter_shim(tmp)
            emitter = Emitter(shim)
            fasm_path = os.path.join(tmp, "partial.fasm")
            with open(fasm_path, "w") as f:
                f.write(partial)
            recs = diff_design(
                self.ref, emitter, "counter", "xcku3p",
                fasm_path, 20261107, "hash", "tier1", tmp,
            )
            by_block = {r["block"]: r for r in recs}
            self.assertEqual(by_block["FRAMING"]["status"], "green")
            self.assertEqual(by_block["FF_INIT"]["status"], "green")
            # Completion is round-trip-only: the block diff still sees
            # the partial golden (3 specified INITs plus all 4 SRVALs,
            # 7 FF bits, not 8).
            self.assertEqual(by_block["FF_INIT"]["total_bits"], 7)

    def test_wrong_ff_init_value_still_fails_roundtrip(self):
        # A genuinely wrong INIT value must still fail the round-trip
        # check: completed .V0 against parsed .V1 is a mismatch. The
        # bits path reads the unflipped sidecar, so LUT/FF/INT stay
        # green through it.
        with tempfile.TemporaryDirectory() as tmp:
            shim = _fake_emitter_shim(tmp)
            emitter = Emitter(shim)
            os.environ["FAKE_PARSE_FF_FLIP"] = "1"
            try:
                recs = diff_design(
                    self.ref, emitter, "blinky", "xcku3p",
                    os.path.join(GOLDEN_DIR, "blinky.xcku3p.fasm"),
                    20261107, "hash", "tier1", tmp,
                )
            finally:
                del os.environ["FAKE_PARSE_FF_FLIP"]
            by_block = {r["block"]: r for r in recs}
            self.assertEqual(by_block["FRAMING"]["status"], "red")
            self.assertEqual(by_block["FRAMING"]["diff_bits"], 1)
            self.assertEqual(by_block["LUT_INIT"]["status"], "green")
            self.assertEqual(by_block["FF_INIT"]["status"], "green")
            self.assertEqual(by_block["INT_PIP"]["status"], "green")

    def test_device_wide_parseback_lines_outside_design_tiles_pass(self):
        # Diff-harness session 4: the real emitter's parse-back
        # renders FF lines for every tile in the device model.
        # Lines on tiles outside the design are dropped before
        # comparing, so FRAMING passes when the design's own tiles
        # match. The bits path reads the unflipped sidecar, so the
        # per-block records are unaffected either way.
        with tempfile.TemporaryDirectory() as tmp:
            shim = _fake_emitter_shim(tmp)
            emitter = Emitter(shim)
            os.environ["FAKE_PARSE_DEVICE_WIDE"] = "1"
            try:
                recs = diff_design(
                    self.ref, emitter, "blinky", "xcku3p",
                    os.path.join(GOLDEN_DIR, "blinky.xcku3p.fasm"),
                    20261107, "hash", "tier1", tmp,
                )
            finally:
                del os.environ["FAKE_PARSE_DEVICE_WIDE"]
            by_block = {r["block"]: r for r in recs}
            self.assertEqual(by_block["FRAMING"]["status"], "green")
            self.assertEqual(by_block["FF_INIT"]["status"], "green")
            self.assertEqual(by_block["LUT_INIT"]["status"], "green")
            self.assertEqual(by_block["INT_PIP"]["status"], "green")

    def test_wrong_value_on_design_tile_still_fails_with_device_wide_lines(self):
        # Scoping drops outside tiles only: a wrong INIT value on
        # the design's own tile still fails FRAMING, even with
        # device-wide lines present in the parse output.
        with tempfile.TemporaryDirectory() as tmp:
            shim = _fake_emitter_shim(tmp)
            emitter = Emitter(shim)
            os.environ["FAKE_PARSE_DEVICE_WIDE"] = "1"
            os.environ["FAKE_PARSE_FF_FLIP"] = "1"
            try:
                recs = diff_design(
                    self.ref, emitter, "blinky", "xcku3p",
                    os.path.join(GOLDEN_DIR, "blinky.xcku3p.fasm"),
                    20261107, "hash", "tier1", tmp,
                )
            finally:
                del os.environ["FAKE_PARSE_DEVICE_WIDE"]
                del os.environ["FAKE_PARSE_FF_FLIP"]
            by_block = {r["block"]: r for r in recs}
            self.assertEqual(by_block["FRAMING"]["status"], "red")
            self.assertEqual(by_block["FRAMING"]["diff_bits"], 1)
            self.assertEqual(by_block["LUT_INIT"]["status"], "green")
            self.assertEqual(by_block["FF_INIT"]["status"], "green")
            self.assertEqual(by_block["INT_PIP"]["status"], "green")

    def test_wrong_srval_value_still_fails_roundtrip(self):
        # Diff-harness session 4: a wrong SRVAL value on the
        # design's AFF fails the round trip. The golden completes
        # AFF.SRVAL to .V0 (Xilinx default SRVAL is 0); the parse
        # renders .V1. The bits path reads the unflipped sidecar,
        # so LUT/FF/INT stay green through it.
        with tempfile.TemporaryDirectory() as tmp:
            shim = _fake_emitter_shim(tmp)
            emitter = Emitter(shim)
            os.environ["FAKE_PARSE_SRVAL_V1"] = "1"
            try:
                recs = diff_design(
                    self.ref, emitter, "blinky", "xcku3p",
                    os.path.join(GOLDEN_DIR, "blinky.xcku3p.fasm"),
                    20261107, "hash", "tier1", tmp,
                )
            finally:
                del os.environ["FAKE_PARSE_SRVAL_V1"]
            by_block = {r["block"]: r for r in recs}
            self.assertEqual(by_block["FRAMING"]["status"], "red")
            self.assertEqual(by_block["FRAMING"]["diff_bits"], 1)
            self.assertEqual(by_block["LUT_INIT"]["status"], "green")
            self.assertEqual(by_block["FF_INIT"]["status"], "green")
            self.assertEqual(by_block["INT_PIP"]["status"], "green")

    def test_own_tile_noise_lines_outside_design_features_pass(self):
        # Diff-harness session 5: the real emitter's parse-back
        # renders lines for unused BELs and attribute pairs on the
        # design's OWN tile (unused FF INIT.V1/SRVAL.V1, ABCDFF/
        # EFGHFF CEUSED/CLKINV/SRINV/SRUSED/MODE/SYNC, CARRY_TYPE,
        # DI0-DI7), plus device-wide lines on other tiles. The
        # FRAMING comparison is scoped to the design's feature
        # set, so all of it is dropped and FRAMING passes when the
        # design's own features match. The bits path reads the
        # sidecar, so the per-block records are unaffected either
        # way. Runs on both goldens: the noise BELs are unused in
        # both designs.
        for design in ("blinky", "counter"):
            with tempfile.TemporaryDirectory() as tmp:
                shim = _fake_emitter_shim(tmp)
                emitter = Emitter(shim)
                os.environ["FAKE_PARSE_OWN_TILE_NOISE"] = "1"
                try:
                    recs = diff_design(
                        self.ref, emitter, design, "xcku3p",
                        os.path.join(GOLDEN_DIR, "%s.xcku3p.fasm" % design),
                        20261107, "hash", "tier1", tmp,
                    )
                finally:
                    del os.environ["FAKE_PARSE_OWN_TILE_NOISE"]
                by_block = {r["block"]: r for r in recs}
                self.assertEqual(by_block["FRAMING"]["status"], "green",
                                 design)
                self.assertEqual(by_block["FF_INIT"]["status"], "green",
                                 design)
                self.assertEqual(by_block["LUT_INIT"]["status"], "green",
                                 design)
                self.assertEqual(by_block["INT_PIP"]["status"], "green",
                                 design)

    def test_wrong_value_on_design_feature_still_fails_with_own_tile_noise(self):
        # Scoping drops lines outside the design's feature set
        # only: a wrong INIT value on the design's own AFF still
        # fails FRAMING, even with own-tile noise present in the
        # parse output. The V1 line shares the (tile, AFF.INIT)
        # key with the golden's V0, so it survives the scope and
        # the mismatch is visible.
        with tempfile.TemporaryDirectory() as tmp:
            shim = _fake_emitter_shim(tmp)
            emitter = Emitter(shim)
            os.environ["FAKE_PARSE_OWN_TILE_NOISE"] = "1"
            os.environ["FAKE_PARSE_FF_FLIP"] = "1"
            try:
                recs = diff_design(
                    self.ref, emitter, "blinky", "xcku3p",
                    os.path.join(GOLDEN_DIR, "blinky.xcku3p.fasm"),
                    20261107, "hash", "tier1", tmp,
                )
            finally:
                del os.environ["FAKE_PARSE_OWN_TILE_NOISE"]
                del os.environ["FAKE_PARSE_FF_FLIP"]
            by_block = {r["block"]: r for r in recs}
            self.assertEqual(by_block["FRAMING"]["status"], "red")
            self.assertEqual(by_block["FRAMING"]["diff_bits"], 1)
            self.assertEqual(by_block["LUT_INIT"]["status"], "green")
            self.assertEqual(by_block["FF_INIT"]["status"], "green")
            self.assertEqual(by_block["INT_PIP"]["status"], "green")

    def test_stray_set_bit_outside_design_features_caught_by_bits_diff(self):
        # Diff-harness session 5: scoping the FRAMING comparison
        # loses no coverage, because the per-block bits diff still
        # catches stray set bits anywhere. Own-tile noise in the
        # parse output is dropped, so FRAMING stays green, while a
        # bit the emitter set in a block the design does not use
        # (DSP48E2) is caught by the bits diff as a red with
        # diff_bits 1.
        with tempfile.TemporaryDirectory() as tmp:
            shim = _fake_emitter_shim(tmp)
            emitter = Emitter(shim)
            os.environ["FAKE_PARSE_OWN_TILE_NOISE"] = "1"
            os.environ["FAKE_EXTRA_BIT_BLOCK"] = "DSP48E2"
            try:
                recs = diff_design(
                    self.ref, emitter, "blinky", "xcku3p",
                    os.path.join(GOLDEN_DIR, "blinky.xcku3p.fasm"),
                    20261107, "hash", "tier1", tmp,
                )
            finally:
                del os.environ["FAKE_PARSE_OWN_TILE_NOISE"]
                del os.environ["FAKE_EXTRA_BIT_BLOCK"]
            by_block = {r["block"]: r for r in recs}
            self.assertEqual(by_block["FRAMING"]["status"], "green")
            self.assertEqual(by_block["DSP48E2"]["status"], "red")
            self.assertEqual(by_block["DSP48E2"]["diff_bits"], 1)
            self.assertEqual(by_block["LUT_INIT"]["status"], "green")
            self.assertEqual(by_block["FF_INIT"]["status"], "green")
            self.assertEqual(by_block["INT_PIP"]["status"], "green")

    def test_garbage_parse_output_is_framing_red_not_crash(self):
        # Critic session 5, FLAG 2: parse output that is not FASM at
        # all used to kill the run with a traceback and no records.
        # Now it is a FRAMING red naming the error, and the per-block
        # records (which compare through the bits path) are intact.
        with tempfile.TemporaryDirectory() as tmp:
            shim = _fake_emitter_shim(tmp)
            emitter = Emitter(shim)
            os.environ["FAKE_PARSE_GARBAGE"] = "1"
            try:
                recs = diff_design(
                    self.ref, emitter, "blinky", "xcku3p",
                    os.path.join(GOLDEN_DIR, "blinky.xcku3p.fasm"),
                    20261107, "hash", "tier1", tmp,
                )
            finally:
                del os.environ["FAKE_PARSE_GARBAGE"]
            self.assertEqual(len(recs), 12)
            by_block = {r["block"]: r for r in recs}
            framing = by_block["FRAMING"]
            self.assertEqual(framing["status"], "red")
            self.assertEqual(framing["diff_bits"], 1)
            self.assertIn("not valid FASM", framing["detail"])
            self.assertIn("no tile.feature separator", framing["detail"])
            self.assertEqual(by_block["LUT_INIT"]["status"], "green")
            self.assertEqual(by_block["LUT_INIT"]["diff_bits"], 0)
            self.assertEqual(by_block["FF_INIT"]["status"], "green")
            self.assertEqual(by_block["INT_PIP"]["status"], "green")


class TestCli(unittest.TestCase):
    def test_full_run_exits_zero_with_96_blocked_records(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = os.path.join(tmp, "run")
            proc = subprocess.run(
                [sys.executable, os.path.join(HARNESS_DIR, "diff.py"),
                 "--out", out],
                capture_output=True, text=True, cwd=HARNESS_DIR,
            )
            self.assertEqual(proc.returncode, 0, proc.stderr)
            recs = [json.loads(l) for l in open(os.path.join(out, "diff.jsonl"))]
            self.assertEqual(len(recs), 96)
            self.assertTrue(all(r["status"] == "blocked" for r in recs))
            self.assertTrue(all(r["diff_bits"] is None for r in recs))
            manifest = json.load(open(os.path.join(out, "manifest.json")))
            self.assertEqual(manifest["tier"], "tier1")
            self.assertEqual(len(manifest["combos"]), 8)

    def test_garbage_parse_emitter_full_run_writes_all_records(self):
        # Critic session 5, FLAG 2, end to end: a bad-parse emitter
        # over all 8 combos must exit 0 with all 96 records and a
        # manifest, never a traceback. Every combo's FRAMING record
        # is red with the error named.
        with tempfile.TemporaryDirectory() as tmp:
            shim = _fake_emitter_shim(tmp)
            env = dict(os.environ)
            env["FAKE_PARSE_GARBAGE"] = "1"
            out = os.path.join(tmp, "run")
            proc = subprocess.run(
                [sys.executable, os.path.join(HARNESS_DIR, "diff.py"),
                 "--emitter-cmd", shim, "--out", out],
                capture_output=True, text=True, cwd=HARNESS_DIR, env=env,
            )
            self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            self.assertNotIn("Traceback", proc.stderr)
            with open(os.path.join(out, "diff.jsonl")) as f:
                recs = [json.loads(l) for l in f]
            self.assertEqual(len(recs), 96)
            framing = [r for r in recs if r["block"] == "FRAMING"]
            self.assertEqual(len(framing), 8)
            for r in framing:
                self.assertEqual(r["status"], "red")
                self.assertEqual(r["diff_bits"], 1)
                self.assertIn("not valid FASM", r["detail"])
            with open(os.path.join(out, "manifest.json")) as f:
                manifest = json.load(f)
            self.assertEqual(len(manifest["combos"]), 8)

    def test_tampered_golden_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            golden_copy = os.path.join(tmp, "golden")
            import shutil
            shutil.copytree(GOLDEN_DIR, golden_copy)
            with open(os.path.join(golden_copy, "blinky.xcku3p.fasm"), "a") as f:
                f.write("SLICE_X0Y0.B6LUT.INIT[63:0] = 0x0\n")
            proc = subprocess.run(
                [sys.executable, os.path.join(HARNESS_DIR, "diff.py"),
                 "--design", "blinky", "--part", "xcku3p",
                 "--golden", golden_copy, "--out", os.path.join(tmp, "run")],
                capture_output=True, text=True, cwd=HARNESS_DIR,
            )
            self.assertEqual(proc.returncode, 3, proc.stdout + proc.stderr)
            self.assertIn("REJECTED", proc.stdout + proc.stderr)

    def test_missing_reference_bit_exits_two(self):
        proc = subprocess.run(
            [sys.executable, os.path.join(HARNESS_DIR, "diff.py"),
             "--reference-bit", "/nonexistent/ref.bit",
             "--out", tempfile.mkdtemp()],
            capture_output=True, text=True, cwd=HARNESS_DIR,
        )
        self.assertEqual(proc.returncode, 2)


if __name__ == "__main__":
    unittest.main()
