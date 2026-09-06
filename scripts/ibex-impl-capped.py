#!/usr/bin/env python3
"""FM-HEL-TOP: capped Ibex place/route/bitstream. Never uncapped. Cap <=120s.
Mac-safe process-group SIGKILL (no GNU timeout). Wall clock independent of I/O.
"""
import os, signal, subprocess, sys, threading, time

cap = int(os.environ.get("IBEX_IMPL_CAP_SEC", "120"))
root = os.environ.get("HELION_ROOT", os.getcwd())
os.chdir(root)

helion = os.path.join(root, "target/debug/helion")
if not os.path.isfile(helion):
    helion = os.path.join(root, "target/release/helion")

out_bits = os.path.join(root, "target/ibex-impl.hbits")
os.makedirs(os.path.dirname(out_bits), exist_ok=True)
try:
    os.remove(out_bits)
except FileNotFoundError:
    pass

if os.path.isfile(helion):
    cmd = [helion, "bitstream", "examples/ysyx_ibex.sv", "-o", out_bits, "--part", "HL10T-C32-1"]
else:
    cmd = [
        "cargo", "run", "-p", "helion-cli", "--release", "--",
        "bitstream", "examples/ysyx_ibex.sv", "-o", out_bits, "--part", "HL10T-C32-1",
    ]

print(f"IBEX_IMPL: cap={cap}s cmd={' '.join(cmd)}", flush=True)
proc = subprocess.Popen(
    cmd,
    preexec_fn=os.setsid if hasattr(os, "setsid") else None,
    stdout=subprocess.PIPE,
    stderr=subprocess.STDOUT,
    text=True,
    bufsize=1,
)

def pump():
    try:
        for line in proc.stdout:
            sys.stdout.write(line)
            sys.stdout.flush()
    except Exception:
        pass

t = threading.Thread(target=pump, daemon=True)
t.start()
t0 = time.time()
try:
    while proc.poll() is None:
        if time.time() - t0 >= cap:
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except Exception:
                proc.kill()
            print(f"\nIBEX_IMPL_CAP: killed after {cap}s", flush=True)
            try:
                proc.wait(timeout=2)
            except Exception:
                pass
            size = os.path.getsize(out_bits) if os.path.isfile(out_bits) else 0
            print(f"IBEX_IMPL: TIMEOUT exit=124 elapsed={cap}s hbits_bytes={size}", flush=True)
            sys.exit(124)
        time.sleep(0.2)
    t.join(timeout=1)
    rc = proc.returncode or 0
    elapsed = time.time() - t0
    size = os.path.getsize(out_bits) if os.path.isfile(out_bits) else 0
    print(f"IBEX_IMPL: exit={rc} elapsed={elapsed:.2f}s hbits_bytes={size} path={out_bits}", flush=True)
    sys.exit(rc)
except KeyboardInterrupt:
    try:
        os.killpg(proc.pid, signal.SIGKILL)
    except Exception:
        proc.kill()
    raise
