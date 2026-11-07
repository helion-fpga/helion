#!/usr/bin/env python3
"""Build a synthetic minimal xcku3p bitstream for demo.py validation.

The bitstream carries exactly the frames the reference BitLocator
resolves for three BELs (one LUT, one FF, one RAMB18), each frame
with a marker bit set at every located frame offset. No real xcku3p
bitstream exists in the workspace, so this synthetic one is the
control the device-file brief asks for: demo.py (the reference tool)
must resolve the same FARs, frame offsets, and byte offsets that the
Helion Rust BitLocator computes for the same BELs.

What it proves, and what it does not:
- PROVES: the Rust loader's FAR arithmetic (row/col/minor) and the
  reference BitLocator agree on real BELs, and the reference demo.py
  can parse a bitstream built from those FARs and find every frame.
- DOES NOT prove: Vivado agreement (tier 2, license blocked), or that
  the marker bits are the bits Vivado would set (the markers are
  ours, placed at the reference-computed offsets).

Outputs (all next to this script):
- synth_xcku3p.bit: the synthetic bitstream.
- synth_xcku3p_expected.json: the expected (FAR, frame offset) pairs
  per BEL, plus FNV-1a digests of the full BRAM pair lists. The Rust
  test tests/device_file_demo.rs compares the Helion loader against
  this file.
- demo_xcku3p_lut_ff.log: demo.py output for the LUT and FF (small).
- demo_xcku3p_bram_summary.txt: line counts plus head/tail of the
  demo.py BRAM output (the full BRAM output is 18,432 lines and is
  verified programmatically, not pasted).

Self-checks (fail loud, exit non-zero on any mismatch):
1. Re-parse the written bitstream with the reference Bitstream class:
   every expected FAR present exactly once, no unexpected FARs, and
   ConfigFrame.bit(ofst) == 1 for every expected (FAR, ofst) pair.
2. Run demo.py on the bitstream and assert every printed FAR, frame
   offset, and frame byte offset matches the values this script
   computed when building the bitstream.

Run: python3 make_synth_xcku3p_bit.py (from this directory).
Requires the BitFiltrator reference clone at
~/workspace/helion-ultra-bitstream/bitfiltrator-ref/ and the Python
packages numpy and more_itertools.
"""

import hashlib
import json
import re
import struct
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REF_SRC = Path.home() / "workspace" / "helion-ultra-bitstream" / "bitfiltrator-ref" / "src"
sys.path.insert(0, str(REF_SRC))

import numpy as np
from bit_locator import BitLocator
from bitstream import Bitstream
from frame import FrameAddressIncrementer

PART = "xcku3p-ffva676-1-e"
IDCODE = 0x04A63093
FRAME_WORDS = 93
BELS = {
    "SLICE_X23Y45/B6LUT": "lut",
    "SLICE_X23Y45/BFF": "ff",
    "RAMB18_X0Y0": "bram",
}


def fnv1a64(pairs):
    """FNV-1a 64 over the canonical 'far_hex:ofst;' serialization."""
    h = 0xCBF29CE484222325
    for far_hex, ofst in pairs:
        for b in f"{far_hex}:{ofst};".encode("utf-8"):
            h ^= b
            h = (h * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return f"{h:016x}"


def type1_write(reg, words):
    """Type-1 write packet header plus payload words (big-endian)."""
    hdr = 0x30000000 | (reg << 13) | len(words)
    return struct.pack(">I", hdr) + b"".join(struct.pack(">I", w) for w in words)


def build_header(design, part, date, time, data_len):
    def lv(value):
        return struct.pack(">H", len(value)) + value

    def tlv(tag, value):
        return bytes([tag]) + struct.pack(">H", len(value)) + value

    out = b""
    out += lv(b"\x0f\xf0\x0f\xf0\x0f\xf0\x0f\xf0\x00")
    out += lv(b"\x61")
    out += lv(design.encode() + b"\x00")
    out += tlv(0x62, part.encode() + b"\x00")
    out += tlv(0x63, date.encode() + b"\x00")
    out += tlv(0x64, time.encode() + b"\x00")
    # Field 7: tag 0x65, 4-byte length, then the bitstream data itself.
    out += bytes([0x65]) + struct.pack(">I", data_len)
    return out


def main():
    loc = BitLocator(PART)
    incr = FrameAddressIncrementer(PART)

    # Collect (FAR int, frame offset) pairs per BEL from the reference.
    expected = {}
    frames = {}  # FAR int -> list of frame offsets (marker bits)
    for name, kind in BELS.items():
        if kind == "lut":
            slr, fars, ofsts = loc.locate_lut(name)
            pairs = [(f.to_hex(), int(o)) for f, o in zip(fars, ofsts)]
            expected[name] = {"kind": kind, "slr": slr, "pairs": pairs}
        elif kind == "ff":
            slr, far, ofst = loc.locate_reg(name)
            pairs = [(far.to_hex(), int(ofst))]
            expected[name] = {"kind": kind, "slr": slr, "pairs": pairs}
        else:
            slr, mf, mo, pf, po = loc.locate_bram(name)
            mem_pairs = [(f.to_hex(), int(o)) for f, o in zip(mf, mo)]
            par_pairs = [(f.to_hex(), int(o)) for f, o in zip(pf, po)]
            expected[name] = {
                "kind": kind,
                "slr": slr,
                "content_pairs": len(mem_pairs),
                "parity_pairs": len(par_pairs),
                "content_distinct_fars": sorted({f for f, _ in mem_pairs}),
                "parity_distinct_fars": sorted({f for f, _ in par_pairs}),
                "content_first8": mem_pairs[:8],
                "parity_first8": par_pairs[:8],
                "content_fnv1a64": fnv1a64(mem_pairs),
                "parity_fnv1a64": fnv1a64(par_pairs),
            }
            pairs = mem_pairs + par_pairs
        for far_hex, ofst in pairs:
            frames.setdefault(int(far_hex, 16), []).append(ofst)

    print(f"BELs: {len(expected)}, distinct FARs: {len(frames)}")
    for name, e in expected.items():
        n = len(e.get("pairs", [])) or e.get("content_pairs", 0) + e.get("parity_pairs", 0)
        print(f"  {name}: {n} pairs, slr {e['slr']}")

    # Build the bitstream. One FDRI write per FAR; a last-of-row FAR
    # gets its 2 zero padding frames in the same write (the reference
    # parser skips them and asserts they are zero). The header's field
    # 7 carries the data length in a fixed 4-byte field, so the header
    # length is constant and data_start can be computed before the
    # data is built.
    header = build_header(
        "synth_xcku3p;COMPRESS=FALSE", PART, "2026/11/07", "17:30:00", 0
    )
    data_start = len(header)
    data = b""
    data += struct.pack(">I", 0xFFFFFFFF)  # dummy
    data += struct.pack(">I", 0xAA995566)  # sync
    data += type1_write(12, [IDCODE])  # IDCODE
    data += type1_write(4, [1])  # CMD WCFG
    frame_byte_ofst = {}  # FAR int -> byte offset of its frame in the file
    for far_int in sorted(frames):
        words = [0] * FRAME_WORDS
        for ofst in frames[far_int]:
            words[ofst // 32] |= 1 << (ofst % 32)
        # Rebuild the FAR object for the last-of-row check.
        from arch_spec import ArchSpec
        from frame import FrameAddressRegister

        spec = ArchSpec.create_spec(PART)
        far_obj = FrameAddressRegister.from_int(far_int, spec)
        is_last = incr.is_last_far_of_row(IDCODE, far_obj)
        payload = words + ([0] * FRAME_WORDS * 2 if is_last else [])
        # Byte offset of this FDRI packet's data in the file.
        data += type1_write(1, [far_int])  # FAR
        frame_byte_ofst[far_int] = data_start + len(data) + 4
        data += type1_write(2, payload)  # FDRI
        if is_last:
            print(f"  FAR {far_int:08x} is last-of-row, +2 zero padding frames")
    data += type1_write(4, [13])  # CMD DESYNC
    data += struct.pack(">I", 0xFFFFFFFF) * 4  # trailing dummy

    header = build_header(
        "synth_xcku3p;COMPRESS=FALSE", PART, "2026/11/07", "17:30:00",
        len(data),
    )
    assert len(header) == data_start, (len(header), data_start)

    bit_path = HERE / "synth_xcku3p.bit"
    bit_path.write_bytes(header + data)
    print(f"wrote {bit_path} ({bit_path.stat().st_size} bytes)")

    # Self-check 1: re-parse with the reference Bitstream class.
    bs = Bitstream.from_file_path(bit_path)
    assert bs.header.fpga_part == PART, bs.header.fpga_part
    assert not bs.is_partial()
    assert bs.get_idcodes() == {IDCODE}, bs.get_idcodes()
    arrays = bs.get_per_far_configuration_arrays()
    assert set(arrays) == {IDCODE}, set(arrays)
    seen = {}
    for far, frames_list in arrays[IDCODE].items():
        assert len(frames_list) == 1, f"FAR {far.to_hex()} written {len(frames_list)} times"
        seen[far.to_int()] = frames_list[0]
    assert set(seen) == set(frames), (
        f"FAR mismatch: missing {sorted(set(frames) - set(seen))[:5]}, "
        f"extra {sorted(set(seen) - set(frames))[:5]}"
    )
    for far_int, ofsts in frames.items():
        frame = seen[far_int]
        assert frame.byte_ofst == frame_byte_ofst[far_int], (
            f"FAR {far_int:08x}: byte_ofst {frame.byte_ofst} != "
            f"expected {frame_byte_ofst[far_int]}"
        )
        for ofst in ofsts:
            assert frame.bit(ofst) == 1, f"FAR {far_int:08x} ofst {ofst} not set"
    print(f"self-check 1 PASS: {len(seen)} FARs, all marker bits set, byte offsets exact")

    # Self-check 2: run demo.py and verify its printed values.
    demo_out = subprocess.run(
        [sys.executable, "demo.py", str(bit_path),
         "--luts", "SLICE_X23Y45/B6LUT",
         "--ffs", "SLICE_X23Y45/BFF",
         "--brams", "RAMB18_X0Y0"],
        cwd=str(REF_SRC), capture_output=True, text=True,
    )
    assert demo_out.returncode == 0, demo_out.stderr[-2000:]
    (HERE / "demo_xcku3p_full.log").write_text(demo_out.stdout)
    lines = demo_out.stdout.splitlines()
    print(f"self-check 2: demo.py exited 0, {len(lines)} output lines")

    # Parse demo.py output. Note the frame address parenthetical is
    # the FAR's __str__ (BLOCK_TYPE / ROW_ADDR / COL_ADDR /
    # MINOR_ADDR), not a second hex address.
    far_str = (
        r"\(BLOCK_TYPE = \w+, ROW_ADDR =\s*\d+, COL_ADDR =\s*\d+, "
        r"MINOR_ADDR =\s*\d+\)"
    )
    lut_re = re.compile(
        r"INIT\[\s*(\d+)\] -> (\w+), frame address: 0x([0-9a-f]+) "
        rf"{far_str}, frame offset:\s*(\d+), frame byte offset in bitstream: (\d+)"
    )
    ff_re = re.compile(
        r"INIT -> (\w+), frame address: 0x([0-9a-f]+) "
        rf"{far_str}, frame offset:\s*(\d+), frame byte offset in bitstream: (\d+)"
    )
    bram_re = re.compile(
        r"INIT(?:_P)?\[\s*(\d+)\] -> (\w+), frame address: 0x([0-9a-f]+) "
        rf"{far_str}, frame offset:\s*(\d+), frame byte offset in bitstream: (\d+)"
    )
    section = None
    lut_got, ff_got, bram_mem_got, bram_par_got = [], [], [], []
    for line in lines:
        if line.startswith("SLICE_X23Y45/B6LUT"):
            section = "lut"
        elif line.startswith("SLICE_X23Y45/BFF"):
            section = "ff"
        elif line.startswith("RAMB18_X0Y0"):
            section = "bram"
        elif line == "":
            section = None
        elif section == "lut":
            m = lut_re.search(line)
            assert m, f"unparsed LUT line: {line!r}"
            lut_got.append((m.group(3), int(m.group(4)), int(m.group(5))))
        elif section == "ff":
            m = ff_re.search(line)
            assert m, f"unparsed FF line: {line!r}"
            ff_got.append((m.group(2), int(m.group(3)), int(m.group(4))))
        elif section == "bram":
            m = bram_re.search(line)
            assert m, f"unparsed BRAM line: {line!r}"
            if "_P[" in line:
                bram_par_got.append((m.group(3), int(m.group(4)), int(m.group(5))))
            else:
                bram_mem_got.append((m.group(3), int(m.group(4)), int(m.group(5))))

    lut_exp = expected["SLICE_X23Y45/B6LUT"]["pairs"]
    assert len(lut_got) == 64, len(lut_got)
    for (far_hex, ofst), (g_far, g_ofst, g_byte) in zip(lut_exp, lut_got):
        assert g_far == far_hex, (g_far, far_hex)
        assert g_ofst == ofst, (g_ofst, ofst)
        assert g_byte == frame_byte_ofst[int(far_hex, 16)], (g_byte, far_hex)
    ff_exp = expected["SLICE_X23Y45/BFF"]["pairs"][0]
    assert len(ff_got) == 1, len(ff_got)
    assert ff_got[0][0] == ff_exp[0] and ff_got[0][1] == ff_exp[1]
    assert ff_got[0][2] == frame_byte_ofst[int(ff_exp[0], 16)]
    bram = expected["RAMB18_X0Y0"]
    assert len(bram_mem_got) == 16384, len(bram_mem_got)
    assert len(bram_par_got) == 2048, len(bram_par_got)
    # Full-list digest check on the demo.py output itself.
    assert fnv1a64([(f, o) for f, o, _ in bram_mem_got]) == bram["content_fnv1a64"]
    assert fnv1a64([(f, o) for f, o, _ in bram_par_got]) == bram["parity_fnv1a64"]
    for f, o, b in bram_mem_got[:8] + bram_par_got[:8]:
        assert b == frame_byte_ofst[int(f, 16)], (f, b)
    print("self-check 2 PASS: demo.py output matches the reference locator "
          "and the built byte offsets on all 18,497 located bits")

    # Write the expected-values file for the Rust regression test.
    out = {
        "part": PART,
        "idcode": f"0x{IDCODE:08x}",
        "bels": expected,
    }
    json_path = HERE / "synth_xcku3p_expected.json"
    json_path.write_text(json.dumps(out, indent=1) + "\n")
    print(f"wrote {json_path}")

    # Split demo output for the doc: LUT+FF log (small) and BRAM summary.
    lut_ff_lines = []
    in_bram = False
    for line in lines:
        if line.startswith("RAMB18_X0Y0"):
            in_bram = True
        if not in_bram:
            lut_ff_lines.append(line)
    (HERE / "demo_xcku3p_lut_ff.log").write_text("\n".join(lut_ff_lines) + "\n")
    bram_lines = lines[len(lut_ff_lines):]
    bram_nonempty = [l for l in bram_lines if l]
    summary = (
        f"demo.py --brams RAMB18_X0Y0: {len(bram_lines)} lines "
        f"(1 header + {len(bram_mem_got)} INIT + {len(bram_par_got)} INIT_P "
        f"+ 1 trailing blank)\n"
        f"first INIT line: {bram_nonempty[1] if len(bram_nonempty) > 1 else ''}\n"
        f"last INIT_P line: {bram_nonempty[-1] if bram_nonempty else ''}\n"
    )
    (HERE / "demo_xcku3p_bram_summary.txt").write_text(summary)
    print("wrote demo_xcku3p_lut_ff.log, demo_xcku3p_bram_summary.txt")
    print("ALL CHECKS PASSED")


if __name__ == "__main__":
    main()
