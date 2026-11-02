#!/usr/bin/env python3
"""rawdiff -- name-free (frame, word, bit) differ for 7-series .bit files.

WHY THIS EXISTS
===============
Every other verifier in this repo compares *named* prjxray segbits features,
e.g. ``CLBLM_R.SLICEM_X0.AFF.ZINI``.  That has three blind spots, and all three
have already cost this project real time:

1. Bits that no feature in the segbits DB names are invisible to a feature
   diff, so such a tool reports MATCH on them.  Roughly a third of the words
   that differ between the Helion and Vivado blinky are in this class.
2. prjxray models a slice flop with exactly two bits, ``AFF.ZINI`` and
   ``AFF.ZRST``.  There is no per-flop INIT *value* feature, yet a flop
   obviously has a power-up value.  Whatever encodes it is unnamed.
3. 72 tile types have no segbits file at all (``CLK_MTBF2``, ``CMT_FIFO_L/R``,
   ``CMT_TOP_*``, ``HCLK_FEEDTHRU_*``, ``CFG_CENTER_*``, ...), so every tile of
   those types is entirely invisible to a feature diff.

This tool never enumerates features.  It walks the raw FDRI payload, finds
every (frame_address, word_index, bit_index) where two bitstreams disagree, and
then *annotates* each one with the named features that happen to cover it -- as
metadata for a human, never as a filter.

USAGE
=====
    python3 work/rawdiff.py OURS.bit REF.bit [options]

    --top N          show only the top N clusters (default 25)
    --explain        add an inference pass over the largest clusters: print the
                     per-frame raw bit and, for each claiming tile, the named
                     flop configuration (ZINI/ZRST BELs, *FFMUX selections,
                     FFSYNC, SMALL) decoded from BOTH bitstreams, so an unnamed
                     bit can be correlated against a specific BEL letter or
                     slice mode.
    --tile TYPE      restrict output to clusters whose tile type matches TYPE
                     (substring, case-insensitive).  Repeatable.
    --word LO        restrict to frame word index LO (default 0).
    --word-hi HI     restrict to frame word index HI (default 100).
    --named-only     only print NAMED bits (some segbits feature covers them).
    --unnamed-only   only print UNNAMED bits (no feature covers them).
    --json PATH      also dump the full bit list as JSON.
    --patch SPEC     patch reference values into OUR bitstream.  Comma
                     separated list of:
                         ADDR:WORD          whole word
                         ADDR:WORD:BIT      single bit
                     ADDR accepts 0x... hex.  Writes OUT (see --out) and then
                     RE-READS the file, RE-DECODES it and ASSERTS every patched
                     position now equals the reference; for a bit-only edit the
                     assertion is on that bit, not the whole word.  It also
                     re-reads the raw bytes at the computed file offset.  Never
                     reports success on the strength of the write alone.
    --out PATH       output for --patch (default: OURS + ".patched.bit").
    --all            with --patch, set every differing bit that the reference
                     has set (does not touch the .bit container).
    --verify-layout  assert the known frame anchor AND validate the
                     payload-word -> file-offset mapping against rare non-zero
                     words.  Run this before trusting any --patch.

TRUST NOTES
-----------
For a named bit, "which feature covers it" is decided by tile geometry
(baseaddr/offset/words from the tilegrid) plus the segbits entries for that tile
type.  A frame can be claimed by several tiles at once (``CLBLM_R`` and
``INT_R`` share a column block, and both are listed), so the report names every
claimant and every feature rather than guessing.

Two coordinate facts, both of which produced silent wrong answers when guessed:

* A tile's word window is CONSTANT across all of its frames; it is ``offset``,
  never ``offset + frame_index``.
* A segbits ``frame_offset`` is relative to the tile's ``baseaddr``, and a
  ``bitpos`` counts from the tile's first word: ``frame = baseaddr +
  frame_offset``, ``word = offset + bitpos // 32``, ``bit = bitpos % 32``.

The layout reconstruction (``PayloadIndex``) is the one validated against the
known frame: address 0x42009b must map to payload index 525301.  It walks
``full_addresses()`` in order and inserts a 202-word pad frame whenever the
far_group changes; ``addr_index * 101`` is NOT the payload offset because of
that padding.  ``--verify-layout`` asserts the known anchor at startup.

For writing, the byte offset of payload word ``p`` is NOT
``'e'-section_start + 4*p``: the section opens with a configuration preamble
(sync words, bus width, 0xaa995566, type-1 register writes) before the FDRI
write, and ``x7decode.payload()`` counts words from the section start.
``_fdri_offset`` locates the type-2 header exactly as ``payload()`` does.  An
earlier version of this tool got this wrong by 59 words and silently wrote into
the preamble while reporting success -- the same failure mode as three earlier
patch scripts in this repo.

DONE-WHEN FOR THIS FILE (self-checks worth re-running after any edit)
--------------------------------------------------------------------
    python3 work/rawdiff.py A.bit B.bit --verify-layout
        -> prints the 0x42009b -> 525301 anchor and validates file alignment.
    python3 work/rawdiff.py /tmp/init/hold0.bit /tmp/init/hold1.bit
        -> must report 0 differing bits (Helion drops the declared FF init).
"""
from __future__ import annotations

import argparse
import collections
import json
import os
import struct
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
if HERE not in sys.path:
    sys.path.insert(0, HERE)

import x7decode as X  # noqa: E402  (trusted decoder; do not rewrite)

# Anchor validated in /tmp/probe/patch2.py::unique_index and re-checked here on
# every run: frame 0x42009b (silicon-proven reference) lives at payload index
# 525301.  If this ever fails, the layout reconstruction is wrong and every
# offset computed below is garbage.
LAYOUT_ANCHOR = (0x42009B, 525301)

DB = X.DB


# --------------------------------------------------------------------------
# payload geometry
# --------------------------------------------------------------------------
class PayloadIndex:
    """Exact (frame address, word) <-> payload word index map.

    ``x7decode.frames_by_address`` inserts one 202-word pad frame at every
    far-group boundary, so ``addr_index * 101`` is not the payload offset.  We
    replay exactly that walk and keep the offsets.
    """

    def __init__(self, addrs=None):
        self.addrs = list(addrs) if addrs is not None else X.full_addresses()
        self.index = {}
        i = 0
        prev = None
        for a in self.addrs:
            g = X.far_group(a)
            if prev is not None and g != prev:
                i += 202
            self.index[a] = i
            i += 101
            prev = g
        self.end = i
        self.npads = self._count_pads()

    def _count_pads(self):
        n = 0
        prev = None
        for a in self.addrs:
            g = X.far_group(a)
            if prev is not None and g != prev:
                n += 1
            prev = g
        return n

    def payload_word(self, addr, word):
        """Payload word index, or None if the address is not in the device map."""
        base = self.index.get(addr)
        if base is None:
            return None
        return base + word

    def verify(self, anchor=LAYOUT_ANCHOR):
        got = self.index.get(anchor[0])
        if got != anchor[1]:
            raise SystemExit(
                "FATAL: layout reconstruction is wrong: %#x -> %s, expected %d. "
                "Refusing to compute offsets that would silently write to the "
                "wrong place." % (anchor[0], got, anchor[1])
            )
        return True

    def verify_file_alignment(self, path):
        """Prove that payload word p really lives at file offset start + 4p.

        This is the check that was missing when three earlier patch scripts
        silently wrote into the configuration preamble.  We take payload words
        that are non-zero and rare in the file and confirm the computed file
        offset holds the big-endian word we expect.  A zero word is not usable
        (every offset matches it), so we skip those.
        """
        raw, start, _n = _fdri_offset(path)
        W = X.payload(path)
        self.verify()
        checked = 0
        counts = collections.Counter(W)
        for p, v in enumerate(W):
            if v == 0 or counts[v] > 8:
                continue
            off = start + p * 4
            got = struct.unpack(">I", raw[off:off + 4])[0]
            if got != v:
                raise SystemExit(
                    "FATAL: payload/file offset mismatch in %s: payload word %d "
                    "is %08x but file offset %d holds %08x. Every patch offset "
                    "would be wrong." % (path, p, v, off, got)
                )
            checked += 1
            if checked >= 32:
                break
        if checked == 0:
            raise SystemExit(
                "FATAL: could not find any rare non-zero payload word in %s to "
                "validate the payload/file offset mapping." % path)
        return checked


def tile_claims():
    """address -> [(tile, tile_type, word_offset, words, baseaddr), ...].

    A tile occupies ``frames`` consecutive frame addresses starting at
    ``baseaddr``, and within EVERY one of those frames it lives in the same
    ``offset .. offset+words`` word window.  The word offset is therefore
    constant across the tile -- it is NOT ``offset + frame_index``.  Getting
    this wrong makes every tile claim the wrong words and silently classifies
    every named bit as UNNAMED.

    ``baseaddr`` is carried along because a segbits entry's frame offset is
    relative to it, so ``frame_offset = addr - baseaddr``.
    """
    tg = X._tilegrid()
    by_addr = collections.defaultdict(list)
    for tile, info in tg.items():
        for bus, g in info.get("bits", {}).items():
            base = g["baseaddr"]
            if isinstance(base, str):
                base = int(base, 16)
            off = g["offset"]
            words = g.get("words", 2)
            for i in range(g["frames"]):
                by_addr[base + i].append((tile, info["type"], off, words, base))
    return by_addr


def segbits_index():
    """tile_type -> {(frame_offset, word_in_frame, bit): [feature names]}.

    prjxray writes a segbit as ``<frame_offset>_<bitpos>`` where ``bitpos`` is
    counted from the first word of the *tile's own* frame block: bit ``bp``
    lives at frame ``baseaddr + fo``, word ``offset + bp // 32``, bit
    ``bp % 32``.  ``x7decode.active_features`` rebases exactly this way.
    Matching on ``(offset, bit)`` directly -- as the first version of this tool
    did -- matches nothing above bit 31 and silently mislabels every real
    feature as UNNAMED.
    """
    cache = {}
    missing = set()

    def get(tile_type):
        if tile_type in cache:
            return cache[tile_type]
        path = "%s/segbits_%s.db" % (DB, tile_type.lower())
        if not os.path.exists(path):
            missing.add(tile_type)
            cache[tile_type] = {}
            return cache[tile_type]
        m = collections.defaultdict(list)
        for name, bits in X.parse_segbits(path):
            for fo, bp, inv in bits:
                m[(fo, bp // 32, bp % 32)].append((name, inv))
        cache[tile_type] = m
        return m

    get.missing = missing
    return get


# --------------------------------------------------------------------------
# differencing
# --------------------------------------------------------------------------
class Bit:
    __slots__ = ("addr", "group", "word", "bit", "ours", "ref", "tiles",
                 "named", "features", "unclaimed")

    def __repr__(self):
        return "<Bit %#x w%d b%d %d->%d>" % (self.addr, self.word, self.bit,
                                             self.ours, self.ref)


def diff_bits(ours_frames, ref_frames, addrs, get_segbits, claims,
              word_lo=0, word_hi=100):
    out = []
    for a in addrs:
        ow = ours_frames.get(a)
        rw = ref_frames.get(a)
        if ow is None or rw is None:
            continue
        for w in range(word_lo, word_hi + 1):
            x = ow[w] ^ rw[w]
            if not x:
                continue
            tile_list = [c for c in claims.get(a, [])
                         if c[2] <= w < c[2] + c[3]]
            for b in range(32):
                if not (x >> b) & 1:
                    continue
                bit = Bit()
                bit.addr = a
                bit.group = X.far_group(a)
                bit.word = w
                bit.bit = b
                bit.ours = (ow[w] >> b) & 1
                bit.ref = (rw[w] >> b) & 1
                bit.tiles = [(t, ty, off) for (t, ty, off, _n, _b) in tile_list]
                feats = []
                for (t, ty, off, words, base) in tile_list:
                    sb = get_segbits(ty)
                    # segbits coordinates: frame_offset is relative to the
                    # tile's baseaddr; bitpos counts from the tile's first word,
                    # so bitpos//32 selects the word within the window.
                    fo = a - base
                    bp = (w - off) * 32 + b
                    for (name, inv) in sb.get((fo, bp // 32, bp % 32), ()):
                        feats.append((name, inv))
                bit.named = bool(feats)
                bit.features = feats
                bit.unclaimed = not bit.tiles
                out.append(bit)
    return out


# --------------------------------------------------------------------------
# clustering
# --------------------------------------------------------------------------
def clusters(bits):
    """Group by (tile type, contiguous frame run, word, bit)."""
    # frame address -> best (largest) tile type claiming it, for grouping.
    # A frame can be claimed by several tiles (CLBLM/INT_R share an address);
    # group by the *set* of types so the cluster label is honest.
    out = collections.defaultdict(list)
    for b in bits:
        types = tuple(sorted({t[1] for t in b.tiles})) or ("<no tile>",)
        out[(types, b.addr, b.word, b.bit)].append(b)
    # merge into contiguous frame runs
    merged = collections.defaultdict(list)
    for (types, a, w, bit), bs in out.items():
        merged[(types, w, bit)].append((a, bs))
    res = []
    for (types, w, bit), items in merged.items():
        items.sort()
        cur = []
        cur_a = []
        last = None
        for a, bs in items:
            if last is not None and a != last + 1:
                res.append(_mk(types, w, bit, cur_a, cur))
                cur, cur_a = [], []
            cur.append(bs)
            cur_a.append(a)
            last = a
        if cur:
            res.append(_mk(types, w, bit, cur_a, cur))
    res.sort(key=lambda c: (-c["count"], c["types"], c["addr_lo"], c["word"],
                            c["bit"]))
    return res


def _mk(types, w, bit, addrs, groups):
    flat = [b for g in groups for b in g]
    return {
        "types": list(types),
        "word": w,
        "bit": bit,
        "addr_lo": addrs[0],
        "addr_hi": addrs[-1],
        "frames": len(addrs),
        "addrs": addrs,
        "count": len(flat),
        "named": any(b.named for b in flat),
        "features": sorted({f for b in flat for f in b.features}),
        "ours_set": sum(b.ours for b in flat),
        "ref_set": sum(b.ref for b in flat),
        "bits": flat,
    }


# --------------------------------------------------------------------------
# reporting
# --------------------------------------------------------------------------
def print_clusters(cl, top, claim=None):
    print("\n=== top %d differing-bit clusters "
          "(tile type | frame run | word | bit) ===" % min(top, len(cl)))
    print("%-4s %-34s %-18s %5s %5s %7s %7s  %s" % (
        "#", "tile type(s)", "frame run", "word", "bit", "ours=1", "ref=1",
        "features"))
    for i, c in enumerate(cl[:top]):
        types = ",".join(c["types"])
        if len(types) > 34:
            types = types[:31] + "..."
        run = "%#x..%#x (%d)" % (c["addr_lo"], c["addr_hi"], c["frames"])
        feats = ",".join(f[0].split(".", 1)[-1] for f in c["features"][:3])
        if len(c["features"]) > 3:
            feats += " +%d" % (len(c["features"]) - 3)
        if not feats:
            feats = "<UNNAMED>"
        print("%-4d %-34s %-18s %5d %5d %7d %7d  %s" % (
            i, types, run, c["word"], c["bit"], c["ours_set"], c["ref_set"],
            feats))
    n_named = sum(1 for c in cl for b in c["bits"] if b.named)
    n_un = sum(1 for c in cl for b in c["bits"] if not b.named)
    n_unc = sum(1 for c in cl for b in c["bits"] if b.unclaimed)
    print("\ntotals: %d differing bits in %d clusters -- %d NAMED, %d UNNAMED, "
          "%d in frames no tile claims" % (
              n_named + n_un, len(cl), n_named, n_un, n_unc))


# --------------------------------------------------------------------------
# --explain
# --------------------------------------------------------------------------
def explain(c, ref_frames, ours_frames, claims, get_segbits, max_rows=64):
    print("\n" + "=" * 78)
    print("EXPLAIN cluster: types=%s word=%d bit=%d frames=%#x..%#x (%d)" % (
        ",".join(c["types"]), c["word"], c["bit"], c["addr_lo"], c["addr_hi"],
        c["frames"]))
    if c["features"]:
        print("  named features covering this (frame_offset, bit):")
        for name, inv in c["features"][:8]:
            print("    %s%s" % (name, "  (!inverted)" if inv else ""))
    else:
        print("  NO segbits feature names this bit -- it is invisible to every "
              "feature-level check in the repo.")
    if any(t[1] not in {x for x in _tile_types_with_segbits()} for t in
           {t for b in c["bits"] for t in b.tiles}):
        missing = sorted({t[1] for b in c["bits"] for t in b.tiles
                          if not os.path.exists(
                              "%s/segbits_%s.db" % (DB, t[1].lower()))})
        if missing:
            print("  tile type(s) with NO segbits file at all: %s" % missing)

    # per-frame value in ours and ref, plus which rows use a flop
    print("\n  per-frame raw bit (row = frame address, ascending):")
    addrs = c["addrs"]
    shown = 0
    for a in addrs:
        if shown >= max_rows:
            print("    ... %d more frames" % (len(addrs) - shown))
            break
        tiles = [t for t in claims.get(a, []) if t[2] <= c["word"] < t[2] + t[3]]
        tnames = ",".join(t[0] for t in tiles) or "<none>"
        ftxt = []
        for (t, ty, off, words, base) in tiles:
            sb = get_segbits(ty)
            fo = a - base
            bp = (c["word"] - off) * 32 + c["bit"]
            hits = sb.get((fo, bp // 32, bp % 32), [])
            ftxt += [h[0].split(".", 1)[-1] for h in hits]
        o = (ours_frames[a][c["word"]] >> c["bit"]) & 1
        r = (ref_frames[a][c["word"]] >> c["bit"]) & 1
        print("    %#x  ours=%d ref=%d  %-24s %s" % (
            a, o, r, tnames, ",".join(ftxt) if ftxt else "<unnamed here>"))
        shown += 1

    # correlate: does the ref bit track "row has a flop"?
    print("\n  correlation attempt:")
    print("    ref set on %d of %d frames; ours set on %d" % (
        c["ref_set"], c["count"], c["ours_set"]))
    _correlate(c, ref_frames, ours_frames, claims, get_segbits)


_TT_CACHE = None


def _tile_types_with_segbits():
    global _TT_CACHE
    if _TT_CACHE is None:
        _TT_CACHE = set()
        for f in os.listdir(DB):
            if f.startswith("segbits_") and f.endswith(".db"):
                _TT_CACHE.add(f[len("segbits_"):-len(".db")].upper())
    return _TT_CACHE


def _correlate(c, ref_frames, ours_frames, claims, get_segbits):
    """Correlate the bit against the flop configuration of the claiming tiles.

    The useful question is not "is this bit named?" -- we already know it is
    not -- but "does this bit track something structural?".  For each differing
    frame we decode the named flop features (``AFF/BFF/CFF/DFF.ZINI``,
    ``.ZRST``, ``*FFMUX.*``, ``FFSYNC``, ``ALUT.SMALL``) from BOTH bitstreams,
    so a reader can see whether the unnamed bit moves together with a
    particular BEL letter, a slice mode, or the reference's flop count.

    A per-flop init-value bit (which prjxray does not name) should turn on
    exactly when a flop exists in one specific BEL letter, and the slice mode
    tells us whether that is the 5FF (``A5FF``) or the full FF (``AFF``).
    """
    # Keep only the ZINI/ZRST positions that are also set in the reference, so
    # we can identify which BELs the reference actually has flops in.
    for ty in sorted({t[1] for b in c["bits"] for t in b.tiles}):
        have = ty in _tile_types_with_segbits()
        print("    [%s] %s" % (
            ty, "no segbits file -- every bit in this tile type is unnamed"
            if not have else "segbits present"))

        rows = []
        for b in c["bits"]:
            for (t, ty2, off, words, base) in claims.get(b.addr, []):
                if ty2 != ty or off > c["word"] < off + words:
                    continue
                rows.append((b.addr, t, off, base, b.ref, b.ours))
        if not rows:
            continue

        if have:
            sb = get_segbits(ty)
            # Is this bit index used by any *flop-related* feature anywhere in
            # the tile type?  Filtered to FF/mode features, because every bit
            # index is shared by dozens of LUT INIT bits and printing those
            # drowns the signal.
            shared = collections.Counter()
            for (fo, wi, bp), feats in sb.items():
                if bp != c["bit"]:
                    continue
                for name, inv in feats:
                    if ".INIT[" in name:
                        continue
                    shared[name.split(".", 1)[-1]] += 1
            if shared:
                print("      bit index %d is named elsewhere in this tile type "
                      "for (non-LUT features): %s" % (c["bit"], dict(shared)))
            else:
                print("      bit index %d is named for NO non-LUT feature in "
                      "this tile type (prjxray has no feature here)." % c["bit"])

        seen = set()
        shown_rows = 0
        for (a, t, off, base, rv, ov) in rows:
            key = (a, t, off, base)
            if key in seen:
                continue
            seen.add(key)
            if shown_rows >= 8:
                print("      ... %d more frames" % (len(seen) - shown_rows
                                                    if len(seen) > shown_rows
                                                    else 0))
                break
            shown_rows += 1
            cfg_r = cfg_o = ""
            if have:
                sb = get_segbits(ty)
                fo = a - base

                def decode(frames):
                    z, r, mux, mode = [], [], [], []
                    for (f2, wi, bp), feats in sb.items():
                        if f2 != fo:
                            continue
                        for name, inv in feats:
                            tail = name.split(".")
                            last = tail[-1]
                            if last not in ("ZINI", "ZRST", "FFSYNC",
                                            "SMALL") and "FFMUX" not in last:
                                continue
                            wno = off + wi
                            if wno < off or wno >= off + 2:
                                continue
                            v = (frames[a][wno] >> bp) & 1
                            if not v:
                                continue
                            if last == "ZINI":
                                z.append(tail[-2])
                            elif last == "ZRST":
                                r.append(tail[-2])
                            elif last == "SMALL":
                                mode.append("%s.SMALL" % tail[-2])
                            elif last == "FFSYNC":
                                mode.append("FFSYNC")
                            else:
                                mux.append(name.split(".", 1)[-1])
                    return sorted(z), sorted(r), sorted(mux), sorted(mode)

                zr, rr, mr, mr2 = decode(ref_frames)
                zo, ro, mo, mo2 = decode(ours_frames)
                cfg_r = "ZINI=%s ZRST=%s mux=%s %s" % (
                    ",".join(zr) or "-", ",".join(rr) or "-",
                    ",".join(mr) or "-", ",".join(mr2) or "")
                cfg_o = "ZINI=%s ZRST=%s mux=%s %s" % (
                    ",".join(zo) or "-", ",".join(ro) or "-",
                    ",".join(mo) or "-", ",".join(mo2) or "")
            print("      %#x %-22s bit ref=%d ours=%d" % (a, t, rv, ov))
            if cfg_r:
                print("          ref : %s" % cfg_r)
                print("          ours: %s" % cfg_o)


# --------------------------------------------------------------------------
# --patch
# --------------------------------------------------------------------------
def _fdri_offset(path):
    """(raw bytes, file offset of the FIRST word x7decode.payload() returns, n).

    This is subtle and got three earlier patch scripts wrong.  ``payload()``
    returns the words of the FDRI payload *after* the type-2 header word, and
    it counts them from the start of the whole ``'e'`` section -- which begins
    with a configuration preamble (eight ``0xffffffff`` sync words, a dummy
    word, the bus-width write, the sync word 0xaa995566 and a series of
    type-1 register writes) before the FDRI write.  So the payload word index
    is NOT relative to the ``'e'`` section start.

    We therefore locate the type-2 header inside the stream exactly the way
    ``payload()`` does and report the file offset of the word *after* it.
    ``PayloadIndex.payload_word()`` indexes from there.
    """
    raw = open(path, "rb").read()
    assert raw[:2] == b"\x00\x09", "not a .bit file: %s" % path
    i = 13
    sec_start = None
    n = None
    while i < len(raw):
        t = raw[i:i + 1]
        i += 1
        if t == b"e":
            n = struct.unpack(">I", raw[i:i + 4])[0]
            i += 4
            sec_start = i
            i += n
        else:
            ln = struct.unpack(">H", raw[i:i + 2])[0]
            i += 2 + ln
    assert sec_start is not None, "no FDRI payload in %s" % path

    nwords = n // 4
    words = struct.unpack(">%dI" % nwords, raw[sec_start:sec_start + nwords * 4])
    j = 0
    data_index = None
    while j < len(words):
        w = words[j]
        typ = w >> 29
        if typ == 1:
            op = (w >> 27) & 3
            reg = (w >> 13) & 0x3FFF
            cnt = w & 0x7FF
            j += 1
            if op == 2 and reg == 2 and cnt == 0:
                w2 = words[j]
                assert (w2 >> 29) == 2, "FDRI header not followed by type-2"
                j += 1
                data_index = j
                break
            if op == 2:
                j += cnt
        elif typ == 2:
            j += 1 + (w & 0x7FFFFFF)
        else:
            j += 1
    assert data_index is not None, "no FDRI type-1/type-2 header pair in %s" % path
    return raw, sec_start + data_index * 4, n - data_index * 4


def do_patch(ours_path, ref_path, specs, out_path, idx, ref_frames, ours_frames,
             all_bits=False, diff=None):
    """Copy reference values into our bitstream, then PROVE it landed.

    Two independent checks, because three earlier patch scripts in this repo
    reported success while writing to unused space:

      1. re-read the output file from disk, decode it with the trusted decoder
         and compare the decoded payload word against the reference;
      2. re-read the raw bytes at the computed file offset and compare the
         big-endian word there too.  This catches a correct in-memory edit that
         was written to the wrong file offset.

    For a bit-only edit the assertion is on that bit, not on the whole word:
    the reference legitimately differs from ours in other bits of the same
    word, and demanding word equality there would be a false failure.
    """
    raw, start, n = _fdri_offset(ours_path)
    buf = bytearray(raw)
    edits = []  # (addr, word, bit_or_None, new_value, masked_expectation)

    def bit_expect(addr, word, bit, refword):
        """Expected value of the patched position after the edit."""
        return (refword >> bit) & 1

    if all_bits:
        if diff is None:
            raise SystemExit("--all needs a diff")
        per = collections.defaultdict(set)
        for b in diff:
            if b.ref == 1:
                per[(b.addr, b.word)].add(b.bit)
        for (a, w), bits in sorted(per.items()):
            newv = ours_frames[a][w]
            for b in bits:
                newv = (newv & ~(1 << b)) | (1 << b)
            edits.append((a, w, None, newv, None))
    else:
        if not specs:
            raise SystemExit("--patch needs at least one ADDR:WORD[:BIT] spec")
        for s in specs:
            parts = s.split(":")
            if len(parts) not in (2, 3):
                raise SystemExit("bad --patch spec %r (want ADDR:WORD[:BIT])" % s)
            a = int(parts[0], 0)
            w = int(parts[1], 0)
            if w < 0 or w > 100:
                raise SystemExit("word %d out of range 0..100 in %r" % (w, s))
            bit = int(parts[2], 0) if len(parts) == 3 else None
            if a not in idx.index:
                raise SystemExit("address %#x is not in the device frame map" % a)
            if bit is not None and not (0 <= bit < 32):
                raise SystemExit("bit %d out of range in %r" % (bit, s))
            rv = ref_frames[a][w]
            if bit is None:
                edits.append((a, w, None, rv, None))
            else:
                cur = ours_frames[a][w]
                newv = (cur & ~(1 << bit)) | (bit_expect(a, w, bit, rv) << bit)
                edits.append((a, w, bit, newv, bit_expect(a, w, bit, rv)))

    for (a, w, bit, newv, _exp) in edits:
        pw = idx.payload_word(a, w)
        off = start + pw * 4
        struct.pack_into(">I", buf, off, newv)

    if out_path is None:
        out_path = ours_path + ".patched.bit"
    if os.path.abspath(out_path) == os.path.abspath(ours_path):
        raise SystemExit("--out must differ from the input; refusing to "
                         "overwrite the bitstream under test")
    with open(out_path, "wb") as fh:
        fh.write(bytes(buf))
    print("patched %d word(s), wrote %s (%d bytes)" % (
        len(edits), out_path, len(buf)))

    # ---- STRICT VERIFICATION: re-read the file, re-decode, assert. ----
    idx2 = PayloadIndex()
    idx2.verify()
    ours2 = X.frames_by_address(idx2.addrs, X.payload(out_path), check=False)
    ref2 = X.frames_by_address(idx2.addrs, X.payload(ref_path), check=False)
    raw2 = open(out_path, "rb").read()
    bad = 0
    for (a, w, bit, newv, exp) in edits:
        got = ours2[a][w]
        rword = ref2[a][w]
        if bit is None:
            ok = got == rword
            detail = "got %08x want %08x" % (got, rword)
        else:
            gb = (got >> bit) & 1
            rb = (rword >> bit) & 1
            ok = gb == rb
            detail = "bit%d got %d want %d" % (bit, gb, rb)
        if not ok:
            bad += 1
            print("  ASSERT FAIL %#x word %d: %s" % (a, w, detail))
            continue
        # second, independent check: the raw bytes on disk
        pw = idx2.payload_word(a, w)
        off = start + pw * 4
        disk = struct.unpack(">I", raw2[off:off + 4])[0]
        if disk != (got & 0xFFFFFFFF):
            bad += 1
            print("  ASSERT FAIL %#x word %d: decoded %08x but on-disk %08x "
                  "at offset %d" % (a, w, got, disk, off))
    if bad:
        raise SystemExit("PATCH FAILED VERIFICATION: %d position(s) wrong" % bad)
    print("VERIFIED: %d patched position(s) re-read from disk, re-decoded, and "
          "equal to the reference." % len(edits))
    return out_path


# --------------------------------------------------------------------------
def main(argv=None):
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("ours")
    ap.add_argument("ref")
    ap.add_argument("--top", type=int, default=25)
    ap.add_argument("--explain", action="store_true")
    ap.add_argument("--tile", action="append", default=[])
    ap.add_argument("--word", type=int, default=None)
    ap.add_argument("--word-hi", type=int, default=None)
    ap.add_argument("--named-only", action="store_true")
    ap.add_argument("--unnamed-only", action="store_true")
    ap.add_argument("--json", dest="jsonout", default=None)
    ap.add_argument("--patch", default=None)
    ap.add_argument("--all", action="store_true")
    ap.add_argument("--out", default=None)
    ap.add_argument("--verify-layout", action="store_true")
    args = ap.parse_args(argv)

    idx = PayloadIndex()
    idx.verify()
    if args.verify_layout:
        print("layout verified: %#x -> %d (payload %d words, %d pad frames)" % (
            LAYOUT_ANCHOR[0], LAYOUT_ANCHOR[1], idx.end, idx.npads))
        print("payload/file offset verified against %d rare non-zero words" % (
            idx.verify_file_alignment(args.ours),))

    ours_all = X.frames_by_address(idx.addrs, X.payload(args.ours), check=False)
    ref_all = X.frames_by_address(idx.addrs, X.payload(args.ref), check=False)
    claims = tile_claims()
    get_segbits = segbits_index()

    words = list(idx.index)
    wlo = args.word if args.word is not None else 0
    whi = args.word_hi if args.word_hi is not None else 100
    bits = diff_bits(ours_all, ref_all, idx.addrs, get_segbits, claims, wlo, whi)

    if args.tile:
        pats = [t.lower() for t in args.tile]
        bits = [b for b in bits if any(
            any(p in ty.lower() for p in pats) for (_t, ty, _o) in b.tiles)]
    if args.named_only:
        bits = [b for b in bits if b.named]
    if args.unnamed_only:
        bits = [b for b in bits if not b.named]

    print("ours: %s" % args.ours)
    print("ref : %s" % args.ref)
    print("differing bits: %d" % len(bits))
    cl = clusters(bits)
    print_clusters(cl, args.top)

    if args.jsonout:
        dump = [{
            "addr": "%#x" % b.addr, "far_group": list(b.group), "word": b.word,
            "bit": b.bit, "ours": b.ours, "ref": b.ref,
            "tiles": [{"tile": t, "type": ty, "frame_offset": off}
                      for (t, ty, off) in b.tiles],
            "named": b.named,
            "features": [{"name": n, "inverted": i} for (n, i) in b.features],
        } for b in bits]
        json.dump(dump, open(args.jsonout, "w"), indent=1)
        print("wrote %s" % args.jsonout)

    if args.explain:
        for c in cl[:args.top]:
            explain(c, ref_all, ours_all, claims, get_segbits)

    if args.patch:
        specs = [s for s in args.patch.split(",") if s]
        do_patch(args.ours, args.ref, specs, args.out, idx, ref_all, ours_all,
                 all_bits=args.all, diff=bits)
    return 0


if __name__ == "__main__":
    sys.exit(main())
