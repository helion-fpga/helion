"""x7v command line.

  python3 -m x7v info      <bit>
  python3 -m x7v decode    <bit> [-o out.fasm]
  python3 -m x7v roundtrip <bit>                       # decode -> encode == input?
  python3 -m x7v bram      <bit>                       # every configured block RAM half
  python3 -m x7v dsp       <bit>                       # every configured DSP48E1
  python3 -m x7v cmt       <bit> [--clkin-mhz F]       # every configured MMCM / PLL
  python3 -m x7v extract   <bit> -o netlist.v
  python3 -m x7v verify    <bit> --design N [--rtl a.vhd ...] [--top T] [--xdc X] [--golden G]
  python3 -m x7v corpus    <corpus.json> [--bit-key bit|golden] [--only a,b]
  python3 -m x7v synthcheck <corpus.json> [--only a,b]  # Helion synthesis vs RTL
  python3 -m x7v svcheck <corpus.json> [--only a,b]     # VHDL translation (emitted SV) vs VHDL
"""
from __future__ import annotations

import argparse
import os
import sys

from . import bit, bram, decode, dsp, encode, fasmio, mmcm, netlist, pins, synthcheck, verify
from .db import Db
from .cosim import is_verilog as cosim_is_verilog


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(prog="x7v")
    ap.add_argument("--part", help="part the bitstream is for (default xc7z020clg484-1, the ZedBoard's)")
    ap.add_argument("--board", help="board: sets the part and, for flow, the pin pools (zedboard, arty-a7-35, basys3, nexys-a7-100t, arty-s7-25)")
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("info"); p.add_argument("bit")
    p = sub.add_parser("decode"); p.add_argument("bit"); p.add_argument("-o")
    p = sub.add_parser("roundtrip"); p.add_argument("bit")
    p = sub.add_parser("bram"); p.add_argument("bit")
    p = sub.add_parser("dsp"); p.add_argument("bit")
    p = sub.add_parser("cmt"); p.add_argument("bit"); p.add_argument("--clkin-mhz", type=float)
    p = sub.add_parser("extract"); p.add_argument("bit"); p.add_argument("-o", required=True)
    p = sub.add_parser("verify"); p.add_argument("bit"); p.add_argument("--design", default="design")
    p.add_argument("--rtl", nargs="*"); p.add_argument("--top"); p.add_argument("--xdc"); p.add_argument("--golden")
    p.add_argument("--cycles", type=int, default=3000); p.add_argument("--workdir")
    p.add_argument("--seed", type=int, default=1, help="stimulus seed")
    p.add_argument("--reset-rate", type=float, default=None, help="chance per cycle that a reset input flips (default 0.01)")
    p.add_argument("--no-reset", action="store_true", help="hold reset inputs at 0 for the whole run (proves a power-on reset)")
    p.add_argument("--reset-at", default=None, metavar="CYCLE[:LEN]", help="no random resets: pulse reset inputs once, at CYCLE for LEN cycles (default 3)")
    p.add_argument("--axi-rw", default=None, metavar="ADDR[=MASK],...",
                   help="PS7 designs: registers the AXI masters must read back as written (MASK: the bits they hold, default all)")
    p = sub.add_parser("micro"); p.add_argument("files", nargs="+"); p.add_argument("--top", default="top")
    p.add_argument("--cycles", type=int, default=1000); p.add_argument("--workdir", default="/tmp/x7v_micro")
    p.add_argument("--translate-only", action="store_true", help="VHDL: check the emitted SV, not the synthesized netlist")
    p = sub.add_parser("flow", help="HDL -> Helion bitstream (zedbit) -> verify, with generated Zedboard pins")
    p.add_argument("files", nargs="+"); p.add_argument("--top", default="top")
    p.add_argument("--cycles", type=int, default=2000); p.add_argument("--workdir", default="/tmp/x7v_flow")
    p = sub.add_parser("svcheck"); p.add_argument("corpus"); p.add_argument("--only")
    p.add_argument("--cycles", type=int, default=3000); p.add_argument("--workdir", default="/tmp/x7v_svcheck")
    p = sub.add_parser("synthcheck"); p.add_argument("corpus"); p.add_argument("--only")
    p.add_argument("--cycles", type=int, default=3000); p.add_argument("--workdir", default="/tmp/x7v_synthcheck")
    p = sub.add_parser("corpus"); p.add_argument("corpus"); p.add_argument("--bit-key", default="bit")
    p.add_argument("--only"); p.add_argument("--cycles", type=int, default=3000); p.add_argument("--workdir")
    a = ap.parse_args(argv)
    if a.part and a.board:
        ap.error("give --part or --board, not both")
    board = pins.board(a.board) if a.board else None
    part = board["part"] if board else (a.part or "xc7z020clg484-1")
    db = Db.for_part(part)

    if a.cmd == "info":
        bs = bit.parse(a.bit, db.part)
        print(bs.header)
        print("commands:", bs.commands())
        print("frames:", len(bs.frames), "pad frames:", len(bs.pad_frames))
        for reg in ("IDCODE", "COR0", "COR1", "CTL0", "MASK"):
            print(f"{reg}: {bs.reg_value(reg):#010x}" if bs.reg_value(reg) is not None else f"{reg}: -")
        return 0
    if a.cmd == "decode":
        bs = bit.parse(a.bit, db.part)
        d = decode.decode(db, bs.frames)
        text = "\n".join(fasmio.to_fasm(d.fasm())) + "\n"
        (open(a.o, "w").write(text) if a.o else sys.stdout.write(text))
        unk = d.unknown()
        print(f"# {len(d.fasm())} features, {sum(len(v) for v in unk.values())} unexplained bits, "
              f"{len(d.ecc_mismatch)} ECC mismatches", file=sys.stderr)
        return 0
    if a.cmd == "bram":
        lines = bram.summary(decode.decode(db, bit.parse(a.bit, db.part).frames))
        print("\n".join(lines) or "no block RAM configured")
        return 0
    if a.cmd == "dsp":
        lines = dsp.summary(decode.decode(db, bit.parse(a.bit, db.part).frames))
        print("\n".join(lines) or "no DSP48E1 configured")
        return 0
    if a.cmd == "cmt":
        lines = mmcm.summary(decode.decode(db, bit.parse(a.bit, db.part).frames), a.clkin_mhz)
        print("\n".join(lines) or "no MMCM or PLL configured")
        return 1 if any("PROBLEM" in l for l in lines) else 0
    if a.cmd == "roundtrip":
        bs = bit.parse(a.bit, db.part)
        d = decode.decode(db, bs.frames)
        fr = encode.encode(db, fasmio.to_fasm(d.fasm()), list(bs.frames))
        nbits = sum(bin(x ^ y).count("1") for far in bs.frames for x, y in zip(bs.frames[far], fr[far]))
        print(f"roundtrip differing bits: {nbits}")
        return 0 if nbits == 0 else 1
    if a.cmd == "extract":
        bs = bit.parse(a.bit, db.part)
        nl = netlist.extract(db, decode.decode(db, bs.frames))
        open(a.o, "w").write(netlist.to_verilog(nl))
        for e in nl.errors:
            print("error:", e, file=sys.stderr)
        for w in nl.warnings:
            print("warning:", w, file=sys.stderr)
        return 0 if not nl.errors else 1
    if a.cmd == "verify":
        if a.reset_rate is not None:
            from . import cosim
            cosim.RESET_RATE = a.reset_rate
        if a.no_reset:
            from . import cosim
            cosim.RESET_PULSE = False
        if a.reset_at:
            from . import cosim
            start, _, length = a.reset_at.partition(":")
            cosim.RESET_AT = (int(start), int(length or 3))
        if a.axi_rw:
            from . import cosim
            for item in a.axi_rw.split(","):
                addr, _, mask = item.partition("=")
                cosim.AXI_RW[int(addr, 0)] = int(mask or "0xffffffff", 0)
        r = verify.verify(db, a.design, a.bit, a.rtl, a.top, a.xdc, a.golden, a.workdir, a.cycles, a.seed)
        print(r.summary())
        return 0 if r.ok else 1
    if a.cmd == "micro":
        bad = 0
        for f in map(os.path.abspath, a.files):
            r = synthcheck.run_micro(f, a.top, f"{a.workdir}/{os.path.basename(f)}", a.cycles, translate_only=a.translate_only)
            print(r.summary())
            bad += not r.ok
        print(f"micro: {len(a.files) - bad} pass, {bad} fail")
        return 1 if bad else 0
    if a.cmd == "flow":
        import subprocess, time
        from . import cosim
        zb = os.environ.get("X7V_ZEDBIT", os.path.join(synthcheck.REPO, "target/release/examples/zedbit"))
        bad = 0
        for f in map(os.path.abspath, a.files):
            name = os.path.basename(f)
            wd = f"{a.workdir}/{name}"
            os.makedirs(wd, exist_ok=True)
            ports = cosim.sv_ports([f], a.top) if cosim.is_verilog([f]) else cosim.vhdl_ports([f], a.top)
            xdc = os.path.join(wd, "pins.xdc")
            pins.write_xdc(ports, xdc, db, a.board)
            bitf = os.path.join(wd, "helion.bit")
            if os.path.exists(bitf):
                os.remove(bitf)
            t = time.time()
            r = subprocess.run([zb, f, xdc, bitf, a.top, f"--part={part}"], capture_output=True, text=True, timeout=7200)
            msg = [l for l in (r.stdout + r.stderr).splitlines() if l.startswith("zedbit")]
            if not os.path.exists(bitf):
                print(f"{name}: FAIL build ({time.time() - t:.0f}s): {msg[-1] if msg else (r.stderr.strip().splitlines() or ['?'])[-1]}")
                bad += 1
                continue
            rep = verify.verify(db, name, bitf, [f], a.top, xdc, None, os.path.join(wd, "verify"), a.cycles)
            print(f"{name}: built in {time.time() - t:.0f}s; " + rep.summary())
            bad += not rep.ok
        print(f"flow: {len(a.files) - bad} pass, {bad} fail")
        return 1 if bad else 0
    if a.cmd == "svcheck":
        items = [it for it in verify.load_corpus(a.corpus) if not cosim_is_verilog(it["rtl"])]
        only = set(a.only.split(",")) if a.only else None
        bad = n = 0
        for it in items:
            if only and it["name"] not in only:
                continue
            n += 1
            r = synthcheck.run_sv(it["name"], it["rtl"], it["top"], it["xdc"], f"{a.workdir}/{it['name']}", a.cycles)
            print(r.summary())
            bad += not r.ok
        print(f"svcheck: {n - bad} pass, {bad} fail")
        return 1 if bad else 0
    if a.cmd == "synthcheck":
        items = verify.load_corpus(a.corpus)
        only = set(a.only.split(",")) if a.only else None
        bad = 0
        for it in items:
            if only and it["name"] not in only:
                continue
            r = synthcheck.run(it["name"], it["rtl"], it["top"], it["xdc"], f"{a.workdir}/{it['name']}", a.cycles)
            print(r.summary())
            bad += not r.ok
        print(f"synthcheck: {sum(1 for it in items if not only or it['name'] in only) - bad} pass, {bad} fail")
        return 1 if bad else 0
    if a.cmd == "corpus":
        items = verify.load_corpus(a.corpus)
        only = set(a.only.split(",")) if a.only else None
        bad = 0
        for it in items:
            if only and it["name"] not in only:
                continue
            bitf = it.get(a.bit_key)
            if not bitf:
                print(f"{it['name']}: no '{a.bit_key}' bitstream, skipped")
                continue
            wd = f"{a.workdir}/{it['name']}" if a.workdir else None
            golden = it.get("golden") if a.bit_key != "golden" else None
            r = verify.verify(db, it["name"], bitf, it.get("rtl"), it.get("top"), it.get("xdc"), golden, wd, a.cycles)
            print(r.summary())
            bad += not r.ok
        return 1 if bad else 0
    return 2


if __name__ == "__main__":
    sys.exit(main())
