"""Tests for cosim: the five fixtures co-simulate clean against
their RTL, and a flipped INIT (wrong netlist) is caught as a
mismatch."""
import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from tests import fixture_ports, load_db, load_fixture  # noqa: E402
from us09v import cosim  # noqa: E402
from us09v.extract import extract, to_verilog  # noqa: E402
from us09v.fasmio import parse_fasm_file  # noqa: E402


class CosimBase(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.db = load_db()

    def run_fixture(self, name, cycles=200, seed=1):
        fasm_path, rtl_path, _pm = load_fixture(name)
        port_ins, port_outs, pin_of = fixture_ports(name)
        tiles = parse_fasm_file(fasm_path)
        nl = extract(self.db, tiles, port_ins, port_outs)
        self.assertEqual(nl.errors, [], f"{name}: {nl.errors}")
        with tempfile.TemporaryDirectory() as d:
            vpath = os.path.join(d, "extracted.v")
            with open(vpath, "w") as f:
                f.write(to_verilog(nl))
            return cosim.run(
                d, [rtl_path], _top(rtl_path), vpath,
                set(nl.ports_in), set(nl.ports_out),
                port_map=pin_of, cycles=cycles, seed=seed,
            )


def _top(rtl_path):
    import re

    with open(rtl_path) as f:
        return re.search(r"module\s+(\w+)", f.read()).group(1)


class TestFixturesCosim(CosimBase):
    def test_and2(self):
        res = self.run_fixture("and2")
        self.assertTrue(res.ok, f"mismatches: {res.mismatches[:5]}")
        self.assertGreater(res.compared, 0)

    def test_blinky(self):
        res = self.run_fixture("blinky")
        self.assertTrue(res.ok, f"mismatches: {res.mismatches[:5]}")

    def test_counter4(self):
        res = self.run_fixture("counter4")
        self.assertTrue(res.ok, f"mismatches: {res.mismatches[:5]}")

    def test_carry8_inc(self):
        res = self.run_fixture("carry8-inc")
        self.assertTrue(res.ok, f"mismatches: {res.mismatches[:5]}")

    def test_wide_mux_f7(self):
        res = self.run_fixture("wide-mux-f7")
        self.assertTrue(res.ok, f"mismatches: {res.mismatches[:5]}")


class TestNegative(CosimBase):
    def test_flipped_lut_init_mismatch(self):
        # Change the and2 LUT from a&b to a|b (same inputs, different
        # function): the rebuild no longer matches the RTL and co-sim
        # must report mismatches. (Flipping one INIT bit would change
        # which inputs the LUT depends on, which is a different
        # failure mode.)
        fasm_path, rtl_path, _pm = load_fixture("and2")
        port_ins, port_outs, pin_of = fixture_ports("and2")
        tiles = parse_fasm_file(fasm_path)
        tiles["SLICE_X10Y10"]["A6LUT.INIT[63:0]"] = 0xEEEEEEEEEEEEEEEE
        nl = extract(self.db, tiles, port_ins, port_outs)
        self.assertEqual(nl.errors, [])
        with tempfile.TemporaryDirectory() as d:
            vpath = os.path.join(d, "extracted.v")
            with open(vpath, "w") as f:
                f.write(to_verilog(nl))
            res = cosim.run(
                d, [rtl_path], _top(rtl_path), vpath,
                set(nl.ports_in), set(nl.ports_out),
                port_map=pin_of, cycles=200, seed=1,
            )
        self.assertFalse(res.ok)
        self.assertGreater(len(res.mismatches), 0)

    def test_flipped_ff_init_mismatch(self):
        # Flip the blinky FF INIT 0 -> 1: the first cycles differ.
        fasm_path, rtl_path, _pm = load_fixture("blinky")
        port_ins, port_outs, pin_of = fixture_ports("blinky")
        tiles = parse_fasm_file(fasm_path)
        del tiles["SLICE_X10Y10"]["AFF.INIT.V1"]
        tiles["SLICE_X10Y10"]["AFF.INIT.V0"] = None
        nl = extract(self.db, tiles, port_ins, port_outs)
        self.assertEqual(nl.errors, [])
        with tempfile.TemporaryDirectory() as d:
            vpath = os.path.join(d, "extracted.v")
            with open(vpath, "w") as f:
                f.write(to_verilog(nl))
            res = cosim.run(
                d, [rtl_path], _top(rtl_path), vpath,
                set(nl.ports_in), set(nl.ports_out),
                port_map=pin_of, cycles=200, seed=1,
            )
        self.assertFalse(res.ok)
        self.assertGreater(len(res.mismatches), 0)


if __name__ == "__main__":
    unittest.main()
