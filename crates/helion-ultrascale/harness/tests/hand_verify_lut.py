#!/usr/bin/env python3
# Hand verification for the diff harness (evidence, not a unit test).
#
# Recomputes one block's reference bits BY HAND, with an independent
# code path: it reads the BitFiltrator JSONs directly with the json
# module only (no harness imports for the computation), applies the
# FAR layout by hand, and only then compares against the harness
# resolver's output. It also recomputes one diff_bits value by hand
# on a small synthetic case (flip 3 LUT bits, count the differences
# with a plain loop).
#
# Run: python3 tests/hand_verify_lut.py
# Exit 0 only when every hand computation matches the harness.
#
# No em dashes anywhere.

import json
import os
import sys

HARNESS_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, HARNESS_DIR)

REF_DIR = os.path.join(HARNESS_DIR, "reference")


def hand_lut_bits():
    """Blinky A6LUT, SLICE_X0Y0, INIT 0x5555555555555555, by hand."""
    with open(os.path.join(REF_DIR, "ULTRASCALE_PLUS.json")) as f:
        arch = json.load(f)
    with open(os.path.join(REF_DIR, "xcku3p.json")) as f:
        dev = json.load(f)
    # Device file, by hand: SLR0, row major 0, CLB column 0.
    row = dev["slrs"]["SLR0"]["rowMajors"]["0"]
    col_major = row["clb_colMajors"]["0"]
    tile_type = row["clb_tileTypes"]["0"]
    assert col_major == 5, col_major
    assert tile_type == "CLEL_R", tile_type
    # FAR layout by hand: block 0, row 0, col 5, minor from LutLoc.
    lut = arch["CLEL_R"]["LutLoc"]["Y_ofst"]["0"]
    minors = lut["minor"]["A6LUT"]
    ofsts = lut["frame_ofst"]["A6LUT"]
    assert len(minors) == 64 and len(ofsts) == 64
    init = 0x5555555555555555
    hand = {}
    for i in range(64):
        far = (0 << 24) | (0 << 18) | (5 << 8) | minors[i]
        hand[(far, ofsts[i])] = (init >> i) & 1
    return hand


def hand_diff_bits(ref_bits, n_flips):
    """Flip the first n_flips reference bits (sorted order) by hand
    and count differing bits with a plain loop."""
    flipped = dict(ref_bits)
    for pos in sorted(ref_bits)[:n_flips]:
        flipped[pos] = 1 - flipped[pos]
    diff = 0
    for pos, req in ref_bits.items():
        if flipped.get(pos, 0) != req:
            diff += 1
    for pos, val in flipped.items():
        if pos not in ref_bits and val == 1:
            diff += 1
    return diff, flipped


def main():
    from diff_engine import compute_diff
    from reference import Tier1Reference

    hand = hand_lut_bits()
    print("hand: blinky A6LUT has %d reference bits" % len(hand))
    print("hand: first 5 (FAR, offset, value):")
    for pos in sorted(hand)[:5]:
        print("  FAR 0x%08x offset %4d value %d" % (pos[0], pos[1], hand[pos]))

    ref = Tier1Reference()
    harness = ref.resolve_lut("xcku3p", 0, 0, "A6LUT", 0x5555555555555555)
    if harness != hand:
        print("MISMATCH: harness resolver differs from hand computation")
        only_hand = set(hand) - set(harness)
        only_harness = set(harness) - set(hand)
        print("  only in hand: %s" % sorted(only_hand)[:5])
        print("  only in harness: %s" % sorted(only_harness)[:5])
        return 1
    print("MATCH: harness LUT resolver equals hand computation on all 64 bits")

    # Hand diff_bits on a small case: flip 3 bits.
    hand_diff, flipped = hand_diff_bits(hand, 3)
    harness_diff = compute_diff(hand, flipped)
    print("hand diff_bits for 3 flipped LUT bits: %d" % hand_diff)
    print("harness compute_diff for the same case: %d" % harness_diff)
    if hand_diff != 3 or harness_diff != 3:
        print("MISMATCH: expected diff_bits 3")
        return 1
    print("MATCH: diff_bits is 3 by hand and by the harness")
    print("HAND VERIFICATION PASSED")
    return 0


if __name__ == "__main__":
    sys.exit(main())
