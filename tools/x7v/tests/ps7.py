#!/usr/bin/env python3
"""PS7 boundary check of x7v (rung 7): examples/zedboard_ps7_axi.sv, an AXI3
register block on the PS7's M_AXI_GP0.

* Always: the boundary cosim reads from the RTL (instance, AXI master, the
  FCLK clock from the XDC's get_pins, FCLKRESETN as one active-low reset).
* On a Helion build of the design (zedbit, or --bit): no bit in the PS
  tiles and none unexplained; extraction cuts the design at the PS7 (FCLK
  reaches the BUFG, every PS7 input is a port, an unused one reads 0,
  port clocks included); verify passes with random resets and with
  --no-reset, with AXI traffic that reads back what it wrote.
* Negative controls, each must FAIL: two read-data bits swapped in the
  bitstream (their interconnect muxes exchange sources), an unused
  interrupt input left on its mux default (1), an unused port clock left
  floating, counter bit 26 (LD7) with its carry LUT cleared (the design's
  counter starts at 0xFFFF_FC00 so that every bit moves within the run),
  an RTL whose SCRATCH ignores WSTRB[1] (caught by the readback check), and
  RTLs that lose a write or read response by taking the next address while
  it is held (caught by the axi check alone).
* The database's always-on features (CFG_CENTER_MID ALWAYS_ON_PROP1-3)
  cleared from the bitstream: the decode check fails (G81).
* A design with no PS7 whose RTL names PS7 in a comment still verifies
  (blinky, built with zedbit): only a bitstream with a PS7 boundary
  co-simulates one.

    python3 tests/ps7.py [--work DIR] [--bit B]    # from tools/x7v; needs a release zedbit or --bit
"""
from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))
from x7v import bit, cosim, decode, encode, fasmio, netlist, ps7, verify  # noqa: E402
from x7v.db import Db  # noqa: E402

REPO = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
ZEDBIT = os.environ.get("X7V_ZEDBIT", os.path.join(REPO, "target", "release", "examples", "zedbit"))
SV = os.path.join(REPO, "examples", "zedboard_ps7_axi.sv")
XDC = os.path.join(REPO, "examples", "zedboard_ps7_axi.xdc")
TOP = "zedboard_ps7_axi"
RW = {0x4000_0004: 0xFFFF_FFFF, 0x4000_000C: 0x7F}   # SCRATCH, LEDS[6:0]


def boundary_checks(db: Db) -> None:
    pl = ps7.ports(db)
    assert sum(w for _, w, d in pl if d == "input") == 1774 and sum(w for _, w, d in pl if d == "output") == 1401
    b = ps7.boundary([SV], TOP, pl)
    assert (b.inst, b.masters) == ("ps7", ["MAXIGP0"]), (b.inst, b.masters)
    assert b.resets == [f"ps7.FCLKRESETN[{k}]" for k in range(4)]
    assert b.pin_of["ps7.MAXIGP0RDATA[5]"] == "PS7_MAXIGP0RDATA5" and b.pin_of["ps7.MAXIGP0ACLK"] == "PS7_MAXIGP0ACLK"
    assert "ps7.FCLKCLK[0]" in cosim.parse_xdc(XDC)[1]
    # FCLKRESETN: every bit is the one reset, inverted.
    st = cosim.make_stimulus(["a", "n0", "n1"], 200, 3, active_low=["n0", "n1"])
    assert all(l[1] == l[2] for l in st) and [l[1] for l in st[:8]] == list("11110001"), st[:8]
    print("ps7: boundary ok")


def mux_pip(ex, tile: str, wire: str):
    """The first configured pip feeding (tile, wire), through always-on pips."""
    for t, w in ex.fab.nodes[ex.fab.node(tile, wire)]:
        tt = ex.fab.tt(ex.db.tilegrid[t]["type"])
        for src, _ in tt.pips_into.get(w, ()):
            if f"{w}.{src}" in ex.active.get(t, ()):
                return t, w, src
            if tt.ppips.get(f"{w}.{src}") == "always":
                r = mux_pip(ex, t, src)
                if r:
                    return r
    return None


def pin_wire(ex, s, pin: str) -> str:
    return ex.fab.tt(ex.db.tilegrid[s.tile]["type"]).sites[s.index]["site_pins"][pin]["wire"]


def rewrite(db: Db, base: str, out: str, drop: list[str], add: list[str]) -> None:
    feats = fasmio.to_fasm(decode.decode(db, bit.parse(base, db.part).frames).fasm())
    assert all(f in feats for f in drop), drop
    feats = [f for f in feats if f not in drop] + add
    bit.rewrite(base, out, encode.encode(db, feats, list(bit.parse(base, db.part).frames)), db.part)


def run(db: Db, work: str, name: str, bitf: str, rtl: str, cycles: int, no_reset: bool = False) -> verify.Report:
    cosim.RESET_PULSE = not no_reset
    cosim.AXI_RW = RW
    rep = verify.verify(db, name, bitf, rtl=[rtl], top=TOP, xdc=XDC, workdir=os.path.join(work, f"x7v_{name}"), cycles=cycles)
    cosim.RESET_PULSE = True
    print(rep.summary())
    return rep


def axi_counts(rep: verify.Report) -> dict[str, int]:
    c = next(c for c in rep.checks if c.name == "axi")
    return {k: int(v) for k, v in re.findall(r"(\w+)=(\d+)", c.detail)}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--work", default=None)
    ap.add_argument("--bit", default=None, help="a build of examples/zedboard_ps7_axi.sv (default: build it with zedbit)")
    ap.add_argument("--cycles", type=int, default=3000)
    a = ap.parse_args()
    work = a.work or tempfile.mkdtemp(prefix="x7v_ps7_")
    os.makedirs(work, exist_ok=True)
    db = Db()
    boundary_checks(db)

    good = a.bit or os.path.join(work, "ps7_axi.bit")
    if not os.path.exists(good):
        subprocess.run([ZEDBIT, SV, XDC, good, TOP], cwd=work, check=True)
    dec = decode.decode(db, bit.parse(good, db.part).frames)
    assert not dec.unknown() and not [t for t in dec.tiles if db.tilegrid[t]["type"].startswith("PSS")], "PS tiles must hold no bits"
    ex = netlist.Extractor(db, dec)
    nl = ex.build()
    s = ps7.site(ex)
    assert not nl.errors, nl.errors[:5]
    assert ex.pin_driver(ex.sites["BUFGCTRL_X0Y0"], "I0") == ("site", "PS7_X0Y0", "FCLKCLK0")
    assert sum(p.startswith("PS7_") for p in nl.ports_out) == 1774 and nl.ports_in["PS7_FCLKCLK0"] == "s_PS7_X0Y0__FCLKCLK0"
    used = {p for p in nl.ports_out if p.startswith("PS7_MAXIGP0") and nl.ports_out[p].startswith("s_")}
    ties = {nl.ports_out[p] for p in nl.ports_out if p.startswith("PS7_") and p not in used}
    # The slave drives 67 inputs (ACLK, 6 handshakes, BID, RID, RDATA, BRESP, RRESP); the rest are tied to 0.
    assert len(used) == 67 and ties == {"1'b0"}, (len(used), ties)
    print("ps7: decode and extract ok")

    results = []
    for no_reset in (False, True):
        rep = run(db, work, "ps7_axi" + ("_no_reset" if no_reset else ""), good, SV, a.cycles, no_reset)
        n = axi_counts(rep)
        results.append(rep.ok and all(n[k] > 0 for k in ("partial", "bursts", "backtoback", "readbacks", "berr", "rerr")))

    # Two RDATA bits whose muxes (one INT tile) can take each other's source: swap them.
    pins = {k: mux_pip(ex, s.tile, pin_wire(ex, s, f"MAXIGP0RDATA{k}")) for k in range(32)}
    i, j = next((i, j) for i in range(32) for j in range(i + 1, 32) if pins[i][0] == pins[j][0]
                and (pins[j][2], pins[i][1]) in ex.fab.tt(db.tilegrid[pins[i][0]]["type"]).pip_pairs
                and (pins[i][2], pins[j][1]) in ex.fab.tt(db.tilegrid[pins[i][0]]["type"]).pip_pairs)
    (t, wi, si), (_, wj, sj) = pins[i], pins[j]
    swapped = os.path.join(work, "rdata_swapped.bit")
    rewrite(db, good, swapped, [f"{t}.{wi}.{si}", f"{t}.{wj}.{sj}"], [f"{t}.{wi}.{sj}", f"{t}.{wj}.{si}"])
    rep = run(db, work, "rdata_swapped", swapped, SV, 1500)
    bad = {re.search(r"MAXIGP0RDATA\[(\d+)\]", it).group(1) for c in rep.checks if c.name == "cosim" for it in c.items if "RDATA" in it}
    results.append([c.name for c in rep.checks if not c.ok] == ["cosim"] and bad == {str(i), str(j)})
    print(f"==> RDATA{i}/RDATA{j} swapped: {'FAIL as expected' if results[-1] else 'UNEXPECTED'}")

    # An unused interrupt input with its tie pip removed reads its mux default (1).
    t, w, src = mux_pip(ex, s.tile, pin_wire(ex, s, "IRQF2P0"))
    untied = os.path.join(work, "irq_untied.bit")
    rewrite(db, good, untied, [f"{t}.{w}.{src}"], [])
    rep = run(db, work, "irq_untied", untied, SV, 1500)
    cos = next(c for c in rep.checks if c.name == "cosim")
    results.append(not cos.ok and all("IRQF2P[0]" in it and "unconnected (0)" in it for it in cos.items[:20]))
    print(f"==> IRQF2P0 untied: {'FAIL as expected' if results[-1] else 'UNEXPECTED'}")

    # G81: the database's always-on CFG_CENTER_MID bits cleared (as a build
    # from a database without required_features.fasm had them): decode fails.
    req = db.required_features
    assert len(req) == 3 and all("ALWAYS_ON_PROP" in f for f in req), req
    no_req = os.path.join(work, "no_required.bit")
    rewrite(db, good, no_req, req, [])
    rep = run(db, work, "no_required", no_req, SV, 300)
    d = next(c for c in rep.checks if c.name == "decode")
    results.append(not d.ok and sorted(it for it in d.items if it.startswith("required feature not set")) == [f"required feature not set: {f}" for f in sorted(req)])
    print(f"==> always-on features cleared: {'FAIL as expected' if results[-1] else 'UNEXPECTED'}")

    # An unused port clock with its ground pip removed floats.
    t, w, src = mux_pip(ex, s.tile, pin_wire(ex, s, "MAXIGP1ACLK"))
    floating = os.path.join(work, "clk_floating.bit")
    rewrite(db, good, floating, [f"{t}.{w}.{src}"], [])
    rep = run(db, work, "clk_floating", floating, SV, 300)
    cos = next(c for c in rep.checks if c.name == "cosim")
    results.append(not cos.ok and all("MAXIGP1ACLK" in it and "unconnected (0)" in it for it in cos.items[:20]))
    print(f"==> MAXIGP1ACLK floating: {'FAIL as expected' if results[-1] else 'UNEXPECTED'}")

    # LD7 (pad U14) is counter bit 26: clear the carry-propagate LUT of its
    # flop, the LUT that reads the flop's Q in a slice whose carry XOR is used
    # (placement decides which slice; a route-through LUT may sit before the pad).
    ologic = re.search(r"s_(OLOGIC_X\d+Y\d+)__OQ", nl.ports_out["U14"]).group(1)
    kind, sl, q = ex.pin_driver(ex.sites[ologic], "D1")
    if kind == "site" and len(q) == 1:
        kind, sl, q = next((d for d in (ex.pin_driver(ex.sites[sl], f"{q}{i}") for i in range(1, 7)) if d[0] == "site"), ("none", sl, q))
    assert kind == "site" and q.endswith("Q"), (kind, sl, q)
    text = netlist.to_verilog(nl)
    carry = {(m[1], m[2]) for m in re.finditer(r"assign s_(SLICE_X\d+Y\d+)__([A-D]) = s_\1__\2_INIT\[\{([^}]*)\}\]", text)
             if f"s_{sl}__{q}" in m[3].split(",") and f"s_{m[1]}__{m[2]}_XOR" in text}
    assert len(carry) == 1, carry
    csl, letter = carry.pop()
    feats = fasmio.to_fasm(dec.fasm())
    sv = ex.sites[csl]
    lut = [f for f in feats if f.startswith(f"{sv.tile}.{sv.fprefix}.{letter}LUT.INIT")]
    assert len(lut) == 1, lut
    cnt26 = os.path.join(work, "cnt26_lut0.bit")
    rewrite(db, good, cnt26, lut, [])
    rep = run(db, work, "cnt26_lut0", cnt26, SV, 1500)
    cos = next(c for c in rep.checks if c.name == "cosim")
    results.append(not cos.ok and any("U14" in it or "led[7]" in it for it in cos.items))
    print(f"==> counter bit 26 carry LUT cleared ({lut[0]}): {'FAIL as expected' if results[-1] else 'UNEXPECTED'}")

    # RTL mutants against the good bitstream; the axi check, which reads the
    # RTL alone, must fail on each: SCRATCH byte 1 written whatever WSTRB[1]
    # says (readback), and a held B or R overwritten by the next write or
    # read (lost on silicon: the response changes before READY, and the
    # first transaction never completes).
    src_text = open(SV).read()
    for name, old, new, want in [
            ("strobe_mutant", "if (w_strb[1]) scratch[15:8]", "scratch[15:8]", "returned"),
            ("b_overwrite", "assign do_write = aw_held & w_held & ~b_pend;", "assign do_write = aw_held & w_held;", "BVALID dropped or B changed"),
            ("ar_overwrite", "assign arready = ~r_pend & ~rst;", "assign arready = ~rst;", "RVALID dropped or R changed")]:
        assert src_text.count(old) == 1, old
        mutant = os.path.join(work, f"{name}.sv")
        open(mutant, "w").write(src_text.replace(old, new))
        rep = run(db, work, name, good, mutant, 1500)
        axi = next(c for c in rep.checks if c.name == "axi")
        results.append(not axi.ok and any(want in it for it in axi.items))
        print(f"==> {name}: {'FAIL as expected' if results[-1] else 'UNEXPECTED'}")

    # Blinky has no PS7; its RTL saying "PS7" must not make cosim look for one.
    blinky_sv = os.path.join(work, "zedboard_blinky.sv")
    open(blinky_sv, "w").write("/* This design does not use the Zynq PS7. */\n" + open(os.path.join(REPO, "examples", "zedboard_blinky.sv")).read())
    blinky = os.path.join(work, "blinky.bit")
    if not os.path.exists(blinky):
        subprocess.run([ZEDBIT, blinky_sv, os.path.join(REPO, "examples", "zedboard.xdc"), blinky, "zedboard_blinky"], cwd=work, check=True)
    sim, cosim.RTL_SIM = cosim.RTL_SIM, "verilator"   # its counter has no reset or initial value
    rep = verify.verify(db, "blinky_ps7_word", blinky, rtl=[blinky_sv], top="zedboard_blinky",
                        xdc=os.path.join(REPO, "examples", "zedboard.xdc"), workdir=os.path.join(work, "x7v_blinky"), cycles=1000)
    cosim.RTL_SIM = sim
    print(rep.summary())
    results.append(rep.ok)
    print(f"==> blinky naming PS7 in a comment: {'PASS as expected' if results[-1] else 'UNEXPECTED'}")

    print(f"ps7: {sum(results)}/{len(results)} as expected ({work})")
    return 0 if all(results) else 1


if __name__ == "__main__":
    sys.exit(main())
