# Vivado Parity Benchmark Script
# Run with: vivado -mode batch -source vivado_bench.tcl
# Target: xc7a35tcpg236-1 (Arty A7)
# 
# This script synthesizes the same designs as bench_helion.sh
# and reports LUT/FF/DSP/BRAM counts for comparison.
#
# Saksham: Run this on the USF cluster or your machine with Vivado,
# then paste back the results.

set designs {
    {blinky "examples/blinky.sv"}
    {counter "examples/counter.sv"}
    {hier "examples/hier.sv"}
    {complex "examples/complex.sv"}
    {picorv32 "third_party/picorv32/picorv32.v"}
    {serv "examples/serv_pin_wrap.sv"}
}

set results_file "bench_results_vivado.csv"
set fp [open $results_file w]
puts $fp "design,luts,ffs,dsps,brams"

foreach {design file} $designs {
    puts "Running $design..."
    
    # Create project
    create_project -force bench_$design ./vivado_bench_$design -part xc7a35tcpg236-1
    
    # Add source
    add_files -norecurse $file
    set_property top [lindex [split $design "_"] 0] [current_fileset]
    # Note: May need to adjust top module name per design
    
    # Synthesize
    synth_design -top [get_property top [current_fileset]]
    
    # Get utilization
    set luts [get_property SLICE_LUTS [get_cells -hierarchical -filter {PRIMITIVE_TYPE =~ LUT.*}]]
    # Actually, use report_utilization
    set util [report_utilization -return_string]
    
    # Parse LUTs (simplified - adjust based on actual output)
    puts "  $design: see detailed report"
    
    close_project
}

close $fp
puts "Done. Results in $results_file"
puts ""
puts "NOTE: This is a template. Adjust top module names and parsing"
puts "based on your Vivado version. Paste back the LUT counts for:"
puts "  blinky, counter, hier, complex, picorv32, serv"
