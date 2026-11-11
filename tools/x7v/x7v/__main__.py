"""x7v command line.

  python3 -m x7v info      <bit>
  python3 -m x7v decode    <bit> [-o out.fasm]
  python3 -m x7v roundtrip <bit>                       # decode -> encode == input?
  python3 -m x7v extract   <bit> -o netlist.v
  python3 -m x7v verify    <bit> --design N [--rtl a.vhd ...] [--top T] [--xdc X] [--golden G]
  python3 -m x7v corpus    <corpus.json> [--bit-key bit|golden] [--only a,b]
"""
from __future__ import annotations

import argparse
import sys

from . import bit, decode, encode, fasmio, netlist, verify
from .db import Db


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(prog="x7v")
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("info"); p.add_argument("bit")
    p = sub.add_parser("decode"); p.add_argument("bit"); p.add_argument("-o")
    p = sub.add_parser("roundtrip"); p.add_argument("bit")
    p = sub.add_parser("extract"); p.add_argument("bit"); p.add_argument("-o", required=True)
    p = sub.add_parser("verify"); p.add_argument("bit"); p.add_argument("--design", default="design")
    p.add_argument("--rtl", nargs="*"); p.add_argument("--top"); p.add_argument("--xdc"); p.add_argument("--golden")
    p.add_argument("--cycles", type=int, default=3000); p.add_argument("--workdir")
    p = sub.add_parser("corpus"); p.add_argument("corpus"); p.add_argument("--bit-key", default="bit")
    p.add_argument("--only"); p.add_argument("--cycles", type=int, default=3000); p.add_argument("--workdir")
    a = ap.parse_args(argv)
    db = Db()

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
        r = verify.verify(db, a.design, a.bit, a.rtl, a.top, a.xdc, a.golden, a.workdir, a.cycles)
        print(r.summary())
        return 0 if r.ok else 1
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
