#!/usr/bin/env python3
# Single-feature sweep: every tile instance x every feature from the
# public DBs, emitted one feature at a time (packed onto disjoint
# bit positions so one emitter run covers many features), diffed
# against the tier-1 reference through the harness.
#
# Packing rule (the only thing that makes the sweep affordable): the
# emitter `bits` subcommand costs ~15-26s per run (it re-renders the
# whole 41,460-frame parse-back), so features are packed into shared
# emits. Two features may share an emit only when no bit position is
# required to two different values by the two features. Positions are
# (FAR, offset) pairs; the packer enforces the rule with a per-FAR-group
# union dict (mode A), or, for LUT and BRAM, by construction after an
# exact analytical disjointness proof over the distinct encodings
# (mode B). A packed emit is never a loss of isolation: every
# feature's diff is computed from its own reference positions only,
# and any emitted set bit no feature constrains is reported as a
# global extra with its exact position.
#
# Pair mode (--pairs): same-bit feature pairs. Two features sharing
# at least one bit position are enumerated from the same DBs, split
# into agree pairs (all shared positions require the same value, so
# the pair is emittable and diffable against the union) and
# contradictory pairs (some shared position requires different
# values, so no assignment satisfies both; counted, never emitted,
# sampled by contra probes that only observe what the emitter does).
# Pair items pack under mode A plus one more exclusion the duplicate
# probe established: the emitter fails loud on a duplicated FASM
# line ("duplicate feature"), so two pair items sharing a feature
# never share an emit (Emit.feat_keys enforces it).
#
# Pair-scope reductions, recorded not hidden:
# - FF/CARRY8/LUTRAM agree pairs are all the same feature on two
#   slices sharing a column-shared bit (32/24/52 distinct types);
#   the sweep keeps one instance per distinct type per tile type.
#   Per-slice pairs in these blocks are all contradictory value
#   alternatives (FF 48, CARRY8 15, LUTRAM 26 per slice).
# - INT agree pairs are 25 genuine distinct-PIP pairs per tile,
#   kept on the same 8 tiles as the single sweep.
# - IO agree pairs are 3976 distinct pairs on the one bridged tile;
#   the sweep keeps one per (shared-position set, site pair), 119
#   pairs. Packing all 3976 needs ~2800 emits (median conflict
#   degree 2914 of 3976, driven by ~34 constrained bits per
#   feature), about 12 hours of `bits` runs, for no new
#   interaction: a pair's outcome depends only on its shared
#   positions plus the two sites, and every distinct
#   (shared set, site pair) is swept.
# - LUT_INIT/BRAM: a LUT/BRAM holds one INIT value, so two INITs
#   on one BEL/RAMB are contradictory and INITs on different BELs
#   are disjoint (proven); zero testable same-bit pairs.
# - Cross-block: no two blocks share a bit position on any sampled
#   slice (verified in code by cross_block_check), so zero pairs.
# - MODE.LUT constrains two must-be-0 bits (it sets no 1-bits);
#   MODE.LUT vs MODE.RAM is contradictory at one shared position.
#
# Single-bit resolution without one emit per bit: LUT INIT and BRAM
# INIT use binary-code values. LUT bit i carries the 6-bit code of i
# across 6 emits (code bit k of i in emit k); BRAM content bit i
# carries the 14-bit code of i across 14 emits, parity the 11-bit
# code across 11 emits. Any permutation of the bit mapping changes
# at least one code bit, so any wrong mapping shows as a diff in at
# least one emit. The report names the exact bit index by inverting
# the position tables, never by guessing.
#
# Band bits: the emitter `bits` output excludes the H-1 band
# (absolute frame bits 1440-1487 of block-0 frames), so band bits
# never appear in a diff. A feature that writes a band bit makes the
# emitter fail loud (BandBitSetByFeature); the runner bisects the
# emit to isolate it and reports it, never swallowing it.
#
# Scope notes, recorded not hidden:
# - INT PIP: the full (tile, PIP) cross product is 27,120 tiles x
#   3778 PIPs = 102,459,360 pairs. It is not enumerated: the PIP
#   encoding is tile-independent by DB construction (one
#   segbits_int.db for all tiles), so the sweep covers every PIP on
#   8 tiles spanning all 4 row majors and both INT sides, plus one
#   PIP on every INT tile instance (which tests every tile's base
#   FAR). The reduction and its justification are in the report.
# - IO_CONFIG: only HPIO_RIGHT_X0Y0 exists in the emitter (one
#   provisional bridge, base FAR 0x000). Its 1560 features are swept
#   in tile-relative coordinates against that provisional base; the
#   absolute placement stays blocked on the xcku3p tilegrid. Every
#   other HPIO/HDIO tile instance is blocked, never skipped.
# - FFMUX/OUTMUX and the site-internal CLE PIP family: FFMUX/OUTMUX
#   (134 lines per CLB tile type) emit fine but the emitter `bits`
#   subcommand fails loud on them (no block in its registry) and the
#   harness registry has no block for them either, so they are
#   blocked with a probe as evidence. The CLE PIP (1 line per tile
#   type) labels INT_PIP in `bits` and IS swept.
# - MODE.SRL: `bits` fails loud on it (block-to-be); blocked. MODE.LUT
#   two must-be-0 bits (it sets no 1-bits) and is swept as an
#   item whose reference constrains those bits to 0.
# - DSP48E2, URAM288, CLK (CMT/RCLK/BUFGCE/MMCM), BRAM config bits,
#   HDIO, XIPHY, INT_INTF: blocked, each with its named unblocker
#   and a probe emit where one exists.
#
# Usage:
#   python3 sweep.py --part xcku3p [--blocks LUT_INIT,...]
#       [--out DIR] [--emitter PATH] [--ref-dir DIR]
#       [--max-lines N] [--dry-run] [--limit-emits N]
#   python3 sweep.py --report-only --out DIR
#       (regenerate the markdown report from an existing sweep.jsonl)
#
# Exit codes: 0 sweep completed (reds are results, not failures);
# 1 harness error; 2 emitter missing.
#
# No em dashes anywhere.

import argparse
import datetime
import itertools
import json
import os
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import numpy as np

from reference import MissingReference, Tier1Reference

HARNESS_DIR = os.path.dirname(os.path.abspath(__file__))
DEFAULT_EMITTER = os.path.normpath(os.path.join(
    HARNESS_DIR, "..", "..", "..", "target", "release", "helion-bitgen"))
CREW_REF_DIR = os.path.expanduser("~/workspace/ultra-bitgen-crew/harness/reference")

PART = "xcku3p"
N_Y_OFST_CLB = 60
N_Y_OFST_BRAM = 24
FRAME_BITS = 2976
BAND_LO, BAND_HI = 1440, 1487

# HPIO bridge: the emitter resolves HPIO_RIGHT_X0Y0 to base FAR 0x000
# (provisional, invented; see helion-bitgen.rs). IO diffs are computed
# in tile-relative coordinates against this base and flagged as such.
HPIO_TILE = "HPIO_RIGHT_X0Y0"
HPIO_BASE_FAR = 0x000

BEL_LETTERS = "ABCDEFGH"
FF_BELS = [l + "FF" for l in BEL_LETTERS] + [l + "FF2" for l in BEL_LETTERS]

# FF group control attributes, from the CLB segbits DBs (32 lines per
# tile type). Resolved through reference.resolve_clb_db_feature.
FF_CONTROL_FEATURES = []
for _grp in ("ABCDFF", "EFGHFF"):
    for _attr, _vals in (
        ("CEUSED", ("V0", "V1")),
        ("CLKINV", ("V0", "V1")),
        ("MODE", ("FF", "LATCH")),
        ("SRINV", ("V0", "V1")),
        ("SRUSED", ("V0", "V1")),
        ("SYNC", ("ASYNC", "SYNC")),
    ):
        for _v in _vals:
            FF_CONTROL_FEATURES.append("%s.%s.%s" % (_grp, _attr, _v))
for _grp in ("ABCDFF2", "EFGHFF2"):
    for _attr in ("CEUSED", "SRUSED"):
        for _v in ("V0", "V1"):
            FF_CONTROL_FEATURES.append("%s.%s.%s" % (_grp, _attr, _v))


def key64(far, off):
    """One uint64 key for a (FAR, offset) position."""
    return (int(far) << 12) | int(off)


def far_of(key):
    return int(key) >> 12


def off_of(key):
    return int(key) & 0xFFF


def far_group_of(far):
    """FAR group: (block type, row major, column major)."""
    return ((int(far) >> 24) & 0x7, (int(far) >> 18) & 0x3F, (int(far) >> 8) & 0x3FF)


# ---------------------------------------------------------------------------
# Geometry: tile instances from the public device file
# ---------------------------------------------------------------------------

class Geometry:
    """Tile instances enumerable from xcku3p.json (BitFiltrator)."""

    def __init__(self, ref, part):
        self.part = part
        dev = ref.device(part)
        slr = dev["slrs"]["SLR0"]
        self.rows = sorted(int(r) for r in slr["rowMajors"])
        self.clb_cols = {}   # row -> list of (x, col_major, tile_type)
        self.bram_cols = {}  # row -> list of x
        self.dsp_cols = {}   # row -> list of x
        for r in self.rows:
            row = slr["rowMajors"][str(r)]
            self.clb_cols[r] = [
                (int(x), int(c), row["clb_tileTypes"][x])
                for x, c in sorted(row["clb_colMajors"].items(), key=lambda kv: int(kv[0]))
            ]
            self.bram_cols[r] = sorted(int(x) for x in row["bram_content_colMajors"])
            self.dsp_cols[r] = sorted(int(x) for x in row["dsp_colMajors"])
        # All rows share the same column layout; assert it once so the
        # enumeration below (which uses row 0 as the template) is sound.
        for r in self.rows[1:]:
            assert self.clb_cols[r] == self.clb_cols[0], "CLB columns differ across rows"
            assert self.bram_cols[r] == self.bram_cols[0], "BRAM columns differ across rows"
        self.n_clb_cols = len(self.clb_cols[0])
        self.n_bram_cols = len(self.bram_cols[0])
        # Slices: X is the logical CLB column index, Y spans all rows.
        self.slices = [
            (x, r * N_Y_OFST_CLB + y_ofst)
            for r in self.rows for y_ofst in range(N_Y_OFST_CLB)
            for x, _, _ in self.clb_cols[0]
        ]
        self.tile_type_of = {}
        for x, _, t in self.clb_cols[0]:
            self.tile_type_of[x] = t
        self.clem_slices = [
            (x, y) for (x, y) in self.slices
            if self.tile_type_of[x] in ("CLEM", "CLEM_R")
        ]
        self.brams = [
            (x, r * N_Y_OFST_BRAM + y_ofst)
            for r in self.rows for y_ofst in range(N_Y_OFST_BRAM)
            for x in self.bram_cols[0]
        ]
        # INT tiles: one per CLB column index, same Y range as slices.
        self.int_tiles = list(self.slices)
        # INT side per tile type: CLEL_R serves from the left (-1),
        # every other CLB type from the right (+1), per reference.py.
        self.int_side = {
            x: (-1 if t == "CLEL_R" else +1)
            for x, _, t in self.clb_cols[0]
        }


# ---------------------------------------------------------------------------
# Binary-code values (single-bit resolution across emits)
# ---------------------------------------------------------------------------

def lut_code_value(k):
    """INIT value whose bit i is code bit k of i, for k in 0..5."""
    v = 0
    for i in range(64):
        if (i >> k) & 1:
            v |= 1 << i
    return v


def bram_code_value(k, nbits):
    """Integer whose bit i is code bit k of i, for i in 0..nbits-1."""
    v = 0
    for i in range(nbits):
        if (i >> k) & 1:
            v |= 1 << i
    return v


def code_of_index(i, nbits_code):
    """Tuple of code bits of i (bit k first)."""
    return tuple((i >> k) & 1 for k in range(nbits_code))


# ---------------------------------------------------------------------------
# Position tables (for inverting a mismatching position to a bit index)
# ---------------------------------------------------------------------------

def lut_pos_to_bit(ref, part, tile_type, y_ofst, bel):
    """{(minor, offset): bit index} for one LUT, from the LutLoc table.

    The same table resolve_lut reads; verified against it in tests.
    """
    lut_loc = ref.arch()[tile_type]["LutLoc"]["Y_ofst"][str(y_ofst)]
    minors = lut_loc["minor"][bel]
    ofsts = lut_loc["frame_ofst"][bel]
    return {(m, o): i for i, (m, o) in enumerate(zip(minors, ofsts))}


def bram_pos_to_bit(ref, part, y_ofst, parity):
    """{(minor, offset): bit index} for one BRAM array, from the tables."""
    loc = ref.arch()["BRAM"][
        "BramMemParityLoc" if parity else "BramMemLoc"
    ]["Y_ofst"][str(y_ofst)]
    minors = loc["minor"]
    ofsts = loc["frame_ofst"]
    return {(m, o): i for i, (m, o) in enumerate(zip(minors, ofsts))}


# ---------------------------------------------------------------------------
# Analytical disjointness proofs (mode B packing)
# ---------------------------------------------------------------------------

def verify_lut_disjoint(ref, part, geom):
    """Prove LUT position sets are disjoint across (Y_ofst, BEL).

    Per tile type, the 480 (Y_ofst, BEL) sets of 64 positions each
    must be pairwise disjoint inside one column's FAR space (columns
    and rows separate FARs, so they cannot collide). Raises
    AssertionError with the colliding positions when it fails.
    """
    for tile_type in ("CLEL_L", "CLEL_R", "CLEM", "CLEM_R"):
        seen = {}
        for y_ofst in range(N_Y_OFST_CLB):
            for bel in [l + "6LUT" for l in BEL_LETTERS]:
                for (minor, off), i in lut_pos_to_bit(
                        ref, part, tile_type, y_ofst, bel).items():
                    if (minor, off) in seen:
                        raise AssertionError(
                            "LUT positions collide on %s Y_ofst %d: "
                            "%s.%s bit %d vs %s" % (
                                tile_type, y_ofst, bel, i,
                                (minor, off), seen[(minor, off)]))
                    seen[(minor, off)] = (y_ofst, bel, i)
    return True


def verify_bram_disjoint(ref, part, geom):
    """Prove BRAM position sets are disjoint across Y_ofst per column.

    Per kind (content, parity), the 24 Y_ofst sets must be pairwise
    disjoint inside one column's block-1 FAR space.
    """
    for parity in (False, True):
        seen = set()
        nbits = 2048 if parity else 16384
        for y_ofst in range(N_Y_OFST_BRAM):
            pos = bram_pos_to_bit(ref, part, y_ofst, parity)
            assert len(pos) == nbits, "BRAM table size %d != %d" % (len(pos), nbits)
            overlap = seen & set(pos)
            if overlap:
                raise AssertionError(
                    "BRAM %s positions collide across Y_ofst at %s"
                    % ("parity" if parity else "content", sorted(overlap)[:5]))
            seen |= set(pos)
    return True


# ---------------------------------------------------------------------------
# Items
# ---------------------------------------------------------------------------

class Item:
    """One sweep item: one feature in one emit.

    `ref` is the reference bit dict when the packer needs it (mode A,
    small dicts); None for mode B items, whose refs the differ
    regenerates from `params`. `params` is everything the differ
    needs to regenerate the reference bits. `feat_keys` is the set of
    feature keys this item's FASM lines carry (one for a single item,
    two for a pair item); the packer never puts two items sharing a
    feature key in one emit, because the emitter fails loud on a
    duplicated FASM line.
    """

    __slots__ = ("block", "key", "group", "fasm", "ref", "params",
                 "feat_keys")

    def __init__(self, block, key, group, fasm, ref, params, feat_keys=None):
        self.block = block
        self.key = key
        self.group = group
        self.fasm = fasm
        self.ref = ref
        self.params = params
        self.feat_keys = (frozenset(feat_keys) if feat_keys is not None
                          else frozenset((key,)))


def far_group_for_slice(ref, part, x, y):
    """FAR group of a slice's CLB column: (0, row major, col major)."""
    row_major = y // N_Y_OFST_CLB
    dev = ref.device(part)
    row = dev["slrs"]["SLR0"]["rowMajors"][str(row_major)]
    col_major = row["clb_colMajors"][str(x)]
    return (0, row_major, int(col_major))


def far_group_for_int(ref, part, x, y):
    """FAR group of an INT tile: (0, row major, INT col major)."""
    from reference import int_col_offset_from_clb
    row_major = y // N_Y_OFST_CLB
    dev = ref.device(part)
    row = dev["slrs"]["SLR0"]["rowMajors"][str(row_major)]
    col_major = int(row["clb_colMajors"][str(x)])
    tile_type = row["clb_tileTypes"][str(x)]
    return (0, row_major, col_major + int_col_offset_from_clb(tile_type))


def far_group_for_bram(ref, part, x, y):
    """FAR group of a RAMB18 column: (1, row major, content col major)."""
    row_major = y // N_Y_OFST_BRAM
    dev = ref.device(part)
    row = dev["slrs"]["SLR0"]["rowMajors"][str(row_major)]
    col_major = row["bram_content_colMajors"][str(x)]
    return (1, row_major, int(col_major))

# ---------------------------------------------------------------------------
# Enumeration: one item per (feature, emit slot)
# ---------------------------------------------------------------------------

def enum_lut(ref, part, geom):
    """LUT_INIT items: every slice x every BEL, 6 code emits.

    Item (slice, BEL, code k) carries INIT = lut_code_value(k); the
    six items of one (slice, BEL) jointly resolve every bit index.
    Mode B packing: disjointness proven by verify_lut_disjoint.
    """
    items = []
    for k in range(6):
        value = lut_code_value(k)
        for (x, y) in geom.slices:
            group = far_group_for_slice(ref, part, x, y)
            for letter in BEL_LETTERS:
                bel = letter + "6LUT"
                items.append(Item(
                    "LUT_INIT",
                    "LUT_INIT:SLICE_X%dY%d:%s:code%d" % (x, y, bel, k),
                    group,
                    "SLICE_X%dY%d.%s.INIT[63:0] = 0x%016x" % (x, y, bel, value),
                    None,
                    {"kind": "lut", "x": x, "y": y, "bel": bel, "code": k},
                ))
    return items


def ff_slice_items(ref, part, x, y):
    """FF_INIT items on one slice: 16 FFs x INIT/SRVAL x 0/1 plus
    the 32 FF group control attributes. Mode A packing."""
    items = []
    group = far_group_for_slice(ref, part, x, y)
    tile = "SLICE_X%dY%d" % (x, y)
    for bel in FF_BELS:
        for attr in ("INIT", "SRVAL"):
            for value in (0, 1):
                fn = ref.resolve_ff if attr == "INIT" else ref.resolve_srval
                bits = fn(part, x, y, bel, value)
                items.append(Item(
                    "FF_INIT",
                    "FF_INIT:%s:%s.%s=%d" % (tile, bel, attr, value),
                    group,
                    "%s.%s.%s = %d" % (tile, bel, attr, value),
                    bits,
                    {"kind": "ff", "x": x, "y": y, "bel": bel,
                     "attr": attr, "value": value},
                ))
    for feat in FF_CONTROL_FEATURES:
        bits = ref.resolve_clb_db_feature(part, x, y, feat)
        items.append(Item(
            "FF_INIT",
            "FF_INIT:%s:%s" % (tile, feat),
            group,
            "%s.%s" % (tile, feat),
            bits,
            {"kind": "ffctrl", "x": x, "y": y, "feature": feat},
        ))
    return items


def enum_ff(ref, part, geom):
    """FF_INIT items: every slice x every FF x INIT/SRVAL x value 0/1,
    plus the 32 FF group control attributes per slice.

    Mode A packing: single-position refs, tiny unions.
    """
    items = []
    for (x, y) in geom.slices:
        items.extend(ff_slice_items(ref, part, x, y))
    return items


def carry8_slice_items(ref, part, x, y):
    """CARRY8 items on one slice: all 24 CARRY8 features."""
    db = ref.segbits("clel_r")
    features = sorted(
        k.split(".", 1)[1] for k in db if ".CARRY8." in k
    )
    assert len(features) == 24, "CARRY8 feature count %d != 24" % len(features)
    items = []
    group = far_group_for_slice(ref, part, x, y)
    tile = "SLICE_X%dY%d" % (x, y)
    for feat in features:
        full = "CARRY8." + feat[len("CARRY8."):] if feat.startswith("CARRY8.") else feat
        bits = ref.resolve_carry8(part, x, y, full)
        items.append(Item(
            "CARRY8",
            "CARRY8:%s:%s" % (tile, full),
            group,
            "%s.%s" % (tile, full),
            bits,
            {"kind": "carry8", "x": x, "y": y, "feature": full},
        ))
    return items


def enum_carry8(ref, part, geom):
    """CARRY8 items: every slice x all 24 CARRY8 features.

    Mode A packing: positions are Y-independent, so alternatives on
    the same attribute (CI.AX vs CI.V0/V1, DUAL_CY4 vs SINGLE_CY8)
    conflict and split into separate emits.
    """
    items = []
    for (x, y) in geom.slices:
        items.extend(carry8_slice_items(ref, part, x, y))
    return items


def int_tile_items(ref, part, x, y):
    """INT_PIP items on one INT tile: every PIP (3778)."""
    db = ref.segbits("int")
    features = sorted(k[len("INT."):] for k in db)
    assert len(features) == 3778, "INT PIP count %d != 3778" % len(features)
    items = []
    group = far_group_for_int(ref, part, x, y)
    tile = "INT_X%dY%d" % (x, y)
    for feat in features:
        bits = ref.resolve_int_pip(part, x, y, feat)
        items.append(Item(
            "INT_PIP",
            "INT_PIP:%s:%s" % (tile, feat),
            group,
            "%s.%s" % (tile, feat),
            bits,
            {"kind": "int", "x": x, "y": y, "feature": feat},
        ))
    return items


def enum_int(ref, part, geom):
    """INT_PIP items: every PIP on 8 tiles, plus 1 PIP on every tile.

    The full cross product (27,120 tiles x 3778 PIPs) is not
    enumerated: the encoding is tile-independent (one segbits_int.db),
    so every PIP is swept on 8 tiles spanning all 4 row majors and
    both INT sides, and every tile instance is swept with one PIP
    (which tests its base FAR). See the module header.
    Mode A packing: positions are Y-independent within a column.
    """
    probe_pip = "PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1"
    assert "INT." + probe_pip in ref.segbits("int"), \
        "probe PIP missing from segbits_int.db"
    items = []
    # 8 tiles: row r, CLB col 0 (CLEL_R, INT on the left) and col 1
    # (CLEM, INT on the right), at the row's first Y.
    eight = []
    for r in geom.rows:
        for col in (0, 1):
            eight.append((col, r * N_Y_OFST_CLB))
    for (x, y) in eight:
        items.extend(int_tile_items(ref, part, x, y))
    # 1 PIP on every INT tile instance.
    for (x, y) in geom.int_tiles:
        if (x, y) in eight:
            continue
        group = far_group_for_int(ref, part, x, y)
        tile = "INT_X%dY%d" % (x, y)
        bits = ref.resolve_int_pip(part, x, y, probe_pip)
        items.append(Item(
            "INT_PIP",
            "INT_PIP:%s:%s" % (tile, probe_pip),
            group,
            "%s.%s" % (tile, probe_pip),
            bits,
            {"kind": "int", "x": x, "y": y, "feature": probe_pip},
        ))
    return items


def enum_bram(ref, part, geom):
    """BRAM items: every RAMB18 x whole-array INIT/INITP, code emits.

    Content: 14 code emits (2^14 = 16384); parity: 11 code emits
    (2^11 = 2048). Mode B packing: disjointness proven by
    verify_bram_disjoint.
    """
    items = []
    for parity, nbits, ncode, name in (
        (False, 16384, 14, "INIT[16383:0]"),
        (True, 2048, 11, "INITP[2047:0]"),
    ):
        block = "BRAM_PARITY" if parity else "BRAM_CONTENT"
        for k in range(ncode):
            value = bram_code_value(k, nbits)
            hexwidth = nbits // 4
            for (x, y) in geom.brams:
                group = far_group_for_bram(ref, part, x, y)
                items.append(Item(
                    block,
                    "%s:RAMB18_X%dY%d:code%d" % (block, x, y, k),
                    group,
                    "RAMB18_X%dY%d.%s = 0x%0*x" % (x, y, name, hexwidth, value),
                    None,
                    {"kind": "bram", "x": x, "y": y, "parity": parity, "code": k},
                ))
    return items


def lutram_features_for_slice(ref, part, x, y):
    """The 44 LUTRAM mode features of one CLEM/CLEM_R slice."""
    tile_type = ref.device(part)["slrs"]["SLR0"]["rowMajors"][
        str(y // N_Y_OFST_CLB)]["clb_tileTypes"][str(x)]
    db_name = {"CLEM": "clem", "CLEM_R": "clem_r"}[tile_type]
    db = ref.segbits(db_name)
    feats = []
    for letter in BEL_LETTERS:
        feats.append("%sLUT.MODE.RAM" % letter)
        feats.append("%sLUT.SIZE.LARGE" % letter)
        feats.append("%sLUT.SIZE.SMALL" % letter)
        if letter != "H":
            feats.append("%sLUT.DI1.DI1" % letter)
            feats.append("%sLUT.DI1.SIN" % letter)
    for w in ("WA7USED", "WA8USED", "WA9USED"):
        feats.append("%s.V0" % w)
        feats.append("%s.V1" % w)
    # MODE.LUT constrains two must-be-0 bits (sets no 1-bits);
    # swept as an item on the same slice.
    for letter in BEL_LETTERS:
        feats.append("%sLUT.MODE.LUT" % letter)
    out = []
    for feat in feats:
        key = "%s.%s" % (tile_type, feat)
        if key not in db:
            raise MissingReference(
                "LUTRAM feature %r not in prjuray-db segbits_%s.db" % (feat, db_name))
        out.append(feat)
    return out


def lutram_slice_items(ref, part, x, y):
    """LUTRAM items on one CLEM/CLEM_R slice: its 44 mode features
    plus 8 MODE.LUT must-be-0 features. Mode A packing."""
    items = []
    group = far_group_for_slice(ref, part, x, y)
    tile = "SLICE_X%dY%d" % (x, y)
    for feat in lutram_features_for_slice(ref, part, x, y):
        bits = ref.resolve_lutram_mode(part, x, y, feat)
        items.append(Item(
            "LUTRAM",
            "LUTRAM:%s:%s" % (tile, feat),
            group,
            "%s.%s" % (tile, feat),
            bits,
            {"kind": "lutram", "x": x, "y": y, "feature": feat},
        ))
    return items


def enum_lutram(ref, part, geom):
    """LUTRAM items: every CLEM/CLEM_R slice x its 44 mode features
    plus 8 MODE.LUT must-be-0 features. Mode A packing."""
    items = []
    for (x, y) in geom.clem_slices:
        items.extend(lutram_slice_items(ref, part, x, y))
    return items


def io_tile_items(ref, part, geom):
    """IO_CONFIG items: all 1560 HPIO_RIGHT features on HPIO_RIGHT_X0Y0.

    Tile-relative reference bits (resolve_hpio); the differ converts
    emitted absolute FARs to tile-relative ones against the
    provisional base FAR 0x000 and flags the comparison as relative.
    Mode A packing.
    """
    db = ref.segbits("hpio_right")
    features = sorted(k[len("HPIO_RIGHT."):] for k in db)
    assert len(features) == 1560, "HPIO feature count %d != 1560" % len(features)
    items = []
    for feat in features:
        bits = ref.resolve_hpio(HPIO_TILE, feat)
        items.append(Item(
            "IO_CONFIG",
            "IO_CONFIG:%s:%s" % (HPIO_TILE, feat),
            (0, 0, 0),
            "%s.%s" % (HPIO_TILE, feat),
            bits,
            {"kind": "io", "tile": HPIO_TILE, "feature": feat},
        ))
    return items


def enum_io(ref, part, geom):
    """IO_CONFIG items: all 1560 HPIO_RIGHT features on HPIO_RIGHT_X0Y0."""
    return io_tile_items(ref, part, geom)


ENUMERATORS = {
    "LUT_INIT": enum_lut,
    "FF_INIT": enum_ff,
    "CARRY8": enum_carry8,
    "BRAM_CONTENT": enum_bram,
    "BRAM_PARITY": enum_bram,
    "INT_PIP": enum_int,
    "LUTRAM": enum_lutram,
    "IO_CONFIG": enum_io,
}

# Feature counts (the swept universe per block), computed from the
# enumeration design, used for the coverage table.
FEATURE_COUNTS = {
    # LUT: every slice x every BEL x every bit index.
    "LUT_INIT": lambda g: len(g.slices) * 8 * 64,
    # FF: every slice x 16 FFs x INIT/SRVAL x value 0/1, plus 32
    # control attributes per slice.
    "FF_INIT": lambda g: len(g.slices) * (16 * 2 * 2 + len(FF_CONTROL_FEATURES)),
    "CARRY8": lambda g: len(g.slices) * 24,
    "BRAM_CONTENT": lambda g: len(g.brams) * 16384,
    "BRAM_PARITY": lambda g: len(g.brams) * 2048,
    # INT: every PIP on 8 tiles + 1 PIP on every other tile.
    "INT_PIP": lambda g: 8 * 3778 + (len(g.int_tiles) - 8),
    "LUTRAM": lambda g: len(g.clem_slices) * (44 + 8),
    "IO_CONFIG": lambda g: 1560,
}


# ---------------------------------------------------------------------------
# Pair sweep (same-bit feature pairs)
# ---------------------------------------------------------------------------

def pair_type_name(item):
    """Feature name without tile coordinates: key is BLOCK:TILE:FEATURE."""
    parts = item.key.split(":", 2)
    return parts[2] if len(parts) == 3 else item.key


def pair_stats(items):
    """(agree, contra) same-bit pair lists over Items.

    Each entry is (item_a, item_b, shared_int_keys). Agree: all shared
    positions require the same value. Contra: some shared position
    requires different values. Disjoint pairs are not listed.
    """
    int_refs = {}
    pos_to_idx = {}
    for idx, item in enumerate(items):
        int_refs[idx] = ref_int_keys(item, item.ref)
        for k in int_refs[idx]:
            pos_to_idx.setdefault(k, []).append(idx)
    pairs = {}
    for pos, idxs in pos_to_idx.items():
        for a, b in itertools.combinations(sorted(set(idxs)), 2):
            pairs.setdefault((a, b), []).append(pos)
    agree, contra = [], []
    for (a, b), shared in pairs.items():
        if all(int_refs[a][p] == int_refs[b][p] for p in shared):
            agree.append((items[a], items[b], shared))
        else:
            contra.append((items[a], items[b], shared))
    return agree, contra


def make_pair_item(a, b):
    """One pair Item from two single Items on the same FAR group.

    The pair's reference is the union of the two features' bits;
    the caller guarantees they agree on every shared position.
    """
    assert a.block == b.block, "pair spans blocks: %s vs %s" % (
        a.block, b.block)
    assert a.group == b.group, "pair spans FAR groups: %s vs %s" % (
        a.group, b.group)
    merged = {}
    for item in (a, b):
        for pos, val in item.ref.items():
            if pos in merged and merged[pos] != val:
                raise AssertionError(
                    "contradictory pair: %s + %s" % (a.key, b.key))
            merged[pos] = val
    return Item(
        a.block,
        "PAIR:%s + %s" % (a.key, b.key),
        a.group,
        a.fasm + "\n" + b.fasm,
        merged,
        {"kind": "pair", "a": a.params, "b": b.params},
        feat_keys=set(a.feat_keys) | set(b.feat_keys),
    )


PAIR_COLUMN_BLOCKS = {
    "FF_INIT": (ff_slice_items, ("CLEL_L", "CLEL_R", "CLEM", "CLEM_R")),
    "CARRY8": (carry8_slice_items, ("CLEL_L", "CLEL_R", "CLEM", "CLEM_R")),
    "LUTRAM": (lutram_slice_items, ("CLEM", "CLEM_R")),
}


def cross_block_check(ref, part, geom):
    """Verify no two blocks share a bit position on sample slices.

    Builds FF + CARRY8 + LUTRAM (CLEM only) + LUT all-ones INIT items
    on one slice per tile type at Y_ofst 0 and 30, and counts pairs
    whose two features belong to different blocks. Returns a stats
    dict; the pair sweep covers zero cross-block pairs when the
    count is zero.
    """
    first_col = {}
    for x, _, t in geom.clb_cols[0]:
        first_col.setdefault(t, x)
    total_cross = 0
    checked = 0
    for t, x in sorted(first_col.items()):
        for y in (0, 30):
            items = []
            items.extend(ff_slice_items(ref, part, x, y))
            items.extend(carry8_slice_items(ref, part, x, y))
            if t in ("CLEM", "CLEM_R"):
                items.extend(lutram_slice_items(ref, part, x, y))
            for letter in BEL_LETTERS:
                bel = letter + "6LUT"
                bits = ref.resolve_lut(part, x, y, bel, 0xFFFFFFFFFFFFFFFF)
                items.append(Item(
                    "LUT_INIT",
                    "LUT_INIT:SLICE_X%dY%d:%s:all1" % (x, y, bel),
                    far_group_for_slice(ref, part, x, y),
                    "SLICE_X%dY%d.%s.INIT[63:0] = 0xFFFFFFFFFFFFFFFF"
                    % (x, y, bel),
                    bits,
                    {"kind": "test"},
                ))
            agree, contra = pair_stats(items)
            total_cross += sum(
                1 for (a, b, _s) in agree + contra if a.block != b.block)
            checked += 1
    return {
        "scope": "%d sample slices (one per tile type x Y_ofst 0, 30)" % checked,
        "cross_block_pairs": total_cross,
    }


def io_site(item):
    """Site prefix of an IO feature (IOB_X0Y15, or PIP)."""
    feat = item.key.split(":", 2)[2]
    return feat.split(".", 1)[0]


def enum_pairs(ref, part, geom):
    """Enumerate same-bit pairs. Returns (pair_items, stats, contra_samples).

    pair_items: agree-pair Items to sweep. stats: per-block dict with
    agree/contra instance and distinct-type counts plus the scope.
    contra_samples: {block: [(a, b, shared), ...]} deterministic
    samples of contradictory pairs for observation probes (at most 4
    per block, first by sorted type name).
    """
    pair_items = []
    stats = {}
    contra_samples = {}
    first_col = {}
    for x, _, t in geom.clb_cols[0]:
        first_col.setdefault(t, x)

    # FF / CARRY8 / LUTRAM: one column per tile type, all 60 slices.
    # Agree pairs are deduped by distinct feature-name type: one
    # instance per type per tile type.
    for block, (builder, tile_types) in PAIR_COLUMN_BLOCKS.items():
        agree_types = set()
        contra_types = set()
        n_agree = n_contra = 0
        swept = 0
        contra_first = {}
        for t in tile_types:
            x = first_col[t]
            feats = []
            for y in range(N_Y_OFST_CLB):
                feats.extend(builder(ref, part, x, y))
            agree, contra = pair_stats(feats)
            n_agree += len(agree)
            n_contra += len(contra)
            seen = set()
            for (a, b, shared) in agree:
                typ = tuple(sorted((pair_type_name(a), pair_type_name(b))))
                agree_types.add(typ)
                if typ in seen:
                    continue
                seen.add(typ)
                pair_items.append(make_pair_item(a, b))
                swept += 1
            for (a, b, shared) in contra:
                typ = tuple(sorted((pair_type_name(a), pair_type_name(b))))
                contra_types.add(typ)
                if typ not in contra_first:
                    contra_first[typ] = (a, b, shared)
        stats[block] = {
            "scope": "one column per tile type (%s), all 60 slices; "
            "one instance per distinct agree type per tile type"
            % ", ".join(tile_types),
            "agree_instances": n_agree,
            "agree_types": len(agree_types),
            "agree_swept": swept,
            "contra_instances": n_contra,
            "contra_types": len(contra_types),
            "contra_swept": 0,
        }
        contra_samples[block] = [
            contra_first[typ] for typ in sorted(contra_types)[:4]]

    # INT: agree pairs kept per tile on the same 8 tiles as the
    # single sweep (the encoding is tile-independent).
    eight = []
    for r in geom.rows:
        for col in (0, 1):
            eight.append((col, r * N_Y_OFST_CLB))
    int_agree_types = set()
    int_contra_types = set()
    int_contra_first = {}
    n_int_agree = n_int_contra = 0
    for (x, y) in eight:
        feats = int_tile_items(ref, part, x, y)
        agree, contra = pair_stats(feats)
        n_int_agree += len(agree)
        n_int_contra += len(contra)
        for (a, b, shared) in agree:
            typ = tuple(sorted((pair_type_name(a), pair_type_name(b))))
            int_agree_types.add(typ)
            pair_items.append(make_pair_item(a, b))
        for (a, b, shared) in contra:
            typ = tuple(sorted((pair_type_name(a), pair_type_name(b))))
            int_contra_types.add(typ)
            if typ not in int_contra_first:
                int_contra_first[typ] = (a, b, shared)
    stats["INT_PIP"] = {
        "scope": "8 tiles (same reduction as the single sweep)",
        "agree_instances": n_int_agree,
        "agree_types": len(int_agree_types),
        "agree_swept": n_int_agree,
        "contra_instances": n_int_contra,
        "contra_types": len(int_contra_types),
        "contra_swept": 0,
    }
    contra_samples["INT_PIP"] = [
        int_contra_first[typ] for typ in sorted(int_contra_types)[:4]]

    # IO: one bridged tile. Agree pairs are deduped by
    # (shared-position set, site pair): one pair per distinct
    # shared-bit interaction (see the module header for why the
    # full 3976 are not packed; packing them needs ~2800 emits).
    feats = io_tile_items(ref, part, geom)
    agree, contra = pair_stats(feats)
    io_contra_first = {}
    seen_io = set()
    swept_io = 0
    for (a, b, shared) in agree:
        group_key = (frozenset(shared),
                     tuple(sorted((io_site(a), io_site(b)))))
        if group_key in seen_io:
            continue
        seen_io.add(group_key)
        pair_items.append(make_pair_item(a, b))
        swept_io += 1
    for (a, b, shared) in contra:
        typ = tuple(sorted((pair_type_name(a), pair_type_name(b))))
        if typ not in io_contra_first:
            io_contra_first[typ] = (a, b, shared)
    stats["IO_CONFIG"] = {
        "scope": "one bridged tile HPIO_RIGHT_X0Y0; one pair per "
        "(shared-position set, site pair)",
        "agree_instances": len(agree),
        "agree_types": len(agree),
        "agree_swept": swept_io,
        "subset_groups": len(seen_io),
        "contra_instances": len(contra),
        "contra_types": len(contra),
        "contra_swept": 0,
    }
    contra_samples["IO_CONFIG"] = [
        io_contra_first[typ] for typ in sorted(io_contra_first)[:4]]

    # LUT / BRAM: analytic zero. A LUT holds one 64-bit INIT and a
    # RAMB holds one INIT/INITP value, so two INITs on one BEL/RAMB
    # are contradictory, and INITs on different BELs/RAMBs are
    # disjoint (proven by verify_lut_disjoint / verify_bram_disjoint).
    # There is no same-bit pair to sweep.
    for block, what in (
        ("LUT_INIT", "one 64-bit INIT per BEL"),
        ("BRAM_CONTENT", "one INIT value per RAMB18"),
        ("BRAM_PARITY", "one INITP value per RAMB18"),
    ):
        stats[block] = {
            "scope": "analytic: %s; no same-bit pairs exist" % what,
            "agree_instances": 0,
            "agree_types": 0,
            "agree_swept": 0,
            "contra_instances": 0,
            "contra_types": 0,
            "contra_swept": 0,
        }
    contra_samples["LUT_INIT"] = []
    contra_samples["BRAM_CONTENT"] = []
    contra_samples["BRAM_PARITY"] = []

    stats["CROSS_BLOCK"] = cross_block_check(ref, part, geom)
    return pair_items, stats, contra_samples


def run_contra_probes(ref, part, emitter_cmd, samples, out_dir, records):
    """Observe what the emitter does with contradictory pairs.

    Each sampled pair is emitted alone; the outcome is recorded as
    an observation (kind contra-probe), never as a red: no assignment
    satisfies both features, so there is no valid expected bitset.
    For each shared position the record carries what each side
    wanted and what the emitter actually set.
    """
    for block in sorted(samples):
        for idx, (a, b, shared) in enumerate(samples[block]):
            name = "contra-%s-%d" % (block, idx)
            fasm_path = os.path.join(out_dir, name + ".fasm")
            bit_path = os.path.join(out_dir, name + ".bit")
            bits_path = os.path.join(out_dir, name + ".bits")
            with open(fasm_path, "w") as f:
                f.write("# contra probe: %s + %s\n%s\n%s\n"
                        % (a.key, b.key, a.fasm, b.fasm))
            ra = ref_int_keys(a, a.ref)
            rb = ref_int_keys(b, b.ref)
            rec = {
                "kind": "contra-probe", "block": block,
                "pair": "%s + %s" % (a.key, b.key),
                "positions": [],
            }
            try:
                run_emitter_emit(emitter_cmd, fasm_path, part, bit_path)
            except (SweepError, subprocess.TimeoutExpired) as err:
                rec["outcome"] = "emit failed loud: %s" % err
                records.append(rec)
                continue
            try:
                run_emitter_bits(emitter_cmd, bit_path, part, bits_path)
            except (SweepError, subprocess.TimeoutExpired) as err:
                rec["outcome"] = "emit ok, bits failed loud: %s" % err
                records.append(rec)
                continue
            emitted = set()
            for far, off, _blk in parse_bits_file(bits_path):
                if block == "IO_CONFIG":
                    emitted.add(key64(far & 0xFF, off))
                else:
                    emitted.add(key64(far, off))
            for k in shared:
                rec["positions"].append({
                    "far": ("frame %d (tile-relative)" % far_of(k)
                            if block == "IO_CONFIG"
                            else "0x%08x" % far_of(k)),
                    "offset": int(off_of(k)),
                    "wanted_by_a": ra[k],
                    "wanted_by_b": rb[k],
                    "observed": 1 if k in emitted else 0,
                })
            rec["outcome"] = "emit ok, bits ok"
            records.append(rec)
            try:
                os.remove(bit_path)
            except OSError:
                pass


# ---------------------------------------------------------------------------
# Reference regeneration (the differ calls this per item)
# ---------------------------------------------------------------------------

def resolve_item(ref, part, item):
    """Reference bits for one item, regenerated from its params."""
    return resolve_item_params(ref, part, item.params)


def resolve_item_params(ref, part, params):
    """Reference bits for one params dict (recurses for pairs)."""
    p = params
    kind = p["kind"]
    if kind == "pair":
        merged = {}
        for sub in (p["a"], p["b"]):
            for pos, val in resolve_item_params(ref, part, sub).items():
                if pos in merged and merged[pos] != val:
                    raise AssertionError(
                        "pair sub-features disagree at %r" % (pos,))
                merged[pos] = val
        return merged
    if kind == "lut":
        return ref.resolve_lut(part, p["x"], p["y"], p["bel"], lut_code_value(p["code"]))
    if kind == "ff":
        fn = ref.resolve_ff if p["attr"] == "INIT" else ref.resolve_srval
        return fn(part, p["x"], p["y"], p["bel"], p["value"])
    if kind == "ffctrl":
        return ref.resolve_clb_db_feature(part, p["x"], p["y"], p["feature"])
    if kind == "carry8":
        return ref.resolve_carry8(part, p["x"], p["y"], p["feature"])
    if kind == "int":
        return ref.resolve_int_pip(part, p["x"], p["y"], p["feature"])
    if kind == "bram":
        nbits = 2048 if p["parity"] else 16384
        value = bram_code_value(p["code"], nbits)
        bits = [(value >> i) & 1 for i in range(nbits)]
        if p["parity"]:
            return ref.resolve_bram_parity(part, p["x"], p["y"], bits)
        return ref.resolve_bram(part, p["x"], p["y"], 18, bits)
    if kind == "lutram":
        return ref.resolve_lutram_mode(part, p["x"], p["y"], p["feature"])
    if kind == "io":
        return ref.resolve_hpio(p["tile"], p["feature"])
    raise AssertionError("unknown item kind %r" % kind)

# ---------------------------------------------------------------------------
# Packing
# ---------------------------------------------------------------------------

class Emit:
    """One emitter run: a set of items with no value conflicts."""

    __slots__ = ("block", "index", "items", "groups", "lines",
                 "emit_id_override", "feat_keys")

    def __init__(self, block, index):
        self.block = block
        self.index = index
        self.items = []
        self.groups = {}  # far_group -> {pos_key: value}
        self.lines = 0
        self.emit_id_override = None
        self.feat_keys = set()

    @property
    def emit_id(self):
        if self.emit_id_override:
            return self.emit_id_override
        return "%s-%03d" % (self.block, self.index)

    def conflicts(self, item):
        """True when the item requires a different value at a position
        this emit already constrains (same far_group only; other
        groups are different FARs and cannot collide), or when the
        item repeats a feature key already in this emit (the emitter
        fails loud on a duplicated FASM line)."""
        if self.feat_keys & set(item.feat_keys):
            return True
        union = self.groups.get(item.group)
        if union is None:
            return False
        for key, val in ref_int_keys(item, item.ref).items():
            cur = union.get(key)
            if cur is not None and cur != val:
                return True
        return False

    def add(self, item):
        union = self.groups.setdefault(item.group, {})
        for key, val in ref_int_keys(item, item.ref).items():
            union.setdefault(key, val)
        self.feat_keys |= set(item.feat_keys)
        self.items.append(item)
        self.lines += item.fasm.count("\n") + 1


def pack_mode_a(block, items, max_lines):
    """Greedy first-fit packing for small-ref items.

    Items are tried against existing emits in order; an item joins
    the first emit where it has no value conflict and the line cap
    is not reached. Deterministic: items arrive in sorted order.
    """
    emits = []
    for item in items:
        placed = False
        for emit in emits:
            if emit.lines >= max_lines:
                continue
            if not emit.conflicts(item):
                emit.add(item)
                placed = True
                break
        if not placed:
            emit = Emit(block, len(emits))
            emit.add(item)
            emits.append(emit)
    return emits


def pack_mode_b(block, items, max_lines):
    """Construction packing for LUT/BRAM items.

    Disjointness was proven analytically (verify_lut_disjoint /
    verify_bram_disjoint), so items are chunked by the line cap in
    deterministic order with no per-item conflict checks. Items must
    arrive grouped by code (all of code k before code k+1) so each
    chunk stays within one code value.
    """
    emits = []
    cur = None
    for item in items:
        if cur is None or cur.lines >= max_lines:
            cur = Emit(block, len(emits))
            emits.append(cur)
        cur.items.append(item)
        cur.lines += 1
    return emits


MODE_B_BLOCKS = {"LUT_INIT", "BRAM_CONTENT", "BRAM_PARITY"}


def pack_block(block, items, max_lines):
    if block in MODE_B_BLOCKS:
        return pack_mode_b(block, items, max_lines)
    return pack_mode_a(block, items, max_lines)


# ---------------------------------------------------------------------------
# Emitter runner (protocol: emit, then bits to a file for streaming)
# ---------------------------------------------------------------------------

class SweepError(Exception):
    pass


def run_emitter_emit(emitter_cmd, fasm_path, part, out_path, timeout=900):
    proc = subprocess.run(
        [emitter_cmd, "emit", "--fasm", fasm_path, "--part", part, "--out", out_path],
        capture_output=True, text=True, timeout=timeout,
    )
    if proc.returncode != 0:
        raise SweepError("emit failed (exit %d): %s" % (
            proc.returncode, proc.stderr.strip()[:1000]))
    return proc


def run_emitter_bits(emitter_cmd, bit_path, part, out_path, timeout=900):
    """Run `bits`, streaming stdout to out_path. Returns the process."""
    with open(out_path, "w") as f:
        proc = subprocess.run(
            [emitter_cmd, "bits", "--bit", bit_path, "--part", part],
            stdout=f, stderr=subprocess.PIPE, text=True, timeout=timeout,
        )
    if proc.returncode != 0:
        raise SweepError("bits failed (exit %d): %s" % (
            proc.returncode, proc.stderr.strip()[:1000]))
    return proc


# ---------------------------------------------------------------------------
# Differ: stream the bits file, one FAR group at a time
# ---------------------------------------------------------------------------

def parse_bits_file(path):
    """Yield (far, offset, block) per set-bit line, in file order.

    The emitter prints lines sorted by (FAR, offset).
    """
    with open(path) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            fields = line.split()
            if len(fields) != 4:
                raise SweepError("malformed bits line: %r" % line[:200])
            if fields[2] != "1":
                raise SweepError("bits line with VALUE != 1: %r" % line[:200])
            yield int(fields[0], 16), int(fields[1]), fields[3]


def ref_int_keys(item, ref_bits):
    """Reference bits as {int key: value}.

    Normal items use key64(far, offset). IO_CONFIG items use
    key64(frame, offset) in tile-relative coordinates (the ref keys
    are (tile, frame, offset) triples).
    """
    out = {}
    if item.block == "IO_CONFIG":
        for (_tile, frame, off), val in ref_bits.items():
            out[key64(frame, off)] = val
    else:
        for (far, off), val in ref_bits.items():
            out[key64(far, off)] = val
    return out


def emitted_int_keys(group, keys_list, is_io):
    """Emitted keys of one FAR group as a sorted numpy array.

    IO_CONFIG groups are relativized: the emitted absolute FAR
    minus the provisional HPIO base (0x000) is the frame minor.
    """
    if is_io:
        keys = [key64(far_of(k) & 0xFF, off_of(k)) for k in keys_list]
    else:
        keys = list(keys_list)
    return np.array(sorted(keys), dtype=np.uint64)


def diff_item_int(ref_int, emitted):
    """(diff_count, mismatches) from {int key: value} refs.

    mismatches is a list of (int key, expected, observed).
    """
    if not ref_int:
        return 0, []
    keys = np.fromiter(ref_int.keys(), dtype=np.uint64, count=len(ref_int))
    expected = np.fromiter(ref_int.values(), dtype=np.uint8, count=len(ref_int))
    observed = np.isin(keys, emitted).astype(np.uint8)
    bad = np.nonzero(observed != expected)[0]
    return len(bad), [
        (int(keys[int(idx)]), int(expected[int(idx)]), int(observed[int(idx)]))
        for idx in bad
    ]


def format_pos(item, int_key):
    """Human-readable position for a mismatch record."""
    if item.block == "IO_CONFIG":
        return {"far": "frame %d (tile-relative)" % far_of(int_key),
                "offset": int(off_of(int_key))}
    return {"far": "0x%08x" % far_of(int_key),
            "offset": int(off_of(int_key))}


def diff_emit(ref, part, emit, bits_path, records):
    """Diff one emit's items against its bits file.

    Streams the bits file grouped by FAR group; per group, diffs
    that group's items and reports global extras (emitted set bits
    no item constrains) and stray bits. Appends per-item records
    and returns (n_green, n_red, n_extra).
    """
    by_group = {}
    for item in emit.items:
        by_group.setdefault(item.group, []).append(item)
    n_green = n_red = n_extra = 0
    seen_groups = set()

    def diff_group_items(group, items, emitted):
        nonlocal n_green, n_red
        union_keys = set()
        for item in items:
            ref_int = ref_int_keys(item, resolve_item(ref, part, item))
            union_keys.update(ref_int.keys())
            diff, mismatches = diff_item_int(ref_int, emitted)
            # Green items are counted, not listed: the JSONL stays
            # small (millions of green items would be ~1GB). The
            # emit record carries the green/red counts; red items
            # are listed here in full.
            if diff == 0:
                n_green += 1
            else:
                n_red += 1
                records.append({
                    "kind": "item", "block": item.block, "key": item.key,
                    "emit": emit.emit_id, "diff_bits": diff,
                    "total_bits": len(ref_int), "status": "red",
                    "coord": "tile-relative" if item.block == "IO_CONFIG" else "absolute",
                    "mismatches": [
                        dict(format_pos(item, k), expected=e, observed=g)
                        for (k, e, g) in mismatches
                    ],
                })
        return union_keys

    def report_extras(keys_list, union_keys, block_label):
        nonlocal n_extra
        for k in keys_list:
            if k not in union_keys:
                records.append({
                    "kind": "extra", "block": emit.block, "emit": emit.emit_id,
                    "far": "0x%08x" % far_of(k), "offset": int(off_of(k)),
                    "detail": "emitted set bit at a position no sweep "
                    "item constrains (block label %s)" % block_label,
                })
                n_extra += 1

    cur_group = None
    cur_keys = []
    cur_block = None

    def flush_group(group, keys_list, block):
        if group is None:
            return
        seen_groups.add(group)
        items = by_group.get(group, [])
        if not items:
            for k in keys_list:
                records.append({
                    "kind": "extra", "block": emit.block, "emit": emit.emit_id,
                    "far": "0x%08x" % far_of(k), "offset": int(off_of(k)),
                    "detail": "emitted set bit in FAR group %s with no "
                    "sweep item; block label %s" % (group, block),
                })
            return len(keys_list)
        is_io = items[0].block == "IO_CONFIG"
        emitted = emitted_int_keys(group, keys_list, is_io)
        union_keys = diff_group_items(group, items, emitted)
        if is_io:
            rel_keys = [key64(far_of(k) & 0xFF, off_of(k)) for k in keys_list]
            for rk, k in zip(rel_keys, keys_list):
                if rk not in union_keys:
                    records.append({
                        "kind": "extra", "block": emit.block, "emit": emit.emit_id,
                        "far": "frame %d (tile-relative)" % far_of(rk),
                        "offset": int(off_of(rk)),
                        "detail": "emitted set bit at a tile-relative "
                        "position no sweep item constrains",
                    })
            return sum(1 for rk in rel_keys if rk not in union_keys)
        extras = [k for k in keys_list if k not in union_keys]
        report_extras(extras, union_keys, block)
        return len(extras)

    total_extra = 0
    for far, off, block in parse_bits_file(bits_path):
        group = far_group_of(far)
        if group != cur_group:
            total_extra += flush_group(cur_group, cur_keys, cur_block) or 0
            cur_group = group
            cur_keys = []
            cur_block = block
        cur_keys.append(key64(far, off))
    total_extra += flush_group(cur_group, cur_keys, cur_block) or 0
    n_extra = total_extra
    # FAR groups with items but no emitted lines at all.
    empty = np.array([], dtype=np.uint64)
    for group, items in by_group.items():
        if group in seen_groups:
            continue
        diff_group_items(group, items, empty)
    return n_green, n_red, n_extra

# ---------------------------------------------------------------------------
# Runner: emits with bisection on failure
# ---------------------------------------------------------------------------

def write_fasm(path, emit):
    with open(path, "w") as f:
        f.write("# sweep emit %s: %d items, block %s\n" % (
            emit.emit_id, len(emit.items), emit.block))
        f.write("# packed: features share this emit only when no bit "
                "position is required to two different values.\n")
        for item in emit.items:
            f.write(item.fasm + "\n")


def run_one_emit(ref, part, emitter_cmd, emit, out_dir, records,
                 keep_bits_on_red=False):
    """Emit + bits + diff for one Emit. Returns (green, red, extra)."""
    base = os.path.join(out_dir, emit.emit_id)
    fasm_path = base + ".fasm"
    bit_path = base + ".bit"
    bits_path = base + ".bits"
    write_fasm(fasm_path, emit)
    t0 = time.time()
    run_emitter_emit(emitter_cmd, fasm_path, part, bit_path)
    t_emit = time.time() - t0
    t0 = time.time()
    run_emitter_bits(emitter_cmd, bit_path, part, bits_path)
    t_bits = time.time() - t0
    n_green, n_red, n_extra = diff_emit(ref, part, emit, bits_path, records)
    if not (keep_bits_on_red and (n_red or n_extra)):
        try:
            os.remove(bits_path)
        except OSError:
            pass
    try:
        os.remove(bit_path)
    except OSError:
        pass
    records.append({
        "kind": "emit", "emit": emit.emit_id, "block": emit.block,
        "items": len(emit.items), "lines": emit.lines,
        "seconds_emit": round(t_emit, 2), "seconds_bits": round(t_bits, 2),
        "green": n_green, "red": n_red, "extra": n_extra, "status": "ok",
    })
    return n_green, n_red, n_extra


def split_emit(emit):
    """Split an Emit's items into two halves (bisection)."""
    mid = len(emit.items) // 2
    left = Emit(emit.block, emit.index * 2)
    right = Emit(emit.block, emit.index * 2 + 1)
    # Rebuild groups/unions for the halves (mode A only; mode B
    # emits never reach bisection with a union to rebuild, and their
    # items carry ref None, so add() would fail: handle below).
    for item in emit.items[:mid]:
        if item.ref is not None:
            left.add(item)
        else:
            left.items.append(item)
            left.lines += 1
    for item in emit.items[mid:]:
        if item.ref is not None:
            right.add(item)
        else:
            right.items.append(item)
            right.lines += 1
    left.emit_id_override = "%s-bis%d" % (emit.emit_id, 0)
    right.emit_id_override = "%s-bis%d" % (emit.emit_id, 1)
    return left, right


def run_jobs(ref, part, emitter_cmd, emits, out_dir, records,
             completed_emit_ids, max_depth=4):
    """Run all emits, bisecting any emit whose emit/bits step fails.

    jobs is a queue of (emit, depth). A failed job splits into two
    jobs at depth+1; past max_depth its items are recorded blocked
    with the failure. Returns (green, red, extra) totals.
    """
    # Emit ids are stable across runs (block + index, plus -bisN
    # suffixes after a split), so resume can skip finished emits.
    queue = [(emit, 0) for emit in emits]
    totals = [0, 0, 0]
    while queue:
        emit, depth = queue.pop(0)
        if emit.emit_id in completed_emit_ids:
            continue
        try:
            g, r, e = run_one_emit(
                ref, part, emitter_cmd, emit, out_dir, records,
                keep_bits_on_red=True)
        except (SweepError, subprocess.TimeoutExpired) as err:
            if depth >= max_depth or len(emit.items) <= 1:
                for item in emit.items:
                    records.append({
                        "kind": "item", "block": item.block, "key": item.key,
                        "emit": emit.emit_id, "diff_bits": None,
                        "total_bits": len(item.ref) if item.ref is not None else None,
                        "status": "blocked",
                        "missing": "emitter step failed: %s" % err,
                        "mismatches": [],
                    })
                records.append({
                    "kind": "emit", "emit": emit.emit_id,
                    "block": emit.block, "items": len(emit.items),
                    "lines": emit.lines, "status": "failed",
                    "error": str(err)[:1000],
                })
                continue
            left, right = split_emit(emit)
            queue.insert(0, (left, depth + 1))
            queue.insert(1, (right, depth + 1))
            continue
        totals[0] += g
        totals[1] += r
        totals[2] += e
        completed_emit_ids.add(emit.emit_id)
    return tuple(totals)


# ---------------------------------------------------------------------------
# Probes: confirm blocked classes fail loud, with the exact error
# ---------------------------------------------------------------------------

PROBES = [
    ("dsp", "DSP48E2_X0Y0.AREG.0",
     "DSP48E2 has no public encoding (site_types pins only)"),
    ("uram", "URAM288_X0Y0.MEM.0",
     "URAM288 has no public encoding (prjuray-db and BitFiltrator "
     "cover RAMB18/36 only)"),
    ("clk-cmt", "CMT_RIGHT_X0Y0.PIP.CLK_HROUTE_X0Y0.CLK_HROUTE_X0Y0",
     "CMT_RIGHT has no emitter tile mapping (no xcku3p tilegrid)"),
    ("clk-rclk", "RCLK_INT_L_X0Y0.BUFCE_LEAF_X0Y0.BUF",
     "RCLK tiles have no emitter mapping and no rclk segbits loaded"),
    ("bram-config", "RAMB18_X0Y0.BRAM.RAMB18E2_L.CLKARDCLKINV.V0",
     "BRAM config bits: segbits_bram.db is not loaded by the emitter "
     "and the harness has no resolver"),
    ("hdio", "HDIO_TOP_RIGHT_X0Y0.IOB_X0Y0.IOSTANDARD_IN.LVCMOS12",
     "HDIO tiles have no emitter mapping (no xcku3p tilegrid)"),
    ("xiphy", "XIPHY_BYTE_RIGHT_X0Y0.BITSLICE_RX_TX_X0Y0.BYPASS.RX_Q5.V0",
     "XIPHY tiles have no emitter mapping (no xcku3p tilegrid)"),
    ("int-intf", "INT_INTF_LEFT_TERM_PSS_X0Y0.OUTPUTS_ENABLED.0",
     "INT_INTF tiles have no emitter mapping (no xcku3p tilegrid)"),
]


def run_probes(emitter_cmd, part, out_dir, records):
    """One emit attempt per blocked class; record the exact outcome."""
    for name, line, why in PROBES:
        fasm_path = os.path.join(out_dir, "probe-%s.fasm" % name)
        bit_path = os.path.join(out_dir, "probe-%s.bit" % name)
        with open(fasm_path, "w") as f:
            f.write("# probe: %s\n%s\n" % (why, line))
        try:
            run_emitter_emit(emitter_cmd, fasm_path, part, bit_path)
            outcome = "emit SUCCEEDED (unexpected: probe was expected "
            outcome += "to fail loud; the block may be sweepable)"
            try:
                os.remove(bit_path)
            except OSError:
                pass
        except (SweepError, subprocess.TimeoutExpired) as err:
            outcome = "emit failed loud: %s" % err
        records.append({
            "kind": "probe", "probe": name, "fasm": line,
            "reason": why, "outcome": outcome,
        })
    # FFMUX and MODE.SRL emit fine but `bits` fails loud: probe both.
    for name, line, why in (
        ("ffmux", "SLICE_X0Y0.FFMUXA1.BYP",
         "FFMUX/OUTMUX have no block in the emitter `bits` registry "
         "or the harness registry"),
        ("srl", "SLICE_X1Y0.ALUT.MODE.SRL",
         "MODE.SRL is a block-to-be; `bits` cannot label it"),
    ):
        fasm_path = os.path.join(out_dir, "probe-%s.fasm" % name)
        bit_path = os.path.join(out_dir, "probe-%s.bit" % name)
        bits_path = os.path.join(out_dir, "probe-%s.bits" % name)
        with open(fasm_path, "w") as f:
            f.write("# probe: %s\n%s\n" % (why, line))
        try:
            run_emitter_emit(emitter_cmd, fasm_path, part, bit_path)
            try:
                run_emitter_bits(emitter_cmd, bit_path, part, bits_path)
                outcome = "emit AND bits succeeded (unexpected)"
            except (SweepError, subprocess.TimeoutExpired) as err:
                outcome = "emit ok, bits failed loud: %s" % err
            try:
                os.remove(bit_path)
            except OSError:
                pass
            try:
                os.remove(bits_path)
            except OSError:
                pass
        except (SweepError, subprocess.TimeoutExpired) as err:
            outcome = "emit failed loud: %s" % err
        records.append({
            "kind": "probe", "probe": name, "fasm": line,
            "reason": why, "outcome": outcome,
        })


# ---------------------------------------------------------------------------
# Blocked inventory (enumerated from the DBs, never emitted)
# ---------------------------------------------------------------------------

def blocked_inventory(ref, geom):
    """(block, count, reason) rows for features that cannot be swept."""
    rows = []
    # FFMUX / OUTMUX / CLE PIP counts per CLB tile type.
    ffmux = outmux = 0
    for name in ("clel_l", "clel_r", "clem", "clem_r"):
        db = ref.segbits(name)
        ffmux += sum(1 for k in db if ".FFMUX" in k)
        outmux += sum(1 for k in db if ".OUTMUX" in k)
    rows.append(("FF_INIT", ffmux + outmux,
                 "FFMUX/OUTMUX lines in the CLB segbits DBs (%d FFMUX + "
                 "%d OUTMUX per tile type x 4 tile types x %d slices): "
                 "emitter `bits` fails loud (no block in its registry) "
                 "and the harness registry has no block; needs an "
                 "emitter + registry decision in a later step"
                 % (ffmux // 4, outmux // 4, len(geom.slices))))
    rows.append(("LUTRAM", 8 * len(geom.clem_slices),
                 "MODE.SRL lines (8 per CLEM/CLEM_R slice): block-to-be; "
                 "emitter `bits` fails loud on them"))
    rows.append(("BRAM_CONTENT", 270,
                 "BRAM config lines in segbits_bram.db: the emitter does "
                 "not load segbits_bram.db and the harness has no "
                 "resolver; the 36,864 segbits_bram.block_ram.db lines "
                 "are the same content/parity positions swept via "
                 "BitFiltrator, not a separate universe"))
    rows.append(("IO_CONFIG", 1640,
                 "HDIO features (812 segbits_hdio_top_right.db + 828 "
                 "segbits_hdio_bot_right.db): no HDIO tile mapping; "
                 "unblocker is the xcku3p tilegrid"))
    rows.append(("IO_CONFIG", 1560 * 25,
                 "HPIO features on tile instances other than the one "
                 "bridged HPIO_RIGHT_X0Y0 (26 IOB sites x 59-60 features "
                 "is the per-tile count; the tile-instance count is "
                 "unknown without a tilegrid): unblocker is the xcku3p "
                 "tilegrid"))
    rows.append(("CLK", 104,
                 "CMT_RIGHT features (segbits_cmt_right.db): tile-"
                 "relative, no CMT tile mapping; unblocker is the "
                 "xcku3p tilegrid"))
    rows.append(("CLK", 0,
                 "CMT_LEFT: no segbits file exists in prjuray-db at "
                 "all; no enumerable features; unblocker is Vivado "
                 "clocking fuzzing"))
    rows.append(("CLK", 5360,
                 "RCLK features across the 15 segbits_rclk_*.db files: "
                 "tile-relative, no RCLK tile mapping and the emitter "
                 "loads no rclk DBs; unblocker is the xcku3p tilegrid"))
    rows.append(("CLK", 50,
                 "INT_INTF_LEFT_TERM_PSS features "
                 "(segbits_int_intf_left_term_pss.db): no INT_INTF tile "
                 "mapping; unblocker is the xcku3p tilegrid"))
    rows.append(("CLK", 1756,
                 "XIPHY_BYTE_RIGHT features (segbits_xiphy_byte_right.db): "
                 "no XIPHY tile mapping; unblocker is the xcku3p tilegrid"))
    rows.append(("DSP48E2", 0,
                 "no enumerable feature list: prjuray-db site_types "
                 "documents DSP48E2 pins only, BitFiltrator ships no DSP "
                 "data; unblocker is the Vivado DSP fuzz job"))
    rows.append(("URAM288", 0,
                 "no enumerable feature list: no public URAM encoding; "
                 "unblocker is the Vivado URAM fuzz job"))
    rows.append(("CLK", 0,
                 "BUFGCE/MMCM/PLL site config: no public encoding in "
                 "either source; unblocker is the Vivado clocking fuzz job"))
    return rows

# ---------------------------------------------------------------------------
# Feature-level aggregation (LUT / BRAM bit indices from mismatches)
# ---------------------------------------------------------------------------

def aggregate_features(ref, part, geom, records):
    """Per-feature reds for the code-scheme blocks.

    Returns {block: {feature_key: [details]}}. LUT feature key:
    (x, y, bel, bit i). BRAM feature key: (x, y, parity, bit i).
    A bit is red when any of its code emits mismatches at its
    position. Green counts are derived arithmetically (universe
    minus reds) by the report, never by materializing million-entry
    sets.
    """
    red_items = [r for r in records
                 if r.get("kind") == "item" and r.get("status") == "red"]
    out = {"LUT_INIT": {}, "BRAM_CONTENT": {}, "BRAM_PARITY": {}}
    lut_reds = [r for r in red_items if r["block"] == "LUT_INIT"]
    bram_reds = [r for r in red_items
                 if r["block"] in ("BRAM_CONTENT", "BRAM_PARITY")]
    lut_map = {}
    if lut_reds:
        for tile_type in ("CLEL_L", "CLEL_R", "CLEM", "CLEM_R"):
            for y_ofst in range(N_Y_OFST_CLB):
                for bel in [l + "6LUT" for l in BEL_LETTERS]:
                    for (minor, off), i in lut_pos_to_bit(
                            ref, part, tile_type, y_ofst, bel).items():
                        lut_map[(tile_type, y_ofst, bel, minor, off)] = i
    bram_map = {}
    if bram_reds:
        for parity in (False, True):
            for y_ofst in range(N_Y_OFST_BRAM):
                for (minor, off), i in bram_pos_to_bit(
                        ref, part, y_ofst, parity).items():
                    bram_map[(parity, y_ofst, minor, off)] = i
    for rec in lut_reds:
        key = rec["key"]
        try:
            _, tile, bel, _code = key.split(":")
            x = int(tile.split("_X")[1].split("Y")[0])
            y = int(tile.split("Y")[1])
        except (ValueError, IndexError):
            continue
        tile_type = geom.tile_type_of[x]
        y_ofst = y % N_Y_OFST_CLB
        for m in rec.get("mismatches", []):
            far_s, off = m["far"], m["offset"]
            minor = int(far_s, 16) & 0xFF
            i = lut_map.get((tile_type, y_ofst, bel, minor, off))
            if i is None:
                continue
            feat = (x, y, bel, i)
            out["LUT_INIT"].setdefault(feat, []).append(
                (rec["emit"], far_s, off, m["expected"], m["observed"]))
    for rec in bram_reds:
        key = rec["key"]
        try:
            _, tile, _code = key.split(":")
            x = int(tile.split("_X")[1].split("Y")[0])
            y = int(tile.split("Y")[1])
        except (ValueError, IndexError):
            continue
        parity = rec["block"] == "BRAM_PARITY"
        y_ofst = y % N_Y_OFST_BRAM
        for m in rec.get("mismatches", []):
            far_s, off = m["far"], m["offset"]
            minor = int(far_s, 16) & 0xFF
            i = bram_map.get((parity, y_ofst, minor, off))
            if i is None:
                continue
            feat = (x, y, parity, i)
            out[rec["block"]].setdefault(feat, []).append(
                (rec["emit"], far_s, off, m["expected"], m["observed"]))
    return out


# ---------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------

def actual_emit_count(records):
    """Actual emits run: emit records in the JSONL, not planned emits.

    Bisection splits planned emits, so the planned count understates
    the run (the full single-feature sweep planned 123 emits and ran
    153). The summary must record the actual count.
    """
    return sum(1 for r in records if r.get("kind") == "emit")


def load_records(path):
    records = []
    if os.path.exists(path):
        with open(path) as f:
            for line in f:
                line = line.strip()
                if line:
                    records.append(json.loads(line))
    return records


def summarize(records, geom):
    """Per-block totals: items/green/red from emit records, red
    items from item records, extras/probes/emits listed."""
    per_block = {}
    for rec in records:
        if rec.get("kind") != "emit":
            continue
        b = per_block.setdefault(rec["block"], {
            "items": 0, "green": 0, "red": 0, "blocked": 0,
            "emits": 0,
        })
        b["items"] += rec.get("items", 0)
        b["green"] += rec.get("green", 0) or 0
        b["red"] += rec.get("red", 0) or 0
        b["emits"] += 1
    for rec in records:
        if rec.get("kind") == "item" and rec.get("status") == "blocked":
            b = per_block.setdefault(rec["block"], {
                "items": 0, "green": 0, "red": 0, "blocked": 0, "emits": 0,
            })
            b["blocked"] += 1
    extras = [r for r in records if r.get("kind") == "extra"]
    probes = [r for r in records if r.get("kind") == "probe"]
    emits = [r for r in records if r.get("kind") == "emit"]
    return per_block, extras, probes, emits


def write_report(ref, part, geom, out_dir, records, report_path,
                 commands, blocked_rows, started, finished):
    per_block, extras, probes, emits = summarize(records, geom)
    agg = aggregate_features(ref, part, geom, records)
    lines = []
    lines.append("# Single-feature sweep report (xcku3p, tier 1)")
    lines.append("")
    lines.append("Sweep of every tile instance x every feature from the "
                 "public DBs, one feature per emit slot, packed onto "
                 "disjoint bit positions. Part: %s. Started: %s. "
                 "Finished: %s." % (part, started, finished))
    lines.append("")
    lines.append("Emitter: the worktree `helion-bitgen` binary "
                 "(target/release/helion-bitgen on fm/ultra-bitgen-sweeps, "
                 "base afa8e911). No emitter code changed in this step; "
                 "this is mechanics plus measurement. No Vivado anywhere; "
                 "tier 1 (zero diff vs public DBs) is the only bar.")
    lines.append("")
    lines.append("Band bits: the emitter `bits` output excludes the H-1 "
                 "band (absolute frame bits 1440-1487 of block-0 frames), "
                 "so band bits never appear in a diff. A feature writing "
                 "a band bit fails loud at emit time and is bisected to "
                 "isolate it.")
    lines.append("")
    lines.append("## Per-block coverage")
    lines.append("")
    lines.append("| Block | Features in universe | Items swept | Green | Red | "
                 "Blocked | Coverage |")
    lines.append("| --- | ---: | ---: | ---: | ---: | ---: | ---: |")
    expected_items = {
        "LUT_INIT": len(geom.slices) * 8 * 6,
        "FF_INIT": len(geom.slices) * (16 * 2 * 2 + len(FF_CONTROL_FEATURES)),
        "CARRY8": len(geom.slices) * 24,
        "BRAM_CONTENT": len(geom.brams) * 14,
        "BRAM_PARITY": len(geom.brams) * 11,
        "INT_PIP": 8 * 3778 + (len(geom.int_tiles) - 8),
        "LUTRAM": len(geom.clem_slices) * 52,
        "IO_CONFIG": 1560,
    }
    for block in ("LUT_INIT", "FF_INIT", "CARRY8", "BRAM_CONTENT",
                  "BRAM_PARITY", "INT_PIP", "LUTRAM", "IO_CONFIG",
                  "CLK", "DSP48E2", "URAM288"):
        total = FEATURE_COUNTS.get(block, lambda g: 0)(geom)
        b = per_block.get(block, {})
        swept = b.get("items", 0)
        green = b.get("green", 0)
        red = b.get("red", 0)
        blocked = b.get("blocked", 0)
        # Blocked inventory counts live outside the item records.
        inv_blocked = sum(c for (bl, c, _r) in blocked_rows if bl == block)
        fully = (block in expected_items
                 and swept == expected_items[block] and swept > 0)
        if block in agg:
            feat_red = len(agg[block])
            if fully:
                feat_green = total - feat_red
                cov = "%.2f%% (%d/%d features green)" % (
                    100.0 * feat_green / total, feat_green, total)
            else:
                cov = "partial run (%d/%d items swept, %d red features)" % (
                    swept, expected_items.get(block, 0), feat_red)
            lines.append("| %s | %d | %d | %d | %d | %d (+%d inventoried) | %s |" % (
                block, total, swept, green, red, blocked, inv_blocked, cov))
        else:
            if fully:
                cov = "%.2f%% (%d/%d features green)" % (
                    100.0 * green / total, green, total) if total else "n/a"
            elif swept:
                cov = "partial run (%d/%d items swept)" % (
                    swept, expected_items.get(block, 0))
            else:
                cov = "not swept"
            lines.append("| %s | %d | %d | %d | %d | %d (+%d inventoried) | %s |" % (
                block, total, swept, green, red, blocked, inv_blocked, cov))
    lines.append("")
    lines.append("Coverage notes:")
    lines.append("- LUT_INIT, BRAM_CONTENT, BRAM_PARITY coverage is "
                 "per bit index (binary-code scheme): a feature is one "
                 "bit position of one BEL / RAMB.")
    lines.append("- INT_PIP coverage is per (tile, PIP) pair in the "
                 "reduced sweep: every PIP on 8 tiles plus 1 PIP on "
                 "every INT tile. The full 102,459,360-pair cross "
                 "product is not enumerated; the encoding is "
                 "tile-independent by DB construction (one "
                 "segbits_int.db) and every tile's base FAR is tested "
                 "by the tile sweep.")
    lines.append("- IO_CONFIG coverage is tile-relative (provisional "
                 "HPIO base FAR 0x000) on the one bridged tile; the "
                 "absolute placement stays blocked on the tilegrid.")
    lines.append("")
    lines.append("## Red list")
    lines.append("")
    red_items = [r for r in records if r.get("kind") == "item"
                 and r.get("status") == "red"]
    if not red_items and not extras:
        lines.append("No reds. No global extras.")
    else:
        lines.append("Red items: %d. Global extras: %d." % (len(red_items), len(extras)))
        lines.append("")
        for block in ("LUT_INIT", "FF_INIT", "CARRY8", "BRAM_CONTENT",
                      "BRAM_PARITY", "INT_PIP", "LUTRAM", "IO_CONFIG"):
            if block in agg:
                reds = agg[block]
                if not reds:
                    continue
                lines.append("### %s: %d red features" % (block, len(reds)))
                for feat in sorted(reds):
                    lines.append("- %s" % (feat,))
                    for (emit_id, far_s, off, exp, obs) in reds[feat][:6]:
                        lines.append("  - emit %s: %s offset %d: expected %d, emitted %d"
                                     % (emit_id, far_s, off, exp, obs))
                lines.append("")
            else:
                block_reds = [r for r in red_items if r["block"] == block]
                if not block_reds:
                    continue
                lines.append("### %s: %d red items" % (block, len(block_reds)))
                for r in block_reds:
                    lines.append("- %s (emit %s, diff %s of %s bits)" % (
                        r["key"], r["emit"], r["diff_bits"], r["total_bits"]))
                    for m in r.get("mismatches", [])[:10]:
                        lines.append("  - %s offset %d: expected %d, emitted %d"
                                     % (m["far"], m["offset"], m["expected"], m["observed"]))
                lines.append("")
        if extras:
            lines.append("### Global extras (emitted set bits no feature constrains)")
            for e in extras[:200]:
                lines.append("- emit %s: %s offset %d: %s" % (
                    e["emit"], e["far"], e["offset"], e["detail"]))
            if len(extras) > 200:
                lines.append("- ... and %d more (see sweep.jsonl)" % (len(extras) - 200))
            lines.append("")
    lines.append("## Blocked inventory")
    lines.append("")
    for (block, count, reason) in blocked_rows:
        lines.append("- %s: %s: %s" % (block, count, reason))
    lines.append("")
    lines.append("## Probes (blocked classes, exact emitter outcome)")
    lines.append("")
    for p in probes:
        lines.append("- %s (`%s`): %s" % (p["probe"], p["fasm"], p["outcome"]))
    lines.append("")
    lines.append("## Emits run")
    lines.append("")
    lines.append("Total emits: %d. Total seconds (emit+bits): %.1f." % (
        len(emits),
        sum((e.get("seconds_emit", 0) + e.get("seconds_bits", 0)) for e in emits)))
    for e in emits:
        lines.append("- %s: %s items, %s lines, emit %.1fs, bits %.1fs, "
                     "green %s, red %s, extra %s, status %s" % (
                         e["emit"], e.get("items"), e.get("lines"),
                         e.get("seconds_emit", 0), e.get("seconds_bits", 0),
                         e.get("green"), e.get("red"), e.get("extra"),
                         e.get("status")))
    lines.append("")
    lines.append("## Commands")
    lines.append("")
    for cmd in commands:
        lines.append("- `%s`" % cmd)
    lines.append("")
    lines.append("Results: sweep.jsonl and sweep_summary.json in %s. "
                 "Per-emit FASM files are kept in the same directory; "
                 ".bit files are deleted after diffing (the FASM plus "
                 "the emitter reproduces them); .bits files are kept "
                 "only for emits with reds or extras." % out_dir)
    lines.append("")
    with open(report_path, "w") as f:
        f.write("\n".join(lines))
    return report_path


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def run_pairs(args, ref, geom, out_dir, jsonl_path, records,
              completed_emit_ids, started, commands):
    """Pair-sweep driver: enumerate, pack, run, probe, summarize."""
    t0 = time.time()
    pair_items, stats, contra_samples = enum_pairs(
        ref, args.part, geom)
    print("pair enumeration in %.1fs" % (time.time() - t0))
    for block in ("FF_INIT", "CARRY8", "LUTRAM", "INT_PIP", "IO_CONFIG",
                  "LUT_INIT", "BRAM_CONTENT", "BRAM_PARITY"):
        s = stats[block]
        print("%s: agree instances %d, types %d, swept %d; "
              "contra instances %d, types %d" % (
                  block, s["agree_instances"], s["agree_types"],
                  s["agree_swept"], s["contra_instances"],
                  s["contra_types"]))
    print("cross-block pairs: %d (%s)" % (
        stats["CROSS_BLOCK"]["cross_block_pairs"],
        stats["CROSS_BLOCK"]["scope"]))
    n_probes = args.contra_probes
    samples = {}
    if n_probes:
        for block, lst in contra_samples.items():
            samples[block] = lst[:n_probes]

    all_emits = []
    for block in ("FF_INIT", "CARRY8", "LUTRAM", "INT_PIP", "IO_CONFIG"):
        items = [i for i in pair_items if i.block == block]
        if not items:
            continue
        t0 = time.time()
        emits = pack_block(block, items, args.max_lines)
        for e in emits:
            e.emit_id_override = "PAIR-%s-%03d" % (block, e.index)
        print("%s: %d pair items in %d emits (%.1fs)" % (
            block, len(items), len(emits), time.time() - t0))
        all_emits.extend(emits)
    if args.limit_emits:
        all_emits = all_emits[:args.limit_emits]
    print("total pair emits planned: %d" % len(all_emits))
    if args.dry_run:
        plan_path = os.path.join(out_dir, "pair_plan.json")
        with open(plan_path, "w") as f:
            json.dump([{"emit": e.emit_id, "block": e.block,
                        "items": len(e.items), "lines": e.lines}
                       for e in all_emits], f, indent=1)
        print("dry run: plan written to %s" % plan_path)
        return 0

    totals = run_jobs(ref, args.part, args.emitter, all_emits, out_dir,
                      records, completed_emit_ids)
    print("pair sweep totals (green, red, extra): %s" % (totals,))
    if samples:
        run_contra_probes(ref, args.part, args.emitter, samples,
                          out_dir, records)
        print("contra probes run: %d" % sum(
            len(v) for v in samples.values()))

    with open(jsonl_path, "w") as f:
        for rec in records:
            f.write(json.dumps(rec, sort_keys=True) + "\n")
    finished = datetime.datetime.now().isoformat(timespec="seconds")
    summary = {
        "part": args.part, "started": started, "finished": finished,
        "mode": "pairs",
        "totals": {"green": totals[0], "red": totals[1], "extra": totals[2]},
        "emits": actual_emit_count(records),
        "planned_emits": len(all_emits),
        "pair_stats": stats,
        "contra_probes": sum(len(v) for v in samples.values()),
        "commands": commands,
        "out_dir": out_dir,
    }
    with open(os.path.join(out_dir, "pair_summary.json"), "w") as f:
        json.dump(summary, f, indent=1, sort_keys=True)
    print("pair summary written to %s" % os.path.join(
        out_dir, "pair_summary.json"))
    print("results in %s" % out_dir)
    return 0


def main(argv=None):
    ap = argparse.ArgumentParser(
        description="Single-feature and pair sweeps")
    ap.add_argument("--part", default=PART)
    ap.add_argument("--blocks", default=",".join(
        ["LUT_INIT", "FF_INIT", "CARRY8", "BRAM_CONTENT", "BRAM_PARITY",
         "INT_PIP", "LUTRAM", "IO_CONFIG"]))
    ap.add_argument("--out", default=None,
                    help="results dir (default: ~/workspace/ultra-bitgen-sweeps-logs/sweeps-<stamp>)")
    ap.add_argument("--emitter", default=DEFAULT_EMITTER)
    ap.add_argument("--ref-dir", default=None,
                    help="reference data dir (default: harness/reference if present, else the crew copy)")
    ap.add_argument("--max-lines", type=int, default=200000)
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--limit-emits", type=int, default=0,
                    help="stop after N emits (0 = no limit)")
    ap.add_argument("--report-only", action="store_true")
    ap.add_argument("--report-path", default=None)
    ap.add_argument("--pairs", action="store_true",
                    help="run the same-bit pair sweep instead of the "
                    "single-feature sweep")
    ap.add_argument("--contra-probes", type=int, default=4,
                    help="contradictory pairs sampled per block for "
                    "observation probes (0 = none)")
    args = ap.parse_args(argv)

    started = datetime.datetime.now().isoformat(timespec="seconds")
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    if args.out:
        out_dir = os.path.expanduser(args.out)
    else:
        out_dir = os.path.expanduser(
            "~/workspace/ultra-bitgen-sweeps-logs/sweeps-%s" % stamp)
    os.makedirs(out_dir, exist_ok=True)
    commands = [" ".join(sys.argv)]

    if args.ref_dir:
        ref_dir = os.path.expanduser(args.ref_dir)
    elif os.path.isdir(os.path.join(HARNESS_DIR, "reference")):
        ref_dir = os.path.join(HARNESS_DIR, "reference")
    else:
        ref_dir = CREW_REF_DIR
    ref = Tier1Reference(ref_dir=ref_dir)
    geom = Geometry(ref, args.part)
    report_path = args.report_path or os.path.join(
        HARNESS_DIR, "SWEEP_SINGLE_FEATURE_REPORT.md")

    jsonl_path = os.path.join(out_dir, "sweep.jsonl")
    records = load_records(jsonl_path)
    completed_emit_ids = set(
        r["emit"] for r in records
        if r.get("kind") == "emit" and r.get("status") == "ok")

    if args.report_only:
        blocked_rows = blocked_inventory(ref, geom)
        write_report(ref, args.part, geom, out_dir, records, report_path,
                     commands, blocked_rows, started,
                     datetime.datetime.now().isoformat(timespec="seconds"))
        print("report written to %s" % report_path)
        return 0

    if not os.path.exists(args.emitter):
        print("error: emitter not found: %s" % args.emitter)
        return 2
    if args.pairs:
        return run_pairs(args, ref, geom, out_dir, jsonl_path, records,
                         completed_emit_ids, started, commands)
    blocks = [b.strip() for b in args.blocks.split(",") if b.strip()]
    for b in blocks:
        if b not in ENUMERATORS and b not in ("BRAM_CONTENT", "BRAM_PARITY"):
            print("error: unknown block %r" % b)
            return 1
    # BRAM_CONTENT and BRAM_PARITY share one enumerator; enumerate once.
    print("geometry: %d slices, %d CLEM slices, %d RAMBs, %d INT tiles"
          % (len(geom.slices), len(geom.clem_slices), len(geom.brams),
             len(geom.int_tiles)))

    # Analytical disjointness proofs for the mode-B blocks.
    if "LUT_INIT" in blocks:
        verify_lut_disjoint(ref, args.part, geom)
        print("LUT disjointness proven (480 sets per tile type)")
    if "BRAM_CONTENT" in blocks or "BRAM_PARITY" in blocks:
        verify_bram_disjoint(ref, args.part, geom)
        print("BRAM disjointness proven (24 Y_ofst sets per kind)")

    all_emits = []
    for block in blocks:
        t0 = time.time()
        if block in ("BRAM_CONTENT", "BRAM_PARITY"):
            # One enumerator covers both kinds; pack each kind under
            # its own block name so emits stay single-block.
            items = [i for i in enum_bram(ref, args.part, geom)
                     if i.block == block]
        else:
            items = ENUMERATORS[block](ref, args.part, geom)
        print("%s: %d items in %.1fs" % (block, len(items), time.time() - t0))
        t0 = time.time()
        emits = pack_block(block, items, args.max_lines)
        print("%s: %d emits in %.1fs" % (block, len(emits), time.time() - t0))
        all_emits.extend(emits)
    if args.limit_emits:
        all_emits = all_emits[:args.limit_emits]
    print("total emits planned: %d" % len(all_emits))
    if args.dry_run:
        plan_path = os.path.join(out_dir, "plan.json")
        with open(plan_path, "w") as f:
            json.dump([{"emit": e.emit_id, "block": e.block,
                        "items": len(e.items), "lines": e.lines}
                       for e in all_emits], f, indent=1)
        print("dry run: plan written to %s" % plan_path)
        return 0

    totals = run_jobs(ref, args.part, args.emitter, all_emits, out_dir,
                      records, completed_emit_ids)
    print("sweep totals (green, red, extra): %s" % (totals,))
    run_probes(args.emitter, args.part, out_dir, records)

    with open(jsonl_path, "w") as f:
        for rec in records:
            f.write(json.dumps(rec, sort_keys=True) + "\n")
    summary = {
        "part": args.part, "started": started,
        "finished": datetime.datetime.now().isoformat(timespec="seconds"),
        "totals": {"green": totals[0], "red": totals[1], "extra": totals[2]},
        "emits": actual_emit_count(records),
        "planned_emits": len(all_emits),
        "out_dir": out_dir,
    }
    with open(os.path.join(out_dir, "sweep_summary.json"), "w") as f:
        json.dump(summary, f, indent=1, sort_keys=True)
    blocked_rows = blocked_inventory(ref, geom)
    write_report(ref, args.part, geom, out_dir, records, report_path,
                 commands, blocked_rows, started, summary["finished"])
    print("report written to %s" % report_path)
    print("results in %s" % out_dir)
    return 0


if __name__ == "__main__":
    sys.exit(main())
