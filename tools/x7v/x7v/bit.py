"""7-series .bit parsing (UG470), independent of Helion's own reader.

Parses the .bit TLV header, walks the configuration packet stream, records
every register write in order, and assigns FDRI payload words to frame
addresses by replaying the FAR auto-increment over the part's frame map
(including the two pad frames the device expects at the end of each row).
"""
from __future__ import annotations

import struct
from dataclasses import dataclass, field

SYNC = 0xAA995566
WORDS_PER_FRAME = 101

REG_NAMES = {
    0x00: "CRC", 0x01: "FAR", 0x02: "FDRI", 0x03: "FDRO", 0x04: "CMD",
    0x05: "CTL0", 0x06: "MASK", 0x07: "STAT", 0x08: "LOUT", 0x09: "COR0",
    0x0A: "MFWR", 0x0B: "CBC", 0x0C: "IDCODE", 0x0D: "AXSS", 0x0E: "COR1",
    0x10: "WBSTAR", 0x11: "TIMER", 0x13: "RBCRC_SW", 0x16: "BOOTSTS",
    0x18: "CTL1", 0x1F: "BSPI",
}
CMD_NAMES = {
    0: "NULL", 1: "WCFG", 2: "MFW", 3: "LFRM", 4: "RCFG", 5: "START",
    6: "RCAP", 7: "RCRC", 8: "AGHIGH", 9: "SWITCH", 10: "GRESTORE",
    11: "SHUTDOWN", 12: "GCAPTURE", 13: "DESYNC", 15: "IPROG", 16: "CRCC",
    17: "LTIMER", 18: "BSPI_READ", 19: "FALL_EDGE",
}
BLOCK_TYPES = {"CLB_IO_CLK": 0, "BLOCK_RAM": 1, "CFG_CLB": 2}


def far_encode(block: int, bottom: int, row: int, col: int, minor: int) -> int:
    return (block << 23) | (bottom << 22) | (row << 17) | (col << 7) | minor


def far_decode(far: int) -> tuple[int, int, int, int, int]:
    return ((far >> 23) & 7, (far >> 22) & 1, (far >> 17) & 0x1F,
            (far >> 7) & 0x3FF, far & 0x7F)


@dataclass
class Packet:
    reg: str
    op: str          # "W" or "R" or "NOP"
    words: list[int]


_CRC_POLY = 0x82F63B78 << 1


def icap_crc(addr: int, data: int, prev: int) -> int:
    """7-series configuration CRC step (CRC-32C over {addr[4:0], data})."""
    val = ((addr & 0x1F) << 32) | data
    crc = prev
    for _ in range(37):
        if (val & 1) != (crc & 1):
            crc ^= _CRC_POLY
        crc >>= 1
        val >>= 1
    return crc


def crc_checks(bs: "Bitstream") -> list[tuple[int, int]]:
    """(written, expected) for every CRC register write in the stream."""
    out = []
    crc = 0
    rev = {v: k for k, v in REG_NAMES.items()}
    for p in bs.packets:
        if p.op != "W":
            continue
        r = rev.get(p.reg)
        if r is None:
            continue
        if p.reg == "CRC":
            out.append((p.words[-1], crc))
            crc = 0
            continue
        if p.reg == "CMD" and p.words and p.words[-1] == 7:
            crc = 0
            continue
        for v in p.words:
            crc = icap_crc(r, v, crc)
    return out


@dataclass
class Bitstream:
    header: dict[str, str]
    packets: list[Packet]
    frames: dict[int, list[int]] = field(default_factory=dict)
    pad_frames: list[list[int]] = field(default_factory=list)
    trailing_words: int = 0

    def writes(self, reg: str) -> list[list[int]]:
        return [p.words for p in self.packets if p.reg == reg and p.op == "W"]

    def commands(self) -> list[str]:
        out = []
        for w in self.writes("CMD"):
            for v in w:
                out.append(CMD_NAMES.get(v, f"CMD{v}"))
        return out

    def reg_value(self, reg: str) -> int | None:
        ws = self.writes(reg)
        return ws[-1][-1] if ws and ws[-1] else None


def parse_header(data: bytes) -> tuple[dict[str, str], int]:
    """Return (fields, offset of the raw config data)."""
    hdr: dict[str, str] = {}
    if data[:2] != b"\x00\x09":
        return hdr, 0  # raw .bin
    off = 2 + 9
    (n,) = struct.unpack(">H", data[off:off + 2])
    off += 2  # n == 1, next byte is key 'a'
    while off < len(data):
        key = chr(data[off])
        off += 1
        if key == "e":
            (ln,) = struct.unpack(">I", data[off:off + 4])
            off += 4
            hdr["length"] = str(ln)
            return hdr, off
        (ln,) = struct.unpack(">H", data[off:off + 2])
        off += 2
        hdr[key] = data[off:off + ln].rstrip(b"\x00").decode("latin-1")
        off += ln
    raise ValueError("no 'e' field in .bit header")


def frame_addresses(part: dict) -> list[tuple[int, bool]]:
    """FAR auto-increment order: list of (far, is_last_frame_of_row)."""
    g = part["global_clock_regions"]
    out: list[tuple[int, bool]] = []
    for bt_name, bt in sorted(BLOCK_TYPES.items(), key=lambda kv: kv[1]):
        for half_name, bottom in (("top", 0), ("bottom", 1)):
            rows = g.get(half_name, {}).get("rows", {})
            for row in sorted(rows, key=int):
                bus = rows[row]["configuration_buses"].get(bt_name)
                if not bus:
                    continue
                cols = bus["configuration_columns"]
                row_frames = []
                for col in sorted(cols, key=int):
                    for minor in range(cols[col]["frame_count"]):
                        row_frames.append(far_encode(bt, bottom, int(row), int(col), minor))
                for i, far in enumerate(row_frames):
                    out.append((far, i == len(row_frames) - 1))
    return out


def parse(path: str, part: dict | None = None) -> Bitstream:
    data = open(path, "rb").read()
    header, off = parse_header(data)
    words = list(struct.unpack(">%dI" % ((len(data) - off) // 4), data[off:off + ((len(data) - off) // 4) * 4]))
    try:
        i = words.index(SYNC) + 1
    except ValueError:
        raise ValueError(f"{path}: no sync word")
    packets: list[Packet] = []
    last_reg = None
    while i < len(words):
        w = words[i]
        typ = w >> 29
        op = (w >> 27) & 3
        if typ == 1:
            reg = (w >> 13) & 0x1F
            cnt = w & 0x7FF
            last_reg = reg
        elif typ == 2:
            reg = last_reg
            cnt = w & 0x7FFFFFF
        else:
            # Non-packet word after DESYNC (trailing NOOP padding, etc).
            i += 1
            continue
        body = words[i + 1:i + 1 + cnt]
        i += 1 + cnt
        if op == 0:
            if packets and packets[-1].op == "NOP":
                packets[-1].words.append(0)
            else:
                packets.append(Packet("NOP", "NOP", [0]))
            continue
        name = REG_NAMES.get(reg, f"R{reg:#x}")
        packets.append(Packet(name, "W" if op == 2 else "R", body))
    bs = Bitstream(header, packets)
    if part is not None:
        assign_frames(bs, part)
    return bs


def assign_frames(bs: Bitstream, part: dict) -> None:
    order = frame_addresses(part)
    index = {far: k for k, (far, _) in enumerate(order)}
    far = 0
    for p in bs.packets:
        if p.op != "W":
            continue
        if p.reg == "FAR":
            far = p.words[-1]
            continue
        if p.reg != "FDRI" or not p.words:
            continue
        payload = p.words
        k = index.get(far)
        if k is None:
            raise ValueError(f"FDRI write at unknown FAR {far:#x}")
        pos = 0
        while pos + WORDS_PER_FRAME <= len(payload):
            if k >= len(order):
                # Trailing pipeline-flush frame(s) after the last address.
                bs.pad_frames.append(payload[pos:pos + WORDS_PER_FRAME])
                pos += WORDS_PER_FRAME
                continue
            addr, last_in_row = order[k]
            bs.frames[addr] = payload[pos:pos + WORDS_PER_FRAME]
            pos += WORDS_PER_FRAME
            k += 1
            if last_in_row:
                for _ in range(2):
                    if pos + WORDS_PER_FRAME <= len(payload):
                        bs.pad_frames.append(payload[pos:pos + WORDS_PER_FRAME])
                        pos += WORDS_PER_FRAME
        bs.trailing_words += len(payload) - pos
        far = order[k][0] if k < len(order) else far


def rewrite(path: str, out: str, frames: dict[int, list[int]], part: dict) -> None:
    """Write `path` again with new frame contents: every frame's ECC and both
    CRC words are recomputed, everything else (header, commands, pad
    frames) is kept. Frames missing from `frames` are written as zeros."""
    from .ecc import with_ecc
    data = open(path, "rb").read()
    _, off = parse_header(data)
    n = (len(data) - off) // 4
    words = list(struct.unpack(">%dI" % n, data[off:off + 4 * n]))
    order = frame_addresses(part)
    index = {far: k for k, (far, _) in enumerate(order)}
    # Packet walk as in parse(), keeping each body's position.
    pkts: list[tuple[int, int, int, int]] = []  # (reg, op, start, count)
    i = words.index(SYNC) + 1
    last_reg = 0
    while i < len(words):
        w = words[i]
        typ, op = w >> 29, (w >> 27) & 3
        if typ == 1:
            last_reg, cnt = (w >> 13) & 0x1F, w & 0x7FF
        elif typ == 2:
            cnt = w & 0x7FFFFFF
        else:
            i += 1
            continue
        pkts.append((last_reg, op, i + 1, cnt))
        i += 1 + cnt
    far = 0
    for reg, op, start, cnt in pkts:
        if op != 2:
            continue
        if reg == 0x01:
            far = words[start + cnt - 1]
        elif reg == 0x02 and cnt:
            k, pos = index[far], start
            while pos + WORDS_PER_FRAME <= start + cnt and k < len(order):
                addr, last_in_row = order[k]
                words[pos:pos + WORDS_PER_FRAME] = with_ecc(frames.get(addr, [0] * WORDS_PER_FRAME))
                pos += WORDS_PER_FRAME * (3 if last_in_row else 1)  # two pad frames end a row
                k += 1
    crc = 0
    for reg, op, start, cnt in pkts:
        if op != 2 or reg not in REG_NAMES:
            continue
        if reg == 0x00:
            words[start + cnt - 1] = crc
            crc = 0
        elif reg == 0x04 and cnt and words[start + cnt - 1] == 7:
            crc = 0
        else:
            for v in words[start:start + cnt]:
                crc = icap_crc(reg, v, crc)
    open(out, "wb").write(data[:off] + struct.pack(">%dI" % n, *words) + data[off + 4 * n:])
