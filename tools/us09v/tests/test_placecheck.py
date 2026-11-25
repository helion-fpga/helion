"""Tests for placecheck: fixtures pass; the negative cases (RAM
LUT on SLICEL, carry-chain gap, control-set violation, capacity
and overlap via the placement-list path) are caught."""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from tests import fixture_ports, load_db, load_fabric, load_fixture  # noqa: E402
from us09v.fasmio import parse_fasm_file  # noqa: E402
from us09v.fabric import Fabric  # noqa: E402
from us09v.placecheck import check_place_fasm, check_place_list  # noqa: E402


class PlaceBase(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.db = load_db()
        cls.fabric = load_fabric(cls.db)

    def check_fixture(self, name):
        fasm_path, _rtl, _pm = load_fixture(name)
        port_ins, _po, _pin = fixture_ports(name)
        tiles = parse_fasm_file(fasm_path)
        return check_place_fasm(self.db, tiles, self.fabric, port_ins=port_ins)


class TestFixturesPlaceOk(PlaceBase):
    def test_and2(self):
        rep = self.check_fixture("and2")
        self.assertEqual(rep.violations, [], rep.violations)
        self.assertEqual(rep.slices, 1)

    def test_blinky(self):
        rep = self.check_fixture("blinky")
        self.assertEqual(rep.violations, [], rep.violations)

    def test_counter4(self):
        rep = self.check_fixture("counter4")
        self.assertEqual(rep.violations, [], rep.violations)

    def test_carry8_inc(self):
        rep = self.check_fixture("carry8-inc")
        self.assertEqual(rep.violations, [], rep.violations)
        self.assertEqual(rep.carry_chains, 1)


class TestNegative(PlaceBase):
    def test_ram_lut_on_slicel(self):
        fasm_path, _rtl, _pm = load_fixture("and2")
        tiles = parse_fasm_file(fasm_path)
        tiles["SLICE_X10Y10"]["ALUT.MODE.RAM"] = None
        rep = check_place_fasm(self.db, tiles, self.fabric)
        self.assertTrue(any("requires SLICEM" in v for v in rep.violations))

    def test_carry_chain_gap(self):
        # Two CARRY8 slices with a gap: Y10 and Y12 carry, Y11 does
        # not. COUT/CIN is a direct tileconn stitch, not a PIP, so no
        # route can express this: Y12's CIN node already contains
        # Y11's COUT wire. The checker must flag it because Y11 has
        # no CARRY8 in use (its COUT is X), even though Y11 is the
        # slice directly below Y12.
        fasm_path, _rtl, _pm = load_fixture("carry8-inc")
        tiles = parse_fasm_file(fasm_path)
        upper = "SLICE_X10Y12"
        tiles[upper] = {
            "A6LUT.INIT[63:0]": 0xAAAAAAAAAAAAAAAA,
            "CARRY8.CARRY_TYPE.SINGLE_CY8": None,
            "CARRY8.DI0.AX": None,
            "FFMUXA1.XORIN": None,
            "AFF.INIT.V1": None,
            "AFF.SRVAL.V1": None,
            "ABCDFF.CEUSED.V0": None,
            "ABCDFF.SRUSED.V0": None,
            "ABCDFF.CLKINV.V0": None,
            "ABCDFF.SYNC.ASYNC": None,
            "ABCDFF.MODE.FF": None,
        }
        port_ins, _po, _pin = fixture_ports("carry8-inc")
        # The site pin CIN reaches the CIN wire only through the CLE
        # CIN PIP (CIN wire -> CIN_PIN wire); without it the pin is
        # undriven and the gap is not what is being tested. A real
        # decoded chain FASM carries this PIP (it is a segbits
        # feature), so include it on the upper slice's CLE tile.
        tiles["CLEL_R_X6Y12"] = {
            "PIP.CLE_CLE_L_SITE_0_CIN_PIN.CLE_CLE_L_SITE_0_CIN": None,
        }
        rep = check_place_fasm(self.db, tiles, self.fabric, port_ins=port_ins)
        self.assertTrue(
            any("chain gap" in v for v in rep.violations),
            f"violations were {rep.violations}",
        )

    def test_control_set_undriven_clock(self):
        # Blinky FASM with the clock port removed: the ABCD half has
        # FFs but CLK1 is undriven.
        fasm_path, _rtl, _pm = load_fixture("blinky")
        tiles = parse_fasm_file(fasm_path)
        rep = check_place_fasm(self.db, tiles, self.fabric)
        self.assertTrue(any("CLK1 is undriven" in v for v in rep.violations))

    def test_place_list_overlap(self):
        rep = check_place_list([
            ("u1", "SLICE_X0Y0", "ALUT"),
            ("u2", "SLICE_X0Y0", "ALUT"),
        ])
        self.assertTrue(any("used by both u1 and u2" in v for v in rep.violations))

    def test_place_list_capacity(self):
        rep = check_place_list(
            [(f"u{i}", "SLICE_X0Y0", f"{c}LUT") for i, c in enumerate("ABCDEFGH")]
            + [("u9", "SLICE_X0Y0", "ALUT")]
        )
        # 9 LUT placements on one slice: 8 distinct BELs plus a
        # duplicate ALUT, so both the overlap and the capacity rules
        # fire; at least the capacity rule must fire.
        self.assertTrue(any("9 LUTs exceeds max 8" in v for v in rep.violations))

    def test_place_list_carry_capacity(self):
        rep = check_place_list([
            ("u1", "SLICE_X0Y0", "CARRY8"),
            ("u2", "SLICE_X0Y0", "CARRY8"),
        ])
        self.assertTrue(any("2 CARRY8 exceeds max 1" in v for v in rep.violations))


if __name__ == "__main__":
    unittest.main()
