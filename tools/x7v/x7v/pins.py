"""Pin assignment for designs without a board XDC (micro tests, generated
designs), for any board in boards/boards.json (ZedBoard by default).
Board-meaningful pins first, so a design built this way can also go on the
real board: clock on the board oscillator, inputs on slide switches, push
buttons and the input Pmods, outputs on the LEDs and the output Pmods;
remaining bits take any other pin of the board's 3.3 V user banks.
"""
from __future__ import annotations

import csv
import json
import os

BOARDS_JSON = os.environ.get(
    "X7V_BOARDS", os.path.join(os.path.dirname(__file__), "..", "..", "..", "boards", "boards.json"))
DEFAULT_BOARD = "zedboard"


def boards() -> dict:
    return json.load(open(BOARDS_JSON))


def board(name: str | None) -> dict:
    all_ = boards()
    key = (name or DEFAULT_BOARD).lower()
    aliases = {"zed": "zedboard", "arty": "arty-a7-35", "arty-a7": "arty-a7-35", "basys-3": "basys3",
               "nexys-a7": "nexys-a7-100t", "nexys4-ddr": "nexys-a7-100t"}
    key = aliases.get(key, key)
    if key not in all_:
        raise ValueError(f"unknown board {name} (known: {', '.join(all_)})")
    b = dict(all_[key])
    b["name"] = key
    return b


def _spare_pins(db, b: dict) -> list[str]:
    out = []
    with open(os.path.join(db.root, db.package, "package_pins.csv")) as f:
        for row in csv.DictReader(f):
            if row["bank"] in b["spare_banks"] and row["site"].startswith("IOB"):
                out.append(row["pin"])
    return out


def assign(ports, db=None, board_name: str | None = None) -> tuple[dict[str, str], str | None]:
    """ports: cosim.Port list. Returns ({bit name: package pin}, clock port)."""
    import re
    from .db import Db
    b = board(board_name)
    db = db or Db.for_part(b["part"])
    used: set[str] = set()
    pins: dict[str, str] = {}
    clock = None
    for p in ports:
        if p.direction == "in" and p.width is None and re.fullmatch(r"(clk|clock|gclk|sysclk|clk_i)", p.name, re.I):
            clock = p.name
            pins[p.name] = b["clock"]
            used.add(b["clock"])
            break
    rest = [x for x in _spare_pins(db, b) if x not in used]
    in_pool = b["switches"] + b["buttons"] + [x for v in b["in_pmods"].values() for x in v]
    out_pool = b["leds"] + b.get("rgb_leds", []) + [x for v in b["out_pmods"].values() for x in v]
    in_pool = [x for x in in_pool if x not in used]
    out_pool = [x for x in out_pool if x not in used]

    def take(pool: list[str]) -> str:
        for x in pool + rest:
            if x not in used:
                used.add(x)
                return x
        raise RuntimeError("out of package pins")

    for p in ports:
        for bit in p.bits():
            if bit in pins:
                continue
            pins[bit] = take(in_pool if p.direction == "in" else out_pool)
    return pins, clock


def write_xdc(ports, path: str, db=None, board_name: str | None = None) -> dict[str, str]:
    pins, clock = assign(ports, db, board_name)
    with open(path, "w") as f:
        for bit, pin in pins.items():
            f.write(f"set_property -dict {{ PACKAGE_PIN {pin} IOSTANDARD LVCMOS33 }} [get_ports {{{bit}}}]\n")
        if clock:
            f.write(f"create_clock -period 10.000 -name sys_clk [get_ports {clock}]\n")
    return pins
