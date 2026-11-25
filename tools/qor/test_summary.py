#!/usr/bin/env python3
"""The `zedbit: summary` contract that tools/qor reads (HELION_GAPS.md G44).

qor used to scrape zedbit's human output with regexes. A reworded line then made
a number parse as `None`, and a `None` gate is a gate that never fires - the
regression it was meant to catch would pass. The contract is now one JSON line,
and this checks both halves of it:

- every key qor gates on is present in a real zedbit summary line, so a rename
  on the emitter side cannot silently disable a gate;
- a summary line missing a key raises instead of returning `None`.

Run: python3 tools/qor/test_summary.py
"""
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, HERE)

import run  # noqa: E402


def check_emitter_and_parser_agree():
    """Every SUMMARY_KEYS entry appears in the emitter's json_object call."""
    src = open(os.path.join(ROOT, "crates/helion-xilinx7/src/xdc_flow.rs")).read()
    start = src.index("fn json_object")
    body = src[start:]
    missing = [k for k in run.SUMMARY_KEYS if f'"{k}"' not in src[index_of_emit(src):start]]
    assert not missing, f"qor expects keys the emitter does not print: {missing}"
    assert body, "json_object helper is gone"


def index_of_emit(src):
    marker = '("lut", built.luts.len() as f64)'
    return src.index(marker)


def check_parser_reads_summary():
    """A synthetic summary line round-trips into every gate key."""
    values = {k: 1.0 for k in run.SUMMARY_KEYS}
    values.update(lut=5.0, ff=6.0, carry=7.0, ramb18=0.0, ramb36=0.0, dsp=0.0,
                  period_ns=10.0, slack_ns=2.5)
    text = "zedbit timing: clock clk: period 10.000 ns, worst slack 2.500 ns\n"
    text += "zedbit: summary " + json.dumps(values) + "\n"
    r = run.parse(text)
    r["wall_s"] = values["t_synth"] + values["t_device"] + values["t_pack_place"] \
        + values["t_route"] + values["t_bitgen"]
    missing = [k for k, _ in run.GATES if r.get(k) is None]
    assert not missing, f"gate keys the parser did not fill: {missing}"
    assert r["lut"] == 5 and isinstance(r["lut"], int), r["lut"]
    assert r["fmax_mhz"] == round(1000 / 7.5, 1), r["fmax_mhz"]
    assert r["clock"] == "clk", r["clock"]


def check_missing_key_is_loud():
    """A renamed key must raise, never silently return None."""
    values = {k: 1.0 for k in run.SUMMARY_KEYS if k != "lut"}
    text = "zedbit: summary " + json.dumps(values) + "\n"
    try:
        run.parse(text)
    except SystemExit as e:
        assert "lut" in str(e), e
        return
    raise AssertionError("a summary line without `lut` was accepted")


def check_old_logs_still_parse():
    """Logs written before the summary line existed keep working."""
    text = ("zedbit: packed 10 LUTs, 11 flops, 12 carry cells, 0 RAMB36 + 0 RAMB18, "
            "0 DSP48, 0 LUT RAMs (0 LUT RAM LUTs), 4 pads; placing\n"
            "zedbit timing: worst clock period 10.000 ns, worst slack 3.000 ns "
            "(0 of 11 endpoints failing), est. fmax 142 MHz on that clock\n")
    r = run.parse(text)
    assert r["lut"] == 10, r["lut"]
    assert r["slack_ns"] == 3.0, r["slack_ns"]


def main():
    for fn in (check_emitter_and_parser_agree, check_parser_reads_summary,
               check_missing_key_is_loud, check_old_logs_still_parse):
        fn()
        print(f"ok  {fn.__name__}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
