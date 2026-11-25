#!/usr/bin/env python3
"""Tests for the Q6 probe scripts (checker and XDC generator).

Run from probe/q6-hpio/:

  python3 tests/test_q6_probe.py

Stdlib only. Fixtures mimic the inventory format
q6_hpio_inventory.tcl writes; the HPIO_RIGHT fixture uses the
xczu3eg tile names and columns as shape only, never as xcku3p data.

No em dashes anywhere.
"""

import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
PROBE_DIR = os.path.dirname(HERE)
sys.path.insert(0, PROBE_DIR)

import q6_check_step1
import q6_gen_io_xdc

HPIO_RIGHT_FIXTURE = """\
# Q6 step 1 HPIO inventory, part xcku3p-ffva676-1-e
PART xcku3p-ffva676-1-e
COUNT HPIO* 2
COUNT HDIO* 0
COUNT CMT* 0
TILE HPIO_RIGHT_X32Y0 HPIO_RIGHT 32 0 X1Y0 3
SITE HPIO_RIGHT_X32Y0 BIAS_X0Y0 BIAS NONE
SITE HPIO_RIGHT_X32Y0 IOB_X2Y0 HPIOB_M AC9
SITE HPIO_RIGHT_X32Y0 IOB_X2Y1 HPIOB_S AD9
TILE HPIO_RIGHT_X32Y30 HPIO_RIGHT 32 0 X1Y0 3
SITE HPIO_RIGHT_X32Y30 BIAS_X0Y30 BIAS NONE
SITE HPIO_RIGHT_X32Y30 IOB_X2Y26 HPIOB_M AC10
SITE HPIO_RIGHT_X32Y30 IOB_X2Y27 HPIOB_S AD10
COUNT_EXACT HPIO_RIGHT 2
COUNT_EXACT HPIO_L 0
COUNT_EXACT HDIO_TOP_RIGHT 0
COUNT_EXACT HDIO_BOT_RIGHT 0
COUNT_EXACT CMT_RIGHT 0
COUNT_EXACT CMT_L 0
"""

HPIO_L_FIXTURE = """\
PART xcku3p-ffva676-1-e
COUNT HPIO* 1
COUNT HDIO* 0
COUNT CMT* 0
TILE HPIO_L_X0Y0 HPIO_L 0 0 X0Y0 2
SITE HPIO_L_X0Y0 IOB_X0Y0 HPIOB_M AA1
SITE HPIO_L_X0Y0 IOB_X0Y1 HPIOB_S AB1
COUNT_EXACT HPIO_RIGHT 0
COUNT_EXACT HPIO_L 1
COUNT_EXACT HDIO_TOP_RIGHT 0
COUNT_EXACT HDIO_BOT_RIGHT 0
COUNT_EXACT CMT_RIGHT 0
COUNT_EXACT CMT_L 0
"""

SINGLE_TILE_FIXTURE = """\
PART xcku3p-ffva676-1-e
COUNT HPIO* 1
COUNT HDIO* 0
COUNT CMT* 0
TILE HPIO_RIGHT_X32Y0 HPIO_RIGHT 32 0 X1Y0 2
SITE HPIO_RIGHT_X32Y0 IOB_X2Y0 HPIOB_M AC9
SITE HPIO_RIGHT_X32Y0 IOB_X2Y1 HPIOB_S AD9
COUNT_EXACT HPIO_RIGHT 1
COUNT_EXACT HPIO_L 0
COUNT_EXACT HDIO_TOP_RIGHT 0
COUNT_EXACT HDIO_BOT_RIGHT 0
COUNT_EXACT CMT_RIGHT 0
COUNT_EXACT CMT_L 0
"""

MISSING_PIN_FIXTURE = """\
PART xcku3p-ffva676-1-e
COUNT HPIO* 1
COUNT HDIO* 0
COUNT CMT* 0
TILE HPIO_RIGHT_X32Y0 HPIO_RIGHT 32 0 X1Y0 2
SITE HPIO_RIGHT_X32Y0 IOB_X2Y0 HPIOB_M NONE
SITE HPIO_RIGHT_X32Y0 IOB_X2Y1 HPIOB_S AD9
COUNT_EXACT HPIO_RIGHT 1
COUNT_EXACT HPIO_L 0
COUNT_EXACT HDIO_TOP_RIGHT 0
COUNT_EXACT HDIO_BOT_RIGHT 0
COUNT_EXACT CMT_RIGHT 0
COUNT_EXACT CMT_L 0
"""


def write_fixture(text):
    fd, path = tempfile.mkstemp(suffix=".txt")
    with os.fdopen(fd, "w") as f:
        f.write(text)
    return path


class CheckStep1Tests(unittest.TestCase):
    def test_hpio_right_present_goes_to_step2(self):
        path = write_fixture(HPIO_RIGHT_FIXTURE)
        try:
            part, counts, tiles = q6_check_step1.parse_inventory(path)
            code, lines = q6_check_step1.verdict(part, counts, tiles)
            self.assertEqual(code, 0)
            self.assertIn("HPIO_RIGHT: 2", lines)
            self.assertTrue(any("HPIO_RIGHT EXISTS" in l for l in lines))
            self.assertTrue(any("HPIO_RIGHT_X32Y0" in l for l in lines))
            self.assertTrue(any("HPIO_RIGHT_X32Y30" in l for l in lines))
        finally:
            os.unlink(path)

    def test_hpio_l_only_stops_and_escalates(self):
        path = write_fixture(HPIO_L_FIXTURE)
        try:
            part, counts, tiles = q6_check_step1.parse_inventory(path)
            code, lines = q6_check_step1.verdict(part, counts, tiles)
            self.assertEqual(code, 1)
            self.assertTrue(any("STOP" in l for l in lines))
            self.assertTrue(any("escalate" in l for l in lines))
            self.assertFalse(any("run step 2" in l.lower() and "do not" not in l.lower()
                                 for l in lines if "VERDICT" in l))
        finally:
            os.unlink(path)

    def test_missing_part_line_fails_loud(self):
        path = write_fixture("COUNT_EXACT HPIO_RIGHT 0\n")
        try:
            with self.assertRaises(ValueError):
                q6_check_step1.parse_inventory(path)
            self.assertEqual(q6_check_step1.main(["x", path]), 1)
        finally:
            os.unlink(path)

    def test_checker_script_exit_zero_on_hpio_right(self):
        path = write_fixture(HPIO_RIGHT_FIXTURE)
        try:
            proc = subprocess.run(
                [sys.executable, os.path.join(PROBE_DIR, "q6_check_step1.py"), path],
                capture_output=True, text=True)
            self.assertEqual(proc.returncode, 0)
            self.assertIn("HPIO_RIGHT EXISTS", proc.stdout)
        finally:
            os.unlink(path)


class GenIoXdcTests(unittest.TestCase):
    def test_two_tiles_four_ports(self):
        path = write_fixture(HPIO_RIGHT_FIXTURE)
        try:
            with tempfile.TemporaryDirectory() as tmp:
                part, chosen, ports = q6_gen_io_xdc.generate(path)
                self.assertEqual(part, "xcku3p-ffva676-1-e")
                self.assertEqual([t["name"] for t in chosen],
                                 ["HPIO_RIGHT_X32Y0", "HPIO_RIGHT_X32Y30"])
                self.assertEqual(len(ports), 4)
                self.assertEqual(ports[0], ("HPIO_RIGHT_X32Y0", "IOB_X2Y0", "AC9"))
                self.assertEqual(ports[1], ("HPIO_RIGHT_X32Y0", "IOB_X2Y1", "AD9"))
                names = q6_gen_io_xdc.write_files(
                    ports,
                    xdc_path=os.path.join(tmp, "q6-io-probe.xdc"),
                    verilog_path=os.path.join(tmp, "q6-io-probe.v"))
                self.assertEqual(names, ["in0", "out0", "in1", "out1"])
                with open(os.path.join(tmp, "q6-io-probe.xdc")) as f:
                    xdc = f.read()
                self.assertIn("set_property PACKAGE_PIN AC9 [get_ports {in0}]", xdc)
                self.assertIn("set_property PACKAGE_PIN AD10 [get_ports {out1}]", xdc)
                self.assertIn("set_property IOSTANDARD LVCMOS12 [get_ports {in0}]", xdc)
                with open(os.path.join(tmp, "q6-io-probe.v")) as f:
                    verilog = f.read()
                self.assertIn("module q6_io_probe(", verilog)
                self.assertIn("IBUF in_buf0 (.I(in0), .O(w0));", verilog)
                self.assertIn("OBUF out_buf1 (.O(out1), .I(w1));", verilog)
        finally:
            os.unlink(path)

    def test_single_tile_probes_one_tile_only(self):
        path = write_fixture(SINGLE_TILE_FIXTURE)
        try:
            part, chosen, ports = q6_gen_io_xdc.generate(path)
            self.assertEqual(len(chosen), 1)
            self.assertEqual(len(ports), 2)
        finally:
            os.unlink(path)

    def test_missing_package_pin_fails_loud(self):
        path = write_fixture(MISSING_PIN_FIXTURE)
        try:
            with self.assertRaises(ValueError) as ctx:
                q6_gen_io_xdc.generate(path)
            self.assertIn("no package pin", str(ctx.exception))
        finally:
            os.unlink(path)

    def test_no_row0_hpio_fails_loud(self):
        path = write_fixture(HPIO_L_FIXTURE.replace("X0Y0", "X0Y1"))
        try:
            with self.assertRaises(ValueError):
                q6_gen_io_xdc.generate(path)
        finally:
            os.unlink(path)


if __name__ == "__main__":
    unittest.main()
