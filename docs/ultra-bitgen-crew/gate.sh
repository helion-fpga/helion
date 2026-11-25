#!/bin/sh
# UltraScale+ bitgen gate (integration-owned).
#
# Runs the four `helion impl` commands (blinky/counter x xcku3p/xcvu3p,
# seed 0) from the integration worktree, then the diff harness, and
# prints the greedy queue (blocks by diff_bits descending, from the
# harness run's JSONL records). Exit 0 only when the impl commands and
# the harness both finish; a red block does not fail the gate script
# itself, the queue IS the work list. No .bit file is expected until
# ROUTE and BITGEN land; the impl commands fail loud at ROUTE today.
#
# Usage: ./gate.sh [worktree]   (default worktree:
#   ~/workspace/helion-ultra-bitgen-integ)
set -e
WORKTREE="${1:-$HOME/workspace/helion-ultra-bitgen-integ}"
HARNESS="$HOME/workspace/ultra-bitgen-crew/harness"
LOGDIR="/tmp/ultra-logs"
mkdir -p "$LOGDIR"
HELION="$WORKTREE/target/debug/helion"

if [ ! -x "$HELION" ]; then
    echo "gate: no helion binary at $HELION; build it first (cargo build -j1 -p helion-cli)" >&2
    exit 2
fi

echo "== impl (seed 0) =="
for design in blinky counter; do
    for part in xcku3p xcvu3p; do
        log="$LOGDIR/gate-$design-$part.log"
        set +e
        (cd "$WORKTREE" && "$HELION" impl "examples/$design.sv" --part "$part" --seed 0 >"$log" 2>&1)
        rc=$?
        set -e
        echo "--- $design/$part exit=$rc"
        grep -E "hang_diag ultra (synth|pack|place|legality|fasm)|hang_diag timing|impl:|ULTRA" "$log" | tail -8
    done
done

echo "== diff harness =="
(cd "$HARNESS" && python3 diff.py > /tmp/ultra-logs/gate-harness.log 2>&1)
echo "harness log: /tmp/ultra-logs/gate-harness.log"
tail -3 /tmp/ultra-logs/gate-harness.log

echo "== greedy queue (blocks by diff_bits descending) =="
python3 - "$HARNESS" <<'PYEOF'
import json, os, sys, glob
harness = sys.argv[1]
runs = sorted(glob.glob(os.path.join(harness, "runs", "*")))
if not runs:
    print("no harness runs found")
    sys.exit(0)
latest = runs[-1]
print(f"run: {latest}")
recs = []
for name in glob.glob(os.path.join(latest, "*.jsonl")):
    with open(name) as f:
        for line in f:
            line = line.strip()
            if line:
                recs.append(json.loads(line))
def key(r):
    d = r.get("diff_bits")
    eff = d if d is not None else r.get("total_bits", 0)
    order = {"red": 0, "blocked": 1, "green": 2}.get(r.get("status"), 3)
    return (-eff, order, r.get("block", ""))
for r in sorted(recs, key=key):
    print(f"{r.get('design')}/{r.get('part')} {r.get('block')}: {r.get('status')} diff_bits={r.get('diff_bits')} total_bits={r.get('total_bits')}")
PYEOF
