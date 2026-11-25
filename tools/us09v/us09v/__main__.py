"""us09v command line: verify FASM or bitstreams, decode bitstreams,
and enumerate small placements.

No em dashes anywhere.
"""
from __future__ import annotations

import argparse
import json
import sys

from . import brute, verify
from .db import Db, default_db_root, verify_cc0


def main(argv: list[str] | None = None) -> int:
    p = argparse.ArgumentParser(prog="us09v", description="UltraScale+ part 3 checkers")
    sub = p.add_subparsers(dest="cmd", required=True)

    pv = sub.add_parser("verify-fasm", help="verify a decoded FASM file")
    pv.add_argument("--fasm", required=True)
    pv.add_argument("--design", default=None)
    pv.add_argument("--db-root", default=None)
    pv.add_argument("--rtl", nargs="*", default=[])
    pv.add_argument("--top", default=None)
    pv.add_argument("--xdc", default=None)
    pv.add_argument("--port-map", default=None, help="JSON file: RTL port bit -> netlist port")
    pv.add_argument("--workdir", default=None)
    pv.add_argument("--cycles", type=int, default=200)
    pv.add_argument("--seed", type=int, default=1)
    pv.add_argument("--part", default=None, help="design part; when it differs from the DB part the checks are provisional")

    pb = sub.add_parser("verify-bit", help="decode a .bit file and verify it")
    pb.add_argument("--bit", required=True)
    pb.add_argument("--design", default=None)
    pb.add_argument("--db-root", default=None)
    pb.add_argument("--part", default="xczu3eg-sbva484-1-e")
    pb.add_argument("--rtl", nargs="*", default=[])
    pb.add_argument("--top", default=None)
    pb.add_argument("--xdc", default=None)
    pb.add_argument("--port-map", default=None)
    pb.add_argument("--workdir", default=None)
    pb.add_argument("--cycles", type=int, default=200)
    pb.add_argument("--seed", type=int, default=1)

    pd = sub.add_parser("decode", help="decode a .bit file to FASM on stdout")
    pd.add_argument("--bit", required=True)
    pd.add_argument("--db-root", default=None)
    pd.add_argument("--part", default="xczu3eg-sbva484-1-e")

    pe = sub.add_parser("brute", help="count legal placements of N cells on a grid")
    pe.add_argument("--cells", type=int, required=True)
    pe.add_argument("--kind", default="LUT", choices=["LUT", "FF", "CARRY8"])
    pe.add_argument("--grid", default="2x2", help="NxN slice grid, e.g. 2x2")

    a = p.parse_args(argv)
    if a.cmd == "brute":
        nx, ny = (int(v) for v in a.grid.lower().split("x"))
        sites = brute.slice_grid(nx, ny)
        cells = [(f"c{i}", a.kind) for i in range(a.cells)]
        n = brute.count_legal_placements(cells, sites)
        print(f"{a.cells} {a.kind} cells on {nx}x{ny} slices: {n} legal placements")
        return 0
    db_root = a.db_root or default_db_root()
    verify_cc0(db_root)
    if a.cmd == "decode":
        lines, code, err = verify.decode_bitstream(a.bit, db_root, a.part)
        print("\n".join(lines))
        if err.strip():
            print(err, file=sys.stderr)
        return code
    port_map = None
    if a.port_map:
        port_map = json.load(open(a.port_map))
    db = Db(db_root)
    design = a.design or __import__("os").path.basename(
        getattr(a, "fasm", getattr(a, "bit", "design"))
    )
    if a.cmd == "verify-fasm":
        rep = verify.verify_fasm(
            design, a.fasm, db, a.rtl or None, a.top, a.xdc, port_map,
            a.workdir, a.cycles, a.seed, part=a.part,
        )
    else:
        rep = verify.verify_bitstream(
            design, a.bit, db_root, a.part, a.rtl or None, a.top, a.xdc,
            port_map, a.workdir, a.cycles, a.seed,
        )
    print(rep.summary())
    return 0 if rep.ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
