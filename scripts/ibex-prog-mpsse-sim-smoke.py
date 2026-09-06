#!/usr/bin/env python3
"""FM-HEL-TOP: capped Ibex .hbits + mpsse-sim CFG_W/STAT smoke. Never uncapped."""
import os, subprocess, sys, time

ROOT = os.environ.get("HELION_ROOT", os.getcwd())
os.chdir(ROOT)
CAP = int(os.environ.get("IBEX_IMPL_CAP_SEC", "120"))
DESIGNS = [d.strip() for d in os.environ.get("IBEX_SMOKE_DESIGNS", "reduced,pinwrap,bare").split(",") if d.strip()]
SKIP_BUILD = os.environ.get("IBEX_SMOKE_SKIP_BUILD", "0") in ("1", "true", "yes")

BUILD = {
    "reduced": ("scripts/ibex-impl-reduced-capped.sh", "target/ibex-reduced-impl.hbits"),
    "pinwrap": ("scripts/ibex-impl-pinwrap-capped.sh", "target/ibex-pinwrap-impl.hbits"),
    "bare": ("scripts/ibex-impl-capped.sh", "target/ibex-impl.hbits"),
}

def find_prog():
    for p in ("target/debug/helion-prog", "target/release/helion-prog"):
        if os.path.isfile(p):
            return p
    return None

prog = find_prog()
if not prog:
    print("IBEX_MPSSE_SMOKE: missing helion-prog binary", flush=True)
    sys.exit(2)

results = []
overall = 0
print(f"IBEX_MPSSE_SMOKE: designs={DESIGNS} cap={CAP}s skip_build={int(SKIP_BUILD)} prog={prog}", flush=True)

for name in DESIGNS:
    if name not in BUILD:
        print(f"IBEX_MPSSE_SMOKE: unknown design {name!r} (use reduced|pinwrap|bare)", flush=True)
        overall = 2
        continue
    build_sh, hbits = BUILD[name]
    if not SKIP_BUILD or not os.path.isfile(hbits):
        env = os.environ.copy()
        env["IBEX_IMPL_CAP_SEC"] = str(CAP)
        env["HELION_ROOT"] = ROOT
        print(f"IBEX_MPSSE_SMOKE: build {name} via {build_sh} (cap={CAP}s)", flush=True)
        br = subprocess.run(["bash", build_sh], env=env)
        if br.returncode != 0:
            print(f"IBEX_MPSSE_SMOKE: BUILD_FAIL design={name} exit={br.returncode}", flush=True)
            results.append((name, "BUILD_FAIL", br.returncode, 0, 0, ""))
            overall = overall or br.returncode
            continue
    if not os.path.isfile(hbits):
        print(f"IBEX_MPSSE_SMOKE: missing {hbits}", flush=True)
        results.append((name, "NO_HBITS", 2, 0, 0, ""))
        overall = overall or 2
        continue
    size = os.path.getsize(hbits)
    t0 = time.time()
    pr = subprocess.run([prog, "--cable", "mpsse-sim", hbits], capture_output=True, text=True)
    wall = time.time() - t0
    out = ((pr.stdout or "") + (pr.stderr or "")).strip()
    print(out, flush=True)
    done = "DONE=1" in out and "STAT" in out and "CRC_ERR=0" in out
    status = "STAT_DONE" if (pr.returncode == 0 and done) else "PROG_FAIL"
    if status != "STAT_DONE":
        overall = overall or (pr.returncode or 1)
    print(f"IBEX_MPSSE_SMOKE: {name} {status} exit={pr.returncode} bytes={size} prog_wall={wall:.3f}s path={hbits}", flush=True)
    print("IBEX_MPSSE_SMOKE: honesty=sim_fabric_DONE_only not_board_DONE not_USB", flush=True)
    results.append((name, status, pr.returncode, size, wall, out.splitlines()[-1] if out else ""))

print("IBEX_MPSSE_SMOKE_SUMMARY:", flush=True)
for name, status, rc, size, wall, line in results:
    print(f"  {name}: {status} exit={rc} bytes={size} prog_wall={wall:.3f}s | {line}", flush=True)
sys.exit(overall)
