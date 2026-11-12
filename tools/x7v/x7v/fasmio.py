"""Write decoded features as canonical FASM (INIT bits grouped to vectors)."""
from __future__ import annotations

import re
from collections import defaultdict

_IDX = re.compile(r"^(?P<name>.*)\[(?P<i>\d+)\]$")


def to_fasm(features: list[str]) -> list[str]:
    vec: dict[str, int] = defaultdict(int)
    width: dict[str, int] = {}
    plain = []
    for f in features:
        m = _IDX.match(f)
        if m:
            i = int(m["i"])
            vec[m["name"]] |= 1 << i
            width[m["name"]] = max(width.get(m["name"], 0), i + 1)
        else:
            plain.append(f)
    out = sorted(plain)
    for name in sorted(vec):
        w = _vector_width(name, width[name])
        out.append(f"{name}[{w - 1}:0] = {w}'h{vec[name]:x}")
    return out


def _vector_width(name: str, at_least: int) -> int:
    leaf = name.rsplit(".", 1)[-1]
    if leaf == "INIT" and "LUT" in name:
        return 64
    for w in (1, 2, 4, 8, 16, 32, 64, 128, 256):
        if w >= at_least:
            return w
    return at_least
