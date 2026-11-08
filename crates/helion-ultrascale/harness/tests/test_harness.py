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
from fasm import FasmError, canonical_fasm, classify, parse_fasm, parse_init_value
from gen_golden import DESIGNS, GOLDEN_DIR, build_fasm, lut_init_64
from reference import MissingReference, Tier1Reference, int_col_offset_from_clb
from roundtrip import (
    canonical_roundtrip_fasm,
    roundtrip_feature_keys,
    scope_fasm_to_feature_keys,
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

    def test_unknown_pip_missing(self):
        with self.assertRaises(MissingReference):
            self.ref.resolve_int_pip("xcku3p", 0, 0, "PIP.NOPE.NOPE")

    def test_dsp_uram_have_no_encoding(self):
        feats = parse_fasm(
            "DSP_X0Y0.DSP48E2.AREG = 1\n"
            "URAM_X0Y0.URAM288.INIT = 0\n"
        )
        for feat in feats:
            _, _, _, miss = resolve_feature(self.ref, "xcku3p", feat)
            self.assertIsNotNone(miss, feat)
            self.assertIn("absent", miss)


class TestDiffEngine(unittest.TestCase):
    def setUp(self):
        self.ref = Tier1Reference()

    def test_compute_diff_counts_both_directions(self):
        ref_bits = {(0x50B, 15): 1, (0x50A, 15): 0, (0x50C, 2): 1}
        emitted = {(0x50B, 15): 0, (0x50A, 15): 0, (0x50C, 2): 1, (0x999, 1): 1}
        # One flipped required bit + one extra set bit = 2.
        self.assertEqual(compute_diff(ref_bits, emitted), 2)
        self.assertEqual(compute_diff(ref_bits, dict(ref_bits)), 0)

    def test_blocked_without_emitter_never_zero(self):
        with tempfile.TemporaryDirectory() as tmp:
            recs = diff_design(
                self.ref, None, "blinky", "xcku3p",
                os.path.join(GOLDEN_DIR, "blinky.xcku3p.fasm"),
                20261107, "hash", "tier1", tmp,
            )
            self.assertEqual(len(recs), 10)
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
            self.assertEqual(len(recs), 10)
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
    def test_full_run_exits_zero_with_40_blocked_records(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = os.path.join(tmp, "run")
            proc = subprocess.run(
                [sys.executable, os.path.join(HARNESS_DIR, "diff.py"),
                 "--out", out],
                capture_output=True, text=True, cwd=HARNESS_DIR,
            )
            self.assertEqual(proc.returncode, 0, proc.stderr)
            recs = [json.loads(l) for l in open(os.path.join(out, "diff.jsonl"))]
            self.assertEqual(len(recs), 40)
            self.assertTrue(all(r["status"] == "blocked" for r in recs))
            self.assertTrue(all(r["diff_bits"] is None for r in recs))
            manifest = json.load(open(os.path.join(out, "manifest.json")))
            self.assertEqual(manifest["tier"], "tier1")
            self.assertEqual(len(manifest["combos"]), 4)

    def test_garbage_parse_emitter_full_run_writes_all_records(self):
        # Critic session 5, FLAG 2, end to end: a bad-parse emitter
        # over all 4 combos must exit 0 with all 40 records and a
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
            self.assertEqual(len(recs), 40)
            framing = [r for r in recs if r["block"] == "FRAMING"]
            self.assertEqual(len(framing), 4)
            for r in framing:
                self.assertEqual(r["status"], "red")
                self.assertEqual(r["diff_bits"], 1)
                self.assertIn("not valid FASM", r["detail"])
            with open(os.path.join(out, "manifest.json")) as f:
                manifest = json.load(f)
            self.assertEqual(len(manifest["combos"]), 4)

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
