#!/usr/bin/env bash
# FM-HEL-HANG: never leave orphan Ibex cargo. Hard-cap ~75s.
# Mac-safe: no GNU timeout — python3 process-group alarm.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
CAP_SEC="${IBEX_CAP_SEC:-75}"
exec python3 - "$CAP_SEC" <<'PY'
import os, signal, subprocess, sys, time
cap = int(sys.argv[1])
cmd = [
    "cargo", "test", "-p", "helion-sv", "--lib", "--",
    "ysyx_ibex_lists_modules_and_synths_top", "--nocapture",
]
proc = subprocess.Popen(
    cmd,
    preexec_fn=os.setsid if hasattr(os, "setsid") else None,
)
t0 = time.time()
try:
    while proc.poll() is None:
        if time.time() - t0 >= cap:
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except Exception:
                proc.kill()
            print(f"IBEX_CAP: killed after {cap}s", flush=True)
            sys.exit(124)
        time.sleep(0.2)
    sys.exit(proc.returncode or 0)
except KeyboardInterrupt:
    try:
        os.killpg(proc.pid, signal.SIGKILL)
    except Exception:
        proc.kill()
    raise
PY
