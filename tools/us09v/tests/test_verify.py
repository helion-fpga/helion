"""Tests for verify: fixtures pass end to end; golden FASM designs
fail with named reasons (undriven sinks, undriven inputs), never
with a crash or a silent pass."""
import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from tests import load_db, load_fixture  # noqa: E402
from us09v import verify as verify_mod  # noqa: E402
from us09v.verify import verify_fasm  # noqa: E402

GOLDEN_DIR = os.path.expanduser(
    "~/workspace/ultra-bitgen-crew/harness/golden"
)


class TestFixtureEndToEnd(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.db = load_db()

    def verify_fixture(self, name, cycles=100):
        fasm_path, rtl_path, pm = load_fixture(name)
        with tempfile.TemporaryDirectory() as d:
            rep = verify_fasm(
                name, fasm_path, self.db,
                rtl_files=[rtl_path],
                top=_top(rtl_path),
                port_map=pm,
                workdir=d, cycles=cycles, seed=1,
            )
        return rep

    def test_and2(self):
        rep = self.verify_fixture("and2")
        self.assertTrue(rep.ok, rep.summary())

    def test_blinky(self):
        rep = self.verify_fixture("blinky")
        self.assertTrue(rep.ok, rep.summary())

    def test_counter4(self):
        rep = self.verify_fixture("counter4")
        self.assertTrue(rep.ok, rep.summary())

    def test_carry8_inc(self):
        rep = self.verify_fixture("carry8-inc")
        self.assertTrue(rep.ok, rep.summary())

    def test_wide_mux_f7(self):
        rep = self.verify_fixture("wide-mux-f7")
        self.assertTrue(rep.ok, rep.summary())


def _top(rtl_path):
    import re

    with open(rtl_path) as f:
        return re.search(r"module\s+(\w+)", f.read()).group(1)


class TestGoldenFailsNamed(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.db = load_db()

    def test_golden_blinky(self):
        path = os.path.join(GOLDEN_DIR, "blinky.xcku3p.fasm")
        rep = verify_fasm("blinky-golden", path, self.db, part="xcku3p")
        self.assertFalse(rep.ok)
        checks = {c.name: c for c in rep.checks}
        # Provisional tiles are noted, place capacity passes.
        self.assertTrue(checks["provisional"].ok)
        self.assertTrue(checks["place"].ok, checks["place"].detail)
        # No routes at all: route fails naming undriven sinks.
        self.assertFalse(checks["route"].ok)
        self.assertIn("undriven sink", checks["route"].detail)
        # Rebuild fails naming the undriven LUT input.
        self.assertFalse(checks["extract"].ok)
        self.assertTrue(
            any("undriven" in e for e in rep.extract_errors),
            f"errors were {rep.extract_errors}",
        )

    def test_golden_counter(self):
        path = os.path.join(GOLDEN_DIR, "counter.xcku3p.fasm")
        rep = verify_fasm("counter-golden", path, self.db, part="xcku3p")
        self.assertFalse(rep.ok)
        checks = {c.name: c for c in rep.checks}
        self.assertTrue(checks["place"].ok, checks["place"].detail)
        self.assertFalse(checks["route"].ok)
        self.assertFalse(checks["extract"].ok)

    def test_golden_carry8(self):
        path = os.path.join(GOLDEN_DIR, "carry8.xcku3p.fasm")
        rep = verify_fasm("carry8-golden", path, self.db, part="xcku3p")
        self.assertFalse(rep.ok)
        checks = {c.name: c for c in rep.checks}
        self.assertTrue(checks["place"].ok, checks["place"].detail)
        self.assertFalse(checks["extract"].ok)


@unittest.skipUnless(
    os.path.exists(verify_mod.DECODER_BIN), "decoder binary not built"
)
class TestDecodePublicPair(unittest.TestCase):
    def test_p11_decode_exit_1_unexplained(self):
        # P-11 is the Session 27 baseline: exit 1 with unexplained
        # bits, FASM still printed. This pins the decoder contract
        # verify_bitstream relies on.
        bit = os.path.expanduser(
            "~/workspace/ultra-bitgen-crew/decoder-validation/bits/P-11_resizer_ultra96.bit"
        )
        db_root = os.path.expanduser("~/workspace/prjuray-db-ref/zynqusp")
        lines, code, err = verify_mod.decode_bitstream(bit, db_root)
        self.assertEqual(code, 1)
        self.assertIn("unexplained", err)
        self.assertGreater(len(lines), 0)


if __name__ == "__main__":
    unittest.main()
