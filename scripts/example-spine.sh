#!/usr/bin/env bash
# W-L5 example spine — one-command headless labs.
# Usage: scripts/example-spine.sh [counter|blinky|uart|scratch_mm|hcore|all]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if [[ -n "${HELION:-}" ]]; then
  H=("$HELION")
else
  H=(cargo run -q -p helion-cli --)
fi

lab="${1:-all}"

run_counter() {
  echo "== counter (empty-XDC gold) =="
  "${H[@]}" report_timing examples/counter.sv
}

run_blinky() {
  echo "== blinky (user create_clock + PACKAGE_PIN) =="
  "${H[@]}" project examples/blinky.prj
}

run_uart() {
  echo "== uart =="
  "${H[@]}" project examples/uart/uart.prj
}

run_scratch() {
  echo "== scratch_mm =="
  "${H[@]}" project examples/scratch_mm/scratch_mm.prj
}

run_hcore() {
  echo "== hcore =="
  "${H[@]}" project examples/hcore/hcore.prj
}

case "$lab" in
  counter) run_counter ;;
  blinky) run_blinky ;;
  uart) run_uart ;;
  scratch_mm) run_scratch ;;
  hcore) run_hcore ;;
  all)
    run_counter
    run_blinky
    run_uart
    run_scratch
    run_hcore
    ;;
  *)
    echo "usage: $0 [counter|blinky|uart|scratch_mm|hcore|all]" >&2
    exit 2
    ;;
esac
