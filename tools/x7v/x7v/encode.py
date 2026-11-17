"""FASM features -> frames (reference encoder for round-trip checks).

Mirrors decode.py exactly: alias (_SING) tiles are encoded through their
alias tile type with the same shift and Y1->Y0 renaming, block types are
honoured, and ECC is recomputed for every frame at the end.
"""
from __future__ import annotations

import re

from .db import Db
from .ecc import with_ecc

_VEC = re.compile(r"^(?P<name>.*)\[(?P<hi>\d+)(?::(?P<lo>\d+))?\]$")


def expand(lines: list[str]) -> list[str]:
    """Expand `X.F[hi:lo] = N'hV` vector lines to per-bit `X.F[NN]` features
    (prjxray zero-pads INIT indices to two digits)."""
    out = []
    for raw in lines:
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        if "=" in line:
            lhs, rhs = (s.strip() for s in line.split("=", 1))
            m = _VEC.match(lhs)
            val = _parse_value(rhs)
            if m:
                hi = int(m["hi"])
                lo = int(m["lo"]) if m["lo"] is not None else hi
                for i in range(lo, hi + 1):
                    if val >> (i - lo) & 1:
                        out.append(f"{m['name']}[{i:02d}]")
            elif val:
                out.append(lhs)
        else:
            out.append(line)
    return out


def _parse_value(v: str) -> int:
    if "'" in v:
        width, rest = v.split("'", 1)
        base = rest[0].lower()
        digits = rest[1:].replace("_", "")
        return int(digits, {"h": 16, "b": 2, "d": 10, "o": 8}[base])
    return int(v, 0)


def _segbits(seg: dict, name: str):
    """`name`'s bits; vector indices match whatever zero-padding the database
    uses (LUT INIT[05], BRAM INIT_00[005], ZINIT_A[5])."""
    if name in seg:
        return seg[name]
    m = _VEC.match(name)
    if m and m["lo"] is None:
        for pad in (1, 2, 3):
            alt = f"{m['name']}[{int(m['hi']):0{pad}d}]"
            if alt in seg:
                return seg[alt]
    return None


def encode(db: Db, features: list[str], frame_list: list[int] | None = None) -> dict[int, list[int]]:
    frames: dict[int, list[int]] = {}
    if frame_list is not None:
        for far in frame_list:
            frames[far] = [0] * 101
    missing = []
    for feat in expand(features):
        tile, _, fname = feat.partition(".")
        t = db.tilegrid.get(tile)
        if t is None:
            missing.append(feat)
            continue
        done = False
        for block, bits in t.get("bits", {}).items():
            alias = bits.get("alias")
            if alias:
                ttype = alias["type"]
                shift = alias["start_offset"] * 32
                lookup = fname
                if alias["start_offset"] == 0:
                    site, _, rest = fname.partition(".")
                    if site.endswith("_Y0"):
                        lookup = site[:-1] + "1" + ("." + rest if rest else "")
            else:
                ttype, shift, lookup = t["type"], 0, fname
            seg = _segbits(db.segbits(ttype, block), f"{ttype}.{lookup}")
            if seg is None:
                continue
            base = int(bits["baseaddr"], 16)
            for sb in seg:
                b = sb.bit - shift
                far = base + sb.minor
                word = bits["offset"] + b // 32
                fr = frames.setdefault(far, [0] * 101)
                if sb.value:
                    fr[word] |= 1 << (b % 32)
                else:
                    fr[word] &= ~(1 << (b % 32)) & 0xFFFFFFFF
            done = True
            break
        if not done:
            missing.append(feat)
    if missing:
        raise KeyError(f"{len(missing)} features have no segbits, e.g. {missing[:5]}")
    return {far: with_ecc(fr) for far, fr in frames.items()}
