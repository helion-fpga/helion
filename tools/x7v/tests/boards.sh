#!/bin/bash
# Build and verify a counter and the PicoRV32 SoC for every board (or the
# boards named on the command line), from HDL to .bit with Helion's v2 front
# end and 7-series backend, then x7v verify against the RTL:
#   * examples/board_counter.sv: x7v flow (pins from the board's pools);
#   * examples/zedboard_pico_soc.sv with third_party/picorv32: zedbit, then
#     x7v verify for 20,000 cycles, with resets and with no reset at all
#     (the power-on reset must bring the processor up by itself).
# Needs a release build of zedbit (cargo build --release -p helion-xilinx7
# --example zedbit), iverilog, and the X-Ray database. Exit 0 only if every
# check of every board passed. Work files go to $X7V_BOARDS_WORK (default a
# temporary directory).
set -u
cd "$(dirname "$0")/.."
R=$(cd ../.. && pwd)
ZEDBIT=${X7V_ZEDBIT:-$R/target/release/examples/zedbit}
WORK=${X7V_BOARDS_WORK:-$(mktemp -d)}
# With no arguments, every board whose family is in the database (an
# artix7 + zynq7 database has no spartan7): the others are reported as SKIP.
DB=${HELION_XILINX_DB:-}
if [ -z "$DB" ]; then
  for c in "$HOME/.cache/helion/prjxray-db" "$HOME/workspace/xilinx-dbsrc" "$HOME/Documents/zedboard-helion-debug/work/prjxray-db" "$R/xilinx-dbsrc"; do
    [ -d "$c" ] && { DB=$c; break; }
  done
fi
family() { case $1 in arty-s7-25) echo spartan7;; zedboard) echo zynq7;; *) echo artix7;; esac; }
if [ $# -gt 0 ]; then
  BOARDS=$*
else
  BOARDS=""
  for B in zedboard arty-a7-35 basys3 nexys-a7-100t arty-s7-25; do
    if [ -f "$DB/$(family $B)/mapping/parts.yaml" ]; then BOARDS="$BOARDS $B"; else echo "$B: SKIP (no $(family $B) in the database at ${DB:-<none>})"; fi
  done
fi
fail=0
for B in $BOARDS; do
  W=$WORK/$B; mkdir -p "$W"
  # Counter.
  if HELION_ZED_FRONT=v2 X7V_ZEDBIT=$ZEDBIT python3 -m x7v --board "$B" flow "$R/examples/board_counter.sv" \
       --top board_counter --workdir "$W/counter" --cycles 3000 > "$W/counter.txt" 2>&1; then
    echo "$B counter: PASS"
  else
    echo "$B counter: FAIL ($W/counter.txt)"; fail=1
  fi
  # PicoRV32 SoC.
  python3 - "$B" "$W/pico.xdc" "$R" <<'PY'
import sys
from x7v import cosim, pins
b, out, r = sys.argv[1:]
files = [f"{r}/third_party/picorv32/picorv32.v", f"{r}/examples/zedboard_pico_soc.sv"]
pins.write_xdc(cosim.sv_ports(files, "zedboard_pico_soc"), out, None, b)
PY
  t0=$(date +%s)
  "$ZEDBIT" "$R/third_party/picorv32/picorv32.v,$R/examples/zedboard_pico_soc.sv" "$W/pico.xdc" "$W/pico.bit" \
    zedboard_pico_soc --front=v2 --board="$B" > "$W/pico_build.txt" 2>&1
  if [ ! -f "$W/pico.bit" ]; then
    echo "$B PicoRV32: FAIL build ($W/pico_build.txt)"; fail=1; continue
  fi
  echo "$B PicoRV32: built in $(( $(date +%s) - t0 ))s; $(grep -o 'worst slack [^(]*([^)]*)' "$W/pico_build.txt")"
  for mode in reset no-reset; do
    extra=""; [ $mode = no-reset ] && extra="--no-reset"
    if python3 -m x7v --board "$B" verify "$W/pico.bit" --design "pico_$B" \
         --rtl "$R/third_party/picorv32/picorv32.v" "$R/examples/zedboard_pico_soc.sv" --top zedboard_pico_soc \
         --xdc "$W/pico.xdc" --cycles 20000 --reset-rate 0.0005 $extra --workdir "$W/verify_$mode" > "$W/pico_$mode.txt" 2>&1; then
      echo "$B PicoRV32 ($mode): PASS, $(grep -o 'cosim .*' "$W/pico_$mode.txt" | head -1)"
    else
      echo "$B PicoRV32 ($mode): FAIL ($W/pico_$mode.txt)"; fail=1
    fi
  done
done
exit $fail
