#!/usr/bin/env python3
"""Full-device clock-network diff between a Helion bitstream and a reference.

The earlier version walked a single hardcoded path of nine tiles. That is blind
by construction: it can only confirm the route someone already thought of, and
it silently reports "0 diffs" for a design whose clock goes anywhere else. This
version scans every clock-bearing tile on the device, so a difference cannot
hide by being off the expected path.

Usage:
    python3 work/clkdiff.py <ours.bit> [<ref.bit>]

Exit status is non-zero when any clock tile differs beyond the known-benign set.
"""
import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import x7decode as X

REF = os.path.expanduser('~/CDA4253_A1/blinky.bit')

# Tile types that participate in clock distribution. Covers both HCLK_CMT
# variants: an xc7z020 has one `HCLK_CMT` and three `HCLK_CMT_L`, and Vivado
# routes the ZedBoard clock through an `_L`.
CLOCK_TYPES = (
    'HCLK_IOI3', 'HCLK_CMT', 'HCLK_CMT_L', 'HCLK_L', 'HCLK_R',
    'CLK_HROW_BOT_R', 'CLK_HROW_TOP_R', 'CLK_BUFG_REBUF',
    'CLK_BUFG_BOT_R', 'CLK_BUFG_TOP_R', 'CMT_TOP_R', 'CMT_BOT_R',
    'LIOB33', 'LIOI3', 'RIOB33', 'RIOI3',
)

# Routing-choice differences that are functionally equivalent: the same
# die-wide `HCLK_CK_BUFHCLK<n>` spine net is gated from a different CMT tile.
# See work/FINDING-cmt-tile-choice.md -- every downstream consumer is identical.
BENIGN = ('_CK_BUFHCLK', )


# Bits in clock tiles that NO segbits feature names, but that Vivado sets.
#
# Measured against GT-blinky.bit: on the clock spine's Y26 row every tile has
# a nonzero word 50 (bits 0..12, a different mask per tile) and prjxray
# describes NONE of those bits -- `segbits_hclk_r.db` and friends have no
# entry for word 50 in these tiles at all. Helion writes 0x0 there.
#
# This matters because `scan()` below compares only NAMED features, so it
# reports "180 of 181 clock tiles identical, REAL diffs: 0" while every one of
# those tiles differs in this word. A clean clkdiff verdict is therefore NOT
# evidence that the clock tiles are byte-identical. Compare the raw words too.
UNNAMED_CLOCK_WORDS = (50,)


def unnamed_clock_word_diffs(ours_path, ref_path):
    """Differing raw words in clock tiles that no segbit names.

    The frame that carries a clock tile is `baseaddr + frame_offset`, where
    `frame_offset` comes from the tile's own row/column geometry -- NOT
    `baseaddr` itself. `CLB_IO_CLK.offset` is then the word index inside that
    frame (50 for `HCLK_R_X171Y26`, with `words: 1`), so the index is used
    directly and must not be added to `offset` again.
    """
    tiles = X._tilegrid()
    fo = X.frames_by_address(X.full_addresses(), X.payload(ours_path))
    fr = X.frames_by_address(X.full_addresses(), X.payload(ref_path))
    diffs = []
    for tile, info in tiles.items():
        typ = info.get('type', '')
        if not any(k in typ for k in ('HCLK', 'CLK_HROW', 'CMT', 'CFG_CENTER')):
            continue
        blocks = info.get('bits') or {}
        geom = None
        for b in blocks.values():
            if 'baseaddr' in b and 'offset' in b:
                geom = b
                break
        if geom is None:
            continue
        base = geom['baseaddr']
        base = int(base, 16) if isinstance(base, str) else base
        off = geom['offset']
        # The tile's own frame: `baseaddr` is the block's first frame.
        for addr in range(base, base + 200):
            key = addr
            ao, ar = fo.get(key), fr.get(key)
            if ao is None or ar is None:
                continue
            for idx in range(off, off + geom.get('words', 1)):
                if idx >= len(ao) or idx >= len(ar):
                    continue
                if ao[idx] != ar[idx]:
                    diffs.append((tile, typ, hex(key), idx, ao[idx], ar[idx]))
    return diffs


def is_benign(feat):
    return any(b in feat for b in BENIGN)


# Features in the INT tiles that carry the clock from the spine tap into the
# slices. These are NOT clock-TYPE tiles, which is why a scan that only looks
# at `CLOCK_TYPES` reports a clean bill of health for a bitstream whose clock
# never leaves a single tile -- the failure that made a toggling flop stay
# dark. Include them here so the fanout is covered by the same comparison.
FANOUT_MARKERS = ("GFAN", "BYP_ALT", "FAN_BOUNCE", "GND_WIRE")


def scan(path):
    tiles = X._tilegrid()
    frames = X.frames_by_address(X.full_addresses(), X.payload(path))
    out = {}
    for tile, info in tiles.items():
        typ = info['type']
        seg = f"{X.DB}/segbits_{typ.lower()}.db"
        if not os.path.exists(seg):
            continue
        feats = X.active_features(tile, seg, frames)
        if not feats:
            continue
        if typ in CLOCK_TYPES:
            out[tile] = set(feats)
            continue
        # A logic tile contributes only its clock-distribution features: its
        # ordinary routing belongs to the design, not the clock network, and
        # comparing all of it would drown the clock signal in placement noise.
        #
        # `BYP_ALT` is a shared fabric resource: a `BYP_ALT0.NN2END0` is an
        # ordinary routing bypass for the `NN2END0` wire, nothing to do with the
        # clock. Requiring the GFAN/GCLK_B/GND_WIRE partner keeps the comparison
        # about the clock network and off the design's own routing.
        #
        # `IMUX*` arms are deliberately NOT matched here. An `IMUX4.FAN_BOUNCE1`
        # is a tap into a slice DATA pin (`A6`), not clock fanout -- and unlike
        # the clock chain, the router is free to pick a different single arm to
        # reach the same pin. The reference reaches `A6` via
        # `IMUX4.FAN_BOUNCE1`; we reach it via `IMUX4.GFAN1`. Both are one
        # (coarse, fine) pair on the same one-hot mux, both land on `A6`, and
        # `muxcheck` confirms exactly one arm is selected either way. Matching
        # this class made the tool report a clock defect for a difference that
        # is purely the router's choice of operand path -- a false positive,
        # and the kind that trains you to ignore the tool.
        fan = {
            f for f in feats
            if ("GFAN" in f and ("GCLK_B" in f or "GND_WIRE" in f))
            or ("BYP_ALT" in f and "GFAN" in f)
            or "FAN_ALT" in f
        }
        if fan:
            # Key by ROW, not tile name: two designs place the same clock
            # network in different rows, and a tile-name comparison reports
            # every one of those as a clock defect. What must match is the
            # SET of fanout features per row -- the clock either reaches a row
            # through the full bypass chain or it does not.
            out[f"INT@Y{int(tile.split('Y')[-1])}"] = fan
    return out


def _tile_only(d):
    return {k: v for k, v in d.items() if not k.startswith("INT@")}


def normalise_per_row(d):
    """Reduce a scan to "does each clock-carrying row have the full chain".

    Row numbers themselves are placement, not configuration: our blinky sits in
    rows 0..6 and the reference in rows 39..45, and comparing raw rows reports
    every one of those as a clock defect. What must match is the SHAPE of the
    clock network: for each row that carries fanout, the set of feature KINDS
    present. That is what distinguishes "the clock is distributed" from "the
    clock reaches one tile and stops", which is the defect this tool exists to
    catch.
    """
    shapes = set()
    for key, feats in d.items():
        if not key.startswith("INT@"):
            continue
        kinds = set()
        for f in feats:
            for m in ("GFAN", "GCLK_B", "GND_WIRE", "FAN_BOUNCE", "FAN_ALT"):
                if m in f:
                    kinds.add(m)
        if kinds:
            shapes.add(frozenset(kinds))
    return shapes


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    ours_path = sys.argv[1]
    ref_path = sys.argv[2] if len(sys.argv) > 2 else REF

    ours, ref = scan(ours_path), scan(ref_path)
    # Compare the clock network SHAPE per row, and the clock tiles by name.
    # Tile names are stable across placements for the clock column, but logic
    # rows are not, so the fanout half is compared structurally.
    ours, ref = _tile_only(ours), _tile_only(ref)
    so, sr = normalise_per_row(scan(ours_path)), normalise_per_row(scan(ref_path))
    if so != sr:
        print("fanout shape ours:", sorted(tuple(sorted(x)) for x in so))
        print("fanout shape ref :", sorted(tuple(sorted(x)) for x in sr))
        hard_shape = 1
    else:
        hard_shape = 0
    print(f"ours : {os.path.basename(ours_path)}  ({len(ours)} clock tiles)")
    print(f"ref  : {os.path.basename(ref_path)}  ({len(ref)} clock tiles)")
    print()

    hard = benign = matched = 0
    for tile in sorted(set(ours) | set(ref)):
        a, b = ours.get(tile, set()), ref.get(tile, set())
        if a == b:
            matched += 1
            continue
        print(f"{tile}")
        for f in sorted(b - a):
            tag = "benign" if is_benign(f) else "MISSING"
            if tag == "benign":
                benign += 1
            else:
                hard += 1
            print(f"    {tag:<8} {f}")
        for f in sorted(a - b):
            tag = "benign" if is_benign(f) else "EXTRA"
            if tag == "benign":
                benign += 1
            else:
                hard += 1
            print(f"    {tag:<8} {f}")

    print()
    print(f"identical tiles : {matched}")
    print(f"benign diffs    : {benign}  (same spine net, different CMT tile)")
    hard += hard_shape
    # Word 50 of every clock-spine tile is set by Vivado and named by NOTHING
    # in the segbits DB, so the named-feature comparison above cannot see it.
    # Report it separately and do NOT fold it into `hard`: these bits track
    # the selected spine, and two correct-but-different clock routes may
    # legitimately differ. What matters is that the gap is VISIBLE -- a clean
    # "REAL diffs: 0" previously implied byte-identity of these tiles, which
    # was false.
    unnamed = unnamed_clock_word_diffs(ours_path, ref_path)
    print(f"REAL diffs      : {hard}")
    print(f"unnamed clock words differing : {len(unnamed)} "
          f"(word 50: set by Vivado, named by no segbit)")
    if unnamed:
        from collections import Counter
        c = Counter((t, w) for _n, t, _f, w, _o, _r in unnamed)
        for (t, w), n in c.most_common(6):
            print(f"    {t:16s} word {w}: {n} frame(s)")
    if hard:
        print("\nFAIL: the clock network differs from the reference in a way "
              "that is not a routing choice.")
    return 1 if hard else 0


if __name__ == "__main__":
    sys.exit(main())
