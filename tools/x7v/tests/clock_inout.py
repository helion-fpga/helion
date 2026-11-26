#!/usr/bin/env python3
"""R5 of the clock check (HELION_GAPS.md G69/G73): a leaf fed through the
neighbouring HCLK tile needs that tile's row clock buffer enabled.

Without the enable the ZedBoard ran no flop or block RAM on those columns
(2026-11-18), while every other check passed. Vivado's reference builds never
take this path (they use the leaf tile's own BUFHCLK8..11), so the rule rests
on the board result: dead without the bit, running with it.

    python3 tests/clock_inout.py            # from tools/x7v; needs the X-Ray database
"""
import os
import sys
from types import SimpleNamespace

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from x7v import clockcheck  # noqa: E402
from x7v.db import Db  # noqa: E402

db = Db()


def r5(tiles):
    dec = SimpleNamespace(tiles={t: SimpleNamespace(features=f) for t, f in tiles.items()})
    return [e for e in clockcheck.check(db, dec).errors if e.startswith("R5")]


leaf_l = {"HCLK_L_X21Y26": ["HCLK_LEAF_CLK_B_BOTL0.HCLK_CK_OUTIN_L0"]}
assert len(r5(leaf_l)) == 1, r5(leaf_l)
assert r5({**leaf_l, "HCLK_R_X22Y26": ["ENABLE_BUFFER.HCLK_CK_BUFHCLK0"]}) == []
# The enable must be for the same row clock.
assert len(r5({**leaf_l, "HCLK_R_X22Y26": ["ENABLE_BUFFER.HCLK_CK_BUFHCLK1"]})) == 1
# The mirror case: an HCLK_R leaf through the HCLK_L tile's BUFHCLK8..11.
# tileconn stitches HCLK_CK_OUTIN_R4..7 to HCLK_CK_INOUT_L0..3 (BUFHCLK8..11).
leaf_r = {"HCLK_R_X22Y26": ["HCLK_LEAF_CLK_B_BOT0.HCLK_CK_OUTIN_R5"]}
assert len(r5(leaf_r)) == 1, r5(leaf_r)
assert r5({**leaf_r, "HCLK_L_X21Y26": ["ENABLE_BUFFER.HCLK_CK_BUFHCLK9"]}) == []
# A leaf in the buffer's own tile is R4's business, not R5's.
assert r5({"HCLK_R_X22Y26": ["HCLK_LEAF_CLK_B_BOT0.HCLK_CK_BUFHCLK0"]}) == []


def r6(tiles):
    dec = SimpleNamespace(tiles={t: SimpleNamespace(features=f) for t, f in tiles.items()})
    return [e for e in clockcheck.check(db, dec).errors if e.startswith("R6")]


# R6: a leaf through the link is an error even with R5's enable set (the
# XC7S25 columns clocked this way had no clock on the Arty S7-25).
assert len(r6({**leaf_l, "HCLK_R_X22Y26": ["ENABLE_BUFFER.HCLK_CK_BUFHCLK0"]})) == 1
assert len(r6(leaf_r)) == 1
assert r6({"HCLK_R_X22Y26": ["HCLK_LEAF_CLK_B_BOT0.HCLK_CK_BUFHCLK0", "ENABLE_BUFFER.HCLK_CK_BUFHCLK0"]}) == []
print("clock_inout: ok")
