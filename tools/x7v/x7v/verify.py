"""One-shot verification of a 7-series bitstream against its design.

Checks, in order (each independent, all reported):
  packets   configuration command sequence equals Vivado's (CRC handling aside)
  ecc       every frame's ECC matches its contents
  decode    every set bit is explained by an X-Ray feature (no unknown bits)
  clocks    clock-network enable rules (clockcheck.py)
  extract   netlist extraction found no undriven/multiply-driven/unmodelled logic
  cosim     extracted netlist equals the RTL, cycle by cycle, on random stimulus
  io        I/O configuration equals the golden (Vivado) build's, tile by tile
"""
from __future__ import annotations

import json
import os
from dataclasses import asdict, dataclass, field

from . import bit, bram, clockcheck, cosim, decode, netlist
from .db import Db

VIVADO_COMMANDS = ["NULL", "RCRC", "SWITCH", "WCFG", "GRESTORE", "LFRM", "START", "DESYNC"]


@dataclass
class Check:
    name: str
    ok: bool
    detail: str = ""
    items: list[str] = field(default_factory=list)


@dataclass
class Report:
    design: str
    bitstream: str
    checks: list[Check] = field(default_factory=list)

    @property
    def ok(self) -> bool:
        return all(c.ok for c in self.checks)

    def summary(self) -> str:
        lines = [f"{self.design}: {'PASS' if self.ok else 'FAIL'}  ({self.bitstream})"]
        for c in self.checks:
            lines.append(f"  {'ok  ' if c.ok else 'FAIL'} {c.name:8s} {c.detail}")
            for it in c.items[:6]:
                lines.append(f"         - {it}")
            if len(c.items) > 6:
                lines.append(f"         ... {len(c.items) - 6} more")
        return "\n".join(lines)


def _io_features(dec: decode.DesignDecode) -> dict[str, set[str]]:
    out = {}
    for t, td in dec.tiles.items():
        if "IOB" in td.tile_type or "IOI" in td.tile_type:
            fs = {f for f in td.features if not f.startswith(("ILOGIC", "OLOGIC")) or "ZINV" in f or "OQUSED" in f or "OMUX" in f or "DATA_RATE_TQ" in f}
            if fs:
                out[t] = fs
    return out


def verify(db: Db, design: str, bitfile: str, rtl: list[str] | None = None, top: str | None = None,
           xdc: str | None = None, golden: str | None = None, workdir: str | None = None,
           cycles: int = 3000, seed: int = 1) -> Report:
    rep = Report(design, bitfile)
    bs = bit.parse(bitfile, db.part)

    cmds = [c for c in bs.commands()]
    crcs = bit.crc_checks(bs)
    bad_crc = [f"written {w:#010x} expected {e:#010x}" for w, e in crcs if w != e]
    # The device refuses a bitstream whose IDCODE is not its own.
    idcode, want_id = bs.reg_value("IDCODE"), db.part["idcode"]
    if idcode != want_id:
        bad_crc.append(f"IDCODE {idcode if idcode is None else hex(idcode)} is not {db.package}'s {want_id:#010x}: the device would refuse it")
    rep.checks.append(Check("packets", cmds == VIVADO_COMMANDS and len(bs.frames) == len(bit.frame_addresses(db.part)) and not bad_crc and len(crcs) == 2,
                            f"{len(bs.frames)} frames, commands {cmds}, {len(crcs)} CRC checks, IDCODE: {len(bad_crc)} problems (device would refuse to start)" if bad_crc else
                            f"{len(bs.frames)} frames, commands {cmds}, {len(crcs)} CRC checks all correct, IDCODE {want_id:#010x} ({db.package})", bad_crc))

    dec = decode.decode(db, bs.frames)
    rep.checks.append(Check("ecc", not dec.ecc_mismatch, f"{len(dec.ecc_mismatch)} frames with wrong ECC",
                            [f"FAR {far:#010x} stored {s:#06x} expected {c:#06x}" for far, (s, c) in list(dec.ecc_mismatch.items())[:20]]))
    unk = dec.unknown()
    n_unk = sum(len(v) for v in unk.values())
    rep.checks.append(Check("decode", n_unk == 0 and not dec.orphan_bits,
                            f"{len(dec.fasm())} features, {n_unk} unexplained bits, {len(dec.orphan_bits)} orphan bits",
                            [f"{t}: {v[:8]}" for t, v in unk.items()]))

    cr = clockcheck.check(db, dec)
    rep.checks.append(Check("clocks", not cr.errors, f"{len(cr.errors)} rule violations, {len(cr.extra)} unexplained enables",
                            cr.errors + [f"extra: {e}" for e in cr.extra]))

    nl = netlist.extract(db, dec)
    halves = sum(h.in_use for hs in bram.tile_halves(dec).values() for h in hs.values())
    rep.checks.append(Check("extract", not nl.errors,
                            f"{len(nl.regs)} flops, {halves} RAMB18 halves ({halves / 2:g} RAMB36), in {sorted(nl.ports_in)}, out {sorted(nl.ports_out)}, {len(nl.errors)} errors",
                            nl.errors))

    if rtl and top and xdc:
        wd = workdir or os.path.join(os.path.dirname(os.path.abspath(bitfile)), f"x7v_{design}")
        os.makedirs(wd, exist_ok=True)
        nv = os.path.join(wd, "netlist.v")
        open(nv, "w").write(netlist.to_verilog(nl))
        try:
            r = cosim.run(wd, rtl, top, xdc, nv, set(nl.ports_in), set(nl.ports_out), cycles=cycles, seed=seed)
            items = [f"cycle {k}: {b} rtl={x} bitstream={y}" for k, b, x, y in r.mismatches[:20]]
            items += [f"output {b} has no pad in the bitstream" for b in r.unmapped_outputs]
            # An input with no pad is one the design never reads (the RTL
            # simulation still drives it, so a real dependency would show
            # up as output mismatches): reported, not a failure.
            items += [f"note: input {b} is unused (no pad in the bitstream)" for b in r.unmapped_inputs]
            rep.checks.append(Check("cosim", r.ok,
                                    f"{r.compared} output samples over {cycles} cycles, {len(r.mismatches)} mismatches, "
                                    f"{r.net_unknown} X/Z from bitstream, {r.rtl_unknown} RTL don't-care", items))
        except Exception as e:  # simulator failures are reported, not hidden
            rep.checks.append(Check("cosim", False, f"simulation failed: {str(e)[:300]}"))

    if golden:
        gb = bit.parse(golden, db.part)
        gd = decode.decode(db, gb.frames)
        a, b = _io_features(dec), _io_features(gd)
        diffs = []
        for t in sorted(set(a) | set(b)):
            x, y = a.get(t, set()), b.get(t, set())
            for f in sorted(y - x):
                diffs.append(f"{t}.{f}: golden only")
            for f in sorted(x - y):
                diffs.append(f"{t}.{f}: this bitstream only")
        rep.checks.append(Check("io", not diffs, f"{len(diffs)} I/O feature differences vs golden", diffs))
    return rep


def load_corpus(path: str) -> list[dict]:
    base = os.path.dirname(os.path.abspath(path))
    items = json.load(open(path))
    for it in items:
        for k in ("bit", "golden", "xdc"):
            if it.get(k):
                it[k] = os.path.expanduser(it[k])
        it["rtl"] = [os.path.expanduser(p) for p in it.get("rtl", [])]
    return items
