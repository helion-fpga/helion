"""Frames -> per-tile FASM features (plus every set bit no feature explains).

A feature is active when all of its positive bits are set, all of its
negative bits are clear, and it has at least one positive bit. Features
made only of negative bits are DB defaults (all-clear encodings) and are
never reported as active; the netlist layer applies them as defaults.

`_SING` I/O tiles are decoded through their tilegrid alias (the regular
I/O tile type, shifted by `start_offset` words, restricted to the aliased
sites), the same convention Project X-Ray's own tools use.
"""
from __future__ import annotations

from collections import defaultdict
from dataclasses import dataclass, field

from .db import Db, SegBit


@dataclass
class TileDecode:
    tile: str
    tile_type: str
    features: list[str] = field(default_factory=list)
    unknown_bits: list[tuple[int, int]] = field(default_factory=list)
    no_db: bool = False


@dataclass
class DesignDecode:
    tiles: dict[str, TileDecode]
    # Frames whose stored ECC (word 50, bits [12:0]) differs from the ECC of
    # their contents: {far: (stored, computed)}.
    ecc_mismatch: dict[int, tuple[int, int]] = field(default_factory=dict)
    # Set bits that fall in no tile's window at all: (far, word, bit).
    orphan_bits: list[tuple[int, int, int]] = field(default_factory=list)

    def fasm(self) -> list[str]:
        out = []
        for t in sorted(self.tiles):
            for f in sorted(self.tiles[t].features):
                out.append(f"{t}.{f}")
        return out

    def unknown(self) -> dict[str, list[tuple[int, int]]]:
        return {t: d.unknown_bits for t, d in self.tiles.items() if d.unknown_bits}


class _Matcher:
    """Precomputed segbits index for one (tile type, block, alias) combo."""

    def __init__(self, seg: dict[str, tuple[SegBit, ...]], shift_bits: int = 0,
                 keep_sites: set[str] | None = None, prefix_from: str = "",
                 bit_limit: int | None = None):
        self.features: list[tuple[str, tuple[tuple[int, int], ...], tuple[tuple[int, int], ...]]] = []
        self.by_bit: dict[tuple[int, int], list[int]] = defaultdict(list)
        for name, bits in seg.items():
            # strip "<TILETYPE>." prefix
            short = name.split(".", 1)[1] if "." in name else name
            # Alias tiles: features of the other half fall outside the
            # shifted window (negative or >= bit_limit) and drop out below.
            pos = []
            neg = []
            ok = True
            for b in bits:
                nb = b.bit - shift_bits
                if nb < 0 or (bit_limit is not None and nb >= bit_limit):
                    ok = False
                    break
                (pos if b.value else neg).append((b.minor, nb))
            if not ok or not pos:
                continue
            idx = len(self.features)
            self.features.append((short, tuple(pos), tuple(neg)))
            for p in pos:
                self.by_bit[p].append(idx)

    def match(self, setbits: set[tuple[int, int]]) -> tuple[list[str], set[tuple[int, int]]]:
        cands = set()
        for b in setbits:
            cands.update(self.by_bit.get(b, ()))
        active = []
        covered: set[tuple[int, int]] = set()
        for idx in cands:
            name, pos, neg = self.features[idx]
            if all(p in setbits for p in pos) and not any(n in setbits for n in neg):
                active.append(name)
                covered.update(pos)
        return active, setbits - covered


def tile_setbits(frames: dict[int, list[int]], bits: dict) -> set[tuple[int, int]]:
    base = int(bits["baseaddr"], 16)
    off = bits["offset"]
    nwords = bits["words"]
    out = set()
    for m in range(bits["frames"]):
        fr = frames.get(base + m)
        if fr is None:
            continue
        for w in range(nwords):
            v = fr[off + w]
            if not v:
                continue
            for b in range(32):
                if v >> b & 1:
                    out.add((m, w * 32 + b))
    return out


def _abs(bits: dict, m: int, b: int) -> tuple[int, int, int]:
    return (int(bits["baseaddr"], 16) + m, bits["offset"] + b // 32, b % 32)


def decode(db: Db, frames_in: dict[int, list[int]]) -> DesignDecode:
    """Decode every tile, then compute unexplained bits globally.

    The per-frame ECC (word 50, bits [12:0]) is checked against the frame's
    contents and then masked out, so it never reaches feature matching.

    Paired tiles (INT + CLB, INT + BRAM, ...) share configuration columns and
    overlap in their bit windows, so a bit is "unknown" only when no active
    feature in ANY tile covers its absolute (frame, word, bit) position.
    """
    from .ecc import ECC_MASK, ECC_WORD, expected_ecc
    ecc_bad: dict[int, tuple[int, int]] = {}
    frames: dict[int, list[int]] = {}
    for far, fr in frames_in.items():
        stored = fr[ECC_WORD] & ECC_MASK
        if stored:
            fr = list(fr)
            fr[ECC_WORD] &= ~ECC_MASK & 0xFFFFFFFF
        frames[far] = fr
    matchers: dict[tuple, _Matcher] = {}
    tiles: dict[str, TileDecode] = {}
    covered: set[tuple[int, int, int]] = set()
    windows: list[tuple[str, dict]] = []
    for tname, t in db.tilegrid.items():
        for block, bits in t.get("bits", {}).items():
            setbits = tile_setbits(frames, bits)
            if not setbits:
                continue
            windows.append((tname, bits))
            ttype = t["type"]
            alias = bits.get("alias")
            if alias:
                key = (alias["type"], block, alias["start_offset"], tuple(sorted(alias["sites"])))
                if key not in matchers:
                    matchers[key] = _Matcher(
                        db.segbits(alias["type"], block),
                        shift_bits=alias["start_offset"] * 32,
                        
                        bit_limit=bits["words"] * 32)
                has_db = db.has_segbits(alias["type"], block)
            else:
                key = (ttype, block)
                if key not in matchers:
                    matchers[key] = _Matcher(db.segbits(ttype, block))
                has_db = db.has_segbits(ttype, block)
            td = tiles.setdefault(tname, TileDecode(tname, ttype))
            if not has_db:
                td.no_db = True
                continue
            active, rest = matchers[key].match(setbits)
            covered.update(_abs(bits, m, b) for m, b in setbits - rest)
            if alias and alias["start_offset"] == 0:
                # The SING tile's only site is the alias tile's Y1 half;
                # report it under the SING tile's own (Y0) naming.
                fixed = []
                for f in active:
                    site, _, restf = f.partition(".")
                    if site.endswith("_Y1"):
                        site = site[:-1] + "0"
                    fixed.append(f"{site}.{restf}" if restf else site)
                active = fixed
            td.features.extend(active)
    # Global unexplained bits, attributed to every tile whose window holds them.
    in_window: set[tuple[int, int, int]] = set()
    for tname, bits in windows:
        block_is_clb = int(bits["baseaddr"], 16) >> 23 == 0
        for m, b in tile_setbits(frames, bits):
            a = _abs(bits, m, b)
            in_window.add(a)
            if a not in covered:
                tiles[tname].unknown_bits.append((m if block_is_clb else 1000 + m, b))
    # ECC, once the features say which bits hold LUT RAM contents (left out).
    from .lutram import ecc_exclude
    skip = ecc_exclude(db, tiles)
    for far, fr in frames_in.items():
        stored = fr[ECC_WORD] & ECC_MASK
        calc = expected_ecc(far, fr, skip.get(far))
        if stored != calc:
            ecc_bad[far] = (stored, calc)
    orphans = []
    for far, fr in frames.items():
        for w, v in enumerate(fr):
            while v:
                b = (v & -v).bit_length() - 1
                v &= v - 1
                if (far, w, b) not in in_window:
                    orphans.append((far, w, b))
    return DesignDecode(tiles, ecc_bad, orphans)


def block_tag(block: str, minor: int) -> int:
    # Encode block type into the minor so CLB and BRAM-content bits stay distinct.
    return minor if block == "CLB_IO_CLK" else 1000 + minor
