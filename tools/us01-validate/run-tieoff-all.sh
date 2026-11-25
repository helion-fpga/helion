#!/bin/sh
# Run check-tieoff.py on all 14 pairs (decoded vs raw prjuray baseline).
# Logs: ~/workspace/us01-validate-logs/tieoff-P-<id>.json
set -e
cd ~/workspace/helion-ultra-bitgen-us01-validate/tools/us01-char
for p in P-11 P-13 P-14 P-15 P-16 P-17 P-23 P-28 P-40 P-41 P-42 P-43 P-44 P-45; do
  echo "=== $p $(date -u +%H:%M:%S) ==="
  python3 check-tieoff.py ~/workspace/us01-char-logs/decoded/$p.fasm \
    ~/workspace/ultra-bitgen-crew/decoder-validation/baseline/$p.fasm \
    > ~/workspace/us01-validate-logs/tieoff-$p.json 2> ~/workspace/us01-validate-logs/tieoff-$p.err
  python3 -c "import json;d=json.load(open('$HOME/workspace/us01-validate-logs/tieoff-$p.json'));print('$p',d['verdicts'],'problems',d['n_problems'])"
done
echo ALLDONE
