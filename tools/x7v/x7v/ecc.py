"""7-series per-frame ECC (13 bits in word 50, bits [12:0]).

Every configuration frame carries a SECDED Hamming code over its other bits
in the low 13 bits of the middle word. It is a pure function of the frame's
contents, so it must be recomputed after any edit to a frame. Same algorithm
as Project X-Ray's libprjxray `xc7series::icap_ecc`.
"""
from __future__ import annotations

ECC_WORD = 50
ECC_MASK = 0x1FFF


def frame_ecc(words: list[int]) -> int:
    ecc = 0
    for idx, word in enumerate(words):
        val = idx * 32
        if idx > 0x25:
            val += 0x1360
        elif idx > 0x6:
            val += 0x1340
        else:
            val += 0x1320
        if idx == ECC_WORD:
            word &= 0xFFFFE000
        i = 0
        while word:
            if word & 1:
                ecc ^= val + i
            word >>= 1
            i += 1
        if idx == 100:
            v = ecc & 0xFFF
            v ^= v >> 8
            v ^= v >> 4
            v ^= v >> 2
            v ^= v >> 1
            ecc ^= (v & 1) << 12
    return ecc & ECC_MASK


def with_ecc(words: list[int]) -> list[int]:
    out = list(words)
    out[ECC_WORD] = (out[ECC_WORD] & ~ECC_MASK & 0xFFFFFFFF) | frame_ecc(out)
    return out
