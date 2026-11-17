#!/usr/bin/env python3
"""MMCM/PLL decode and model checks (no Helion code).

* Always: counter decoding (EDGE half periods, NO_COUNT, 6-bit 0 = 64,
  phase in eighths), the measured lock/filter table (M = 10 OPTIMIZED is
  what Vivado wrote for examples/zedboard_mmcm.sv), and that a fractional
  extra is reported, not guessed.
* With X7V_MMCM_REF (a directory holding Vivado 2017.3's zedboard_mmcm.bit,
  built from examples/zedboard_mmcm.{sv,xdc}, see tests/cmt_sweep_jobs.sh):
  the build extracts clean and co-simulates equal to the RTL, and four
  negative controls fail: CLKOUT1 divide 8 -> 9 (cosim), ZINV_RST cleared so
  the MMCM sits in reset (cosim), the CLKFBOUT2IN feedback pip removed
  (extract), one LKTABLE bit flipped (extract).

    python3 tests/cmt.py            # from tools/x7v; needs the X-Ray database
"""
import copy
import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from x7v import mmcm  # noqa: E402

F = {"CLKFBOUT_CLKOUT1_HIGH_TIME[0]", "CLKFBOUT_CLKOUT1_HIGH_TIME[2]", "CLKFBOUT_CLKOUT1_LOW_TIME[0]",
     "CLKFBOUT_CLKOUT1_LOW_TIME[2]", "CLKFBOUT_CLKOUT1_OUTPUT_ENABLE[0]",
     "DIVCLK_DIVCLK_HIGH_TIME[0]", "DIVCLK_DIVCLK_LOW_TIME[0]", "DIVCLK_DIVCLK_NO_COUNT[0]",
     # CLKOUT0: 3 high, 4 low, EDGE: divide 7, high 3.5; phase 2 + 5/8
     "CLKOUT0_CLKOUT1_HIGH_TIME[0]", "CLKOUT0_CLKOUT1_HIGH_TIME[1]", "CLKOUT0_CLKOUT1_LOW_TIME[2]",
     "CLKOUT0_CLKOUT2_EDGE[0]", "CLKOUT0_CLKOUT2_DELAY_TIME[1]", "CLKOUT0_CLKOUT1_PHASE_MUX[0]",
     "CLKOUT0_CLKOUT1_PHASE_MUX[2]", "CLKOUT0_CLKOUT1_OUTPUT_ENABLE[0]",
     "CLKOUT5_CLKOUT2_FRACTIONAL_PHASE_MUX_F[0]", "CLKOUT5_CLKOUT2_FRACTIONAL_PHASE_MUX_F[2]",
     # CLKOUT1: both fields 0 = 64 + 64
     "CLKOUT1_CLKOUT1_OUTPUT_ENABLE[0]",
     "IN_USE", "COMP.Z_ZHOLD", "LOCKREG3_RESERVED[0]", "FILTREG1_RESERVED[3]"}
lk = 0xe73e8fa401
F |= {f"LKTABLE[{i}]" for i in range(40) if lk >> i & 1} | {f"TABLE[{i}]" for i in range(10) if 0x3d4 >> i & 1}
c = mmcm.decode_cmt("MMCME2_ADV", F)
assert (c.m, c.d, c.comp) == (10, 1, "Z_ZHOLD"), c
o0, o1 = c.counters["CLKOUT0"], c.counters["CLKOUT1"]
assert (o0.divide, o0.high_half, o0.phase8, o0.enabled) == (7, 7, 21, True), o0
assert (o1.divide, o1.high_half) == (128, 128), o1
assert not c.unexplained and not c.problems, (c.unexplained, c.problems)
assert mmcm.table_match(c) == ["HIGH", "OPTIMIZED"], mmcm.table_match(c)
bad = mmcm.decode_cmt("MMCME2_ADV", F ^ {"LKTABLE[0]"})
assert not mmcm.table_match(bad) and any("lock/filter" in e for e in mmcm.check(bad))
frac = mmcm.decode_cmt("MMCME2_ADV", F | {"CLKOUT5_CLKOUT2_FRACTIONAL_FRAC_WF_F[0]"})
assert any("FRAC_WF_F" in p for p in frac.problems), frac.problems
# CLKOUT0's falling-edge phase mux must repeat PHASE_MUX (5 here), as Vivado writes it.
assert mmcm.decode_cmt("MMCME2_ADV", F - {"CLKOUT5_CLKOUT2_FRACTIONAL_PHASE_MUX_F[0]"}).problems
assert mmcm.expected_power(c) == 0x9900  # CLKOUT0 PHASE_MUX 5
print("cmt: decode ok")

ref = os.environ.get("X7V_MMCM_REF")
if ref:
    from x7v import bit, cosim, decode, netlist
    from x7v.db import Db
    db = Db()
    ex = os.path.join(os.path.dirname(__file__), "..", "..", "..", "examples")
    rtl, xdc = [os.path.join(ex, "zedboard_mmcm.sv")], os.path.join(ex, "zedboard_mmcm.xdc")
    dec = decode.decode(db, bit.parse(os.path.join(ref, "zedboard_mmcm.bit"), db.part).frames)
    tile = next(t for t, td in dec.tiles.items() if any(f.startswith("MMCME2_ADV.IN_USE") for f in td.features))

    def run(d, label):
        nl = netlist.extract(db, d)
        with tempfile.TemporaryDirectory() as wd:
            nv = os.path.join(wd, "net.v")
            open(nv, "w").write(netlist.to_verilog(nl))
            r = cosim.run(wd, rtl, "zedboard_mmcm", xdc, nv, set(nl.ports_in), set(nl.ports_out), cycles=1500)
        print(f"cmt: {label}: {len(nl.errors)} extract errors, {len(r.mismatches)} cosim mismatches")
        return nl.errors, r

    errs, r = run(dec, "Vivado build")
    assert not errs and r.ok and r.compared > 0, (errs[:3], r.mismatches[:3])

    def mutate(drop=(), add=()):
        d = copy.deepcopy(dec)
        for f in drop:
            d.tiles[tile].features.remove(f)
        for f in add:
            d.tiles[tile].features.append(f)
        return d

    # CLKOUT1 (125 MHz, 4 + 4) -> 4 + 5: divide 9.
    errs, r = run(mutate(add=["MMCME2_ADV.CLKOUT1_CLKOUT1_LOW_TIME[0]"]), "CLKOUT1 divide 9")
    assert not errs and r.mismatches, "a wrong divider must show in cosim"
    errs, r = run(mutate(drop=["MMCME2_ADV.ZINV_RST"]), "ZINV_RST cleared")
    assert r.mismatches, "an MMCM held in reset must show in cosim"
    fb = next(f for f in dec.tiles[tile].features if f.endswith("CLKFBOUT2IN"))
    errs, _ = run(mutate(drop=[fb]), "feedback pip removed")
    assert any("feedback loop not closed" in e for e in errs), errs[:3]
    lk0 = "MMCME2_ADV.LKTABLE[0]"
    flip = mutate(drop=[lk0]) if lk0 in dec.tiles[tile].features else mutate(add=[lk0])
    errs, _ = run(flip, "LKTABLE bit flipped")
    assert any("lock/filter" in e for e in errs), errs[:3]
print("cmt: ok")
