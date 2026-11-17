#!/usr/bin/env python3
"""Encodings x7v takes from Vivado bitstreams rather than X-Ray (no Helion code).

* DSP48E1 AREG = 1 / BREG = 1: Vivado 2017.3 sets AREG_0's frame-27 bit
  alone (examples/zedboard_dsp_fill*.sv builds, all 384 slices); X-Ray has
  no AREG_1/BREG_1, so x7v adds them (db.py). AREG = 0 must still decode as
  AREG_0 only, and a used slice with no AREG bit at all is a problem.
* Block RAM content frames (FAR block type 1): Vivado writes no ECC (word 50
  is 0 in all 1,792 such frames of the examples/zedboard_bram_fill.sv build).
* With X7V_VIVADO_REF (see below), the BRAM build's own quirks: address buses
  routed through the tile cascade (CASCINBOT / CASCOUT_*_ACTIVE), one write
  enable fed to WEA0..3 from a LUT's O6 and its xMUX = O6, and a CARRY4 whose
  carry in is an all-clear CARRY4 below.

    python3 tests/vivado_enc.py            # from tools/x7v; needs the X-Ray database
"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from x7v import decode, dsp  # noqa: E402
from x7v.db import Db  # noqa: E402
from x7v.ecc import ECC_MASK, ECC_WORD, expected_ecc, frame_ecc, with_ecc  # noqa: E402

db = Db()
for tt in ("DSP_L", "DSP_R"):
    m = decode._Matcher(db.segbits(tt))
    for site, (a27, a26), (b27, b26) in (("DSP_0", (111, (113, 137)), (38, (40, 48))),
                                         ("DSP_1", (271, (273, 297)), (198, (200, 208)))):
        p = f"DSP48.{site}."
        # Vivado's AREG = BREG = 1: the two frame-27 bits alone, fully explained.
        active, rest = m.match({(27, a27), (27, b27)})
        assert sorted(active) == [p + "AREG_1", p + "BREG_1"] and not rest, (tt, site, active, rest)
        # AREG = 0 (X-Ray's AREG_0, all three bits): AREG_0 only, not AREG_1 too.
        active, rest = m.match({(26, a26[0]), (26, a26[1]), (27, a27)})
        assert active == [p + "AREG_0"] and not rest, (tt, site, active, rest)
        active, rest = m.match({(26, b26[0]), (26, b26[1]), (27, b27)})
        assert active == [p + "BREG_0"] and not rest, (tt, site, active, rest)

d = dsp.decode_dsp({"AREG_1", "BREG_1", "ZAREG_2_ACASCREG_1", "ZBREG_2_BCASCREG_1"})
assert (d.areg, d.acascreg, d.breg, d.bcascreg, d.problems) == (1, 1, 1, 1, []), d
d = dsp.decode_dsp({"AREG_0", "BREG_2", "ZAREG_2_ACASCREG_1", "ZBREG_2_BCASCREG_1"})
assert (d.areg, d.breg, d.problems) == (0, 2, []), d
# Negative control: no AREG/BREG bit is not an encoding Vivado writes.
d = dsp.decode_dsp({"ZAREG_2_ACASCREG_1", "ZBREG_2_BCASCREG_1"})
assert len(d.problems) == 2 and all("none set" in p for p in d.problems), d.problems

fr = [0x9E3779B1 * (i + 1) & 0xFFFFFFFF for i in range(101)]
fr[ECC_WORD] = 0
assert frame_ecc(fr) != 0
assert expected_ecc(0x00800100, fr) == 0 and with_ecc(fr, 0x00800100)[ECC_WORD] & ECC_MASK == 0
assert expected_ecc(0x00000100, fr) == frame_ecc(fr) and with_ecc(fr, 0x00000100)[ECC_WORD] & ECC_MASK == frame_ecc(fr)
print("vivado_enc: encodings ok")

# With X7V_VIVADO_REF (a directory holding Vivado 2017.3's zedboard_bram_fill.bit,
# zedboard_dsp_fill.bit and zedboard_dsp_fill_prims.bit, built from examples/ with
# their XDC): every bit explained, every ECC as Vivado wrote it, the BRAM build
# extracts clean (address buses cascaded between tiles, a CARRY4 fed by an
# all-clear one below), and clearing one CASCOUT_ARD_ACTIVE a reader depends on
# is an error.
ref = os.environ.get("X7V_VIVADO_REF")
if ref:
    import copy
    from x7v import bit, bram, netlist
    for name, halves, dsps in (("zedboard_bram_fill", 240, 0), ("zedboard_dsp_fill", 0, 192),
                               ("zedboard_dsp_fill_prims", 0, 192)):
        dec = decode.decode(db, bit.parse(os.path.join(ref, name + ".bit"), db.part).frames)
        assert not dec.unknown() and not dec.ecc_mismatch and not dec.orphan_bits, name
        assert sum(h.in_use for hs in bram.tile_halves(dec).values() for h in hs.values()) == halves, name
        ds = [d for v in dsp.tile_dsps(dec).values() for d in v.values()]
        assert len(ds) == dsps and not any(d.problems for d in ds), name
        if name == "zedboard_bram_fill":
            nl = netlist.extract(db, dec)
            assert not nl.errors, nl.errors[:5]
            # SLICE_X54Y38 takes PRECYINIT.CIN from SLICE_X54Y37, which has no carry feature.
            v = netlist.to_verilog(nl)
            couts = set(re.findall(r"\bs_SLICE_X\d+Y\d+__COUT\b", v))
            assert "s_SLICE_X54Y37__COUT" in couts and all(f"wire {c};" in v for c in couts), sorted(couts)[:5]
            t = next(t for t, td in sorted(dec.tiles.items()) if any(".BRAM_CASCINBOT_ADDRARDADDR" in f for f in td.features))
            below = next(b for b, td in dec.tiles.items() if td.tile_type == dec.tiles[t].tile_type
                         and b.rsplit("Y", 1)[0] == t.rsplit("Y", 1)[0] and int(b.rsplit("Y", 1)[1]) == int(t.rsplit("Y", 1)[1]) - 5)
            bad = copy.deepcopy(dec)
            bad.tiles[below].features.remove("CASCOUT_ARD_ACTIVE")
            errs = netlist.extract(db, bad).errors
            assert any(e.startswith(f"{t}: reads ADDRARDADDR from the tile below") for e in errs), errs[:5]
        print(f"vivado_enc: {name} ok")
print("vivado_enc: ok")
