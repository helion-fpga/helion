#!/usr/bin/env python3
"""templates-fasm.py - US-01 characterization (W-CHAR).

Enumerates distinct FASM feature templates in a decoded pair file:
tile coordinates are replaced by X/Y, values after '=' are replaced
by <V>, and each template is counted with its distinct values (up to 8).
Usage: python3 templates-fasm.py <fasm-file> [substring-filter]
Output: JSON with per-template counts and values.
"""
import json
import re
import sys
from collections import Counter, defaultdict

TILE_RE = re.compile(r"^(.+?)_X\d+Y\d+\.(.*)$")


def template(line):
    line = line.strip()
    if not line:
        return None, None
    if "=" in line:
        feat, val = line.split("=", 1)
        feat, val = feat.strip(), val.strip()
    else:
        feat, val = line, ""
    m = TILE_RE.match(feat)
    if not m:
        return "UNPARSED", val
    tile, rest = m.group(1), m.group(2)
    rest = re.sub(r"\[\d+:\d+\]", "[R]", rest)
    return tile + "_X/Y." + rest, val


def main():
    path = sys.argv[1]
    filt = sys.argv[2] if len(sys.argv) > 2 else ""
    counts = Counter()
    values = defaultdict(Counter)
    total = 0
    with open(path) as f:
        for line in f:
            t, v = template(line)
            if t is None:
                continue
            if filt and filt not in t:
                continue
            total += 1
            counts[t] += 1
            values[t][v] += 1
    out = []
    for t, c in counts.most_common():
        vals = values[t]
        out.append({
            "template": t,
            "count": c,
            "distinct_values": len(vals),
            "top_values": dict(vals.most_common(8)),
        })
    print(json.dumps({"file": path, "filter": filt, "total": total,
                      "distinct_templates": len(counts),
                      "templates": out}, indent=1))


if __name__ == "__main__":
    main()
