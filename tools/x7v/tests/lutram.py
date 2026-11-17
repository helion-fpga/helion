#!/usr/bin/env python3
"""LUT RAM checks against Vivado 2017.3 builds (xc7z020clg484-1).

    X7V_LUTRAM_REF=~/helion-zed-builds/vivado_lutram python3 tests/lutram.py   # from tools/x7v

The reference directory (kept out of the repo) holds Vivado's bitstreams of
tools/svcheck/tests_hard/lutram_*.sv (`<name>/<name>.bit`), of the
PicoRV32 SoC (`ref_pico/zedboard_pico_soc.bit`, 12 RAM32M), the pin files
(`rtl/<name>.xdc`) and Xilinx's unisim models of the LUT RAM primitives
(`unisim/RAM*.v`). Checks:

* decode: 0 unexplained bits and 0 ECC mismatches on every build (the ECC
  leaves LUT RAM contents out); without that rule 16 frames in 3 builds
  mismatch, which the test also asserts, so the rule is needed;
* verify: Vivado's lutram_prims (every primitive) equals its RTL with the
  unisim models, and a copy with one RAM LUT's contents inverted does not;
  Vivado's PicoRV32 (`ref_pico/` also holds its RTL and XDC) equals its RTL.
"""
from __future__ import annotations

import glob
import os
import sys
import tempfile

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from x7v import bit, decode, ecc, verify  # noqa: E402
from x7v.db import Db  # noqa: E402

REPO = os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", ".."))


def main() -> int:
    ref = os.path.expanduser(os.environ.get("X7V_LUTRAM_REF", ""))
    if not ref or not os.path.isdir(ref):
        print("lutram: skipped (set X7V_LUTRAM_REF to the Vivado LUT RAM builds)")
        return 0
    db = Db.for_part("xc7z020clg484-1")
    bits = sorted(glob.glob(f"{ref}/lutram_*/lutram_*.bit")) + [f"{ref}/ref_pico/zedboard_pico_soc.bit"]
    plain_bad = 0
    for b in bits:
        bs = bit.parse(b, db.part)
        d = decode.decode(db, bs.frames)
        unk = sum(len(v) for v in d.unknown().values())
        assert unk == 0 and not d.ecc_mismatch, f"{b}: {unk} unexplained bits, {len(d.ecc_mismatch)} ECC mismatches"
        plain_bad += sum(1 for far, fr in bs.frames.items() if fr[50] & 0x1FFF != ecc.expected_ecc(far, fr))
    assert plain_bad > 0, "the plain ECC also matches: the LUT RAM rule is untested"
    print(f"lutram: {len(bits)} Vivado builds decode with 0 unexplained bits and 0 ECC mismatches ({plain_bad} without the LUT RAM rule)")

    name = "lutram_prims"
    rtl = [f"{REPO}/tools/svcheck/tests_hard/{name}.sv"] + sorted(glob.glob(f"{ref}/unisim/RAM*.v"))
    good = f"{ref}/{name}/{name}.bit"
    with tempfile.TemporaryDirectory() as wd:
        r = verify.verify(db, name, good, rtl, name, f"{ref}/rtl/{name}.xdc", None, f"{wd}/good", 3000)
        assert r.ok, r.summary()
        # Negative control: invert the contents of one RAM LUT (RAM64M port A, Vivado's SLICE_X108Y44).
        bs = bit.parse(good, db.part)
        d = decode.decode(db, bs.frames)
        t = "CLBLM_R_X69Y44"
        assert "SLICEM_X0.ALUT.RAM" in d.tiles[t].features
        g = db.tilegrid[t]["bits"]["CLB_IO_CLK"]
        frames = {k: list(v) for k, v in bs.frames.items()}
        for i in range(64):
            sb = db.segbits("CLBLM_R", "CLB_IO_CLK")[f"CLBLM_R.SLICEM_X0.ALUT.INIT[{i:02d}]"][0]
            frames[int(g["baseaddr"], 16) + sb.minor][g["offset"] + sb.bit // 32] ^= 1 << (sb.bit % 32)
        bad = f"{wd}/flipped.bit"
        bit.rewrite(good, bad, frames, db.part)
        r2 = verify.verify(db, name, bad, rtl, name, f"{ref}/rtl/{name}.xdc", None, f"{wd}/bad", 3000)
        cos = [c for c in r2.checks if c.name == "cosim"]
        assert cos and not cos[0].ok, "inverted LUT RAM contents went unnoticed"
    # PicoRV32 (12 RAM32M, and a LUT loop whose O5 ignores the input the loop closes).
    pd = f"{ref}/ref_pico"
    with tempfile.TemporaryDirectory() as wd:
        r = verify.verify(db, "ref_pico", f"{pd}/zedboard_pico_soc.bit", [f"{pd}/zedboard_pico_soc.sv", f"{pd}/picorv32.v"],
                          "zedboard_pico_soc", f"{pd}/zedboard_pico_soc_ref.xdc", None, wd, 3000)
        assert r.ok, r.summary()
    print("lutram: Vivado's PicoRV32 equals its RTL over 3,000 cycles")
    print("lutram: Vivado's lutram_prims equals its RTL (unisim models); one LUT RAM with its contents inverted fails cosim")
    print("lutram: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
