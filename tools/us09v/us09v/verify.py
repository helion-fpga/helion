"""Verify one design end to end (plan part 3): decode the bitstream
with the decoder worker's `helion-bitgen decode` binary, rebuild the
netlist from the decoded FASM alone, run the place/route/connectivity
checkers, and co-simulate the rebuild against the RTL.

A design is trusted only when every check passes. A failure names
the check and the first few violations; nothing is reported as a
pass on a partial result.

Decoder selection (N34): the decoder binary must be built from the
current branch. The default is the release binary of this checkout,
and it refuses to run when it is missing or older than the decoder
sources in this checkout, because a binary built before the N7-port
decode change fails decode with unexplained band bits that are not
real. An explicit US09V_DECODER_BIN is the caller's assertion about
which decoder to use and is used as-is once it exists.

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


def _repo_root() -> str:
    """Repo root of this checkout (four levels above
    tools/us09v/us09v/verify.py)."""
    return os.path.dirname(
        os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    )


_REBUILD_CMD = "cargo build --release -j2 -p helion-ultrascale --bin helion-bitgen"

DECODER_BIN = os.environ.get(
    "US09V_DECODER_BIN",
    os.path.join(_repo_root(), "target", "release", "helion-bitgen"),
)

_DECODER_FROM_ENV = "US09V_DECODER_BIN" in os.environ


def _newest_decoder_source() -> tuple[str, float] | None:
    """Newest mtime among the files that determine what the
    helion-bitgen decoder binary does: the helion-ultrascale crate
    sources, its Cargo.toml, and its data files. Returns None when
    none of those paths exist (verify.py running outside a checkout);
    the caller then skips the staleness gate but still enforces
    that the binary exists."""
    crate = os.path.join(_repo_root(), "crates", "helion-ultrascale")
    cands = [os.path.join(crate, "Cargo.toml")]
    src = os.path.join(crate, "src")
    if os.path.isdir(src):
        for dirpath, _dirnames, filenames in os.walk(src):
            for name in filenames:
                if name.endswith(".rs"):
                    cands.append(os.path.join(dirpath, name))
    data = os.path.join(crate, "data")
    if os.path.isdir(data):
        for dirpath, _dirnames, filenames in os.walk(data):
            for name in filenames:
                cands.append(os.path.join(dirpath, name))
    newest: tuple[str, float] | None = None
    for path in cands:
        try:
            mtime = os.path.getmtime(path)
        except OSError:
            continue
        if newest is None or mtime > newest[1]:
            newest = (path, mtime)
    return newest


def resolve_decoder_bin() -> str:
    """Resolve the decoder binary, failing loud instead of silently
    measuring with the wrong decoder (N34).

    An explicit US09V_DECODER_BIN is used as-is once it exists.
    The default must be a decoder built from this checkout: a
    missing default, or one older than the decoder sources in this
    checkout, raises instead of running, because it may predate
    the N7-port decode change.
    """
    path = DECODER_BIN
    if not os.path.isfile(path):
        if _DECODER_FROM_ENV:
            raise RuntimeError(
                f"US09V_DECODER_BIN points at a missing file: {path}. "
                "Point it at a helion-bitgen binary built from the current branch."
            )
        raise RuntimeError(
            f"decoder binary not built: {path} does not exist. "
            f"Build it from this checkout with {_REBUILD_CMD} "
            f"(run in {_repo_root()}), or set US09V_DECODER_BIN to a "
            "helion-bitgen binary built from the current branch."
        )
    if not _DECODER_FROM_ENV:
        newest = _newest_decoder_source()
        if newest is not None:
            src_path, src_mtime = newest
            if src_mtime > os.path.getmtime(path):
                raise RuntimeError(
                    f"stale decoder binary: {path} predates the decoder sources "
                    f"in this checkout ({src_path} is newer than the binary). "
                    "A binary built before the N7-port decode change fails decode "
                    "with unexplained band bits that are not real. Rebuild it from "
                    f"this checkout with {_REBUILD_CMD} (run in {_repo_root()}), "
                    "or set US09V_DECODER_BIN to a helion-bitgen binary built "
                    "from the current branch."
                )
    return path


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
        [resolve_decoder_bin(), "decode", "--bit", bit_path, "--db-root", db_root, "--part", part],
        capture_output=True, text=True,
    )
    lines = [ln for ln in r.stdout.splitlines() if ln.strip()]
    return lines, r.returncode, r.stderr


def _ports_input_error(
    rtl_files: list[str] | None, top: str | None
) -> str | None:
    """N37: RTL files and a top module go together. One without the
    other means the caller asked for ports but left out the half the
    ports step needs, so failing here names the missing flag
    instead of letting place/route/extract fail later on an
    undriven clock. Returns None when the pair is consistent:
    both given, or neither given (a decode-only run, where ports
    are legitimately skipped)."""
    if rtl_files and not top:
        return (
            "RTL files given but no --top: cannot determine the top "
            "module, so ports cannot be derived. Pass --top <module> "
            "with --rtl."
        )
    if top and not rtl_files:
        return (
            "--top given but no --rtl: cannot derive ports without "
            "RTL files. Pass --rtl <file> with --top, or drop --top "
            "for a decode-only run."
        )
    return None


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
    # N37: fail loud here, before place/route/extract, when the
    # caller gave RTL files without a top or a top without RTL
    # files. Without this the ports block below is silently skipped
    # and the run fails downstream naming an undriven clock.
    ports_err = _ports_input_error(rtl_files, top)
    if ports_err is not None:
        rep.add("ports", False, ports_err)
        return rep
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
