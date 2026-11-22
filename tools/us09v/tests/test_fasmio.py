"""Tests for fasmio: parsing, INIT/FF/group attributes, canonical output."""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from us09v.fasmio import (  # noqa: E402
    ff_attr,
    group_attr,
    init_value,
    parse_fasm,
    parse_pip,
    to_fasm,
)


class TestParse(unittest.TestCase):
    def test_plain_and_valued(self):
        tiles = parse_fasm([
            "SLICE_X0Y0.A6LUT.INIT[63:0] = 0x5555555555555555",
            "SLICE_X0Y0.FFMUXA1.D6",
            "SLICE_X0Y0.AFF.INIT = 1'b0",
            "INT_X0Y0.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1",
        ])
        self.assertEqual(tiles["SLICE_X0Y0"]["A6LUT.INIT[63:0]"], 0x5555555555555555)
        self.assertIsNone(tiles["SLICE_X0Y0"]["FFMUXA1.D6"])
        self.assertEqual(tiles["SLICE_X0Y0"]["AFF.INIT"], 0)
        self.assertIsNone(tiles["INT_X0Y0"]["PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1"])

    def test_value_spellings(self):
        tiles = parse_fasm([
            "SLICE_X0Y0.A6LUT.INIT[63:0] = 64'h5555555555555555",
            "SLICE_X0Y0.B6LUT.INIT[63:0] = 64'b1010",
            "SLICE_X0Y0.C6LUT.INIT[63:0] = 42",
        ])
        self.assertEqual(tiles["SLICE_X0Y0"]["A6LUT.INIT[63:0]"], 0x5555555555555555)
        self.assertEqual(tiles["SLICE_X0Y0"]["B6LUT.INIT[63:0]"], 0b1010)
        self.assertEqual(tiles["SLICE_X0Y0"]["C6LUT.INIT[63:0]"], 42)

    def test_comments_and_blank_lines(self):
        tiles = parse_fasm(["# comment", "", "SLICE_X0Y0.AFF.INIT.V1"])
        self.assertEqual(list(tiles), ["SLICE_X0Y0"])


class TestInitValue(unittest.TestCase):
    def test_grouped(self):
        self.assertEqual(
            init_value({"A6LUT.INIT[63:0]": 0x8888888888888888}, "A"),
            0x8888888888888888,
        )

    def test_per_bit(self):
        feats = {"ALUT.INIT[0]": None, "ALUT.INIT[3]": 1, "ALUT.INIT[5]": 0}
        self.assertEqual(init_value(feats, "A"), 0b1001)

    def test_missing(self):
        self.assertIsNone(init_value({}, "A"))


class TestFfAttr(unittest.TestCase):
    def test_v_spelling(self):
        # The V suffix IS the value: V0 is INIT 0, V1 is INIT 1.
        # (The segbits bit is NOT the value: V0 sets the bit,
        # V1 clears it, bit = NOT(INIT).)
        self.assertEqual(ff_attr({"AFF.INIT.V0": None}, "AFF", "INIT"), 0)
        self.assertEqual(ff_attr({"AFF.INIT.V1": None}, "AFF", "INIT"), 1)
        self.assertEqual(ff_attr({"AFF.SRVAL.V1": None}, "AFF", "SRVAL"), 1)
        self.assertEqual(ff_attr({"AFF.SRVAL.V0": None}, "AFF", "SRVAL"), 0)

    def test_valued_spelling(self):
        self.assertEqual(ff_attr({"AFF.INIT": 1}, "AFF", "INIT"), 1)
        self.assertEqual(ff_attr({"AFF.INIT": 0}, "AFF", "INIT"), 0)

    def test_missing(self):
        self.assertIsNone(ff_attr({}, "AFF", "INIT"))


class TestParsePip(unittest.TestCase):
    def test_plain(self):
        self.assertEqual(
            parse_pip("PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1"),
            ("BOUNCE_E_0_FT1", "INODE_E_1_FT1"),
        )

    def test_fwd_keeps_direction(self):
        # PIP.D.S.FWD means S drives D, same as plain PIP.D.S.
        self.assertEqual(
            parse_pip("PIP.BYPASS_E8.INT_NODE_IMUX_5_INT_OUT0.FWD"),
            ("BYPASS_E8", "INT_NODE_IMUX_5_INT_OUT0"),
        )

    def test_rev_swaps_direction(self):
        # PIP.D.S.REV means D drives S, i.e. plain PIP.S.D.
        self.assertEqual(
            parse_pip("PIP.BYPASS_E8.INT_NODE_IMUX_5_INT_OUT0.REV"),
            ("INT_NODE_IMUX_5_INT_OUT0", "BYPASS_E8"),
        )

    def test_non_pip(self):
        self.assertIsNone(parse_pip("AFF.INIT.V0"))
        self.assertIsNone(parse_pip("PIP.ONLYONE"))
        self.assertIsNone(parse_pip("PIP.A.B.OTHER"))
        self.assertIsNone(parse_pip("PIP.A.B.FWD.EXTRA"))


class TestGroupAttr(unittest.TestCase):
    def test_v_spelling(self):
        # V1 sets, V0 clears, per INTERFACES.md.
        self.assertEqual(group_attr({"ABCDFF.CEUSED.V1": None}, "ABCDFF", "CEUSED"), 1)
        self.assertEqual(group_attr({"ABCDFF.CEUSED.V0": None}, "ABCDFF", "CEUSED"), 0)
        self.assertIsNone(group_attr({}, "ABCDFF", "CEUSED"))


class TestToFasm(unittest.TestCase):
    def test_round_trip(self):
        tiles = parse_fasm([
            "SLICE_X0Y0.A6LUT.INIT[63:0] = 0x5555555555555555",
            "SLICE_X0Y0.FFMUXA1.D6",
        ])
        lines = to_fasm(tiles)
        self.assertEqual(lines, [
            "SLICE_X0Y0.A6LUT.INIT[63:0] = 0x5555555555555555",
            "SLICE_X0Y0.FFMUXA1.D6",
        ])


if __name__ == "__main__":
    unittest.main()
