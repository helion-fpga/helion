# Generate the frozen golden FASM files and their placement metadata.
#
# The golden designs are the placed-and-routed targets for the diff:
#   blinky: 1 LUT (inverter) + 1 FF (toggle), clk in, led out
#   counter: 4 LUTs (4-bit incrementer) + 4 FFs, clk in, led out
# Sources: examples/blinky.sv and examples/counter.sv in the worktree.
#
# LUT INIT values are COMPUTED here from the boolean function of each
# output bit (input I0 is LUT input bit 0, I1 bit 1, ...). They are
# never hand-typed hex: the script is the derivation, and the tests
# recompute the same functions independently.
#
# Placement is hand-chosen and frozen: everything in SLICE_X0Y0 on the
# control part xcku3p (a real slice: row major 0, CLB column 0,
# tile type CLEL_R per the BitFiltrator xcku3p device file).
#
# INT and IO tile coordinates are PROVISIONAL (see the header written
# into every golden file): INT tile X is the logical CLB column index,
# not a verified Vivado tile X, because no xcku3p tilegrid exists in
# any public DB yet. The PIP and IO feature strings themselves are
# verified to exist in prjuray-db segbits_int.db / segbits_hpio_right.db
# (this script fails loud if one does not). The route is the frozen
# chosen target, not a Vivado-produced route; routability validation
# awaits the router worker plus the xcku3p tilegrid.
#
# xcvu3p goldens reuse the xcku3p placement verbatim and are flagged
# provisional in their meta: the xcvu3p device file does not exist yet,
# so no reference bit resolves for xcvu3p and every block reports
# blocked until the device-file worker lands it.
#
# Seed is a recorded constant. Placement hash is sha256 over the
# canonical placement record. A run whose recomputed hash differs from
# the meta is rejected, not compared.
#
# No em dashes anywhere.

import hashlib
import json
import os

from reference import Tier1Reference

HARNESS_DIR = os.path.dirname(os.path.abspath(__file__))
GOLDEN_DIR = os.path.join(HARNESS_DIR, "golden")

SEED = 20261107

# PIP features chosen as the frozen route. Each must exist in
# prjuray-db segbits_int.db (checked below).
BLINKY_PIPS = [
    "PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1",
    "PIP.INT_NODE_GLOBAL_0_INT_OUT0.GCLK_B_0_0",
]
COUNTER_PIPS = BLINKY_PIPS + [
    "PIP.IMUX_E0.INODE_E_5_FT1",
]

# IO features chosen as the frozen pinout. Each must exist in
# prjuray-db segbits_hpio_right.db (checked below).
CLK_IO = "IOB_X0Y0.IOSTANDARD_IN.LVCMOS12"
LED_IO = "IOB_X0Y1.IOSTANDARD_OUT.LVCMOS12_IDRIVE_I2_SLEW_SLEW_SLOW"


def lut_init_64(fn):
    """64-bit LUT INIT for a function of input bits (b0..b5)."""
    init = 0
    for i in range(64):
        bits = [(i >> k) & 1 for k in range(6)]
        if fn(bits):
            init |= 1 << i
    return init


# Boolean functions. b0 = I0 (LUT input 0), b1 = I1, ...
def blinky_inv(b):
    return 1 - b[0]


def cnt0(b):
    return 1 - b[0]


def cnt1(b):
    return b[1] ^ b[0]


def cnt2(b):
    return b[2] ^ (b[1] & b[0])


def cnt3(b):
    return b[3] ^ (b[2] & b[1] & b[0])


DESIGNS = {
    "blinky": {
        "luts": [("A6LUT", blinky_inv)],
        "ffs": ["AFF"],
    },
    "counter": {
        "luts": [
            ("A6LUT", cnt0),
            ("B6LUT", cnt1),
            ("C6LUT", cnt2),
            ("D6LUT", cnt3),
        ],
        "ffs": ["AFF", "BFF", "CFF", "DFF"],
    },
}

PIPS = {"blinky": BLINKY_PIPS, "counter": COUNTER_PIPS}

# All golden logic is placed in one slice; the tile is part of the
# frozen placement, not per-design data. The round-trip
# canonicalization (roundtrip.py) uses it to complete FASM: an FF
# with no INIT line in the input gets the default .V0 there.
LOGIC_TILE = "SLICE_X0Y0"


def design_ff_locations(design):
    """(tile, bel) for every FF in the design, from the golden definition.

    Raises KeyError for an unknown design; the caller maps it to
    FasmError so the round-trip stage reports blocked, never a
    traceback.
    """
    return [(LOGIC_TILE, bel) for bel in DESIGNS[design]["ffs"]]

HEADER = """# golden FASM: {design} on {part}
# seed: {seed}
# source: examples/{design}.sv in the worktree (blinky: registered
# inverter, led = q; counter: 4-bit incrementer, led = cnt[3]).
# placement: all logic in SLICE_X0Y0 (xcku3p: row major 0, CLB column 0,
# tile type CLEL_R per the BitFiltrator xcku3p device file).
# PROVISIONAL: INT and HPIO tile coordinates below use the logical CLB
# column index as the tile X, NOT a verified Vivado tile X. No xcku3p
# tilegrid exists in any public DB yet. The PIP and IO feature strings
# are verified to exist in prjuray-db segbits_int.db and
# segbits_hpio_right.db. The route is the frozen chosen target, not a
# Vivado-produced route. The harness resolves INT PIPs with the
# xczu3eg-derived rule INT column major = CLB column major - 1,
# flagged absolute-provisional in every record.
"""


def build_fasm(design, part):
    spec = DESIGNS[design]
    lines = [HEADER.format(design=design, part=part, seed=SEED).rstrip()]
    for bel, fn in spec["luts"]:
        init = lut_init_64(fn)
        lines.append(
            "SLICE_X0Y0.%s.INIT[63:0] = 0x%016x" % (bel, init)
        )
    for bel in spec["ffs"]:
        lines.append("SLICE_X0Y0.%s.INIT = 0" % bel)
        lines.append("SLICE_X0Y0.%s.SRVAL = 0" % bel)
    for pip in PIPS[design]:
        lines.append("INT_X0Y0.%s" % pip)
    lines.append("HPIO_RIGHT_X0Y0.%s" % CLK_IO)
    lines.append("HPIO_RIGHT_X0Y0.%s" % LED_IO)
    return "\n".join(lines) + "\n"


def placement_record(design, part):
    spec = DESIGNS[design]
    sites = []
    for bel, fn in spec["luts"]:
        sites.append({"tile": "SLICE_X0Y0", "bel": bel, "role": "lut"})
    for bel in spec["ffs"]:
        sites.append({"tile": "SLICE_X0Y0", "bel": bel, "role": "ff"})
    provisional_tiles = ["INT_X0Y0", "HPIO_RIGHT_X0Y0"]
    return {
        "design": design,
        "part": part,
        "seed": SEED,
        "sites": sites,
        "provisional_tiles": provisional_tiles,
        "notes": (
            "INT/HPIO tile X is the logical CLB column index, not a "
            "verified Vivado tile X (no xcku3p tilegrid in any public DB). "
            "xcvu3p placement reuses the xcku3p coordinates verbatim and "
            "is provisional until the xcvu3p device file lands."
            if part == "xcvu3p" else
            "INT/HPIO tile X is the logical CLB column index, not a "
            "verified Vivado tile X (no xcku3p tilegrid in any public DB)."
        ),
    }


def placement_hash(record, fasm_text):
    """sha256 over the canonical placement record plus the canonical FASM.

    Binding the hash to the FASM text is what makes a tampered golden
    detectable: editing either the placement or any feature line changes
    the hash and the run is rejected, not compared.
    """
    from fasm import canonical_fasm

    canon = json.dumps(record, sort_keys=True, separators=(",", ":"))
    canon += "\n" + canonical_fasm(fasm_text)
    return hashlib.sha256(canon.encode()).hexdigest()


def verify_features_exist(ref):
    """Fail loud if a golden feature string is not in its segbits DB."""
    int_db = ref.segbits("int")
    for design, pips in PIPS.items():
        for pip in pips:
            key = "INT." + pip
            if key not in int_db:
                raise SystemExit("golden PIP %s not in segbits_int.db" % key)
    hpio_db = ref.segbits("hpio_right")
    for feat in (CLK_IO, LED_IO):
        key = "HPIO_RIGHT." + feat
        if key not in hpio_db:
            raise SystemExit("golden IO %s not in segbits_hpio_right.db" % key)


def main():
    ref = Tier1Reference()
    verify_features_exist(ref)
    os.makedirs(GOLDEN_DIR, exist_ok=True)
    for design in ("blinky", "counter"):
        for part in ("xcku3p", "xcvu3p"):
            fasm = build_fasm(design, part)
            record = placement_record(design, part)
            phash = placement_hash(record, fasm)
            meta = dict(record)
            meta["placement_hash"] = phash
            meta["fasm_sha256"] = hashlib.sha256(fasm.encode()).hexdigest()
            fasm_path = os.path.join(GOLDEN_DIR, "%s.%s.fasm" % (design, part))
            meta_path = os.path.join(GOLDEN_DIR, "%s.%s.meta.json" % (design, part))
            with open(fasm_path, "w") as f:
                f.write(fasm)
            with open(meta_path, "w") as f:
                json.dump(meta, f, indent=2, sort_keys=True)
                f.write("\n")
            print("%s %s: placement_hash %s" % (design, part, phash))


if __name__ == "__main__":
    main()
