# Re-extract timing data from a routed checkpoint (after timing_data.tcl changes).
# Usage: vivado -mode batch -source redump.tcl -tclargs <post_route.dcp> <outdir>
lassign $argv dcp out
open_checkpoint $dcp
source [file join [file dirname [info script]] timing_data.tcl]
dump_timing_data [file normalize $out]
