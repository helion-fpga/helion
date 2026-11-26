#!/usr/bin/env python3
"""Tests for tools/vivado_reports/parse.py.

Fixtures are small Vivado 2017.3 text reports written in the exact
2017.3 ASCII table format: plus/minus borders, pipe columns, setext
section headings.

    python3 -m unittest test_parse -v
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from parse import parse_all, parse_report  # noqa: E402

TIMING_FULL = """Design Timing Summary
---------------------

    WNS(ns)      TNS(ns)  TNS Failing Endpoints  TNS Total Endpoints      WHS(ns)      THS(ns)  THS Failing Endpoints  THS Total Endpoints     WPWS(ns)     TPWS(ns)  TPWS Failing Endpoints  TPWS Total Endpoints
    -------      -------  ---------------------  -------------------      -------      -------  ---------------------  -------------------     --------     --------  ----------------------  --------------------
     -0.432       -2.156                     12                  843        0.018        0.000                      0                  843      0.250        0.000                       0                    0

All user specified timing constraints are met.

Clock Summary
-------------

    Clock     Waveform(ns)         Period(ns)      Frequency(MHz)
    -----     ------------         ----------      --------------
    clk          {0.000 5.000}         10.000             100.000
    clk_fast     {0.000 2.500}          5.000             200.000
"""

TIMING_NO_CLOCKS = """Design Timing Summary
---------------------

    WNS(ns)      TNS(ns)  TNS Failing Endpoints  TNS Total Endpoints      WHS(ns)      THS(ns)  THS Failing Endpoints  THS Total Endpoints     WPWS(ns)     TPWS(ns)  TPWS Failing Endpoints  TPWS Total Endpoints
    -------      -------  ---------------------  -------------------      -------      -------  ---------------------  -------------------     --------     --------  ----------------------  --------------------
      0.123        0.000                      0                  843        0.018        0.000                      0                  843      0.250        0.000                       0                    0
"""

UTIL_FULL = """1. Slice Logic
-------------

+----------------------------+------+-------+-----------+-------+
|         Site Type          | Used | Fixed | Available | Util% |
+----------------------------+------+-------+-----------+-------+
| Slice LUTs                 | 1234 |     0 |     53200 |  2.32 |
|   LUT as Logic             | 1200 |     0 |     53200 |  2.26 |
|   LUT as Memory            |   34 |     0 |     17400 |  0.20 |
|     LUT as Distributed RAM |    6 |     0 |           |       |
|     LUT as Shift Register  |    4 |     0 |           |       |
| Slice Registers            |  200 |     0 |    106400 |  0.19 |
|   Register as Flip Flop    |  200 |     0 |    106400 |  0.19 |
|   Register as Latch        |    0 |     0 |    106400 |  0.00 |
| F7 Muxes                   |    5 |     0 |     26600 |  0.02 |
| F8 Muxes                   |    1 |     0 |     13300 |  0.01 |
+----------------------------+------+-------+-----------+-------+

3. Memory
---------

+-----------------+------+-------+-----------+-------+
|    Site Type    | Used | Fixed | Available | Util% |
+-----------------+------+-------+-----------+-------+
| Block RAM Tile  |    2 |     0 |       140 |  1.43 |
|   RAMB36/FIFO*  |    1 |     0 |       140 |  0.71 |
|   RAMB18        |    2 |     0 |       280 |  0.71 |
+-----------------+------+-------+-----------+-------+

4. DSP
------

+-----------+------+-------+-----------+-------+
| Site Type | Used | Fixed | Available | Util% |
+-----------+------+-------+-----------+-------+
| DSPs      |    3 |     0 |       220 |  1.36 |
+-----------+------+-------+-----------+-------+

5. IO and GT Specific
---------------------

+-----------------+------+-------+-----------+-------+
|    Site Type    | Used | Fixed | Available | Util% |
+-----------------+------+-------+-----------+-------+
| Bonded IOB      |   12 |     0 |       200 |  6.00 |
+-----------------+------+-------+-----------+-------+

6. Clocking
-----------

+----------------+------+-------+-----------+-------+
|   Site Type    | Used | Fixed | Available | Util% |
+----------------+------+-------+-----------+-------+
| BUFGCTRL       |    1 |     0 |        32 |  3.13 |
| BUFG           |    0 |     0 |        32 |  0.00 |
| MMCME2_ADV     |    1 |     0 |         6 | 16.67 |
| PLLE2_ADV      |    0 |     0 |         6 |  0.00 |
+----------------+------+-------+-----------+-------+
"""

UTIL_SLICE_ONLY = """1. Slice Logic
-------------

+----------------------------+------+-------+-----------+-------+
|         Site Type          | Used | Fixed | Available | Util% |
+----------------------------+------+-------+-----------+-------+
| Slice LUTs                 |  100 |     0 |     53200 |  0.19 |
|   LUT as Logic             |   90 |     0 |     53200 |  0.17 |
|   LUT as Memory            |   10 |     0 |     17400 |  0.06 |
| Slice Registers            |  200 |     0 |    106400 |  0.19 |
| F7 Muxes                   |    5 |     0 |     26600 |  0.02 |
| F8 Muxes                   |    1 |     0 |     13300 |  0.01 |
+----------------------------+------+-------+-----------+-------+
"""

POWER_FULL = """Power analysis from Implemented netlist. Activity derived from constraints files, simulation files or vectorless analysis.

Total On-Chip Power (W):   2.456
Design Power Budget (W):   Unspecified
Power Budget Margin (W):   NA
Dynamic (W):               1.900
Device Static (W):         0.556
Effective TJA (C/W):       4.2
Max Ambient (C):           85.0
Junction Temperature (C):  35.3
Confidence Level:          Medium

On-Chip Power
-------------

+----------------+-----------+-------+
|                | Power (W) | %     |
+----------------+-----------+-------+
| Clocks         |     0.123 |  5.0  |
| Signals        |     0.234 |  9.5  |
| Logic          |     0.456 | 18.6  |
| BRAM           |     0.078 |  3.2  |
| DSP            |     0.012 |  0.5  |
| IO             |     0.345 | 14.0  |
+----------------+-----------+-------+
"""

POWER_NO_BREAKDOWN = """Total On-Chip Power (W):   2.456
Dynamic (W):               1.900
Device Static (W):         0.556
Junction Temperature (C):  35.3
Confidence Level:          Low
"""

CLOCK_UTIL_FULL = """Clock Utilization
-----------------

+----------------+---------------+------+----------+------+-----+
| Clock Region   | Global Clocks | BUFG | BUFGCTRL | MMCM | PLL |
+----------------+---------------+------+----------+------+-----+
| X0Y0           |             2 |    1 |        1 |    0 |   0 |
| X1Y0           |             1 |    1 |        0 |    1 |   0 |
+----------------+---------------+------+----------+------+-----+
"""

CONTROL_SETS_FULL = """Control Sets Report
-------------------

Total control sets: 12
Unique control sets: 8
"""

CONTROL_SETS_TOTAL_ONLY = """Control Sets Report
-------------------

Total control sets: 3
"""

DRC_FULL = """DRC Report
----------

+----------+----------+-------+
| Rule     | Severity | Count |
+----------+----------+-------+
| UCIO-1   | Warning  |     3 |
| CFGBVS-1 | Warning  |     1 |
| REQP-1839| Error    |     2 |
+----------+----------+-------+
"""

DRC_NONE = """DRC Report
----------

No DRC violations found.
"""

METHODOLOGY_FULL = """Methodology Report
------------------

+----------+----------+-------+
| Rule     | Severity | Count |
+----------+----------+-------+
| TIMING-17| Warning  |     5 |
| XDCB-5   | Advisory |     2 |
| SYNTH-4  | Warning  |     1 |
+----------+----------+-------+
"""


class TestTimingSummary(unittest.TestCase):
    def test_full(self):
        out = parse_report("timing_summary", TIMING_FULL)
        self.assertEqual(out["wns"], -0.432)
        self.assertEqual(out["tns"], -2.156)
        self.assertEqual(out["whs"], 0.018)
        self.assertEqual(out["ths"], 0.0)
        self.assertEqual(out["wpws"], 0.25)
        self.assertEqual(out["tpws"], 0.0)
        self.assertEqual(out["failing_endpoints"], {"tns": 12, "ths": 0, "tpws": 0})
        self.assertEqual(out["total_endpoints"], {"tns": 843, "ths": 843, "tpws": 0})
        self.assertEqual(
            out["clocks"],
            [
                {
                    "clock": "clk",
                    "waveform_ns": [0.0, 5.0],
                    "period_ns": 10.0,
                    "frequency_mhz": 100.0,
                },
                {
                    "clock": "clk_fast",
                    "waveform_ns": [0.0, 2.5],
                    "period_ns": 5.0,
                    "frequency_mhz": 200.0,
                },
            ],
        )

    def test_no_clock_section(self):
        out = parse_report("timing_summary", TIMING_NO_CLOCKS)
        self.assertEqual(out["wns"], 0.123)
        self.assertEqual(out["tns"], 0.0)
        self.assertIsNone(out["clocks"])

    def test_empty(self):
        out = parse_report("timing_summary", "")
        self.assertEqual(
            out,
            {
                "wns": None,
                "tns": None,
                "whs": None,
                "ths": None,
                "wpws": None,
                "tpws": None,
                "failing_endpoints": {"tns": None, "ths": None, "tpws": None},
                "total_endpoints": {"tns": None, "ths": None, "tpws": None},
                "clocks": None,
            },
        )


class TestUtilization(unittest.TestCase):
    def test_full(self):
        out = parse_report("utilization", UTIL_FULL)
        self.assertEqual(
            out["slice_luts"], {"used": 1234, "available": 53200, "percent": 2.32}
        )
        self.assertEqual(
            out["lut_as_logic"], {"used": 1200, "available": 53200, "percent": 2.26}
        )
        self.assertEqual(
            out["lut_as_memory"], {"used": 34, "available": 17400, "percent": 0.2}
        )
        self.assertEqual(
            out["slice_registers"],
            {"used": 200, "available": 106400, "percent": 0.19},
        )
        self.assertEqual(
            out["f7_muxes"], {"used": 5, "available": 26600, "percent": 0.02}
        )
        self.assertEqual(
            out["f8_muxes"], {"used": 1, "available": 13300, "percent": 0.01}
        )
        self.assertEqual(
            out["block_ram_tile"], {"used": 2, "available": 140, "percent": 1.43}
        )
        self.assertEqual(
            out["ramb36"], {"used": 1, "available": 140, "percent": 0.71}
        )
        self.assertEqual(
            out["ramb18"], {"used": 2, "available": 280, "percent": 0.71}
        )
        self.assertEqual(
            out["dsps"], {"used": 3, "available": 220, "percent": 1.36}
        )
        self.assertEqual(
            out["bonded_iob"], {"used": 12, "available": 200, "percent": 6.0}
        )
        self.assertEqual(
            out["bufgctrl"], {"used": 1, "available": 32, "percent": 3.13}
        )
        self.assertEqual(
            out["mmcme2_adv"], {"used": 1, "available": 6, "percent": 16.67}
        )

    def test_partial_section(self):
        out = parse_report("utilization", UTIL_SLICE_ONLY)
        self.assertEqual(
            out["slice_luts"], {"used": 100, "available": 53200, "percent": 0.19}
        )
        self.assertIsNone(out["block_ram_tile"])
        self.assertIsNone(out["dsps"])
        self.assertIsNone(out["bufgctrl"])

    def test_empty(self):
        out = parse_report("utilization", "")
        self.assertTrue(all(value is None for value in out.values()))
        self.assertEqual(len(out), 13)


class TestPower(unittest.TestCase):
    def test_full(self):
        out = parse_report("power", POWER_FULL)
        self.assertEqual(out["total_on_chip_w"], 2.456)
        self.assertEqual(out["dynamic_w"], 1.9)
        self.assertEqual(out["device_static_w"], 0.556)
        self.assertEqual(out["junction_temp_c"], 35.3)
        self.assertEqual(out["confidence_level"], "Medium")
        self.assertEqual(
            out["breakdown"],
            [
                {"name": "Clocks", "power_w": 0.123, "percent": 5.0},
                {"name": "Signals", "power_w": 0.234, "percent": 9.5},
                {"name": "Logic", "power_w": 0.456, "percent": 18.6},
                {"name": "BRAM", "power_w": 0.078, "percent": 3.2},
                {"name": "DSP", "power_w": 0.012, "percent": 0.5},
                {"name": "IO", "power_w": 0.345, "percent": 14.0},
            ],
        )

    def test_no_breakdown(self):
        out = parse_report("power", POWER_NO_BREAKDOWN)
        self.assertEqual(out["total_on_chip_w"], 2.456)
        self.assertEqual(out["confidence_level"], "Low")
        self.assertIsNone(out["breakdown"])

    def test_empty(self):
        out = parse_report("power", "")
        self.assertEqual(
            out,
            {
                "total_on_chip_w": None,
                "dynamic_w": None,
                "device_static_w": None,
                "junction_temp_c": None,
                "confidence_level": None,
                "breakdown": None,
            },
        )


class TestClockUtilization(unittest.TestCase):
    def test_full(self):
        out = parse_report("clock_utilization", CLOCK_UTIL_FULL)
        self.assertEqual(
            out["regions"],
            [
                {
                    "region": "X0Y0",
                    "global_clocks": 2,
                    "bufg": 1,
                    "bufgctrl": 1,
                    "mmcm": 0,
                    "pll": 0,
                },
                {
                    "region": "X1Y0",
                    "global_clocks": 1,
                    "bufg": 1,
                    "bufgctrl": 0,
                    "mmcm": 1,
                    "pll": 0,
                },
            ],
        )

    def test_empty(self):
        self.assertEqual(parse_report("clock_utilization", ""), {"regions": None})

    def test_no_region_table(self):
        self.assertEqual(
            parse_report("clock_utilization", "Clock Utilization\n-----------------\n\nnothing here\n"),
            {"regions": None},
        )


class TestControlSets(unittest.TestCase):
    def test_full(self):
        out = parse_report("control_sets", CONTROL_SETS_FULL)
        self.assertEqual(out, {"total": 12, "unique": 8})

    def test_total_only(self):
        out = parse_report("control_sets", CONTROL_SETS_TOTAL_ONLY)
        self.assertEqual(out, {"total": 3, "unique": None})

    def test_empty(self):
        self.assertEqual(
            parse_report("control_sets", ""), {"total": None, "unique": None}
        )


class TestDrc(unittest.TestCase):
    def test_full(self):
        out = parse_report("drc", DRC_FULL)
        self.assertEqual(
            out["rules"],
            [
                {"rule_id": "UCIO-1", "severity": "Warning", "count": 3},
                {"rule_id": "CFGBVS-1", "severity": "Warning", "count": 1},
                {"rule_id": "REQP-1839", "severity": "Error", "count": 2},
            ],
        )

    def test_no_violations(self):
        self.assertEqual(parse_report("drc", DRC_NONE), {"rules": []})

    def test_empty(self):
        self.assertEqual(parse_report("drc", ""), {"rules": None})


class TestMethodology(unittest.TestCase):
    def test_full(self):
        out = parse_report("methodology", METHODOLOGY_FULL)
        self.assertEqual(
            out["rules"],
            [
                {"rule_id": "TIMING-17", "severity": "Warning", "count": 5},
                {"rule_id": "XDCB-5", "severity": "Advisory", "count": 2},
                {"rule_id": "SYNTH-4", "severity": "Warning", "count": 1},
            ],
        )

    def test_empty(self):
        self.assertEqual(parse_report("methodology", ""), {"rules": None})


class TestDispatch(unittest.TestCase):
    def test_aliases(self):
        self.assertEqual(
            parse_report("report_timing_summary", TIMING_NO_CLOCKS)["wns"], 0.123
        )
        self.assertEqual(parse_report("timing", TIMING_NO_CLOCKS)["wns"], 0.123)
        self.assertEqual(
            parse_report("report_drc", DRC_NONE), {"rules": []}
        )

    def test_unknown_kind(self):
        with self.assertRaises(ValueError):
            parse_report("nope", "")

    def test_parse_all(self):
        out = parse_all(
            {"timing_summary": TIMING_NO_CLOCKS, "drc": DRC_FULL}
        )
        self.assertEqual(out["timing_summary"]["wns"], 0.123)
        self.assertEqual(len(out["drc"]["rules"]), 3)


class TestCli(unittest.TestCase):
    def _run_cli(self, args, stdin_text=None):
        here = os.path.dirname(os.path.abspath(__file__))
        proc = subprocess.run(
            [sys.executable, os.path.join(here, "parse.py")] + args,
            input=stdin_text,
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        return json.loads(proc.stdout)

    def test_cli_file(self):
        with tempfile.NamedTemporaryFile(
            "w", suffix=".rpt", delete=False
        ) as handle:
            handle.write(TIMING_FULL)
            path = handle.name
        try:
            out = self._run_cli(["timing_summary", path])
        finally:
            os.unlink(path)
        self.assertEqual(out["wns"], -0.432)
        self.assertEqual(len(out["clocks"]), 2)

    def test_cli_stdin(self):
        out = self._run_cli(["drc", "-"], stdin_text=DRC_FULL)
        self.assertEqual(len(out["rules"]), 3)
        self.assertEqual(out["rules"][0]["rule_id"], "UCIO-1")


if __name__ == "__main__":
    unittest.main()
