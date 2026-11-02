"""Decode 7-series .bit frames and inspect tile features.

Used for the ZedBoard debug: build the full device frame map from part.json,
parse the FDRI payload, and report which segbits features are set in a tile.
"""
import json
import struct
import sys

import os as _os
DB = _os.environ.get("HELION_XILINX_DB", "work/prjxray-db").rstrip("/")
if not DB.endswith("zynq7"):
    DB = _os.path.join(DB, "zynq7")
PART = DB + "/xc7z020clg484-1/part.json"
TILEGRID = DB + "/xc7z020/tilegrid.json"

BLOCK = {"CLB_IO_CLK": 0, "BLOCK_RAM": 1}
BUS = {0: "CLB_IO_CLK", 1: "BLOCK_RAM"}


def full_addresses():
    p = json.load(open(PART))
    out = []
    for block, bus_name in [(0, "CLB_IO_CLK"), (1, "BLOCK_RAM")]:
        for name, half in [("top", 0), ("bottom", 1)]:
            region = p["global_clock_regions"][name]
            for row in sorted(region["rows"], key=int):
                buses = region["rows"][row]["configuration_buses"]
                if bus_name not in buses:
                    continue
                cols = buses[bus_name]["configuration_columns"]
                for col in sorted(cols, key=int):
                    for minor in range(cols[col]["frame_count"]):
                        out.append(
                            (block << 23)
                            | (half << 22)
                            | (int(row) << 17)
                            | (int(col) << 7)
                            | minor
                        )
    out.sort()
    return out


def tile_addresses():
    d = json.load(open(TILEGRID))
    out = set()
    for t in d.values():
        for bus, g in t.get("bits", {}).items():
            base = int(g["baseaddr"], 16)
            for i in range(g["frames"]):
                out.add(base + i)
    return sorted(out)


def far_group(a):
    return ((a >> 23) & 7, (a >> 22) & 1, (a >> 17) & 0x1F)


def payload(path):
    b = open(path, "rb").read()
    assert b[:2] == b"\x00\x09"
    i = 13
    stream = None
    while i < len(b):
        t = b[i : i + 1]
        i += 1
        if t == b"e":
            n = struct.unpack(">I", b[i : i + 4])[0]
            i += 4
            stream = b[i : i + n]
            i += n
        else:
            n = struct.unpack(">H", b[i : i + 2])[0]
            i += 2 + n
    words = list(struct.unpack(">%dI" % (len(stream) // 4), stream))
    j = 0
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
                j += 1
                assert (w2 >> 29) == 2
                n = w2 & 0x7FFFFFF
                return words[j : j + n]
            if op == 2:
                j += cnt
        elif typ == 2:
            j += 1 + (w & 0x7FFFFFF)
        else:
            j += 1
    raise RuntimeError("no FDRI payload")


def frames_by_address(addrs, frm_payload, check=True):
    out = {}
    i = 0
    prev = None
    for a in addrs:
        g = far_group(a)
        if prev is not None and g != prev:
            if check:
                assert all(w == 0 for w in frm_payload[i : i + 202]), "pad not zero"
            i += 202
        frame = frm_payload[i : i + 101]
        if not frame:
            raise RuntimeError("payload exhausted")
        out[a] = frame
        i += 101
        prev = g
    if check:
        rest = frm_payload[i:]
        assert all(w == 0 for w in rest), "trailing data after last frame"
    return out


_TILEGRID_CACHE = None


def _tilegrid():
    # Parsed once. This used to re-read and re-parse the whole 29k-tile JSON on
    # every call, which made a full-device census take ~10 minutes instead of
    # seconds (1000 calls measured at 256 s, all of it in json.load).
    global _TILEGRID_CACHE
    if _TILEGRID_CACHE is None:
        with open(TILEGRID) as fh:
            _TILEGRID_CACHE = json.load(fh)
    return _TILEGRID_CACHE


def tile_geom(tile):
    return _tilegrid()[tile]["bits"]


_SEGBITS_CACHE = {}


def parse_segbits(path):
    # Cached: the scanner calls this once per tile, and a full-device census
    # walks thousands of tiles. Re-reading the .db each time dominated the
    # runtime (~10 min for xc7z020) for no benefit, since the files are
    # immutable within a run.
    if path in _SEGBITS_CACHE:
        return _SEGBITS_CACHE[path]
    feats = []
    for line in open(path):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split()
        bits = []
        for tok in parts[1:]:
            inv = tok.startswith("!")
            f, b = tok.lstrip("!").split("_")
            bits.append((int(f), int(b), inv))
        feats.append((parts[0], bits))
    _SEGBITS_CACHE[path] = feats
    return feats
    return feats


def active_features(tile, seg_path, frames):
    geom = tile_geom(tile)
    out = []
    for name, bits in parse_segbits(seg_path):
        ok = True
        for fo, bp, inv in bits:
            a = geom["CLB_IO_CLK"]["baseaddr"]
            if isinstance(a, str):
                a = int(a, 16)
            addr = a + fo
            word = geom["CLB_IO_CLK"]["offset"] + bp // 32
            bit = bp % 32
            if addr not in frames:
                ok = False
                break
            got = (frames[addr][word] >> bit) & 1
            if got != (0 if inv else 1):
                ok = False
                break
        if ok:
            out.append(name)
    return out


def main():
    bit = sys.argv[1]
    mode = sys.argv[2] if len(sys.argv) > 2 else "full"
    addrs = full_addresses() if mode == "full" else tile_addresses()
    frames = frames_by_address(addrs, payload(bit))
    print("frames", len(frames), "addresses", hex(min(frames)), hex(max(frames)))
    for spec in sys.argv[3:]:
        # type:tile
        typ, tile = spec.split(":", 1)
        seg = "%s/segbits_%s.db" % (DB, typ.lower())
        feats = active_features(tile, seg, frames)
        print("\n== %s (%s) ==" % (tile, typ))
        for f in feats:
            print(" ", f)


if __name__ == "__main__":
    main()
