"""Tests for db: CC0 verification and site index resolution."""
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from tests import load_db  # noqa: E402
from us09v.db import verify_cc0  # noqa: E402


class TestVerifyCc0(unittest.TestCase):
    def test_real_clone(self):
        db = load_db()
        self.assertTrue(db.clone.endswith("prjuray-db-ref"))
        with open(os.path.join(db.clone, "COPYING")) as f:
            self.assertIn("CC0 1.0 Universal", f.read())

    def test_non_cc0_refused(self):
        with tempfile.TemporaryDirectory() as d:
            zynqusp = os.path.join(d, "zynqusp")
            os.makedirs(zynqusp)
            with open(os.path.join(d, "COPYING"), "w") as f:
                f.write("All rights reserved.\n")
            with self.assertRaises(ValueError):
                verify_cc0(zynqusp)

    def test_missing_copying_refused(self):
        with tempfile.TemporaryDirectory() as d:
            zynqusp = os.path.join(d, "zynqusp")
            os.makedirs(zynqusp)
            with self.assertRaises(FileNotFoundError):
                verify_cc0(zynqusp)


class TestSiteIndex(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.db = load_db()

    def test_slice(self):
        tile, idx = self.db.site_index("SLICE_X10Y10")
        self.assertEqual(tile, "CLEL_R_X6Y10")
        self.assertEqual(idx, 0)

    def test_iob(self):
        tile, idx = self.db.site_index("IOB_X2Y155")
        self.assertEqual(tile, "HPIO_RIGHT_X32Y150")
        ttype = self.db.tilegrid[tile]["type"]
        self.assertEqual(
            self.db.tile_type(ttype)["sites"][idx]["type"], "HPIOB_SNGL"
        )

    def test_iob_hdio_first(self):
        # IOB_X0Y0 is the first HDIOB_M site (X0Y0) on
        # HDIO_BOT_RIGHT_X9Y0: tile_type index 4, not the tilegrid
        # dict position 19. The old dict-position code returned 19,
        # which indexes a different site type's entry in the
        # tile_type list and misresolves the site's pins.
        tile, idx = self.db.site_index("IOB_X0Y0")
        self.assertEqual(tile, "HDIO_BOT_RIGHT_X9Y0")
        self.assertEqual(idx, 4)
        ttype = self.db.tilegrid[tile]["type"]
        self.assertEqual(
            self.db.tile_type(ttype)["sites"][idx]["type"], "HDIOB_M"
        )

    def test_iob_hpio_non_last(self):
        # IOB_X2Y130 is a non-last HPIO site (HPIOB_M, IOBPAIR_26):
        # the old dict-position code returned 26, the tile_type
        # index is 4.
        tile, idx = self.db.site_index("IOB_X2Y130")
        self.assertEqual(tile, "HPIO_RIGHT_X32Y150")
        self.assertEqual(idx, 4)
        ttype = self.db.tilegrid[tile]["type"]
        self.assertEqual(
            self.db.tile_type(ttype)["sites"][idx]["type"], "HPIOB_M"
        )

    def test_site_to_tile(self):
        self.assertEqual(
            self.db.site_to_tile["SLICE_X10Y10"], ("CLEL_R_X6Y10", "SLICEL")
        )


if __name__ == "__main__":
    unittest.main()
