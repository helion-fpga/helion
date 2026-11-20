#!/usr/bin/env python3
"""Generate the checked-in band-code sample fixture for the in-crate test.

Reads the decoder-validation corpus (h1_ecc_frames_P-*.npz plus
h1_ecc_lutram_masks.pkl), computes the Rust-equivalent MAP-A (masked)
and H76 predictions in Python, and writes a deterministic sample of
frames to crates/helion-ultrascale/tests/fixtures/band_code_sample.bin:

    u32 n, then per record: u32 far, 93 u32 words, 93 u32 mask words,
    u64 stored band. All little endian.

Sample (seed 20261120): 120 random block-0 frames, 40 LUTRAM-bearing
block-0 frames, 20 Q frames (MAP-A residual exactly w), 20 block-1
frames, 5 all-zero-nonband block-0 frames. The script prints the
exact counts; copy them into tests/band_code.rs as the expected
constants. No em dashes anywhere.
"""

import glob
import json
import pickle
import random
import struct
import sys
from pathlib import Path

VALIDATION_DIR = Path.home() / "workspace" / "ultra-bitgen-crew" / "decoder-validation"
OUT = (
    Path(__file__).resolve().parent.parent
    / "tests"
    / "fixtures"
    / "band_code_sample.bin"
)
SEED = 20261120
Q_RESIDUAL = 0xF0000000000F


def load_formulas():
    with open(VALIDATION_DIR / "h1_ecc_sparse_maps.json", encoding="utf-8") as f:
        sparse = json.load(f)
    map_a = {int(k): int(v) for k, v in sparse["A"].items()}
    with open(
        VALIDATION_DIR / "h1_ecc_followup2_formula_h76.json", encoding="utf-8"
    ) as f:
        h76 = {
            int(k): v["sources"]
            for k, v in json.load(f)["formula"].items()
        }
    return map_a, h76


def stored_band(words):
    return int(words[45]) | ((int(words[46]) & 0xFFFF) << 32)


def predict_map_a(words, mask_int, map_a):
    pred = 0
    for wi in range(93):
        word = int(words[wi])
        while word:
            low = word & -word
            bit = low.bit_length() - 1
            absb = wi * 32 + bit
            word ^= low
            if 1440 <= absb <= 1487:
                continue
            if (mask_int >> absb) & 1:
                continue
            pred ^= map_a.get(absb, 0)
    return pred


def predict_h76(words, h76):
    pred = 0
    for j in range(48):
        parity = 0
        for s in h76[1440 + j]:
            if (int(words[s // 32]) >> (s % 32)) & 1:
                parity ^= 1
        if parity:
            pred |= 1 << j
    return pred


def main():
    import numpy as np

    map_a, h76 = load_formulas()
    with open(VALIDATION_DIR / "h1_ecc_lutram_masks.pkl", "rb") as f:
        masks = pickle.load(f)
    rng = random.Random(SEED)
    b0_plain = []
    b0_lutram = []
    b0_q = []
    b0_zero = []
    b1 = []
    for path in sorted(glob.glob(str(VALIDATION_DIR / "h1_ecc_frames_P-*.npz"))):
        pair = "P-" + Path(path).name.split("P-")[1].split(".")[0]
        data = np.load(path)
        fars = data["far"]
        words = data["words"]
        pm = masks[pair]
        for i in range(len(fars)):
            far = int(fars[i])
            block = (far >> 24) & 7
            w = words[i]
            if block == 1:
                b1.append((far, w, 0))
                continue
            if block != 0:
                continue
            mask_int = pm.get(far, 0)
            stored = stored_band(w)
            nonband_zero = all(
                int(x) == 0 for x in list(w[:45]) + [int(w[46]) & 0xFFFF0000]
                + list(w[47:])
            ) and stored == 0
            if nonband_zero:
                b0_zero.append((far, w, mask_int))
                continue
            pred_a = predict_map_a(w, mask_int, map_a)
            if pred_a != stored:
                if pred_a ^ stored == Q_RESIDUAL:
                    b0_q.append((far, w, mask_int))
                else:
                    print(f"unexpected MAP-A residual, FAR {far}", file=sys.stderr)
                    return 1
            elif mask_int:
                b0_lutram.append((far, w, mask_int))
            else:
                b0_plain.append((far, w, mask_int))
    rng.shuffle(b0_plain)
    rng.shuffle(b0_lutram)
    rng.shuffle(b0_q)
    rng.shuffle(b1)
    rng.shuffle(b0_zero)
    sample = (
        b0_plain[:120]
        + b0_lutram[:40]
        + b0_q[:20]
        + b1[:20]
        + b0_zero[:5]
    )
    rng.shuffle(sample)
    map_a_exact = 0
    h76_exact = 0
    q_count = 0
    for far, w, mask_int in sample:
        stored = stored_band(w)
        if predict_map_a(w, mask_int, map_a) == stored:
            map_a_exact += 1
        else:
            q_count += 1
        if predict_h76(w, h76) == stored:
            h76_exact += 1
    OUT.parent.mkdir(parents=True, exist_ok=True)
    with open(OUT, "wb") as f:
        f.write(struct.pack("<I", len(sample)))
        for far, w, mask_int in sample:
            mask_words = [0] * 93
            m = mask_int
            while m:
                low = m & -m
                pos = low.bit_length() - 1
                mask_words[pos // 32] |= 1 << (pos % 32)
                m ^= low
            f.write(struct.pack("<I", far))
            f.write(struct.pack("<93I", *[int(x) for x in w]))
            f.write(struct.pack("<93I", *mask_words))
            f.write(struct.pack("<Q", stored_band(w)))
    print(f"wrote {OUT} ({len(sample)} frames)")
    print(f"sample MAP-A exact: {map_a_exact}")
    print(f"sample H76 exact: {h76_exact}")
    print(f"sample Q frames: {q_count}")
    print(f"sample block-1 frames: {sum(1 for far, _, _ in sample if ((far >> 24) & 7) == 1)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
