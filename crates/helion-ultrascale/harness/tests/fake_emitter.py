#!/usr/bin/env python3
# Fake emitter for harness tests ONLY. Never used by a real diff run.
#
# Implements the emitter protocol (see emitter.py) by construction:
# emit writes a fake .bit (sync word + xcku3p IDCODE + marker) and
# stashes the input FASM in a sidecar; parse prints the sidecar back;
# bits resolves the sidecar FASM through the harness reference (so the
# fake emitter is exactly correct unless FAKE_FLIP_COUNT flips the
# first N reference bits, which is how the tests exercise red paths).
# FAKE_PARSE_CORRUPT=1 makes parse return a corrupted FASM.
# FAKE_PARSE_GARBAGE=1 makes parse return text that is not FASM at
# all (critic session 5, FLAG 2): the harness must record a FRAMING
# red, never die with a traceback.
# FAKE_PARSE_FF_FLIP=1 makes parse flip the AFF INIT value (0 to 1,
# 1 to 0) in the returned FASM: the round-trip check must fail on a
# genuinely wrong INIT value while the bits path (which reads the
# unflipped sidecar) stays green.
# FAKE_PARSE_DEVICE_WIDE=1 appends the lines a real emitter's
# parse-back renders for tiles outside the design (diff-harness
# session 4): every FF of an untouched SLICE tile, INIT.V1 and
# SRVAL.V1 (bit 0 renders V1 through the exact-complement rule).
# The round-trip check must drop those lines (the comparison is
# scoped to the design's tiles) and still pass when the design's
# own tiles match.
# FAKE_PARSE_SRVAL_V1=1 appends SLICE_X0Y0.AFF.SRVAL.V1 to the parse
# output: a wrong SRVAL value for the design's AFF. The round-trip
# check must fail on it (the golden completes AFF.SRVAL to .V0).
# FAKE_PARSE_OWN_TILE_NOISE=1 appends the lines a real emitter's
# parse-back renders for unused BELs and attribute pairs on the
# design's OWN tile (diff-harness session 5): the exact-complement
# rule renders the all-! side of every attribute pair on an
# untouched tile (57 lines per CLEL_L tile, counted from the real
# segbits_clel_l.db: 16 INIT.V1 + 16 SRVAL.V1 + CEUSED 4 + CLKINV 2
# + SRINV 2 + SRUSED 4 + MODE 2 + SYNC 2 + CARRY_TYPE 1 + DI0-DI7
# 8). The noise covers every class with lines that are not in
# either golden design (unused FF BELs EFF..HFF and AFF2..HFF2,
# the ABCDFF/EFGHFF attribute pairs, CARRY_TYPE and DI0-DI7), plus
# the session-4 device-wide lines for SLICE_X9Y9. The FRAMING
# check must drop them all (feature-scoped comparison) and still
# pass when the design's own features match; the per-block bits
# diff still catches stray set bits.
#
# No em dashes anywhere.

import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from reference import Tier1Reference
from diff_engine import resolve_feature
from fasm import parse_fasm


def main():
    cmd = sys.argv[1]
    args = dict(zip(sys.argv[2::2], sys.argv[3::2]))
    if cmd == "emit":
        fasm_path, part, out_path = args["--fasm"], args["--part"], args["--out"]
        with open(fasm_path, "rb") as f:
            fasm_bytes = f.read()
        with open(out_path, "wb") as f:
            f.write(b"\xaa\x99\x55\x66")
            if part == "xcku3p":
                f.write((0x04A63093).to_bytes(4, "big"))
            f.write(b"FAKEBIT")
        with open(out_path + ".fasm", "wb") as f:
            f.write(fasm_bytes)
        return 0
    if cmd == "parse":
        if os.environ.get("FAKE_PARSE_GARBAGE") == "1":
            sys.stdout.write("this is not fasm at all\n")
            return 0
        bit_path = args["--bit"]
        with open(bit_path + ".fasm") as f:
            text = f.read()
        if os.environ.get("FAKE_PARSE_CORRUPT") == "1":
            text = text.replace("A6LUT.INIT[63:0] = 0x5555555555555555",
                                "A6LUT.INIT[63:0] = 0x5555555555555554", 1)
        if os.environ.get("FAKE_PARSE_FF_FLIP") == "1":
            def _flip(m):
                return "AFF.INIT = %d" % (1 - int(m.group(1)))
            text = re.sub(r"AFF\.INIT = ([01])", _flip, text, count=1)
        if os.environ.get("FAKE_PARSE_DEVICE_WIDE") == "1":
            # Lines the real emitter's parse-back renders for tiles
            # outside the design: every FF of an untouched slice,
            # INIT.V1 and SRVAL.V1 (bit 0 renders V1).
            extra = "".join(
                "SLICE_X9Y9.%sFF.INIT.V1\nSLICE_X9Y9.%sFF.SRVAL.V1\n"
                % (bel, bel) for bel in "ABCDEFGH"
            )
            text += extra
        if os.environ.get("FAKE_PARSE_SRVAL_V1") == "1":
            text += "SLICE_X0Y0.AFF.SRVAL.V1\n"
        if os.environ.get("FAKE_PARSE_OWN_TILE_NOISE") == "1":
            # Device-wide lines outside the design, as session 4.
            extra = "".join(
                "SLICE_X9Y9.%sFF.INIT.V1\nSLICE_X9Y9.%sFF.SRVAL.V1\n"
                % (bel, bel) for bel in "ABCDEFGH"
            )
            # Unused FF BELs on the design's own tile (EFF..HFF and
            # the FF2 BELs are unused in both goldens): INIT.V1 and
            # SRVAL.V1, bit 0 renders V1 through the exact-complement
            # rule.
            extra += "".join(
                "SLICE_X0Y0.%s.INIT.V1\nSLICE_X0Y0.%s.SRVAL.V1\n"
                % (bel, bel)
                for bel in ("EFF", "FFF", "GFF", "HFF",
                            "AFF2", "BFF2", "CFF2", "DFF2",
                            "EFF2", "FFF2", "GFF2", "HFF2")
            )
            # Unused attribute pairs on the design's own tile, all-!
            # side rendered (feature spellings verbatim from
            # segbits_clel_l.db).
            extra += (
                "SLICE_X0Y0.ABCDFF.CEUSED.V0\n"
                "SLICE_X0Y0.ABCDFF.CLKINV.V0\n"
                "SLICE_X0Y0.ABCDFF.SRINV.V0\n"
                "SLICE_X0Y0.ABCDFF.SRUSED.V0\n"
                "SLICE_X0Y0.ABCDFF.MODE.FF\n"
                "SLICE_X0Y0.ABCDFF.SYNC.ASYNC\n"
                "SLICE_X0Y0.EFGHFF.CEUSED.V0\n"
                "SLICE_X0Y0.EFGHFF.CLKINV.V0\n"
                "SLICE_X0Y0.EFGHFF.SRINV.V0\n"
                "SLICE_X0Y0.EFGHFF.SRUSED.V0\n"
                "SLICE_X0Y0.EFGHFF.MODE.FF\n"
                "SLICE_X0Y0.EFGHFF.SYNC.ASYNC\n"
                "SLICE_X0Y0.CARRY8.CARRY_TYPE.SINGLE_CY8\n"
                "SLICE_X0Y0.CARRY8.DI0.AX\n"
                "SLICE_X0Y0.CARRY8.DI1.BX\n"
                "SLICE_X0Y0.CARRY8.DI2.CX\n"
                "SLICE_X0Y0.CARRY8.DI3.DX\n"
                "SLICE_X0Y0.CARRY8.DI4.EX\n"
                "SLICE_X0Y0.CARRY8.DI5.FX\n"
                "SLICE_X0Y0.CARRY8.DI6.GX\n"
                "SLICE_X0Y0.CARRY8.DI7.HX\n"
            )
            text += extra
        sys.stdout.write(text)
        return 0
    if cmd == "bits":
        bit_path, part = args["--bit"], args["--part"]
        with open(bit_path + ".fasm") as f:
            features = parse_fasm(f.read(), bit_path + ".fasm")
        ref = Tier1Reference()
        bitset = {}
        per_block = {}
        for feat in features:
            block, bits, _, miss = resolve_feature(ref, part, feat)
            assert not miss, "fake emitter hit missing reference: %s" % miss
            bitset.update(bits)
            per_block.setdefault(block, {}).update(bits)
        flips = int(os.environ.get("FAKE_FLIP_COUNT", "0"))
        flip_block = os.environ.get("FAKE_FLIP_BLOCK")
        pool = per_block.get(flip_block, bitset) if flip_block else bitset
        # Flips apply to absolute (FAR, offset) bits only; tile-relative
        # IO bits have tuple keys and are not part of the absolute bitset.
        pool = {p: v for p, v in pool.items() if isinstance(p[0], int)}
        for pos in sorted(pool)[:flips]:
            bitset[pos] = 1 - bitset[pos]
        # Print block-labelled lines (emitter protocol v1). Only
        # absolute (FAR, offset) bits are printed: tile-relative IO
        # bits have tuple keys and are not part of the absolute bitset
        # the real emitter protocol carries.
        for block, bits in sorted(per_block.items()):
            absolute = {p: v for p, v in bits.items() if isinstance(p[0], int)}
            for (far, off), val in sorted(absolute.items()):
                print("0x%08x %d %d %s" % (far, off, bitset[(far, off)], block))
        extra_block = os.environ.get("FAKE_EXTRA_BIT_BLOCK")
        if extra_block:
            # Synthetic: the emitter set a bit in a block the design
            # does not use. Tests the unexercised-block red path.
            print("0x00000000 0 1 %s" % extra_block)
        return 0
    print("unknown subcommand %s" % cmd, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
