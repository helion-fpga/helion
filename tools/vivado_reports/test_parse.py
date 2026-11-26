#!/usr/bin/env python3
"""Tests for tools/vivado_reports/parse.py.

Fixtures are small Vivado 2017.3 text reports in the exact 2017.3
formats, verified against real reports from the cluster:
- every report starts with a Table of Contents repeating the titles,
- power headlines are a two-column pipe table in 1. Summary,
- the on-chip breakdown is 1.1 On-Chip Components,
- the Intra Clock Table is whitespace-aligned with a dash row,
- control sets live in a Status and Count table,
- DRC/methodology have 1. REPORT SUMMARY key: value lines and a
  Rule/Severity/Description/Violations table,
- clock regions are named like X1Y0.

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


def _ws_table(rows):
    """Build a whitespace-aligned table with a dash row, 2017.3 style.

    Column widths are fixed; the first column is left-aligned, the rest
    right-aligned, one space between columns. Blank cells stay blank, so
    a row may fill only its trailing columns.
    """
    widths = [10, 9, 9, 22, 20, 9, 9, 22, 20, 9, 9, 23, 21]
    headers = [
        "Clock", "WNS(ns)", "TNS(ns)", "TNS Failing Endpoints",
        "TNS Total Endpoints", "WHS(ns)", "THS(ns)",
        "THS Failing Endpoints", "THS Total Endpoints", "WPWS(ns)",
        "TPWS(ns)", "TPWS Failing Endpoints", "TPWS Total Endpoints",
    ]
    lines = [" ".join(name.ljust(w) for name, w in zip(headers, widths))]
    lines.append(" ".join("-" * w for w in widths))
    for row in rows:
        cells = []
        for i, (value, w) in enumerate(zip(row, widths)):
            if i == 0:
                cells.append(str(value).ljust(w))
            elif value == "":
                cells.append(" " * w)
            else:
                cells.append(str(value).rjust(w))
        lines.append(" ".join(cells))
    return "\n".join(lines)


TIMING_FULL = """Table of Contents
-----------------
1. Design Timing Summary
2. Clock Summary
3. Intra Clock Table

Design Timing Summary
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

Intra Clock Table
-----------------

""" + _ws_table(
    [
        ["clk", "-0.432", "-2.156", "12", "843", "0.018", "0.000",
         "0", "843", "0.250", "0.000", "0", "0"],
        ["sys_clk", "", "", "", "", "", "", "", "",
         "4.500", "0.000", "0", "2"],
    ]
) + """
"""

TIMING_NO_CLOCKS = """Design Timing Summary
---------------------

    WNS(ns)      TNS(ns)  TNS Failing Endpoints  TNS Total Endpoints      WHS(ns)      THS(ns)  THS Failing Endpoints  THS Total Endpoints     WPWS(ns)     TPWS(ns)  TPWS Failing Endpoints  TPWS Total Endpoints
    -------      -------  ---------------------  -------------------      -------      -------  ---------------------  -------------------     --------     --------  ----------------------  --------------------
      0.123        0.000                      0                  843        0.018        0.000                      0                  843      0.250        0.000                       0                    0
"""

UTIL_FULL = """Table of Contents
-----------------
1. Slice Logic
2. Slice Logic Distribution
3. Memory
4. DSP
5. IO and GT Specific
6. Clocking

1. Slice Logic
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

POWER_SUMMARY_TABLE = """+--------------------------------+------------+
| Total On-Chip Power (W)        |      0.106 |
| Design Power Budget (W)        | Unspecified* |
| Dynamic (W)                    |      0.080 |
| Device Static (W)              |      0.026 |
| Effective TJA (C/W)            |        4.2 |
| Max Ambient (C)                |       85.0 |
| Junction Temperature (C)       |       25.4 |
| Confidence Level               |     Medium |
+--------------------------------+------------+
"""

POWER_FULL = """Table of Contents
-----------------
1. Summary
1.1 On-Chip Components

1. Summary
----------

""" + POWER_SUMMARY_TABLE + """
1.1 On-Chip Components
----------------------

+------------+-----------+------+-----------+---------------+
| On-Chip    | Power (W) | Used | Available | Utilization (%) |
+------------+-----------+------+-----------+---------------+
| Clocks     |     0.012 |  123 |       456 |           5.0   |
| Signals    |     0.023 |  234 |       567 |           9.5   |
| Logic      |     0.045 |  345 |       678 |          18.6   |
| BRAM       |     0.008 |   12 |        34 |           3.2   |
| DSP        |     0.001 |    3 |        22 |           0.5   |
| IO         |     0.017 |   45 |       100 |          14.0   |
+------------+-----------+------+-----------+---------------+
"""

POWER_NO_BREAKDOWN = """Table of Contents
-----------------
1. Summary
1.1 On-Chip Components

1. Summary
----------

""" + POWER_SUMMARY_TABLE

CLOCK_UTIL_FULL = """Table of Contents
-----------------
4. Clock Regions: Key Resource Utilization
5. Clock Regions : Global Clock Summary

4. Clock Regions: Key Resource Utilization
-------------------------------------------

+----------------+------+----------+------+-----+
| Clock Region   | BUFG | BUFGCTRL | MMCM | PLL |
+----------------+------+----------+------+-----+
| X1Y0           |    1 |        1 |    0 |   0 |
| X1Y1           |    1 |        0 |    1 |   0 |
+----------------+------+----------+------+-----+

5. Clock Regions : Global Clock Summary
----------------------------------------

+----------------+---------------+----------+
| Clock Region   | Global Clocks | BUFGCTRL |
+----------------+---------------+----------+
| X1Y0           |             2 |        1 |
| X1Y1           |             1 |        0 |
+----------------+---------------+----------+
"""

CONTROL_SETS_FULL = """Table of Contents
-----------------
1. Summary

1. Summary
----------

+--------------------------------------------------------------+---------+
| Status                                                       | Count   |
+--------------------------------------------------------------+---------+
| Number of unique control sets                                |       8 |
| Unused register locations in slices containing registers     |    1234 |
+--------------------------------------------------------------+---------+
"""

DRC_SUMMARY_LINES = """Netlist: top.dcp
Floorplan: placed
Design limits: none
Ruledeck: default
Max violations: 10
Violations found: 6
"""

DRC_FULL = """Table of Contents
-----------------
1. REPORT SUMMARY
2. DRC DETAILS

1. REPORT SUMMARY
-----------------

""" + DRC_SUMMARY_LINES + """
+----------+----------+--------------------------------+------------+
| Rule     | Severity | Description                    | Violations |
+----------+----------+--------------------------------+------------+
| UCIO-1   | Warning  | Unconstrained IO               |          3 |
| CFGBVS-1 | Warning  | Configuration bank voltage     |          1 |
| REQP-1839| Error    | Required pins                  |          2 |
+----------+----------+--------------------------------+------------+
"""

DRC_NONE = """DRC Report
----------

No DRC violations found.
"""

DRC_ZERO = """Table of Contents
-----------------
1. REPORT SUMMARY

1. REPORT SUMMARY
-----------------

Netlist: top.dcp
Floorplan: placed
Design limits: none
Ruledeck: default
Max violations: 10
Violations found: 0
"""

METHODOLOGY_FULL = """Table of Contents
-----------------
1. REPORT SUMMARY
2. METHODOLOGY DETAILS

1. REPORT SUMMARY
-----------------

Netlist: top.dcp
Floorplan: placed
Design limits: none
Ruledeck: default
Max violations: 10
Violations found: 8

+----------+----------+-------------+------------+
| Rule     | Severity | Description | Violations |
+----------+----------+-------------+------------+
| TIMING-17| Warning  | Late clock  |          5 |
| XDCB-5   | Advisory | Constraint  |          2 |
| SYNTH-4  | Warning  | Inference   |          1 |
+----------+----------+-------------+------------+
"""

DRC_SUMMARY = {
    "netlist": "top.dcp",
    "floorplan": "placed",
    "design_limits": "none",
    "ruledeck": "default",
    "max_violations": 10,
    "violations_found": 6,
}


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
        self.assertEqual(
            out["intra_clock"],
            [
                {
                    "clock": "clk",
                    "wns": -0.432,
                    "tns": -2.156,
                    "tns_failing_endpoints": 12,
                    "tns_total_endpoints": 843,
                    "whs": 0.018,
                    "ths": 0.0,
                    "ths_failing_endpoints": 0,
                    "ths_total_endpoints": 843,
                    "wpws": 0.25,
                    "tpws": 0.0,
                    "tpws_failing_endpoints": 0,
                    "tpws_total_endpoints": 0,
                },
                {
                    "clock": "sys_clk",
                    "wns": None,
                    "tns": None,
                    "tns_failing_endpoints": None,
                    "tns_total_endpoints": None,
                    "whs": None,
                    "ths": None,
                    "ths_failing_endpoints": None,
                    "ths_total_endpoints": None,
                    "wpws": 4.5,
                    "tpws": 0.0,
                    "tpws_failing_endpoints": 0,
                    "tpws_total_endpoints": 2,
                },
            ],
        )

    def test_no_clock_section(self):
        out = parse_report("timing_summary", TIMING_NO_CLOCKS)
        self.assertEqual(out["wns"], 0.123)
        self.assertEqual(out["tns"], 0.0)
        self.assertIsNone(out["clocks"])
        self.assertIsNone(out["intra_clock"])

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
                "intra_clock": None,
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
        self.assertEqual(out["total_on_chip_w"], 0.106)
        self.assertEqual(out["dynamic_w"], 0.08)
        self.assertEqual(out["device_static_w"], 0.026)
        self.assertEqual(out["junction_temp_c"], 25.4)
        self.assertEqual(out["confidence_level"], "Medium")
        self.assertEqual(
            out["breakdown"],
            [
                {"name": "Clocks", "power_w": 0.012, "percent": 5.0},
                {"name": "Signals", "power_w": 0.023, "percent": 9.5},
                {"name": "Logic", "power_w": 0.045, "percent": 18.6},
                {"name": "BRAM", "power_w": 0.008, "percent": 3.2},
                {"name": "DSP", "power_w": 0.001, "percent": 0.5},
                {"name": "IO", "power_w": 0.017, "percent": 14.0},
            ],
        )

    def test_no_breakdown(self):
        out = parse_report("power", POWER_NO_BREAKDOWN)
        self.assertEqual(out["total_on_chip_w"], 0.106)
        self.assertEqual(out["confidence_level"], "Medium")
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
                    "region": "X1Y0",
                    "bufg": 1,
                    "bufgctrl": 1,
                    "mmcm": 0,
                    "pll": 0,
                    "global_clocks": 2,
                },
                {
                    "region": "X1Y1",
                    "bufg": 1,
                    "bufgctrl": 0,
                    "mmcm": 1,
                    "pll": 0,
                    "global_clocks": 1,
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
        self.assertEqual(
            out,
            {"total": None, "unique": 8, "unused_register_locations": 1234},
        )

    def test_empty(self):
        self.assertEqual(
            parse_report("control_sets", ""),
            {"total": None, "unique": None, "unused_register_locations": None},
        )


class TestDrc(unittest.TestCase):
    def test_full(self):
        out = parse_report("drc", DRC_FULL)
        self.assertEqual(out["summary"], DRC_SUMMARY)
        self.assertEqual(
            out["rules"],
            [
                {
                    "rule_id": "UCIO-1",
                    "severity": "Warning",
                    "description": "Unconstrained IO",
                    "count": 3,
                },
                {
                    "rule_id": "CFGBVS-1",
                    "severity": "Warning",
                    "description": "Configuration bank voltage",
                    "count": 1,
                },
                {
                    "rule_id": "REQP-1839",
                    "severity": "Error",
                    "description": "Required pins",
                    "count": 2,
                },
            ],
        )

    def test_no_violations(self):
        out = parse_report("drc", DRC_NONE)
        self.assertEqual(out["rules"], [])
        self.assertEqual(
            out["summary"],
            {
                "netlist": None,
                "floorplan": None,
                "design_limits": None,
                "ruledeck": None,
                "max_violations": None,
                "violations_found": None,
            },
        )

    def test_zero_violations_found(self):
        out = parse_report("drc", DRC_ZERO)
        self.assertEqual(out["rules"], [])
        self.assertEqual(out["summary"]["violations_found"], 0)

    def test_empty(self):
        out = parse_report("drc", "")
        self.assertEqual(out["rules"], None)


class TestMethodology(unittest.TestCase):
    def test_full(self):
        out = parse_report("methodology", METHODOLOGY_FULL)
        self.assertEqual(out["summary"]["violations_found"], 8)
        self.assertEqual(out["summary"]["ruledeck"], "default")
        self.assertEqual(
            out["rules"],
            [
                {
                    "rule_id": "TIMING-17",
                    "severity": "Warning",
                    "description": "Late clock",
                    "count": 5,
                },
                {
                    "rule_id": "XDCB-5",
                    "severity": "Advisory",
                    "description": "Constraint",
                    "count": 2,
                },
                {
                    "rule_id": "SYNTH-4",
                    "severity": "Warning",
                    "description": "Inference",
                    "count": 1,
                },
            ],
        )

    def test_empty(self):
        out = parse_report("methodology", "")
        self.assertEqual(out["rules"], None)


class TestDispatch(unittest.TestCase):
    def test_aliases(self):
        self.assertEqual(
            parse_report("report_timing_summary", TIMING_NO_CLOCKS)["wns"], 0.123
        )
        self.assertEqual(parse_report("timing", TIMING_NO_CLOCKS)["wns"], 0.123)
        self.assertEqual(
            parse_report("report_drc", DRC_NONE)["rules"], []
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
        self.assertEqual(len(out["intra_clock"]), 2)

    def test_cli_stdin(self):
        out = self._run_cli(["drc", "-"], stdin_text=DRC_FULL)
        self.assertEqual(len(out["rules"]), 3)
        self.assertEqual(out["rules"][0]["rule_id"], "UCIO-1")


if __name__ == "__main__":
    unittest.main()
