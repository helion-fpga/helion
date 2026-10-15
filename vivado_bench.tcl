# Vivado Parity Benchmark Script
# Run with: vivado -mode batch -source vivado_bench.tcl
# Target: xc7a35tcpg236-1 (Arty A7)
#
# Synthesizes the same designs as bench_helion.sh and reports
# LUT/FF/DSP/BRAM counts for Helion vs Vivado comparison.

set part "xc7a35tcpg236-1"

# Each entry: {name file top}
set designs {
    {blinky   "blinky.sv"       "blinky"}
    {counter  "counter.sv"      "counter"}
    {hier     "hier.sv"         "hier"}
    {complex  "complex.sv"      "complex"}
    {picorv32 "picorv32.v"      "picorv32"}
    {serv     "serv_pin_wrap.sv" "serv_pin_wrap"}
}

set results_file "bench_results_vivado.csv"
set fp [open $results_file w]
puts $fp "design,luts,ffs,dsps,brams"

foreach entry $designs {
    set name [lindex $entry 0]
    set file [lindex $entry 1]
    set top  [lindex $entry 2]
    puts "=== Running $name (top=$top) ==="

    create_project -force bench_$name ./vivado_bench_$name -part $part
    add_files -norecurse $file
    update_compile_order -fileset sources_1

    synth_design -top $top -part $part

    # Utilization numbers
    set luts  [llength [get_cells -hierarchical -filter {PRIMITIVE_TYPE =~ LUT.*}]]
    set ffs   [llength [get_cells -hierarchical -filter {PRIMITIVE_TYPE =~ FD.*}]]
    set dsps  [llength [get_cells -hierarchical -filter {REF_NAME =~ DSP*}]]
    set brams [llength [get_cells -hierarchical -filter {REF_NAME =~ RAMB*}]]

    puts "$name: luts=$luts ffs=$ffs dsps=$dsps brams=$brams"
    puts $fp "$name,$luts,$ffs,$dsps,$brams"

    close_project
    file delete -force ./vivado_bench_$name
}

close $fp
puts ""
puts "Done. Results in $results_file"
