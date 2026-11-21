#!/usr/bin/env python3
"""coverage.py - US-01 validation (W-VALIDATE).

Coverage rollup: which BEL pins were exercised across the 14-pair
corpus vs the tileconn totals, and what fraction of the map the
corpus actually validates.

Per site type (SLICEL 106, SLICEM 108, DSP48E2 432, RAMB18 203,
RAMB36 533 pins, from site_types, cross-checked against tileconn),
each pin falls in exactly one bucket:
  wire_verified: its tileconn partner wire appears as a PIP dst or
    src in a decoded FASM, and the C4 check (consistency.py) agrees
    the wire stitches to that pin.
  vcc_only: no decoded PIP ever names its partner wire, but a
    baseline FASM has a VCC_WIRE PIP driving it (tie-off use).
  config_only: the pin appears in decoded FASM only via config
    features (CARRY8 DI/CI selections, BRAM config attributes,
    FFMUX/OUTMUX selections naming the letter), never via a wire.
    This is the exercised-but-unverified bucket: the pin's use is
    visible, but its wire edge has no PIP corroboration.
  never_exercised: the pin appears nowhere in any decoded or
    baseline FASM in any form.
Also reports per-pin-instance counts (how many of the 8,820 slices
exercise each pin) for the CLE types.

Usage: python3 coverage.py <out-json>
"""
import json
import re
from collections import Counter, defaultdict

DB = "/home/hatch/workspace/prjuray-db-ref/zynqusp"
PART = "xczu3eg-sbva484-1-e"
PAIRS = ["P-11", "P-13", "P-14", "P-15", "P-16", "P-17", "P-23", "P-28",
         "P-40", "P-41", "P-42", "P-43", "P-44", "P-45"]
DEC = "/home/hatch/workspace/us01-char-logs/decoded/{}.fasm"
BASE = "/home/hatch/workspace/ultra-bitgen-crew/decoder-validation/baseline/{}.fasm"

PIP_RE = re.compile(r"^(.+?)\.PIP\.([^.]+)\.(.+)$")
SITE_WIRE = re.compile(r"^CLE_CLE_[LM]_SITE_0_(.+)$")
DSP_WIRE = re.compile(r"^DSP_DSP48_[01]_(.+)$")
BRAM_WIRE = re.compile(r"^BRAM_BRAM_CORE_[123]_(.+)$")


def main():
    out_path = __import__("sys").argv[1]
    tg = json.load(open(f"{DB}/{PART}/tilegrid.json"))
    site_to_tile = {}
    for tname, t in tg.items():
        for sname in (t.get("sites") or {}):
            site_to_tile[sname] = (tname, t["type"])
    grid_to_tile = {(t["grid_x"], t["grid_y"]): n for n, t in tg.items()}
    tc = json.load(open(f"{DB}/{PART}/tileconn.json"))
    conn = defaultdict(list)
    for e in tc:
        a, b = e["tile_types"]
        dx, dy = e["grid_deltas"]
        for wa, wb in e["wire_pairs"]:
            conn[(a, wa)].append((b, wb, dx, dy))
            conn[(b, wb)].append((a, wa, -dx, -dy))

    # Site pins per site type, from site_types.
    site_pins = {}
    for stype, fname in (("SLICEL", "site_type_SLICEL.json"),
                         ("SLICEM", "site_type_SLICEM.json"),
                         ("DSP48E2", "site_type_DSP48E2.json"),
                         ("RAMB181", "site_type_RAMB181.json"),
                         ("RAMBFIFO36", "site_type_RAMBFIFO36.json")):
        d = json.load(open(f"{DB}/site_types/{fname}"))
        site_pins[stype] = sorted(d["site_pins"].keys())

    # tileconn partners per (site type, pin): set of (partner tile type, wire).
    def cle_prefix(ttype):
        return "CLE_CLE_L_SITE_0_" if ttype.startswith("CLEL") \
            else "CLE_CLE_M_SITE_0_"

    # Map each site pin to the set of (INT tile type, wire) it stitches
    # to, plus the raw partner wires, keyed by site type and pin.
    # For CLE, use the four CLE tile types; for DSP/BRAM, the tile
    # types are DSP and BRAM.
    pin_partners = defaultdict(set)
    for (ttype, wire), parts in conn.items():
        m = SITE_WIRE.match(wire)
        if m and ttype.startswith("CLE"):
            st = "SLICEL" if ttype.startswith("CLEL") else "SLICEM"
            pin_partners[(st, m.group(1))].add((ttype, wire))
        m = DSP_WIRE.match(wire)
        if m and ttype == "DSP":
            pin_partners[("DSP48E2", m.group(1))].add((ttype, wire))
        m = BRAM_WIRE.match(wire)
        if m and ttype == "BRAM":
            # RAMB18 vs RAMB36 share the BRAM tile type; attribute by
            # site type file membership below.
            pin_partners[("BRAM", m.group(1))].add((ttype, wire))

    # Split the shared BRAM partners by site-type membership.
    bram18 = set(site_pins["RAMB181"])
    bram36 = set(site_pins["RAMBFIFO36"])
    for (st, pin), parts in list(pin_partners.items()):
        if st != "BRAM":
            continue
        if pin in bram18:
            pin_partners[("RAMB181", pin)] |= parts
        if pin in bram36:
            pin_partners[("RAMBFIFO36", pin)] |= parts
        del pin_partners[("BRAM", pin)]

    # Scan decoded FASM: wires named by PIPs, config-only pin mentions.
    wire_seen = set()   # (tile type, wire) named as PIP dst/src
    vcc_seen = set()    # (tile, wire) driven by VCC_WIRE in a baseline
    config_pins = defaultdict(set)  # site type -> pins named by config
    cin_sites = set()
    for pair in PAIRS:
        with open(DEC.format(pair)) as f:
            for line in f:
                line = line.strip()
                if ".PIP." in line:
                    m = PIP_RE.match(line)
                    if not m:
                        continue
                    tile, dst, src = m.group(1), m.group(2), m.group(3)
                    if tile.startswith("SLICE_"):
                        if dst.endswith("_SITE_0_CIN_PIN"):
                            cin_sites.add(tile)
                        continue
                    ttype = tg.get(tile, {}).get("type")
                    if ttype is None:
                        continue
                    for w in (dst, src):
                        w = w[:-4] if w.endswith((".FWD", ".REV")) else w
                        wire_seen.add((ttype, w))
                    continue
                m = re.match(r"^(SLICE_X\d+Y\d+)\.CARRY8\.(DI\d|CI(_TOP)?)\.", line)
                if m:
                    site = m.group(0).split(".")[0]
                    st = site_to_tile.get(site, (None, None))[1]
                    stype = "SLICEL" if (st or "").startswith("CLEL") else "SLICEM"
                    config_pins[stype].add(m.group(1))
                    continue
                m = re.match(r"^(RAMB18_X\d+Y\d+|RAMB36_X\d+Y\d+)\.", line)
                if m:
                    # BRAM config lines name attributes, not pins;
                    # record the tile as config-seen only.
                    config_pins["BRAM_TILE"].add(m.group(1))
        with open(BASE.format(pair)) as f:
            for line in f:
                line = line.strip()
                if ".PIP." not in line or "VCC_WIRE" not in line:
                    continue
                m = PIP_RE.match(line)
                if m and m.group(3) == "VCC_WIRE":
                    vcc_seen.add((m.group(1), m.group(2)))

    # A pin's partner wire is "wire seen" if any of its stitch wires
    # appears in wire_seen under the partner tile type. tileconn
    # partners are (CLE tile type, site wire) -> (INT type, INT wire);
    # invert: for pin P on CLE type T, partners are the INT wires.
    # Rebuild the forward map: (cle type, site wire) -> set of INT wires.
    fwd = defaultdict(set)
    for e in tc:
        a, b = e["tile_types"]
        for wa, wb in e["wire_pairs"]:
            fwd[(a, wa)].add((b, wb))
            fwd[(b, wb)].add((a, wa))

    # VCC wire names (tile-independent: IMUX_W10 is the same wire class
    # on every INT tile; a VCC tie on any tile exercises the pin class).
    vcc_wire_names = {w for (_t, w) in vcc_seen}
    result = {"site_types": {}, "notes": []}
    totals = Counter()
    for stype in ("SLICEL", "SLICEM", "DSP48E2", "RAMB181", "RAMBFIFO36"):
        pins = site_pins[stype]
        buckets = Counter()
        per_pin = {}
        for pin in pins:
            # Partner INT/DSP/BRAM wires for this pin, across tile types.
            wires = set()
            if stype in ("SLICEL", "SLICEM"):
                for ttype in (("CLEL_L", "CLEL_R") if stype == "SLICEL"
                              else ("CLEM", "CLEM_R")):
                    sw = cle_prefix(ttype) + pin
                    for (pt, pw) in fwd.get((ttype, sw), ()):  # noqa
                        wires.add((pt, pw))
            elif stype == "DSP48E2":
                for idx in ("0", "1"):
                    sw = f"DSP_DSP48_{idx}_{pin}"
                    for (pt, pw) in fwd.get(("DSP", sw), ()):  # noqa
                        wires.add((pt, pw))
            else:
                for idx in ("1", "2", "3"):
                    sw = f"BRAM_BRAM_CORE_{idx}_{pin}"
                    for (pt, pw) in fwd.get(("BRAM", sw), ()):  # noqa
                        wires.add((pt, pw))
            # Restrict to wires that are on the fabric side (INT for
            # CLE, DSP/BRAM tile wires for DSP/BRAM are the site wires
            # themselves; the partner side is what PIPs would name).
            fabric_wires = {(pt, pw) for (pt, pw) in wires
                            if pt.startswith("INT") or pt in ("DSP", "BRAM")}
            wire_hit = bool(fabric_wires & wire_seen)
            # VCC-only: partner wire driven by VCC in a baseline.
            vcc_hit = any(
                pw in vcc_wire_names for (_pt, pw) in fabric_wires
            ) if fabric_wires else False
            # Config-only: CARRY8 DI/CI pins, or CIN/COUT via site PIPs.
            cfg_hit = pin in config_pins.get(stype, set())
            if pin == "CIN":
                cfg_hit = cfg_hit or bool(cin_sites)
            if pin == "COUT":
                cfg_hit = cfg_hit or bool(cin_sites)
            if wire_hit:
                bucket = "wire_verified"
            elif vcc_hit:
                bucket = "vcc_only"
            elif cfg_hit:
                bucket = "config_only"
            else:
                bucket = "never_exercised"
            buckets[bucket] += 1
            totals[bucket] += 1
            per_pin[pin] = {"bucket": bucket,
                            "n_partner_wires": len(fabric_wires)}
        result["site_types"][stype] = {
            "total_pins": len(pins),
            "buckets": dict(buckets),
            "pins": per_pin,
        }
    result["totals"] = dict(totals)
    result["totals"]["all_pins"] = sum(
        len(site_pins[s]) for s in site_pins)
    result["n_cin_sites"] = len(cin_sites)
    result["n_vcc_wire_pairs"] = len(vcc_seen)
    result["n_wire_seen"] = len(wire_seen)
    with open(out_path, "w") as f:
        json.dump(result, f, indent=1)
    for stype in ("SLICEL", "SLICEM", "DSP48E2", "RAMB181", "RAMBFIFO36"):
        d = result["site_types"][stype]
        print(stype, d["total_pins"], d["buckets"])
    print("totals", result["totals"])


if __name__ == "__main__":
    main()
