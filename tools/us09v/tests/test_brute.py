"""Tests for brute: exact enumeration counts on small grids."""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from us09v.brute import count_legal_placements, enumerate_placements, slice_grid  # noqa: E402
from us09v.placecheck import check_place_list  # noqa: E402


class TestEnumeration(unittest.TestCase):
    def test_one_lut_one_slice(self):
        # 1 LUT on 1 slice: 8 letters.
        self.assertEqual(
            count_legal_placements([("c0", "LUT")], slice_grid(1, 1)), 8
        )

    def test_one_lut_two_slices(self):
        # 1 LUT on 2 slices: 2 sites x 8 letters.
        self.assertEqual(
            count_legal_placements([("c0", "LUT")], slice_grid(2, 1)), 16
        )

    def test_two_luts_one_slice(self):
        # 2 distinct LUTs on 1 slice: 8 x 7 ordered assignments.
        self.assertEqual(
            count_legal_placements(
                [("c0", "LUT"), ("c1", "LUT")], slice_grid(1, 1)
            ),
            56,
        )

    def test_one_ff_one_slice(self):
        # 1 FF on 1 slice: 16 FF BELs (8 letters x Q/Q2).
        self.assertEqual(
            count_legal_placements([("c0", "FF")], slice_grid(1, 1)), 16
        )

    def test_one_carry8_one_slice(self):
        self.assertEqual(
            count_legal_placements([("c0", "CARRY8")], slice_grid(1, 1)), 1
        )

    def test_two_carry8_two_slices(self):
        # 2 CARRY8 cells on 2 slices: one per slice, ordered.
        self.assertEqual(
            count_legal_placements(
                [("c0", "CARRY8"), ("c1", "CARRY8")], slice_grid(2, 1)
            ),
            2,
        )

    def test_enumerated_placements_are_legal(self):
        legal, _pruned = enumerate_placements(
            [("c0", "LUT"), ("c1", "FF"), ("c2", "CARRY8")],
            slice_grid(2, 1),
        )
        self.assertGreater(len(legal), 0)
        for placement in legal:
            self.assertTrue(check_place_list(placement).ok, placement)

    def test_full_slice(self):
        # 8 LUTs on 1 slice: 8! orderings.
        self.assertEqual(
            count_legal_placements(
                [(f"c{i}", "LUT") for i in range(8)], slice_grid(1, 1)
            ),
            40320,
        )


if __name__ == "__main__":
    unittest.main()
