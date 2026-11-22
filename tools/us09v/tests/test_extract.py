"""Tests for extract: the five fixtures rebuild clean, and the
negative cases (LUTRAM, CARRY8 DI.DI, unknown feature, missing FF
INIT, missing OUTMUX on a consumed output) fail loud with the reason
named."""
import os
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from tests import FIXTURES, byp_shape_tiles, fixture_ports, load_db, load_fabric, load_fixture  # noqa: E402
from us09v.extract import extract, to_verilog  # noqa: E402
from us09v.fasmio import parse_fasm_file  # noqa: E402


class ExtractFixtureBase(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.db = load_db()
        cls.fabric = load_fabric(cls.db)

    def extract_fixture(self, name):
        fasm_path, _rtl, _pm = load_fixture(name)
        port_ins, port_outs, _pin_of = fixture_ports(name)
        tiles = parse_fasm_file(fasm_path)
        nl = extract(self.db, tiles, port_ins, port_outs)
        return nl, tiles


class TestFixturesExtractClean(ExtractFixtureBase):
    def test_and2(self):
        nl, _ = self.extract_fixture("and2")
        self.assertEqual(nl.errors, [])
        self.assertEqual(nl.warnings, [])
        self.assertIn("p_SLICE_X10Y10_A1", nl.ports_in)
        self.assertIn("p_SLICE_X10Y10_AMUX", nl.ports_out)

    def test_blinky(self):
        nl, _ = self.extract_fixture("blinky")
        self.assertEqual(nl.errors, [])
        self.assertEqual(nl.warnings, [])

    def test_counter4(self):
        nl, _ = self.extract_fixture("counter4")
        self.assertEqual(nl.errors, [])
        self.assertEqual(nl.warnings, [])

    def test_carry8_inc(self):
        nl, _ = self.extract_fixture("carry8-inc")
        self.assertEqual(nl.errors, [])
        self.assertEqual(nl.warnings, [])

    def test_wide_mux_f7_warns(self):
        nl, _ = self.extract_fixture("wide-mux-f7")
        self.assertEqual(nl.errors, [])
        self.assertEqual(len(nl.warnings), 1)
        self.assertIn("F7/F8/F9 mux used", nl.warnings[0])
        self.assertIn("not yet validated", nl.warnings[0])

    def test_fwd_pips_resolve(self):
        # Rewriting every blinky PIP in the .FWD spelling must not
        # change the rebuild: driver() sees the suffixed PIPs.
        # (Fixture PIPs are directional, so no .REV form applies.)
        fasm_path, _rtl, _pm = load_fixture("blinky")
        port_ins, port_outs, _pin_of = fixture_ports("blinky")
        tiles = parse_fasm_file(fasm_path)
        for tile, feats in tiles.items():
            for f in list(feats):
                if f.startswith("PIP."):
                    feats[f + ".FWD"] = feats.pop(f)
        nl = extract(self.db, tiles, port_ins, port_outs)
        self.assertEqual(nl.errors, [], nl.errors)
        self.assertEqual(nl.warnings, [])


class TestVerilogOutput(ExtractFixtureBase):
    def test_no_self_assign_on_ports(self):
        # Regression: expr() used to return the port name, so
        # to_verilog emitted `assign p = p` for input ports.
        nl, _ = self.extract_fixture("and2")
        text = to_verilog(nl)
        self.assertNotIn("assign p_SLICE_X10Y10_A1 = p_SLICE_X10Y10_A1", text)
        self.assertIn("assign s_SLICE_X10Y10__A1 = p_SLICE_X10Y10_A1", text)

    def test_byp_shape_declares_lut_o_wire(self):
        # The BYP shape resolves the FF D through the fabric back
        # to the LUT O site pin A_O: the extracted netlist must
        # declare s_SLICE_X1Y0__A_O as an alias of the LUT O6 wire,
        # or iverilog fails at elaboration on the undefined wire.
        tiles = byp_shape_tiles()
        port_ins = {"clk": ("SLICE_X1Y0", "CLK1")}
        port_outs = {"led": ("SLICE_X1Y0", "AQ")}
        nl = extract(self.db, tiles, port_ins, port_outs)
        self.assertEqual(nl.errors, [], nl.errors)
        text = to_verilog(nl)
        self.assertIn("wire s_SLICE_X1Y0__A_O;", text)
        self.assertIn("assign s_SLICE_X1Y0__A_O = s_SLICE_X1Y0__A;", text)
        with tempfile.TemporaryDirectory() as d:
            vpath = os.path.join(d, "extracted.v")
            with open(vpath, "w") as f:
                f.write(text)
            r = subprocess.run(
                ["iverilog", "-g2012", "-o", os.path.join(d, "out.vvp"), vpath],
                capture_output=True, text=True,
            )
            self.assertEqual(r.returncode, 0, f"{r.stdout}{r.stderr}")

    def test_iverilog_compiles(self):
        for name in ("and2", "blinky", "counter4", "carry8-inc", "wide-mux-f7"):
            nl, _ = self.extract_fixture(name)
            with tempfile.TemporaryDirectory() as d:
                vpath = os.path.join(d, "extracted.v")
                with open(vpath, "w") as f:
                    f.write(to_verilog(nl))
                r = subprocess.run(
                    ["iverilog", "-g2012", "-o", os.path.join(d, "out.vvp"), vpath],
                    capture_output=True, text=True,
                )
                self.assertEqual(r.returncode, 0, f"{name}: {r.stdout}{r.stderr}")


class TestNegative(ExtractFixtureBase):
    def test_lutram_blocked(self):
        # A CLEM slice with a RAM-mode LUT fails loud.
        clem_site = next(
            s for s, (t, _ty) in self.db.site_to_tile.items()
            if self.db.tilegrid[t]["type"] == "CLEM"
        )
        tiles = {clem_site: {"CLUT.MODE.RAM": None, "C6LUT.INIT[63:0]": 0}}
        nl = extract(self.db, tiles)
        self.assertTrue(any("RAM mode is not modelled" in e for e in nl.errors))

    def test_carry_di_di_blocked(self):
        fasm_path, _rtl, _pm = load_fixture("carry8-inc")
        tiles = parse_fasm_file(fasm_path)
        site = "SLICE_X10Y10"
        tiles[site]["CARRY8.DI0.DI0"] = None
        del tiles[site]["CARRY8.DI0.AX"]
        port_ins, port_outs, _ = fixture_ports("carry8-inc")
        nl = extract(self.db, tiles, port_ins, port_outs)
        self.assertTrue(any("DI0.DI0 source is not modelled" in e for e in nl.errors))

    def test_unknown_feature_warned(self):
        fasm_path, _rtl, _pm = load_fixture("and2")
        tiles = parse_fasm_file(fasm_path)
        tiles["SLICE_X10Y10"]["A6LUT.FOO"] = None
        port_ins, port_outs, _ = fixture_ports("and2")
        nl = extract(self.db, tiles, port_ins, port_outs)
        self.assertEqual(nl.errors, [])
        self.assertTrue(any("unclassified feature" in w and "A6LUT.FOO" in w for w in nl.warnings))

    def test_missing_ff_init_errors(self):
        fasm_path, _rtl, _pm = load_fixture("blinky")
        tiles = parse_fasm_file(fasm_path)
        del tiles["SLICE_X10Y10"]["AFF.INIT.V0"]
        port_ins, port_outs, _ = fixture_ports("blinky")
        nl = extract(self.db, tiles, port_ins, port_outs)
        self.assertTrue(any("no INIT feature" in e for e in nl.errors))

    def test_missing_outmux_on_consumed_output_errors(self):
        # and2 without OUTMUXA, plus a real PIP that consumes AMUX:
        # a PIP sourced by a wire in the AMUX node's stitched set.
        fasm_path, _rtl, _pm = load_fixture("and2")
        tiles = parse_fasm_file(fasm_path)
        del tiles["SLICE_X10Y10"]["OUTMUXA.D6"]
        nid = self.fabric.node("CLEL_R_X6Y10", "CLE_CLE_L_SITE_0_AMUX")
        int_member = next(
            (t, w) for t, w in self.fabric.nodes[nid] if t.startswith("INT_")
        )
        tt = self.fabric.tt("INT")
        dst = next(
            d for d, pairs in tt["pips_into"].items()
            if any(s == int_member[1] for s, _b in pairs)
        )
        tiles.setdefault(int_member[0], {})[f"PIP.{dst}.{int_member[1]}"] = None
        port_ins, port_outs, _ = fixture_ports("and2")
        nl = extract(self.db, tiles, port_ins, port_outs)
        self.assertTrue(
            any("has no selection but AMUX is consumed" in e for e in nl.errors),
            f"errors were {nl.errors}",
        )


if __name__ == "__main__":
    unittest.main()
