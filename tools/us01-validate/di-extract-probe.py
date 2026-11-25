#!/usr/bin/env python3
"""di-extract-probe.py - US-01 validation (W-VALIDATE).

Empirical probe of the us09v extractor on one real DI-fed slice
(SLICE_X11Y16, P-11): call the real Extractor.pin_driver and
Extractor._carry_di on the decoded FASM and on the raw prjuray
baseline FASM, and report what each returns.

Questions answered:
  1. Does pin_driver resolve a VCC-tied LUT input (C6) on decoded
     FASM, or report it undriven? (VCC-default rule gap.)
  2. Does _carry_di model CARRY8.DI2.DI2, or fail loud with 1'bx?
  3. Does injecting the single missing VCC PIP (the VCC-default rule
     applied as a PIP) make C6 resolve to const 1?
     Note: the raw baseline cannot be fed to the extractor directly;
     it uses a different vocabulary (tile names like CLEL_L_X7Y16,
     ALUT.INIT[63:0] = 64'b... binary literals) that the decoder
     normalizes into site-named hex INIT lines. The VCC PIP itself,
     INT_X7Y16.PIP.IMUX_W20.VCC_WIRE, is present in the baseline
     (verified by grep) and absent from the decoded FASM.
Usage: python3 di-extract-probe.py
Output: printed verdicts.
"""
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "us09v"))

from us09v.db import Db
from us09v.extract import Extractor
from us09v.fasmio import parse_fasm_file

DEC = os.path.expanduser("~/workspace/us01-char-logs/decoded/P-11.fasm")
SITE = "SLICE_X11Y16"


def probe(path, label, inject_vcc=False):
    print(f"=== {label}: {path} ===")
    db = Db(os.path.expanduser("~/workspace/prjuray-db-ref/zynqusp"))
    tiles = parse_fasm_file(path)
    if inject_vcc:
        tiles.setdefault("INT_X7Y16", {})["PIP.IMUX_W20.VCC_WIRE"] = None
    ex = Extractor(db, tiles)
    s = ex.sites[SITE]
    for pin in ("C1", "C2", "C3", "C4", "C5", "C6"):
        print(f"  pin_driver {pin}: {ex.pin_driver(s, pin)}")
    ex.errors.clear()
    di = ex._carry_di(s, 2, "C", lambda c: f"bypass_{c}X")
    print(f"  _carry_di(i=2, C): {di!r}")
    print(f"  errors after _carry_di: {ex.errors}")
    ex.errors.clear()
    di_bypass = ex._carry_di(s, 0, "A", lambda c: f"bypass_{c}X")
    print(f"  _carry_di(i=0, A) (DI0.AX bypass): {di_bypass!r}")
    print(f"  errors after bypass _carry_di: {ex.errors}")


probe(DEC, "decoded")
probe(DEC, "decoded + injected VCC PIP for IMUX_W20", inject_vcc=True)
