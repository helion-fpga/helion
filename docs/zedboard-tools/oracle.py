#!/usr/bin/env python3
"""Refuse to compare against anything that is not Vivado's bitstream.

This project has now had TWO circular-oracle incidents:

  1. `/tmp/ref/blinky.bit` was overwritten by a Helion build, so every diff
     compared Helion against itself and reported confident "MATCH" results.
  2. `work/ref/BLINK-242.bit` looks like a reference and sits beside the real
     one, but is a Helion build (size 4045681 vs Vivado's 4045666).

Both were caught only by checksumming. Make it impossible to forget.

Usage:
    from oracle import assert_oracle, VIVADO_ORACLE
    assert_oracle(path)          # raises unless this is Vivado's own output

    python3 work/oracle.py       # print the state of every known artifact
"""
import hashlib
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REF = os.environ.get("HELION_REF_DIR") or os.path.join(HERE, "ref")

# Vivado's own output for the ZedBoard blinky. Flashed, LD0 blinks on silicon.
VIVADO_ORACLE_MD5 = "3f6d7cfa8a5db206e5cf6fd0e3568324"
VIVADO_ORACLE_SIZE = 4045666

# A second, independent Vivado build of the same source: same byte count,
# different md5. Useful as a sanity check that 4045666 is a property of the
# tool, not of one run.
VIVADO_SECOND_MD5 = "b06c9299e4ebebbd1d4dba326dce150d"

# Helion's builds are a different size. This is the cheap tell.
HELION_SIZE = 4045681

VIVADO_ORACLE = os.path.join(REF, "GT-blinky.bit")


def fingerprint(path):
    with open(path, "rb") as f:
        data = f.read()
    return hashlib.md5(data).hexdigest(), len(data)


def assert_oracle(path):
    """Raise unless `path` is byte-for-byte Vivado's bitstream."""
    if not os.path.exists(path):
        raise SystemExit(f"oracle: {path} does not exist")
    md5, size = fingerprint(path)
    if md5 == VIVADO_ORACLE_MD5:
        return True
    if size == HELION_SIZE:
        raise SystemExit(
            f"oracle: REFUSING TO COMPARE -- {path}\n"
            f"  md5 {md5}, {size} bytes, which is HELION's size, not Vivado's\n"
            f"  ({VIVADO_ORACLE_SIZE}). This is one of our own builds.\n"
            f"  A diff against it is a Helion-vs-Helion comparison.\n"
            f"  Use {VIVADO_ORACLE} instead."
        )
    raise SystemExit(
        f"oracle: REFUSING TO COMPARE -- {path}\n"
        f"  md5 {md5}, {size} bytes; expected Vivado's "
        f"{VIVADO_ORACLE_MD5} / {VIVADO_ORACLE_SIZE}."
    )


def describe(path):
    md5, size = fingerprint(path)
    if md5 == VIVADO_ORACLE_MD5:
        who = "VIVADO  <- the oracle, LD0 verified blinking"
    elif md5 == VIVADO_SECOND_MD5:
        who = "VIVADO  second independent build"
    elif size == HELION_SIZE:
        who = "HELION  (not an oracle)"
    else:
        who = "UNKNOWN"
    return f"{os.path.basename(path):24s} {md5}  {size:8d}  {who}"


if __name__ == "__main__":
    targets = sys.argv[1:] or [
        os.path.join(REF, n) for n in sorted(os.listdir(REF)) if n.endswith(".bit")
    ]
    for t in targets:
        try:
            print(describe(t))
        except FileNotFoundError:
            print(f"{t}: missing")
    print()
    # Explicit paths are being checked BECAUSE someone may be about to use
    # them as an oracle: verdict on each, not just a label.
    if sys.argv[1:]:
        bad = [t for t in targets if fingerprint(t)[0] != VIVADO_ORACLE_MD5]
        if bad:
            raise SystemExit(
                "NOT AN ORACLE: " + ", ".join(bad) + "\n"
                "These are not Vivado's output and must not be diffed against."
            )
    assert_oracle(VIVADO_ORACLE)
    print(f"OK: {VIVADO_ORACLE} is the verified oracle.")
