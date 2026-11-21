"""Verify one design end to end (plan part 3): decode the bitstream
with the decoder worker's `helion-bitgen decode` binary, rebuild the
netlist from the decoded FASM alone, run the place/route/connectivity
checkers, and co-simulate the rebuild against the RTL.

A design is trusted only when every check passes. A failure names
the check and the first few violations; nothing is reported as a
pass on a partial result.

No em dashes anywhere.
"""
from __future__ import annotations

import os
import subprocess
from dataclasses import dataclass, field

from . import cosim, placecheck, routecheck
from .db import Db, default_db_root, verify_cc0
from .extract import Extractor, to_verilog
from .fabric import Fabric
from .fasmio import parse_fasm_file, to_fasm

DECODER_BIN = os.environ.get(
    "US09V_DECODER_BIN",
    os.path.expanduser("~/workspace/helion-ultra-bitgen-decoder/target/release/helion-bitgen"),
)


@dataclass
class CheckResult:
    name: str
    ok: bool
    detail: str = ""
    count: int = 0


@dataclass
class Report:
    design: str
    checks: list[CheckResult] = field(default_factory=list)
    decode_exit: int | None = None
    decode_stderr: str = ""
    extract_errors: list[str] = field(default_factory=list)
    extract_warnings: list[str] = field(default_factory=list)

    @property
    def ok(self) -> bool:
        return all(c.ok for c in self.checks)

    def add(self, name: str, ok: bool, detail: str = "", count: int = 0):
        self.checks.append(CheckResult(name, ok, detail, count))

    def summary(self) -> str:
        lines = [f"{self.design}: {'PASS' if self.ok else 'FAIL'}"]
        for c in self.checks:
            mark = "ok" if c.ok else "FAIL"
            lines.append(f"  [{mark}] {c.name}: {c.detail}")
        return "\n".join(lines)


def decode_bitstream(bit_path: str, db_root: str, part: str = "xczu3eg-sbva484-1-e") -> tuple[list[str], int, str]:
    """Run the decoder binary. Returns (fasm lines, exit code, stderr).
    Exit 1 with unexplained bits still returns the FASM printed on
    stdout (incomplete); the caller decides what that means."""
    r = subprocess.run(
        [DECODER_BIN, "decode", "--bit", bit_path, "--db-root", db_root, "--part", part],
        capture_output=True, text=True,
    )
    lines = [ln for ln in r.stdout.splitlines() if ln.strip()]
    return lines, r.returncode, r.stderr


def verify_fasm(
    design: str,
    fasm_path: str,
    db: Db,
    rtl_files: list[str] | None = None,
    top: str | None = None,
    xdc: str | None = None,
    port_map: dict[str, str] | None = None,
    workdir: str | None = None,
    cycles: int = 200,
    seed: int = 1,
    part: str | None = None,
) -> Report:
    """Verify a design from decoded FASM text. Co-sim runs only when
    RTL files and a top are given. `part` is the design's part; when
    it differs from the DB's part the tilegrid does not describe the
    design and the checks are provisional."""
    rep = Report(design)
    tiles = parse_fasm_file(fasm_path)
    rep.add("parse", True, f"{len(tiles)} tiles, {sum(len(v) for v in tiles.values())} features")
    fabric = Fabric(db)
    # Provisional tiles: golden xcku3p designs whose SLICE sites have
    # no tilegrid entry, or whose part differs from the DB's part
    # (a slice name can collide with a real slice of another part).
    # Site-type checks become notes, and tile types are inferred
    # from tile name prefixes.
    missing = any(
        t.startswith("SLICE_") and t not in db.site_to_tile for t in tiles
    )
    part_mismatch = part is not None and part != db.part
    provisional = missing or part_mismatch
    if provisional:
        why = (
            f"design part {part} != DB part {db.part}"
            if part_mismatch
            else "SLICE tiles have no tilegrid entry"
        )
        rep.add("provisional", True, why + "; site-type and tile-type checks are provisional")
    # Ports first: the place and route checkers need to know which
    # control/input pins are driven by primary inputs (testbench or
    # pads). Ports come from the XDC (package pins, outputs traced
    # back to slice pins) or from an explicit slice-pin port map.
    port_ins: dict = {}
    port_outs: dict = {}
    cosim_pin_of: dict | None = None
    if rtl_files and top and (xdc or port_map):
        from .cosim import sv_ports
        from .ports import explicit_ports_in_out, ports_from_xdc

        ports = sv_ports(rtl_files, top)
        in_bits = {b for p in ports if p.direction == "in" for b in p.bits()}
        out_bits = {b for p in ports if p.direction == "out" for b in p.bits()}
        if xdc is not None:
            port_ins, port_outs, cosim_pin_of, perr = ports_from_xdc(
                db, tiles, fabric, xdc, in_bits, out_bits
            )
            if perr:
                rep.add("ports", False, "; ".join(perr[:5]))
                return rep
            rep.add(
                "ports",
                True,
                f"{len(port_ins)} inputs, {len(port_outs)} outputs from XDC",
            )
        else:
            try:
                port_ins, port_outs, cosim_pin_of = explicit_ports_in_out(
                    port_map, in_bits, out_bits
                )
            except ValueError as e:
                rep.add("ports", False, str(e))
                return rep
            rep.add(
                "ports",
                True,
                f"{len(port_ins)} inputs, {len(port_outs)} outputs from port map",
            )
    # Place legality.
    pr = placecheck.check_place_fasm(
        db, tiles, fabric, port_ins=port_ins, provisional=provisional
    )
    rep.add(
        "place",
        pr.ok,
        f"{pr.slices} slices, {pr.carry_chains} carry slices; "
        + ("; ".join(pr.violations[:5]) if pr.violations else "no violations")
        + ("; notes: " + "; ".join(pr.notes) if pr.notes else ""),
    )
    # Route legality.
    rr = routecheck.check_route_fasm(
        db, tiles, fabric, port_ins=port_ins, provisional=provisional
    )
    if rr.skipped:
        rep.add("route", False, "skipped: " + "; ".join(rr.notes))
    else:
        rep.add(
            "route",
            rr.ok,
            f"{rr.pips} PIPs, {rr.nets} nets; "
            + ("; ".join(rr.violations[:5]) if rr.violations else "no violations"),
        )
    # Rebuild from FASM alone. Without a port map the extractor has
    # no primary inputs and every undriven input is an error.
    ex = Extractor(db, tiles, port_ins, port_outs)
    nl = ex.build()
    rep.extract_errors = list(ex.errors)
    rep.extract_warnings = list(ex.warnings)
    rep.add(
        "extract",
        not ex.errors,
        f"{len(nl.sites)} sites modelled; "
        + ("; ".join(ex.errors[:5]) if ex.errors else "no errors")
        + (f"; {len(ex.warnings)} warnings" if ex.warnings else ""),
    )
    if ex.errors:
        return rep
    if rtl_files and top and workdir:
        vpath = os.path.join(workdir, "extracted.v")
        with open(vpath, "w") as f:
            f.write(to_verilog(nl))
        try:
            cr = cosim.run(
                workdir, rtl_files, top, vpath,
                set(nl.ports_in), set(nl.ports_out),
                xdc=xdc, port_map=cosim_pin_of, cycles=cycles, seed=seed,
            )
        except RuntimeError as e:
            rep.add("cosim", False, str(e)[:500])
            return rep
        rep.add(
            "cosim",
            cr.ok,
            f"{cr.compared} comparisons, {len(cr.mismatches)} mismatches, "
            f"{cr.rtl_unknown} RTL unknowns, {cr.net_unknown} net unknowns; "
            + ("; ".join(f"cycle {k} {b}: rtl={r} net={n}" for k, b, r, n in cr.mismatches[:5])
               if cr.mismatches else "no mismatches")
            + (f"; unmapped outputs: {', '.join(cr.unmapped_outputs)}" if cr.unmapped_outputs else "")
            + (f"; unmapped inputs: {', '.join(cr.unmapped_inputs)}" if cr.unmapped_inputs else ""),
            cr.compared,
        )
    return rep


def verify_bitstream(
    design: str,
    bit_path: str,
    db_root: str,
    part: str = "xczu3eg-sbva484-1-e",
    rtl_files: list[str] | None = None,
    top: str | None = None,
    xdc: str | None = None,
    port_map: dict[str, str] | None = None,
    workdir: str | None = None,
    cycles: int = 200,
    seed: int = 1,
) -> Report:
    """Verify a design from a .bit file: decode, then verify_fasm on
    the decoded FASM. The decoded FASM is written to
    <workdir>/decoded.fasm when workdir is given."""
    verify_cc0(db_root)
    db = Db(db_root)
    fasm_lines, exit_code, stderr = decode_bitstream(bit_path, db_root, part)
    rep = Report(design)
    rep.decode_exit = exit_code
    rep.decode_stderr = stderr
    if exit_code != 0:
        rep.add("decode", False, f"exit {exit_code}: {stderr.strip().splitlines()[-1] if stderr.strip() else 'no stderr'}")
        # Still run the checkers on the partial FASM: they may catch
        # more, but the design cannot pass.
        rep.checks[0].ok = False
    else:
        rep.add("decode", True, f"{len(fasm_lines)} FASM lines, exit 0")
    if workdir:
        os.makedirs(workdir, exist_ok=True)
        fasm_path = os.path.join(workdir, "decoded.fasm")
        with open(fasm_path, "w") as f:
            f.write("\n".join(fasm_lines) + "\n")
    else:
        import tempfile

        fd, fasm_path = tempfile.mkstemp(suffix=".fasm")
        with os.fdopen(fd, "w") as f:
            f.write("\n".join(fasm_lines) + "\n")
    sub = verify_fasm(
        design, fasm_path, db, rtl_files, top, xdc, port_map, workdir,
        cycles, seed, part=part,
    )
    rep.checks.extend(sub.checks)
    rep.extract_errors = sub.extract_errors
    rep.extract_warnings = sub.extract_warnings
    return rep


def default_db() -> Db:
    root = default_db_root()
    verify_cc0(root)
    return Db(root)
