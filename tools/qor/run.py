#!/usr/bin/env python3
"""Helion quality-of-results and runtime benchmark on the ZedBoard (gap G21).

Builds each design with the zedbit example, compares the numbers with
baseline.json (regression gate, exit 1) and with vivado.json / targets.json
(goals, reported only). See README.md.
"""
import argparse
import json
import os
import re
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
ZED = "examples/zedboard.xdc"

# name, set, sources, XDC, top, extra environment
DESIGNS = [
    ("blinky", "quick", "examples/zedboard_blinky.sv", ZED, "zedboard_blinky", {}),
    ("counter", "quick", "tools/svcheck/tests/counter.sv", ZED, "counter", {}),
    ("complex", "quick", "tools/svcheck/tests/complex.sv", ZED, "complex", {}),
    ("hier_bitsel", "quick", "tools/svcheck/tests/hier_bitsel.sv", ZED, "top", {}),
    ("pico", "quick", "examples/zedboard_pico_soc.sv,third_party/picorv32/picorv32.v", "examples/zedboard_pico_soc.xdc", "zedboard_pico_soc", {}),
    ("bram_fill", "full", "examples/zedboard_bram_fill.sv", "examples/zedboard_bram_fill.xdc", "zedboard_bram_fill", {}),
    ("dsp_fill_prims", "full", "examples/zedboard_dsp_fill_prims.sv", "examples/zedboard_dsp_fill.xdc", "zedboard_dsp_fill_prims", {}),
    ("ibex", "full", "examples/ysyx_ibex.sv,examples/ibex_obs_wrap.sv", "tools/qor/ibex_obs.xdc", "ibex_obs_wrap", {"HELION_DEFINES": "VERILATOR"}),
]

COUNTS = [("lut", r"(\d+) LUTs"), ("ff", r"(\d+) flops"), ("carry", r"(\d+) carry cells"),
          ("ramb36", r"(\d+) RAMB36"), ("ramb18", r"(\d+) RAMB18"), ("dsp", r"(\d+) DSP48")]
TIMES = [("t_synth", r"synthesized in ([\d.]+)s"), ("t_pack_place", r"placed in ([\d.]+)s"),
         ("t_route", r"routed in ([\d.]+)s"), ("t_device", r"device ([\d.]+)s,"), ("t_bitgen", r"bitgen ([\d.]+)s")]

COUNT_KEYS = ("lut", "ff", "carry", "ramb18", "ramb36", "dsp")
# key, direction in which a change is worse (+1: higher is worse)
GATES = [("lut", 1), ("ff", 1), ("carry", -1), ("ramb18", -1), ("ramb36", -1), ("dsp", -1),
         ("slack_ns", -1), ("wall_s", 1), ("t_route", 1)]


def parse(text):
    """Numbers from zedbit's output (stdout and stderr together)."""
    r = {}
    packed = re.search(r"^zedbit: packed (.*)$", text, re.M)
    for k, pat in COUNTS:
        m = packed and re.search(pat, packed.group(1))
        r[k] = int(m.group(1)) if m else None
    m = re.search(r"(\d+) LUT levels", text)
    r["lut_levels"] = int(m.group(1)) if m else None
    for k, pat in TIMES:
        m = re.search(pat, text)
        r[k] = float(m.group(1)) if m else None
    clocks = re.findall(r"zedbit timing: clock (\S+): period ([\d.]+) ns", text)
    m = re.search(r"period ([\d.]+) ns, worst slack (-?[\d.]+) ns \((\d+) of (\d+) endpoints failing", text)
    if m:
        p, s = float(m.group(1)), float(m.group(2))
        r.update(clock="/".join(c for c, cp in clocks if float(cp) == p) or None, period_ns=p, slack_ns=s,
                 failing=int(m.group(3)), endpoints=int(m.group(4)),
                 fmax_mhz=round(1000 / (p - s), 1) if p > s else None)
    r["front"] = "zb" if "using the zb front end" in text else "v2"
    return r


def build(zedbit, d, out_dir, timeout=None, tag=""):
    name, _, files, xdc, top, env = d
    bit, log = (os.path.join(out_dir, name + tag + ext) for ext in (".bit", ".log"))
    t = time.monotonic()
    with open(log, "w") as f:  # streamed, so a long build can be watched with tail -f
        try:
            rc = subprocess.run([zedbit, files, xdc, bit, top, "--ignore-timing"], cwd=ROOT, timeout=timeout,
                                env={**os.environ, **env}, stdout=f, stderr=subprocess.STDOUT).returncode
        except subprocess.TimeoutExpired:
            rc = f"timeout ({timeout:g} s)"
    with open(log) as f:
        text = f.read()
    r = parse(text)
    r.update(ok=rc == 0, wall_s=round(time.monotonic() - t, 1), load1=round(os.getloadavg()[0]))
    if not r["ok"]:
        r["error"] = f"exit {rc}: " + (text.strip().splitlines() or ["no output"])[-1][:300]
    return r, bit


def vs_vivado(name, r, viv):
    v = viv["designs"].get(name, {})
    ref, hx, m = v.get("ref"), v.get("hx"), {}
    if ref and r.get("lut") is not None:
        m["lut_ratio"] = round(r["lut"] / ref["lut"], 2)
    if ref and r.get("fmax_mhz") and r.get("period_ns") == ref["period_ns"]:  # same clock only
        m["fmax_ratio"] = round(r["fmax_mhz"] / ref["fmax_mhz"], 2)
    if ref and r["ok"]:
        m["wall_ratio"] = round(r["wall_s"] / ref["runtime_s"], 1)
    if hx and r.get("t_route") is not None:
        m["route_ratio_hx"] = round(r["t_route"] / hx["route_s"], 1)
    if hx and r.get("slack_ns") is not None:
        m["slack_gap_hx"] = round(hx["wns_all_ns"] - r["slack_ns"], 3)  # > 0: Helion is pessimistic
    return m


def compare(base, cur):
    """Regressions (fail the run) and warnings against the baseline."""
    fails, warns = [], []
    for name, c in cur.items():
        b = base.get(name)
        if b is None:
            warns.append(f"{name}: not in the baseline (run --accept to add it)")
            continue
        if b["ok"] and not c["ok"]:
            fails.append(f"{name}: built before, now fails: {c.get('error')}")
        if c["ok"] and not b["ok"]:
            warns.append(f"{name}: now builds (failed in the baseline); --accept records it")
        if b.get("deterministic") and not c.get("deterministic", True):
            fails.append(f"{name}: two builds no longer give the same .bit (G22)")
        for k, worse in GATES:
            bv, cv = b.get(k), c.get(k)
            # Counts are gated even for a build that packed but did not finish (G49); timing only when both finished.
            if bv is None or (k not in COUNT_KEYS and not (b["ok"] and c["ok"])):
                continue
            if cv is None:
                fails.append(f"{name}: {k} no longer reported (zedbit stopped earlier, or its output changed: update parse())")
                continue
            d = (cv - bv) * worse
            msg = f"{name}: {k} {bv} -> {cv}"
            if k == "slack_ns":
                if d > 0.3:
                    fails.append(msg + " ns (worse by more than 0.3 ns)")
            elif k in ("wall_s", "t_route"):
                ratio = cv / bv if bv else float("inf")
                if d > 10 and ratio > 2:  # 10 s floor: small builds are all noise
                    fails.append(msg + f" s ({ratio:.1f}x, over 2x)")
                elif d > 10 and ratio > 1.5:
                    warns.append(msg + f" s ({ratio:.1f}x; runtime is noisy, fails over 2x)")
            elif d > 0.02 * bv:
                fails.append(msg + (" (worse by more than 2%)" if worse > 0 else " (fewer hard blocks: logic fell back to LUTs)"))
    return fails, warns


def check_targets(targets, cur):
    out = []
    for t in targets:
        r = cur.get(t["design"])
        v = None if r is None else r.get(t["metric"])
        if r is None:
            res = "not run"
        elif v is None:
            res = "n/a"
        else:
            good = ("min" not in t or v >= t["min"]) and ("max" not in t or v <= t["max"]) and ("eq" not in t or v == t["eq"])
            res = "pass" if good else "FAIL"
        out.append(dict(t, value=v, result=res))
    return out


def fmt(v):
    return "-" if v is None else ("yes" if v is True else "no" if v is False else str(v))


def table(cur):
    cols = [("design", None), ("built", "ok"), ("LUT", "lut"), ("FF", "ff"), ("carry", "carry"), ("RAMB18", "ramb18"),
            ("RAMB36", "ramb36"), ("DSP", "dsp"), ("levels", "lut_levels"), ("clock", "clock"), ("period", "period_ns"),
            ("slack", "slack_ns"), ("Fmax", "fmax_mhz"), ("synth s", "t_synth"), ("pack+place s", "t_pack_place"),
            ("route s", "t_route"), ("wall s", "wall_s"), ("load", "load1"), ("LUT xVivado G1/G8", "lut_ratio"),
            ("Fmax/Vivado G5", "fmax_ratio"), ("wall xVivado", "wall_ratio"), ("route x hx G4", "route_ratio_hx"),
            ("slack gap hx G7", "slack_gap_hx")]
    rows = [[n] + [fmt(r.get(k)) for _, k in cols[1:]] for n, r in cur.items()]
    print("| " + " | ".join(h for h, _ in cols) + " |\n|" + "---|" * len(cols))
    for row in rows:
        print("| " + " | ".join(row) + " |")


def load(name):
    with open(os.path.join(HERE, name)) as f:
        return json.load(f)


def git(*args):
    return subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True).stdout.strip()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--set", choices=["quick", "full"], default="quick")
    ap.add_argument("--accept", action="store_true", help="write this run's numbers into baseline.json")
    ap.add_argument("--json", help="write the results to this file")
    ap.add_argument("--timeout", type=float, help="seconds per build; a build killed by it counts as failed")
    ap.add_argument("--only", help="comma-separated design names: build just these (any set)")
    ap.add_argument("--selftest", action="store_true", help="check the parser and the gate, build nothing")
    a = ap.parse_args()
    if a.selftest:
        return selftest()
    db = os.path.expanduser("~/.cache/helion/prjxray-db")
    if "HELION_XILINX_DB" not in os.environ and os.path.isdir(db):
        os.environ["HELION_XILINX_DB"] = db
    subprocess.run(["cargo", "build", "-q", "--release", "-p", "helion-xilinx7", "--example", "zedbit"], cwd=ROOT, check=True)
    tdir = json.loads(subprocess.check_output(["cargo", "metadata", "--format-version", "1", "--no-deps"], cwd=ROOT))["target_directory"]
    zedbit = os.path.join(tdir, "release", "examples", "zedbit")
    out_dir = os.path.join(tdir, "qor")
    os.makedirs(out_dir, exist_ok=True)
    commit = git("rev-parse", "--short", "HEAD") + ("+dirty" if git("status", "--porcelain", "--untracked-files=no", "--", ".", ":!tools/qor") else "")
    date = time.strftime("%Y-%m-%d %H:%M")
    viv = load("vivado.json")
    print(f"qor: {a.set} set, Helion {commit}, {date}, load {os.getloadavg()[0]:.0f}, logs in {out_dir}", flush=True)

    cur = {}
    for d in DESIGNS:
        if (a.only and d[0] not in a.only.split(",")) or (not a.only and a.set == "quick" and d[1] != "quick"):
            continue
        print(f"qor: {d[0]} ...", flush=True)
        r, bit = build(zedbit, d, out_dir, a.timeout)
        if d[0] == "blinky":  # G22: same settings twice, byte-identical .bit
            r2, bit2 = build(zedbit, d, out_dir, a.timeout, "_2")
            r["deterministic"] = r["ok"] and r2["ok"] and open(bit, "rb").read() == open(bit2, "rb").read()
        r.update(vs_vivado(d[0], r, viv), commit=commit, date=date)
        cur[d[0]] = r
        print(f"qor: {d[0]}: {'built' if r['ok'] else 'FAILED, ' + r['error']} in {r['wall_s']} s", flush=True)

    print()
    table(cur)
    if "blinky" in cur:
        print(f"\nG22 determinism: blinky built twice, .bit {'byte-identical' if cur['blinky']['deterministic'] else 'DIFFERS'}")
    tres = check_targets(load("targets.json"), cur)
    print("\nTargets (goals; they do not fail the run):")
    for t in tres:
        bound = " ".join(f"{k} {t[k]}" for k in ("min", "max", "eq") if k in t)
        print(f"  {t['gap']:4} {t['design']:15} {t['metric']:14} {fmt(t['value']):>8}  ({bound})  {t['result']:7} {t['goal']}")

    bpath = os.path.join(HERE, "baseline.json")
    base = load("baseline.json")["designs"] if os.path.exists(bpath) else None
    fails, warns = compare(base, cur) if base is not None else ([], ["no baseline.json yet: run with --accept"])
    for w in warns:
        print("warning:", w)
    for f in fails:
        print("REGRESSION:", f)
    if a.json:
        with open(a.json, "w") as f:
            json.dump(dict(commit=commit, date=date, set=a.set, designs=cur, targets=tres, regressions=fails, warnings=warns), f, indent=1)
    if a.accept:
        designs = dict(base or {})
        designs.update(cur)
        with open(bpath, "w") as f:
            json.dump({"designs": designs}, f, indent=1, sort_keys=True)
            f.write("\n")
        print(f"qor: baseline.json updated from this run ({', '.join(cur)})")
        return 0
    print(f"\nqor: {len(fails)} regression(s), {len(warns)} warning(s)")
    return 1 if fails else 0


def selftest():
    log = """zedbit synth: v2 front end, 60 LUTs, 27 flops, 5 LUT levels
zedbit: synthesized in 0.1s; loading the device
zedbit: packed 60 LUTs, 27 flops, 3 carry cells, 1 RAMB36 + 2 RAMB18, 4 DSP48, 9 pads; placing
zedbit: placed in 2.7s
zedbit: routed in 17.0s
zedbit timing: clock clk_half_bufg: period 20.000 ns
zedbit timing: clock clk_bufg: period 10.000 ns
zedbit timing: worst clock period 20.000 ns, worst slack 6.359 ns (0 of 1538 endpoints failing), est. fmax 73 MHz on that clock
zedbit timing: synth 0.1s, device 1.6s, pack+place 2.7s, route 17.0s, bitgen 1.4s
"""
    r = parse(log)
    want = dict(lut=60, ff=27, carry=3, ramb36=1, ramb18=2, dsp=4, lut_levels=5, t_synth=0.1, t_pack_place=2.7, t_route=17.0,
                t_device=1.6, t_bitgen=1.4, clock="clk_half_bufg", period_ns=20.0, slack_ns=6.359, failing=0, endpoints=1538,
                fmax_mhz=73.3, front="v2")
    assert r == want, r
    # Older output (no hard-block counts) still parses what it has.
    old = parse("zedbit: packed 4406 LUTs, 1536 flops, 0 carry cells, 15 pads; placing\n")
    assert (old["lut"], old["carry"], old["dsp"], old.get("slack_ns")) == (4406, 0, None, None)
    b = dict(r, ok=True, wall_s=30.0, deterministic=True)
    ok = lambda **kw: compare({"d": b}, {"d": dict(b, **kw)})
    assert ok() == ([], [])
    assert ok(lut=61) == ([], [])                     # 1.7%: within 2%
    assert len(ok(lut=62)[0]) == 1                    # 3.3%: regression
    assert ok(lut=40, ff=20) == ([], [])              # better is fine
    assert len(ok(carry=0)[0]) == 1 and ok(carry=9) == ([], [])
    assert ok(slack_ns=6.1) == ([], []) and len(ok(slack_ns=6.0)[0]) == 1
    assert ok(t_route=27.0) == ([], [])               # under the 10 s floor
    assert ok(wall_s=50.0)[0] == [] and len(ok(wall_s=50.0)[1]) == 1  # 1.7x: warning
    assert len(ok(wall_s=70.0)[0]) == 1               # 2.3x: regression
    assert len(ok(ok=False, error="x")[0]) == 1
    assert len(ok(deterministic=False)[0]) == 1
    assert len(ok(slack_ns=None)[0]) == 1             # parser lost a field
    assert compare({"d": dict(b, ok=False)}, {"d": dict(b, ok=False)}) == ([], [])
    nb = dict(b, ok=False, slack_ns=None, t_route=None)  # packed, did not finish
    assert compare({"d": nb}, {"d": dict(nb, dsp=0)})[0] != []          # counts still gated
    assert compare({"d": nb}, {"d": dict(nb, wall_s=500.0)}) == ([], [])  # runtime not
    assert len(compare({"d": nb}, {"d": dict(b)})[1]) == 1             # now builds: warning
    t = check_targets([{"gap": "G1", "design": "d", "metric": "lut", "max": 50, "goal": ""},
                       {"gap": "G9", "design": "d", "metric": "ok", "eq": True, "goal": ""},
                       {"gap": "G4", "design": "x", "metric": "t_route", "max": 1, "goal": ""}], {"d": b})
    assert [x["result"] for x in t] == ["FAIL", "pass", "not run"], t
    viv = {"designs": {"d": {"ref": {"lut": 30, "period_ns": 20.0, "fmax_mhz": 100.0, "runtime_s": 10.0},
                             "hx": {"route_s": 17.0, "wns_all_ns": 8.359}}}}
    assert vs_vivado("d", b, viv) == dict(lut_ratio=2.0, fmax_ratio=0.73, wall_ratio=3.0, route_ratio_hx=1.0, slack_gap_hx=2.0)
    assert "fmax_ratio" not in vs_vivado("d", dict(b, period_ns=10.0), viv)
    print("qor selftest: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
