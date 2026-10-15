#!/bin/bash
# Helion Vivado Parity Benchmark Harness
# Runs Helion synth on benchmark designs and collects QoR metrics.
# For Vivado comparison, see vivado_bench.tcl (to be run by Saksham).

set -e

HELION="./target/debug/helion"
BENCH_DIR="examples"
RESULTS="bench_results_helion.csv"

echo "design,cells,luts,ffs,dsps,brams,runtime_s" > $RESULTS

run_bench() {
    local design=$1
    local file=$2
    echo "Running $design..."
    local start=$(date +%s)
    local output=$($HELION synth "$file" 2>&1 | grep -E "^synth ")
    local end=$(date +%s)
    local runtime=$((end - start))
    
    # Parse: synth <name> cells=<n> luts=<n> ...
    local cells=$(echo "$output" | grep -oP 'cells=\K[0-9]+' || echo "0")
    local luts=$(echo "$output" | grep -oP 'luts=\K[0-9]+' || echo "0")
    local ffs=$(echo "$output" | grep -oP 'ffs=\K[0-9]+' || echo "0")
    local dsps=$(echo "$output" | grep -oP 'dsps=\K[0-9]+' || echo "0")
    local brams=$(echo "$output" | grep -oP 'brams=\K[0-9]+' || echo "0")
    
    echo "$design,$cells,$luts,$ffs,$dsps,$brams,$runtime" >> $RESULTS
    echo "  $design: cells=$cells luts=$luts (${runtime}s)"
}

# Benchmark designs
run_bench "blinky" "$BENCH_DIR/blinky.sv"
run_bench "counter" "$BENCH_DIR/counter.sv"
run_bench "hier" "$BENCH_DIR/hier.sv"
run_bench "complex" "$BENCH_DIR/complex.sv"
run_bench "picorv32" "third_party/picorv32/picorv32.v"
run_bench "serv" "$BENCH_DIR/serv_pin_wrap.sv"

echo ""
echo "Results written to $RESULTS"
cat $RESULTS
