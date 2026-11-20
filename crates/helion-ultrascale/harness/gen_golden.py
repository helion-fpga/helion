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
    # uram: one URAM288 on URAM_X0Y0, six UG974 attributes at their
    # defaults. No FFs, no PIPs, no IO: the minimal URAM exercise.
    # The "luts"/"ffs" shape is shared with the other designs so
    # design_ff_locations works (ffs is empty); the attribute lines
    # live in build_uram_fasm, not in the boolean-function table (a
    # memory has no boolean function). There is deliberately NO
    # content line: URAM288 has no INIT attributes in UG974, so no
    # content value can be expressed in FASM until job 5 lands the
    # encoding. Writing an INIT line would invent an attribute.
    "uram": {
        "luts": [],
        "ffs": [],
    },
    # lutram: one RAM64X1S (64x1 single-port distributed RAM) on
    # SLICE_X1Y0 ALUT. No FFs, no PIPs, no IO: the minimal LUTRAM
    # exercise. The "luts"/"ffs" shape is shared with the other
    # designs so design_ff_locations works (ffs is empty); the tile,
    # INIT value, and mode lines live in build_lutram_fasm, not in
    # the boolean-function table (a RAM has no boolean function).
    "lutram": {
        "luts": [],
        "ffs": [],
    },
}

# URAM golden placement and attributes. URAM_X0Y0 is PROVISIONAL:
# no xcku3p tilegrid exists in any public DB, so no URAM tile
# coordinate is verified; the device file gives only tile types
# (URAM_URAM_FT / URAM_URAM_DELAY_FT, site type URAM288). The tile
# spelling follows the harness's existing URAM tile convention
# (test_dsp_uram_have_no_encoding uses URAM_X0Y0). Job 5 resolves
# the real site with get_sites at run time and does not depend on
# this spelling. The six attribute names and defaults are UG974
# 2023.1, verified against the URAM288 page 2026-11-19; see
# data/URAM288_SEARCH.md.
URAM_TILE = "URAM_X0Y0"
URAM_ATTRS = [
    ("CASCADE_ORDER_A", '"NONE"'),
    ("CASCADE_ORDER_B", '"NONE"'),
    ("OREG_A", '"FALSE"'),
    ("OREG_B", '"FALSE"'),
    ("EN_ECC_RD_A", '"FALSE"'),
    ("EN_ECC_RD_B", '"FALSE"'),
]

URAM_HEADER = """# golden FASM: uram on {part}
# seed: {seed}
# source: one URAM288 (288K-bit UltraRAM, 4096 words x 72 bits) on
# URAM_X0Y0, six UG974 attributes at defaults.
# placement: URAM_X0Y0 is PROVISIONAL (no xcku3p tilegrid in any
# public DB; the device file names only tile types URAM_URAM_FT /
# URAM_URAM_DELAY_FT with site type URAM288). One URAM288 only: no
# FF, no route, no IO. The design is the minimal URAM exercise, not
# a full system; routing of address/data/clock is the router
# worker's scope, and the content sweep is job 5's scope.
# content: NO content line is written. URAM288 has no INIT
# attributes in UG974 (contrast RAMB36E2 INIT_00..INIT_7F), so the
# 294912 content bits cannot be expressed in FASM until job 5 lands
# the encoding. The six lines below are the whole golden; every one
# resolves to blocked until then.
# PROVISIONAL: the xcvu3p golden reuses the xcku3p placement verbatim
# and is provisional until the xcvu3p device file lands.
"""

# LUTRAM golden placement and INIT. SLICE_X1Y0 is xcku3p row major 0,
# CLB column 1, tile type CLEM per the BitFiltrator xcku3p device file
# (column 1 is the first CLEM column). The INIT value is the XOR
# pattern 0x6996966996696996 (32 bits set), the same pattern the
# crate's bitloc tests hand-check.
LUTRAM_TILE = "SLICE_X1Y0"
LUTRAM_INIT = 0x6996966996696996

LUTRAM_HEADER = """# golden FASM: lutram on {part}
# seed: {seed}
# source: RAM64X1S (64x1 single-port distributed RAM) on SLICE_X1Y0 ALUT.
# placement: SLICE_X1Y0 (xcku3p: row major 0, CLB column 1, tile type CLEM
# per the BitFiltrator xcku3p device file). One RAM64X1S only: no FF, no
# route, no IO. The design is the minimal LUTRAM exercise, not a full
# system; routing of address/data/clock is the router worker's scope.
# derivation: the LUTRAM content bits ARE the LUT INIT bits (same
# encoding), proven three ways, see docs/ultra-bitgen-crew/subproblems/
# US-04-lutram-table.md: (1) BitFiltrator's own extraction code states
# the INIT bits "ended up having the same encoding for LUTRAMs and
# standard LUTs when we extracted the data" (extract_encoding_from_
# logic_loc.py); its pipeline extracts CLEM LutLoc from Vivado
# logic-location files of all-LUTRAM columns and CLEL LutLoc from a LUT
# INIT fuzzing sweep. (2) The vendored tables are byte-identical across
# all four CLB tile types (0 differing pairs of 30,720 per pair).
# (3) Every prjuray xLUT.INIT[i] line equals the LutLoc Y0 pair (512 of
# 512 per tile type). The mode lines below resolve through the vendored
# segbits_clem.db: ALUT.MODE.RAM, ALUT.DI1.DI1, ALUT.SIZE.LARGE,
# WA7/8/9USED.V0. Their SELECTION for a 64x1S follows the f4pga Project
# X-Ray DRAM documentation (7-series feature family, same concepts under
# UltraScale+ spellings): 64x1S needs xLUT.RAM plus the data-in mux to
# the LUT's own input; SMALL only for 32-bit use; WA7/8USED only for
# 128x1/256x1 use. That selection is a documented cross-family
# inference, not a Vivado-verified UltraScale+ fact: tier 2 (a Vivado
# bitstream diff) remains its arbiter. The bit LOCATIONS are proven.
# PROVISIONAL: the xcvu3p golden reuses the xcku3p placement verbatim and
# is provisional until the xcvu3p device file lands.
"""

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

    A design entry may carry an explicit "ff_locations" list of
    (tile, bel) pairs (US-07 generated designs, which place FFs on
    several slices); entries without it keep the frozen single-slice
    behaviour (all FFs on SLICE_X0Y0).
    """
    entry = DESIGNS[design]
    if "ff_locations" in entry:
        return list(entry["ff_locations"])
    return [(LOGIC_TILE, bel) for bel in entry["ffs"]]

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


def build_uram_fasm(design, part):
    """FASM for the uram golden: one URAM288 on URAM_X0Y0.

    The line spellings must match the Rust `is_uram288_entry`
    detector's expectations (tile or feature names URAM); the
    attribute names and defaults are UG974 2023.1, verified against
    the URAM288 page. There is no content line by design: URAM288
    has no INIT attribute to write.
    """
    lines = [URAM_HEADER.format(design=design, part=part, seed=SEED).rstrip()]
    for name, value in URAM_ATTRS:
        lines.append('%s.URAM288.%s = %s' % (URAM_TILE, name, value))
    return "\n".join(lines) + "\n"


def build_lutram_fasm(design, part):
    """FASM for the lutram golden: one RAM64X1S on SLICE_X1Y0 ALUT.

    The mode lines match the Rust `lutram_fasm` /
    `lutram_slice_fasm` output exactly; the INIT line matches after
    roundtrip normalization only (the Rust emitter renders Verilog
    `64'h...`, the golden uses the harness `0x...` spelling, same
    value; see roundtrip.py). The duplication is deliberate, the two
    implementations are in different languages and the tests pin
    them together.
    """
    lines = [LUTRAM_HEADER.format(design=design, part=part, seed=SEED).rstrip()]
    lines.append(
        "%s.A6LUT.INIT[63:0] = 0x%016x" % (LUTRAM_TILE, LUTRAM_INIT)
    )
    lines.append("%s.ALUT.MODE.RAM" % LUTRAM_TILE)
    lines.append("%s.ALUT.DI1.DI1" % LUTRAM_TILE)
    lines.append("%s.ALUT.SIZE.LARGE" % LUTRAM_TILE)
    lines.append("%s.WA7USED.V0" % LUTRAM_TILE)
    lines.append("%s.WA8USED.V0" % LUTRAM_TILE)
    lines.append("%s.WA9USED.V0" % LUTRAM_TILE)
    return "\n".join(lines) + "\n"


def build_fasm(design, part):
    if design == "uram":
        return build_uram_fasm(design, part)
    if design == "lutram":
        return build_lutram_fasm(design, part)
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
    if design == "uram":
        return {
            "design": design,
            "part": part,
            "seed": SEED,
            "sites": [{"tile": URAM_TILE, "bel": "URAM288", "role": "uram"}],
            "provisional_tiles": [URAM_TILE],
            "notes": (
                "One URAM288 on URAM_X0Y0 (provisional tile: no xcku3p "
                "tilegrid in any public DB; tile types URAM_URAM_FT / "
                "URAM_URAM_DELAY_FT per the BitFiltrator device file). "
                "No INT/HPIO tiles. xcvu3p placement reuses the xcku3p "
                "coordinates verbatim and is provisional until the "
                "xcvu3p device file lands."
                if part == "xcvu3p" else
                "One URAM288 on URAM_X0Y0 (provisional tile: no xcku3p "
                "tilegrid in any public DB; tile types URAM_URAM_FT / "
                "URAM_URAM_DELAY_FT per the BitFiltrator device file). "
                "No INT/HPIO tiles."
            ),
        }
    if design == "lutram":
        return {
            "design": design,
            "part": part,
            "seed": SEED,
            "sites": [{"tile": LUTRAM_TILE, "bel": "A6LUT", "role": "lutram"}],
            "provisional_tiles": [],
            "notes": (
                "One RAM64X1S on SLICE_X1Y0 ALUT (CLEM tile). No INT/HPIO tiles. "
                "xcvu3p placement reuses the xcku3p coordinates verbatim and "
                "is provisional until the xcvu3p device file lands."
                if part == "xcvu3p" else
                "One RAM64X1S on SLICE_X1Y0 ALUT (CLEM tile). No INT/HPIO tiles."
            ),
        }
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
    clem_db = ref.segbits("clem")
    for feat in (
        "ALUT.MODE.RAM",
        "ALUT.DI1.DI1",
        "ALUT.SIZE.LARGE",
        "WA7USED.V0",
        "WA8USED.V0",
        "WA9USED.V0",
    ):
        key = "CLEM." + feat
        if key not in clem_db:
            raise SystemExit("golden LUTRAM feature %s not in segbits_clem.db" % key)


def main():
    ref = Tier1Reference()
    verify_features_exist(ref)
    os.makedirs(GOLDEN_DIR, exist_ok=True)
    for design in ("blinky", "counter", "uram", "lutram"):
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
