#!/usr/bin/env python3
"""Helion vs Vivado on the 309-design RTL sweep.

Runs zedbit on every sweep unit with an empty XDC: zedbit prints its packed
counts before it checks pins, then stops at the first missing pin. Those counts
are joined with Vivado 2017.3's post-route counts for the same part
(campus/recon/impl/impl_<part>.csv).

Usage: helion_vs_vivado.py ZEDBIT VIVADO_CSV OUT_CSV [-j N] [--only u1,u2] [--timeout S]
       helion_vs_vivado.py --selftest
"""
import argparse, csv, os, statistics as st, subprocess, sys, tempfile
from concurrent.futures import ThreadPoolExecutor, as_completed

# Paths default to the Mac layout; the cluster jobs set HVV_REL / HVV_SWEEP.
REL = os.path.expanduser(os.environ.get("HVV_REL", "~/helion-release"))
SWEEP = os.path.expanduser(os.environ.get("HVV_SWEEP", "~/helion-vivado-data/campus/recon/rtlsweep"))
ROOTS = {"rel": REL, "zed": os.path.expanduser("~/helion-zed-builds/vivado_jobs/rtl"), "micro": os.path.join(SWEEP, "micro")}
sys.path.insert(0, os.path.join(REL, "tools/qor"))
from run import parse  # noqa: E402  (same reader as the QoR harness)

# Helion count, Vivado column. Vivado's bram counts RAMB18E1 and RAMB36E1 cells alike.
PAIRS = [("lut", "lut"), ("ff", "ff"), ("carry", "carry"), ("bram", "bram"), ("dsp", "dsp")]


def src(f):
    head, _, rest = f.partition("/")
    return os.path.join(ROOTS[head], rest)


def units():
    for m in ("micro", "corpus", "extra", "vhdl2008"):
        for line in open(f"{SWEEP}/manifest_{m}.txt"):
            if line.strip():
                yield line.strip().split("|")


def run_one(zedbit, xdc, tmp, u, timeout):
    unit, _lang, top, files = u
    cmd = [zedbit, ",".join(src(f) for f in files.split(",")), xdc, os.path.join(tmp, unit + ".bit"), top]
    try:
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout, cwd=REL)
        text = p.stdout + p.stderr
    except subprocess.TimeoutExpired:
        text = "zedbit: timeout"
    r = parse(text)
    r["bram"] = None if r["ramb18"] is None else r["ramb18"] + r["ramb36"]
    lines = [l for l in text.splitlines() if l.strip()]
    r["helion_stage"] = "packed" if r["lut"] is not None else "failed"
    r["helion_msg"] = (lines[-1] if lines else "")[:160]
    return unit, r


def ratio(h, v):
    """Helion/Vivado; None when either side is missing; inf when Vivado has 0 and Helion does not."""
    if h is None or v in (None, ""):
        return None
    v = float(v)
    return (1.0 if h == 0 else float("inf")) if v == 0 else h / v


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("zedbit"); ap.add_argument("vivado_csv"); ap.add_argument("out_csv")
    ap.add_argument("-j", type=int, default=6)
    ap.add_argument("--only")
    ap.add_argument("--skip", help="comma-separated substrings: leave out designs whose unit name contains one")
    ap.add_argument("--shard", help="I/N: run every Nth design starting at I (one Slurm job each)")
    ap.add_argument("--timeout", type=float, default=600)
    a = ap.parse_args()
    viv = {r["unit"]: r for r in csv.DictReader(open(a.vivado_csv))}
    todo = [u for u in units() if not a.only or u[0] in a.only.split(",")]
    if a.skip:
        todo = [u for u in todo if not any(k in u[0] for k in a.skip.split(","))]
    if a.shard:
        i, n = map(int, a.shard.split("/")); todo = todo[i::n]
    tmp = tempfile.mkdtemp(prefix="hvv_")
    xdc = os.path.join(tmp, "empty.xdc"); open(xdc, "w").close()
    # Rows are appended as each design finishes, so a killed run resumes where it stopped.
    done = {r["unit"]: r for r in csv.DictReader(open(a.out_csv))} if os.path.exists(a.out_csv) else {}
    rows = [done[u[0]] for u in todo if u[0] in done]
    todo = [u for u in todo if u[0] not in done]
    f = open(a.out_csv, "a", newline=""); w = None
    with ThreadPoolExecutor(a.j) as ex:
        futs = [ex.submit(run_one, a.zedbit, xdc, tmp, u, a.timeout) for u in todo]
        for i, fut in enumerate(as_completed(futs)):
            unit, r = fut.result()
            v = viv.get(unit, {})
            row = {"unit": unit, "vivado_status": v.get("status", "missing"), "vivado_mode": v.get("mode", ""),
                   "helion_stage": r["helion_stage"]}
            for hk, vk in PAIRS:
                row[f"h_{hk}"], row[f"v_{vk}"] = r[hk], v.get(vk, "")
                row[f"r_{hk}"] = ratio(r[hk], v.get(vk)) if v.get("status") == "ok" else None
            row["helion_msg"] = r["helion_msg"]
            if w is None:
                w = csv.DictWriter(f, fieldnames=list(row))
                if not done: w.writeheader()
            w.writerow(row); f.flush(); rows.append(row)
            print(f"[{len(done) + i + 1}/{len(done) + len(todo)}] {unit}: {r['helion_stage']} lut={r['lut']} ff={r['ff']}", file=sys.stderr, flush=True)
    f.close()
    rows = [{k: (float(v) if k.startswith("r_") and v not in (None, "") else v) for k, v in r.items()} for r in rows]
    rows = [{k: (None if k.startswith("r_") and v == "" else v) for k, v in r.items()} for r in rows]
    summarize(rows)


def summarize(rows):
    both = [r for r in rows if r["vivado_status"] == "ok" and r["helion_stage"] == "packed"]
    print(f"designs {len(rows)}; both packed/built {len(both)}; Helion failed where Vivado built "
          f"{sum(r['vivado_status'] == 'ok' and r['helion_stage'] == 'failed' for r in rows)}; "
          f"Vivado failed where Helion packed {sum(r['vivado_status'] == 'fail' and r['helion_stage'] == 'packed' for r in rows)}")
    for k, _ in PAIRS:
        rs = [r[f"r_{k}"] for r in both if r[f"r_{k}"] not in (None, float("inf"))]
        inf = sum(r[f"r_{k}"] == float("inf") for r in both)
        if rs:
            q = sorted(rs)
            print(f"{k:6} median {st.median(q):.2f}  p90 {q[9 * len(q) // 10]:.2f}  >1.5x {sum(x > 1.5 for x in q)}  "
                  f"<0.67x {sum(x < 0.67 for x in q)}  Helion>0 where Vivado=0: {inf}")


def selftest():
    assert src("micro/m.v").endswith("rtlsweep/micro/m.v") and src("rel/a/b.sv") == os.path.join(REL, "a/b.sv")
    assert ratio(10, "5") == 2.0 and ratio(0, "0") == 1.0 and ratio(3, "0") == float("inf") and ratio(None, "4") is None
    r = parse("zedbit: packed 60 LUTs, 27 flops, 3 carry cells, 1 RAMB36 + 2 RAMB18, 4 DSP48, 0 LUT RAMs (0 LUT RAM LUTs), 9 pads; placing\n")
    assert (r["lut"], r["ff"], r["carry"], r["ramb36"], r["ramb18"], r["dsp"]) == (60, 27, 3, 1, 2, 4), r
    assert sum(1 for _ in units()) == 309
    print("selftest ok")


if __name__ == "__main__":
    selftest() if sys.argv[1:] == ["--selftest"] else main()
