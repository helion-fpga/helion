#!/bin/sh
# Run consistency.py on all 14 pairs; carry-live.py on all 14 pairs.
# Logs: ~/workspace/us01-validate-logs/consistency-P-<id>.json, carrylive-P-<id>.json
set -e
cd ~/workspace/helion-ultra-bitgen-us01-validate/tools/us01-validate
for p in P-11 P-13 P-14 P-15 P-16 P-17 P-23 P-28 P-40 P-41 P-42 P-43 P-44 P-45; do
  echo "=== consistency $p $(date -u +%H:%M:%S) ==="
  python3 consistency.py ~/workspace/us01-char-logs/decoded/$p.fasm \
    ~/workspace/us01-validate-logs/consistency-$p.json 2> ~/workspace/us01-validate-logs/consistency-$p.err
done
cd ~/workspace/helion-ultra-bitgen-us01-validate/tools/us01-char
for p in P-11 P-13 P-14 P-15 P-16 P-17 P-23 P-28 P-40 P-41 P-42 P-43 P-44 P-45; do
  echo "=== carry-live $p $(date -u +%H:%M:%S) ==="
  python3 carry-live.py ~/workspace/us01-char-logs/decoded/$p.fasm \
    > ~/workspace/us01-validate-logs/carrylive-$p.json 2> ~/workspace/us01-validate-logs/carrylive-$p.err
  cat ~/workspace/us01-validate-logs/carrylive-$p.json
done
echo ALLDONE
