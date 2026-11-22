"""Tests for routecheck: fixtures pass; the negative cases (bogus
PIP, double driver, undriven sink, driverless PIP loop) are caught;
golden PIPs exist in the INT tile type."""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from tests import fixture_ports, load_db, load_fabric, load_fixture  # noqa: E402
from us09v.fasmio import parse_fasm_file  # noqa: E402
from us09v.routecheck import check_route_fasm, pip_exists_in_tile_type  # noqa: E402

GOLDEN_DIR = os.path.expanduser(
    "~/workspace/ultra-bitgen-crew/harness/golden"
)


class RouteBase(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.db = load_db()
        cls.fabric = load_fabric(cls.db)

    def check_fixture(self, name):
        fasm_path, _rtl, _pm = load_fixture(name)
        port_ins, _po, _pin = fixture_ports(name)
        tiles = parse_fasm_file(fasm_path)
        return check_route_fasm(self.db, tiles, self.fabric, port_ins=port_ins)


class TestFixturesRouteOk(RouteBase):
    def test_and2(self):
        rep = self.check_fixture("and2")
        self.assertEqual(rep.violations, [], rep.violations)
        self.assertEqual(rep.pips, 0)

    def test_blinky(self):
        rep = self.check_fixture("blinky")
        self.assertEqual(rep.violations, [], rep.violations)
        self.assertGreater(rep.pips, 0)
        self.assertGreater(rep.nets, 0)

    def test_counter4(self):
        rep = self.check_fixture("counter4")
        self.assertEqual(rep.violations, [], rep.violations)

    def test_carry8_inc(self):
        rep = self.check_fixture("carry8-inc")
        self.assertEqual(rep.violations, [], rep.violations)

    def test_wide_mux_f7(self):
        rep = self.check_fixture("wide-mux-f7")
        self.assertEqual(rep.violations, [], rep.violations)


class TestNegative(RouteBase):
    def test_bogus_pip(self):
        fasm_path, _rtl, _pm = load_fixture("and2")
        tiles = parse_fasm_file(fasm_path)
        tiles.setdefault("INT_X6Y10", {})["PIP.NOPE_WIRE.ALSO_NOPE"] = None
        rep = check_route_fasm(self.db, tiles, self.fabric)
        self.assertTrue(any("does not exist in INT" in v for v in rep.violations))

    def test_bogus_pip_does_not_abort_net_checks(self):
        # A bogus PIP plus a genuinely undriven sink: both must be
        # reported. A PIP-existence violation used to abort the
        # driver/sink/tree checks entirely (P-11: 48 BRAM-tile
        # violations hid every net check).
        fasm_path, _rtl, _pm = load_fixture("blinky")
        tiles = parse_fasm_file(fasm_path)
        tiles.setdefault("INT_X6Y10", {})["PIP.NOPE_WIRE.ALSO_NOPE"] = None
        for t in list(tiles):
            if t.startswith("INT_"):
                del tiles[t]
        tiles.setdefault("INT_X6Y10", {})["PIP.NOPE_WIRE.ALSO_NOPE"] = None
        port_ins, _po, _pin = fixture_ports("blinky")
        rep = check_route_fasm(self.db, tiles, self.fabric, port_ins=port_ins)
        self.assertTrue(any("does not exist in INT" in v for v in rep.violations))
        self.assertTrue(
            any("undriven sink" in v for v in rep.violations),
            f"violations were {rep.violations}",
        )

    def test_config_only_site_tile_is_not_a_route_violation(self):
        # A BRAM site tile carrying config features only (no PIPs)
        # is not a route violation: the route checker only verifies
        # PIPs. (P-11: 48 RAMB18 tiles with config features were
        # wrongly flagged as "PIP features on unknown tile".)
        fasm_path, _rtl, _pm = load_fixture("and2")
        tiles = parse_fasm_file(fasm_path)
        tiles["RAMB18_X1Y10"] = {
            "RAMB18E2_L.CLKARDCLKINV.V0": None,
            "RAMB18E2_L.CLOCK_DOMAINS.COMMON": None,
        }
        port_ins, _po, _pin = fixture_ports("and2")
        rep = check_route_fasm(self.db, tiles, self.fabric, port_ins=port_ins)
        self.assertEqual(rep.violations, [], rep.violations)

    def test_double_driver(self):
        # Two PIPs into the same INT wire: the blinky feedback net
        # plus a second PIP driving its first hop's dst wire.
        fasm_path, _rtl, _pm = load_fixture("blinky")
        tiles = parse_fasm_file(fasm_path)
        port_ins, _po, _pin = fixture_ports("blinky")
        # Find a PIP in the fixture and add a second PIP into its dst.
        int_tile = next(t for t in tiles if t.startswith("INT_"))
        dst, src = next(
            (f.split(".", 2)[1], f.split(".", 2)[2])
            for f in tiles[int_tile] if f.startswith("PIP.")
        )
        tt = self.fabric.tt("INT")
        other_src = next(
            s for s, _b in tt["pips_into"].get(dst, []) if s != src
        )
        tiles[int_tile][f"PIP.{dst}.{other_src}"] = None
        rep = check_route_fasm(self.db, tiles, self.fabric, port_ins=port_ins)
        self.assertTrue(
            any("drivers" in v for v in rep.violations),
            f"violations were {rep.violations}",
        )

    def test_undriven_sink(self):
        # Blinky FASM with the feedback PIPs removed: the LUT input
        # A1 is a sink with no driver.
        fasm_path, _rtl, _pm = load_fixture("blinky")
        tiles = parse_fasm_file(fasm_path)
        for t in list(tiles):
            if t.startswith("INT_"):
                del tiles[t]
        port_ins, _po, _pin = fixture_ports("blinky")
        rep = check_route_fasm(self.db, tiles, self.fabric, port_ins=port_ins)
        self.assertTrue(
            any("undriven sink" in v and "A1" in v for v in rep.violations),
            f"violations were {rep.violations}",
        )

    def test_driverless_pip_loop(self):
        # Two PIPs forming a loop with no real driver: each node has
        # a PIP driver, but neither is reachable from a real driver.
        tiles = {"SLICE_X10Y10": {"A6LUT.INIT[63:0]": 0x5555555555555555,
                                   "OUTMUXA.D6": None}}
        tt = self.fabric.tt("INT")
        # Pick two wires with PIPs both ways on INT_X6Y10.
        pair = None
        for (s, d) in tt["pip_pairs"]:
            if (d, s) in tt["pip_pairs"]:
                pair = (s, d)
                break
        self.assertIsNotNone(pair, "no bidirectional PIP pair in INT")
        s, d = pair
        tiles["INT_X6Y10"] = {f"PIP.{d}.{s}": None, f"PIP.{s}.{d}": None}
        rep = check_route_fasm(self.db, tiles, self.fabric)
        self.assertTrue(
            any("driven only by a PIP" in v for v in rep.violations),
            f"violations were {rep.violations}",
        )


class TestFwdRevNormalization(RouteBase):
    def test_fwd_spelling_same_as_plain(self):
        # Rewriting every blinky PIP in the .FWD spelling must not
        # change the route report: same PIP count, same violations.
        base = self.check_fixture("blinky")
        fasm_path, _rtl, _pm = load_fixture("blinky")
        port_ins, _po, _pin = fixture_ports("blinky")
        tiles = parse_fasm_file(fasm_path)
        for tile, feats in tiles.items():
            for f in list(feats):
                if f.startswith("PIP."):
                    feats[f + ".FWD"] = feats.pop(f)
        rep = check_route_fasm(self.db, tiles, self.fabric, port_ins=port_ins)
        self.assertEqual(rep.pips, base.pips)
        self.assertEqual(rep.violations, base.violations)

    def test_rev_spelling_of_bidirectional_pip(self):
        # The three spellings of one bidirectional PIP (plain, .FWD,
        # and .REV with swapped components) must normalize to the
        # same PIP: same count, same violations, no "does not exist".
        tt = self.fabric.tt("INT")
        pair = None
        for (s, d) in tt["pip_pairs"]:
            if (d, s) in tt["pip_pairs"]:
                pair = (s, d)
                break
        self.assertIsNotNone(pair, "no bidirectional PIP pair in INT")
        s, d = pair
        slice_tile = {"SLICE_X10Y10": {
            "A6LUT.INIT[63:0]": 0x5555555555555555,
            "OUTMUXA.D6": None,
        }}
        reports = []
        for feat in (f"PIP.{d}.{s}", f"PIP.{d}.{s}.FWD", f"PIP.{s}.{d}.REV"):
            tiles = dict(slice_tile)
            tiles["INT_X6Y10"] = {feat: None}
            rep = check_route_fasm(self.db, tiles, self.fabric)
            reports.append(rep)
        for rep in reports:
            self.assertEqual(rep.pips, 1)
            self.assertFalse(
                any("does not exist" in v for v in rep.violations),
                rep.violations,
            )
        self.assertEqual(reports[1].violations, reports[0].violations)
        self.assertEqual(reports[2].violations, reports[0].violations)

    def test_plain_plus_fwd_counts_once(self):
        # A plain PIP and its .FWD spelling on the same tile are
        # the same PIP, not a double driver.
        tt = self.fabric.tt("INT")
        pair = None
        for (s, d) in tt["pip_pairs"]:
            if (d, s) in tt["pip_pairs"]:
                pair = (s, d)
                break
        self.assertIsNotNone(pair, "no bidirectional PIP pair in INT")
        s, d = pair
        tiles = {"SLICE_X10Y10": {
            "A6LUT.INIT[63:0]": 0x5555555555555555,
            "OUTMUXA.D6": None,
        }}
        tiles["INT_X6Y10"] = {f"PIP.{d}.{s}": None, f"PIP.{d}.{s}.FWD": None}
        rep = check_route_fasm(self.db, tiles, self.fabric)
        self.assertEqual(rep.pips, 1)


class TestGoldenPips(RouteBase):
    def test_golden_pips_exist(self):
        # The harness golden FASM PIPs exist in the INT tile type
        # (tile-type level: golden tile coordinates are provisional).
        checked = 0
        for name in ("blinky", "counter", "carry8"):
            path = os.path.join(GOLDEN_DIR, f"{name}.xcku3p.fasm")
            if not os.path.exists(path):
                continue
            tiles = parse_fasm_file(path)
            for tile, feats in tiles.items():
                for f in feats:
                    if f.startswith("PIP."):
                        _p, dst, src = f.split(".", 2)
                        self.assertTrue(
                            pip_exists_in_tile_type(self.db, "INT", dst, src),
                            f"{name}: PIP {dst}.{src} missing from INT",
                        )
                        checked += 1
        self.assertGreater(checked, 0, "no golden PIPs checked")


if __name__ == "__main__":
    unittest.main()
