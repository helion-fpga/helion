#!/usr/bin/env python3
"""Frames out of a Xilinx 7-series / UltraScale / UltraScale+ .bit, and bit diffs.

Works on *debug* bitstreams (BITSTREAM.GENERAL.DEBUGBITSTREAM YES): there every
frame is its own FDRI packet followed by a LOUT write of that frame's address,
so no device-specific frame-address sequencing is needed.

  bitframes.py dump  a.bit            -> one line per frame: FAR, nonzero words
  bitframes.py diff  base.bit var.bit -> one line per flipped bit:
                                         FAR  block row col minor  word bit  0->1|1->0
  bitframes.py selftest
"""
import gzip
import struct
import sys

SYNC = b"\xaa\x99\x55\x66"
# Config registers (UG470 / UG570 type-1 packet addresses): LOUT is 0x08.
REG_FAR, REG_FDRI, REG_IDCODE, REG_LOUT = 0x01, 0x02, 0x0C, 0x08


def words(path):
    data = (gzip.open if path.endswith(".gz") else open)(path, "rb").read()
    i = data.find(SYNC)
    if i < 0:
        raise ValueError(f"{path}: no sync word")
    body = data[i + 4:]
    return struct.unpack(f">{len(body) // 4}I", body[: len(body) // 4 * 4])


def packets(ws):
    """Yield (register, payload words) for every write packet."""
    i, reg = 0, None
    while i < len(ws):
        h = ws[i]
        i += 1
        kind = h >> 29
        if kind == 1:
            op, reg, n = (h >> 27) & 3, (h >> 13) & 0x3FFF, h & 0x7FF
        elif kind == 2:
            op, n = (h >> 27) & 3, h & 0x7FFFFFF
        else:
            continue  # NOOP / dummy / pad
        if op == 2 and n:
            yield reg, ws[i:i + n]
        i += n if op == 2 else 0


def frames(path):
    """Return (idcode, {far: tuple(words)}) from a debug bitstream."""
    idcode, out, pending = None, {}, None
    for reg, pay in packets(words(path)):
        if reg == REG_IDCODE:
            idcode = pay[0]
        elif reg == REG_FDRI:
            pending = tuple(pay)
        elif reg == REG_LOUT and pending is not None:
            out[pay[0]] = pending
            pending = None
    if not out:
        raise ValueError(f"{path}: no FDRI+LOUT pairs; was DEBUGBITSTREAM set?")
    return idcode, out


def is_ultrascale(idcode):
    # IDCODE bits [27:24]: 3 on 7-series (xc7z020 0x?3727093), 4 on UltraScale(+) (xcku3p 0x?4A63093).
    return idcode is not None and ((idcode >> 24) & 0xF) >= 4


def far_fields(far, us):
    """(block type, row, column, minor); 7-series row includes the top/bottom bit as +/-."""
    if us:  # UltraScale+: [26:24] block [23:18] row [17:8] col [7:0] minor
        return (far >> 24) & 7, (far >> 18) & 0x3F, (far >> 8) & 0x3FF, far & 0xFF
    top = "B" if (far >> 22) & 1 else "T"
    return (far >> 23) & 7, f"{top}{(far >> 17) & 0x1F}", (far >> 7) & 0x3FF, far & 0x7F


def diff(a, b):
    ida, fa = frames(a)
    idb, fb = frames(b)
    if ida != idb:
        raise ValueError(f"IDCODE differs: {ida:#x} vs {idb:#x}")
    us = is_ultrascale(ida)
    rows = []
    for far in sorted(set(fa) | set(fb)):
        wa, wb = fa.get(far, ()), fb.get(far, ())
        for w in range(max(len(wa), len(wb))):
            x = (wa[w] if w < len(wa) else 0) ^ (wb[w] if w < len(wb) else 0)
            for bit in range(32):
                if x >> bit & 1:
                    up = (wb[w] if w < len(wb) else 0) >> bit & 1
                    rows.append((far, *far_fields(far, us), w, bit, "0->1" if up else "1->0"))
    return rows


def selftest():
    # Hand-built stream: sync, IDCODE, FAR, two (FDRI frame, LOUT far) pairs.
    def t1(reg, n):
        return (1 << 29) | (2 << 27) | (reg << 13) | n
    # Literal headers as Vivado 2017.3 writes them (xcku3p debug bitstream), so a
    # wrong register constant cannot pass by agreeing with itself.
    assert t1(REG_LOUT, 1) == 0x30010001 and t1(REG_FDRI, 93) == 0x3000405D
    fw = 3
    s = [t1(REG_IDCODE, 1), 0x04A63093, t1(REG_FAR, 1), 0,
         t1(REG_FDRI, fw), 1, 0, 0, t1(REG_LOUT, 1), 0x100,
         t1(REG_FDRI, fw), 0, 0, 0x80000000, t1(REG_LOUT, 1), 0x101]
    import os, tempfile
    p = tempfile.mktemp(suffix=".bit")
    q = tempfile.mktemp(suffix=".bit")
    open(p, "wb").write(b"hdr" + SYNC + struct.pack(f">{len(s)}I", *s))
    s2 = list(s)
    s2[5] = 0  # clear bit 0 of frame 0x100 word 0
    open(q, "wb").write(b"hdr" + SYNC + struct.pack(f">{len(s2)}I", *s2))
    idc, fr = frames(p)
    assert idc == 0x04A63093 and fr == {0x100: (1, 0, 0), 0x101: (0, 0, 0x80000000)}, fr
    d = diff(p, q)
    assert d == [(0x100, 0, 0, 1, 0, 0, 0, "1->0")], d
    assert far_fields(0x0300_0F0C, True) == (3, 0, 0x0F, 0x0C)
    os.remove(p); os.remove(q)
    print("selftest ok")


def main(argv):
    if argv[:1] == ["selftest"]:
        return selftest()
    if argv[:1] == ["dump"] and len(argv) == 2:
        idc, fr = frames(argv[1])
        us = is_ultrascale(idc)
        print(f"# idcode {idc:#010x} frames {len(fr)} words/frame {len(next(iter(fr.values())))}")
        for far, ws in sorted(fr.items()):
            nz = sum(1 for w in ws if w)
            print(f"{far:#010x}\t{far_fields(far, us)}\t{nz}")
        return
    if argv[:1] == ["diff"] and len(argv) == 3:
        print("FAR\tblock\trow\tcol\tminor\tword\tbit\tchange")
        for r in diff(argv[1], argv[2]):
            print(f"{r[0]:#010x}\t" + "\t".join(map(str, r[1:])))
        return
    sys.exit(__doc__)


if __name__ == "__main__":
    main(sys.argv[1:])
