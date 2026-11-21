#!/bin/bash
# decode-all.sh - US-01 characterization (W-CHAR)
# Decodes all 14 xczu3eg pairs with the US-09 decoder (helion-bitgen
# decode on branch fm/ultra-bitgen-us01-char) and records per-pair
# line counts, exit codes, and unexplained-bit counts.
# Inputs are the .bit files already on disk in decoder-validation/bits
# (md5-verified against the corpus manifest before this run; see
# ~/workspace/us01-char-logs/bits-md5.txt). No downloads.
# Usage: bash tools/us01-char/decode-all.sh
set -u
BIN="$HOME/workspace/helion-ultra-bitgen-us01-char/target/release/helion-bitgen"
DB="$HOME/workspace/prjuray-db-ref/zynqusp"
BITS="$HOME/workspace/ultra-bitgen-crew/decoder-validation/bits"
OUT="$HOME/workspace/us01-char-logs/decoded"
LOG="$HOME/workspace/us01-char-logs"
mkdir -p "$OUT"
total_unexpl=0
total_lines=0
for bit in "$BITS"/P-*.bit; do
  id=$(basename "$bit" | cut -d_ -f1)
  "$BIN" decode --db-root "$DB" --part xczu3eg-sbva484-1-e --bit "$bit" \
    > "$OUT/$id.fasm" 2> "$LOG/decode-$id.log"
  rc=$?
  lines=$(wc -l < "$OUT/$id.fasm")
  unexpl=$(grep -c '^unexplained: FAR' "$LOG/decode-$id.log" || true)
  # NOTE: do NOT count '^unexplained:' bare: the log ends with a summary
  # line 'unexplained: N set bits no feature explains' which is not a
  # bit (see decoder-validation/UNEXPLAINED_ATTRIBUTION.md line 38).
  echo "$id rc=$rc lines=$lines unexplained=$unexpl" | tee -a "$LOG/decode-summary.txt"
  total_unexpl=$((total_unexpl + unexpl))
  total_lines=$((total_lines + lines))
done
echo "TOTAL lines=$total_lines unexplained=$total_unexpl" | tee -a "$LOG/decode-summary.txt"
