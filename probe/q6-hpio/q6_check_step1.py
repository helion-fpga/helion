#!/usr/bin/env python3
"""Q6 step 1 verdict checker: read the HPIO inventory, print the verdict.

Usage:

  python3 q6_check_step1.py q6-step1-hpio-inventory.txt

Prints the HPIO_RIGHT / HPIO_L counts, the row-0 HPIO tiles with
their columns, rows, clock regions, and IOB site counts, and the
verdict that decides the next step:

- HPIO_RIGHT present: step 2 can run; the golden's tile type exists.
- No HPIO_RIGHT, HPIO_L present: STOP, escalate to the CTO. The
  golden's tile type does not exist on this part, segbits_hpio_right.db
  does not apply to the real IO tiles, and no public HPIO_L encoding
  exists. Do not run step 2 against HPIO_L tiles.
- Neither present: STOP, the inventory is incomplete or the part
  string is wrong; re-run step 1 and check the part string.

Exit code 0 when HPIO_RIGHT is present, 1 otherwise (including a
malformed inventory, which fails loud instead of printing a verdict).

No em dashes anywhere.
"""

import sys


def parse_inventory(path):
    """Parse a q6 step 1 inventory file.

    Returns (part, exact_counts, tiles) where exact_counts maps tile
    type to count and tiles is a list of dicts.
    """
    part = None
    exact_counts = {}
    tiles = []
    by_name = {}
    with open(path) as f:
        for raw in f:
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            fields = line.split()
            if fields[0] == "PART":
                part = fields[1]
            elif fields[0] == "COUNT_EXACT":
                exact_counts[fields[1]] = int(fields[2])
            elif fields[0] == "TILE":
                tile = {
                    "name": fields[1],
                    "tile_type": fields[2],
                    "column": int(fields[3]),
                    "row": int(fields[4]),
                    "clock_region": fields[5],
                    "site_count": int(fields[6]),
                    "sites": [],
                }
                tiles.append(tile)
                by_name[tile["name"]] = tile
            elif fields[0] == "SITE":
                tile = by_name.get(fields[1])
                if tile is None:
                    raise ValueError("SITE for unknown tile %s" % fields[1])
                tile["sites"].append(
                    {"name": fields[2], "site_type": fields[3], "pin": fields[4]}
                )
    if part is None:
        raise ValueError("inventory has no PART line: %s" % path)
    return part, exact_counts, tiles


def verdict(part, exact_counts, tiles):
    """Return (exit_code, lines) for the inventory."""
    lines = ["part: %s" % part]
    for key in ("HPIO_RIGHT", "HPIO_L", "HDIO_TOP_RIGHT", "HDIO_BOT_RIGHT",
                "CMT_RIGHT", "CMT_L"):
        lines.append("%s: %d" % (key, exact_counts.get(key, 0)))
    hpio_right = exact_counts.get("HPIO_RIGHT", 0)
    hpio_l = exact_counts.get("HPIO_L", 0)
    row0 = [t for t in tiles
            if t["tile_type"].startswith("HPIO") and t["clock_region"].endswith("Y0")]
    row0.sort(key=lambda t: t["name"])
    lines.append("row-0 HPIO tiles: %d" % len(row0))
    for tile in row0:
        iobs = [s for s in tile["sites"] if s["name"].startswith("IOB_")]
        lines.append(
            "  %s type %s column %d row %d region %s IOB sites %d"
            % (tile["name"], tile["tile_type"], tile["column"], tile["row"],
               tile["clock_region"], len(iobs))
        )
    if hpio_right > 0:
        lines.append("VERDICT: HPIO_RIGHT EXISTS on %s; run step 2." % part)
        return 0, lines
    if hpio_l > 0:
        lines.append(
            "VERDICT: STOP. No HPIO_RIGHT on %s, %d HPIO_L tiles instead. "
            "The golden tile type does not exist on this part; escalate "
            "to the CTO. Do not run step 2." % (part, hpio_l)
        )
        return 1, lines
    lines.append(
        "VERDICT: STOP. No HPIO tile of either type in the inventory; "
        "the inventory is incomplete or the part string is wrong."
    )
    return 1, lines


def main(argv):
    if len(argv) != 2:
        print("usage: python3 q6_check_step1.py <q6-step1-hpio-inventory.txt>")
        return 2
    try:
        part, exact_counts, tiles = parse_inventory(argv[1])
    except (OSError, ValueError, IndexError) as exc:
        print("inventory unreadable, no verdict: %s" % exc)
        return 1
    code, lines = verdict(part, exact_counts, tiles)
    print("\n".join(lines))
    return code


if __name__ == "__main__":
    sys.exit(main(sys.argv))
