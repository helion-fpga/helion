#!/usr/bin/env bash
# FM-HEL-TOP gap4: never leave orphan sha256 cargo/synth. Hard-cap ~60s.
# Prefer corpus wrapper sha256_phase_d.v (w_mem top). Full sha256_all.v hangs.
# Mac-safe: no GNU timeout — python3 process-group alarm.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
CAP_SEC="${SHA256_CAP_SEC:-60}"
RTL="${SHA256_RTL:-/workspace/fm-hel-corpus/vendored/logikbench/sha256/rtl/sha256_phase_d.v}"
if [[ ! -f "$RTL" ]]; then
  RTL="$ROOT/corpus-pass/sha256/rtl/sha256_phase_d.v"
fi
# Prefer release binary (FM-HEL-OPT-P2-1). HELION= overrides; else release, debug, cargo.
if [[ -z "${HELION:-}" ]]; then
  if [[ -f "$ROOT/target/release/helion" ]]; then
    HELION="$ROOT/target/release/helion"
  elif [[ -f "$ROOT/target/debug/helion" ]]; then
    HELION="$ROOT/target/debug/helion"
  else
    HELION=""
  fi
fi
echo "SHA256_SYNTH: cap=${CAP_SEC}s helion=${HELION:-cargo run -p helion-cli --release}" >&2
exec python3 - "$CAP_SEC" "$HELION" "$RTL" <<'PY'
import os, signal, subprocess, sys, time
cap = int(sys.argv[1]); helion = sys.argv[2]; path = sys.argv[3]
if helion and os.path.isfile(helion):
    cmd = [helion, "synth", path]
else:
    cmd = ["cargo", "run", "-p", "helion-cli", "--release", "--", "synth", path]
proc = subprocess.Popen(cmd, preexec_fn=os.setsid if hasattr(os, "setsid") else None)
t0 = time.time()
try:
    while proc.poll() is None:
        if time.time() - t0 >= cap:
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except Exception:
                proc.kill()
            print(f"SHA256_CAP: killed after {cap}s", flush=True)
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
