"""Zedboard pin assignment for designs without a board XDC (micro tests,
generated designs). Board-meaningful pins first, so a design built this way
can also go on the real board: clock on the 100 MHz oscillator (Y9), inputs
on slide switches / push buttons / PMOD JA-JB, outputs on the LEDs /
PMOD JC-JD; remaining bits take any other PL pin of banks 13/33/34/35.
"""
from __future__ import annotations

import csv
import os

CLOCK = "Y9"
SWITCHES = ["F22", "G22", "H22", "F21", "H19", "H18", "H17", "M15"]
BUTTONS = ["P16", "R16", "N15", "R18", "T18"]           # BTNC, BTND, BTNL, BTNR, BTNU
LEDS = ["T22", "T21", "U22", "U21", "V22", "W22", "U19", "U14"]
PMOD_JA = ["Y11", "AA11", "Y10", "AA9", "AB11", "AB10", "AB9", "AA8"]
PMOD_JB = ["W12", "W11", "V10", "W8", "V12", "W10", "V9", "V8"]
PMOD_JC = ["AB7", "AB6", "Y4", "AA4", "R6", "T6", "T4", "U4"]
PMOD_JD = ["V7", "W7", "V5", "V4", "W6", "W5", "U6", "U5"]

DB = os.path.expanduser(os.environ.get("X7V_DB", "~/.cache/helion/prjxray-db/zynq7"))


def _pl_pins() -> list[str]:
    path = os.path.join(DB, "xc7z020clg484-1", "package_pins.csv")
    out = []
    with open(path) as f:
        for row in csv.DictReader(f):
            if row["bank"] in ("13", "33", "34", "35") and row["site"].startswith("IOB"):
                out.append(row["pin"])
    return out


def assign(ports) -> tuple[dict[str, str], str | None]:
    """ports: cosim.Port list. Returns ({bit name: package pin}, clock port)."""
    import re
    used: set[str] = set()
    pins: dict[str, str] = {}
    clock = None
    for p in ports:
        if p.direction == "in" and p.width is None and re.fullmatch(r"(clk|clock|gclk|sysclk|clk_i)", p.name, re.I):
            clock = p.name
            pins[p.name] = CLOCK
            used.add(CLOCK)
            break
    rest = [x for x in _pl_pins() if x not in used]
    in_pool = SWITCHES + BUTTONS + PMOD_JA + PMOD_JB
    out_pool = LEDS + PMOD_JC + PMOD_JD
    in_pool = [x for x in in_pool if x not in used]
    out_pool = [x for x in out_pool if x not in used]

    def take(pool: list[str]) -> str:
        for x in pool + rest:
            if x not in used:
                used.add(x)
                return x
        raise RuntimeError("out of package pins")

    for p in ports:
        for b in p.bits():
            if b in pins:
                continue
            pins[b] = take(in_pool if p.direction == "in" else out_pool)
    return pins, clock


def write_xdc(ports, path: str) -> dict[str, str]:
    pins, clock = assign(ports)
    with open(path, "w") as f:
        for b, pin in pins.items():
            f.write(f"set_property -dict {{ PACKAGE_PIN {pin} IOSTANDARD LVCMOS33 }} [get_ports {{{b}}}]\n")
        if clock:
            f.write(f"create_clock -period 10.000 -name sys_clk [get_ports {clock}]\n")
    return pins
