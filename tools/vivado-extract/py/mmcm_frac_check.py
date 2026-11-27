#!/usr/bin/env python3
"""Check XAPP888's fractional MMCM counter formulas against xraydecode.py output.

  xraydecode.py <fuzzdir> ... > decoded.tsv ; mmcm_frac_check.py decoded.tsv

For every row that changes only CLKFBOUT_MULT_F or CLKOUT0_DIVIDE_F (plus its _PHASE),
predicts the X-Ray fields from the value and compares them with what Vivado wrote.
CLKFBOUT's falling-edge fields live in CLKOUT6's second register, CLKOUT0's in CLKOUT5's.
"""
import re
import sys


def frac_fields(value, phase=0.0):
    """XAPP888 mmcm_frac_count_calc: D.f (f in eighths) and phase in degrees -> register fields."""
    d, f = int(value), round((value - int(value)) * 8)
    if f == 0:
        return None
    odd_and_frac = 8 * (d & 1) + f
    eighths = round(phase / 360 * value * 8)  # coarse phase in eighth VCO periods
    return {
        "DELAY_TIME": eighths >> 3,
        "PHASE_MUX": eighths & 7,
        "HIGH_TIME": (d >> 1) - (odd_and_frac <= 8),
        "LOW_TIME": (d >> 1) - (odd_and_frac <= 9),
        "EDGE": int(odd_and_frac >= 9),
        "FRAC": f,
        "FRAC_EN": 1,
        "FRAC_WF_R": int(1 <= odd_and_frac <= 8),
        "FRAC_WF_F": int(2 <= odd_and_frac <= 9 or (f == 1 and d == 2)),
        "PHASE_MUX_F": ((eighths & 7) + (odd_and_frac >> 1)) & 7,
    }


def vivado_fields(changed, counter, second):
    """Final values of one counter's fields from 'TILE:BEL.FIELD=a->b' tokens (base: 10.0)."""
    out = {"DELAY_TIME": 0, "PHASE_MUX": 0, "HIGH_TIME": 5, "LOW_TIME": 5, "EDGE": 0, "FRAC": 0, "FRAC_EN": 0, "FRAC_WF_R": 0,
           "FRAC_WF_F": 0, "PHASE_MUX_F": 0}
    names = {f"{counter}_CLKOUT2_DELAY_TIME": "DELAY_TIME", f"{counter}_CLKOUT1_PHASE_MUX": "PHASE_MUX",
             f"{counter}_CLKOUT1_HIGH_TIME": "HIGH_TIME", f"{counter}_CLKOUT1_LOW_TIME": "LOW_TIME",
             f"{counter}_CLKOUT2_EDGE": "EDGE", f"{counter}_CLKOUT2_FRAC": "FRAC",
             f"{counter}_CLKOUT2_FRAC_EN": "FRAC_EN", f"{counter}_CLKOUT2_FRAC_WF_R": "FRAC_WF_R",
             f"{second}_CLKOUT2_FRACTIONAL_FRAC_WF_F": "FRAC_WF_F",
             f"{second}_CLKOUT2_FRACTIONAL_PHASE_MUX_F": "PHASE_MUX_F"}
    for tok in changed.split():
        m = re.match(r"[^:]+:[^.]+\.(\w+)=\d+->(\d+)$", tok)
        if m and m[1] in names:
            out[names[m[1]]] = int(m[2])
    return out


def main():
    if sys.argv[1:] == ["selftest"]:
        assert frac_fields(10.125) == dict(HIGH_TIME=4, LOW_TIME=4, EDGE=0, FRAC=1, FRAC_EN=1,
                                           FRAC_WF_R=1, FRAC_WF_F=0, PHASE_MUX_F=0,
                                           DELAY_TIME=0, PHASE_MUX=0)
        assert frac_fields(10.5, 270)["PHASE_MUX_F"] == 1 and frac_fields(10.5, 270)["DELAY_TIME"] == 7
        assert frac_fields(7.5)["EDGE"] == 1 and frac_fields(7.5)["PHASE_MUX_F"] == 6
        assert frac_fields(8.0) is None
        return print("selftest ok")
    ok = bad = 0
    for line in open(sys.argv[1]):
        f = line.rstrip("\n").split("\t")
        if len(f) < 3 or f[0] == "id":
            continue
        props = f[1].split()
        if len(props) != 3:
            continue
        kv = dict(zip(props[0].split(","), map(float, props[2].split(","))))
        prop = next((k for k in kv if k in ("CLKFBOUT_MULT_F", "CLKOUT0_DIVIDE_F")), None)
        if not prop or set(kv) - {prop, prop.split("_")[0] + "_PHASE"}:
            continue
        props = [prop, None, str(kv[prop])]
        want = frac_fields(kv[prop], kv.get(prop.split("_")[0] + "_PHASE", 0.0))
        if want is None:
            continue
        counter, second = ("CLKFBOUT", "CLKOUT6") if props[0] == "CLKFBOUT_MULT_F" else ("CLKOUT0", "CLKOUT5")
        got = vivado_fields(f[2], counter, second)
        diff = {k: (want[k], got[k]) for k in want if want[k] != got[k]}
        ok += not diff
        bad += bool(diff)
        if diff:
            print(f"MISMATCH {f[0]} {props[0]}={props[2]} (formula, vivado): {diff}")
    print(f"{ok} rows match the formula, {bad} do not")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
