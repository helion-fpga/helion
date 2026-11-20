# Tests for the single-feature sweep (sweep.py) and the CLB DB
# resolver added to reference.py.
#
# The sweep enumerates millions of features; these tests cover the
# mechanics on small scopes (one slice, one tile, a handful of
# features) plus the packing/diffing invariants the whole sweep
# rests on. Run with: python3 -m unittest discover -s tests
#
# No em dashes anywhere.

import os
import sys
import unittest

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from reference import MissingReference, Tier1Reference
import sweep
from sweep import (
    FF_CONTROL_FEATURES, Geometry, Item, bram_code_value,
    bram_pos_to_bit, code_of_index, diff_item_int, emitted_int_keys,
    enum_carry8, enum_int, enum_io, lut_code_value, lut_pos_to_bit,
    pack_mode_a, pack_mode_b, ref_int_keys, verify_bram_disjoint,
    verify_lut_disjoint,
)


class TestCodes(unittest.TestCase):
    def test_lut_codes_distinguish_all_bits(self):
        # Every pair of bit indices differs in at least one code bit.
        codes = [code_of_index(i, 6) for i in range(64)]
        self.assertEqual(len(set(codes)), 64)
        for i in range(64):
            v = lut_code_value(0)
            self.assertEqual((v >> i) & 1, (i >> 0) & 1)
        for k in range(6):
            v = lut_code_value(k)
            for i in range(64):
                self.assertEqual((v >> i) & 1, (i >> k) & 1)

    def test_bram_codes_distinguish_all_bits(self):
        codes = [code_of_index(i, 14) for i in range(16384)]
        self.assertEqual(len(set(codes)), 16384)
        codes_p = [code_of_index(i, 11) for i in range(2048)]
        self.assertEqual(len(set(codes_p)), 2048)
        for k in (0, 7, 13):
            v = bram_code_value(k, 16384)
            for i in (0, 1, 100, 8192, 16383):
                self.assertEqual((v >> i) & 1, (i >> k) & 1)


class TestGeometry(unittest.TestCase):
    def setUp(self):
        self.ref = Tier1Reference()
        self.geom = Geometry(self.ref, "xcku3p")

    def test_slice_counts(self):
        self.assertEqual(len(self.geom.rows), 4)
        self.assertEqual(self.geom.n_clb_cols, 113)
        self.assertEqual(len(self.geom.slices), 113 * 240)
        self.assertEqual(len(self.geom.brams), 10 * 96)
        self.assertEqual(len(self.geom.int_tiles), 113 * 240)

    def test_tile_types(self):
        self.assertEqual(self.geom.tile_type_of[0], "CLEL_R")
        self.assertEqual(self.geom.tile_type_of[1], "CLEM")
        self.assertEqual(len(self.geom.clem_slices), (46 + 6) * 240)


class TestDisjointness(unittest.TestCase):
    def setUp(self):
        self.ref = Tier1Reference()
        self.geom = Geometry(self.ref, "xcku3p")

    def test_lut_disjoint(self):
        self.assertTrue(verify_lut_disjoint(self.ref, "xcku3p", self.geom))

    def test_bram_disjoint(self):
        self.assertTrue(verify_bram_disjoint(self.ref, "xcku3p", self.geom))

    def test_lut_pos_to_bit_matches_resolve(self):
        # The inversion table agrees with resolve_lut on every bit.
        for (x, y, bel) in ((0, 0, "A6LUT"), (1, 59, "H6LUT"), (0, 239, "D6LUT")):
            tile_type = self.geom.tile_type_of[x]
            y_ofst = y % 60
            table = lut_pos_to_bit(self.ref, "xcku3p", tile_type, y_ofst, bel)
            bits = self.ref.resolve_lut("xcku3p", x, y, bel, 0xFFFFFFFFFFFFFFFF)
            self.assertEqual(len(bits), 64)
            # Each table entry's position must be in the resolved set
            # with value 1, and the resolved set must equal the table.
            resolved_pos = {(f & 0xFF, o) for (f, o) in bits}
            self.assertEqual(resolved_pos, set(table))
            self.assertEqual(len(table), 64)

    def test_bram_pos_to_bit_matches_resolve(self):
        for parity in (False, True):
            nbits = 2048 if parity else 16384
            table = bram_pos_to_bit(self.ref, "xcku3p", 0, parity)
            self.assertEqual(len(table), nbits)
            bits = [1] * nbits
            if parity:
                resolved = self.ref.resolve_bram_parity("xcku3p", 0, 0, bits)
            else:
                resolved = self.ref.resolve_bram("xcku3p", 0, 0, 18, bits)
            self.assertEqual(len(resolved), nbits)
            resolved_pos = {(f & 0xFF, o) for (f, o) in resolved}
            self.assertEqual(resolved_pos, set(table))


class TestClbDbResolver(unittest.TestCase):
    def setUp(self):
        self.ref = Tier1Reference()

    def test_ff_control_exact_bits(self):
        # ABCDFF.CEUSED.V1 sets 14_07 on a CLEL_R slice (DB line).
        bits = self.ref.resolve_clb_db_feature(
            "xcku3p", 0, 0, "ABCDFF.CEUSED.V1")
        self.assertEqual(len(bits), 1)
        (far, off), val = next(iter(bits.items()))
        self.assertEqual((far & 0xFF, off, val), (14, 7, 1))
        # V0 is the exact complement at the same position.
        bits0 = self.ref.resolve_clb_db_feature(
            "xcku3p", 0, 0, "ABCDFF.CEUSED.V0")
        self.assertEqual(bits0, {(far, off): 0})

    def test_ff_control_unknown_fails_loud(self):
        with self.assertRaises(MissingReference):
            self.ref.resolve_clb_db_feature("xcku3p", 0, 0, "ABCDFF.NOSUCH.V0")

    def test_ff_control_count(self):
        self.assertEqual(len(FF_CONTROL_FEATURES), 32)
        db = self.ref.segbits("clel_r")
        for feat in FF_CONTROL_FEATURES:
            self.assertIn("CLEL_R." + feat, db)


class TestPacker(unittest.TestCase):
    def _item(self, key, group, ref_bits):
        return Item("TEST", key, group, "TILE." + key, ref_bits,
                    {"kind": "test"})

    def test_disjoint_items_share_emit(self):
        a = self._item("a", (0, 0, 5), {(0x500, 10): 1, (0x500, 11): 0})
        b = self._item("b", (0, 0, 5), {(0x501, 10): 1})
        emits = pack_mode_a("TEST", [a, b], 100)
        self.assertEqual(len(emits), 1)
        self.assertEqual(len(emits[0].items), 2)

    def test_value_conflict_splits_emit(self):
        a = self._item("a", (0, 0, 5), {(0x500, 10): 1})
        b = self._item("b", (0, 0, 5), {(0x500, 10): 0})
        emits = pack_mode_a("TEST", [a, b], 100)
        self.assertEqual(len(emits), 2)

    def test_same_value_sharing_allowed(self):
        a = self._item("a", (0, 0, 5), {(0x500, 10): 1})
        b = self._item("b", (0, 0, 5), {(0x500, 10): 1})
        emits = pack_mode_a("TEST", [a, b], 100)
        self.assertEqual(len(emits), 1)

    def test_other_groups_do_not_conflict(self):
        a = self._item("a", (0, 0, 5), {(0x500, 10): 1})
        b = self._item("b", (0, 0, 6), {(0x500, 10): 0})
        emits = pack_mode_a("TEST", [a, b], 100)
        self.assertEqual(len(emits), 1)

    def test_line_cap_respected(self):
        items = [self._item("i%d" % i, (0, 0, 5 + i), {(0x500 + i, 0): 1})
                 for i in range(5)]
        emits = pack_mode_a("TEST", items, 2)
        self.assertEqual(len(emits), 3)
        self.assertTrue(all(e.lines <= 2 for e in emits))

    def test_mode_b_chunks_in_order(self):
        items = [self._item("i%d" % i, (0, 0, 5), None) for i in range(5)]
        emits = pack_mode_b("TEST", items, 2)
        self.assertEqual(len(emits), 3)
        self.assertEqual([i.key for e in emits for i in e.items],
                         ["i%d" % i for i in range(5)])


class TestDiffer(unittest.TestCase):
    def test_diff_item_int_counts_mismatch(self):
        ref_int = {sweep.key64(0x500, 10): 1, sweep.key64(0x500, 11): 0,
                   sweep.key64(0x501, 3): 1}
        emitted = np.array(
            [sweep.key64(0x500, 10), sweep.key64(0x501, 3)], dtype="uint64")
        diff, mismatches = diff_item_int(ref_int, emitted)
        self.assertEqual(diff, 0)
        emitted2 = np.array(
            [sweep.key64(0x500, 11)], dtype="uint64")
        diff2, mismatches2 = diff_item_int(ref_int, emitted2)
        self.assertEqual(diff2, 3)
        self.assertEqual(len(mismatches2), 3)

    def test_io_relativization(self):
        # Emitted absolute FAR 0x00000002 offset 67 relativizes to
        # frame 2 offset 67 under the provisional HPIO base 0x000.
        keys = [sweep.key64(0x2, 67)]
        rel = emitted_int_keys((0, 0, 0), keys, True)
        self.assertEqual(int(rel[0]), sweep.key64(2, 67))
        ref_bits = {("HPIO_RIGHT_X0Y0", 2, 67): 1}
        item = Item("IO_CONFIG", "k", (0, 0, 0), "f", ref_bits, {})
        int_keys = ref_int_keys(item, ref_bits)
        self.assertEqual(int_keys, {sweep.key64(2, 67): 1})
        diff, _ = diff_item_int(int_keys, rel)
        self.assertEqual(diff, 0)


class TestEnumerationScopes(unittest.TestCase):
    def setUp(self):
        self.ref = Tier1Reference()
        self.geom = Geometry(self.ref, "xcku3p")

    def test_int_feature_count(self):
        db = self.ref.segbits("int")
        self.assertEqual(len(db), 3778)

    def test_carry8_features(self):
        items = enum_carry8(self.ref, "xcku3p", self.geom)
        self.assertEqual(len(items), len(self.geom.slices) * 24)

    def test_io_features(self):
        items = enum_io(self.ref, "xcku3p", self.geom)
        self.assertEqual(len(items), 1560)
        self.assertTrue(all(i.group == (0, 0, 0) for i in items))

    def test_lutram_feature_count_per_slice(self):
        feats = sweep.lutram_features_for_slice(self.ref, "xcku3p", 1, 0)
        self.assertEqual(len(feats), 52)

    def test_int_eight_tiles_cover_both_sides(self):
        items = enum_int(self.ref, "xcku3p", self.geom)
        # The 8 full-sweep tiles are (x in (0, 1), y a row start).
        full = [i for i in items if i.params.get("x") in (0, 1)
                and i.params.get("y", 0) % 60 == 0]
        self.assertEqual(len(full), 8 * 3778)
        # Plus one probe PIP on every other INT tile.
        self.assertEqual(len(items), 8 * 3778 + (len(self.geom.int_tiles) - 8))


if __name__ == "__main__":
    unittest.main()
