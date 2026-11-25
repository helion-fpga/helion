# Reference build: Vivado's own synth -> place -> route -> bitstream on one design,
# plus every report Helion cross-checks, plus Vivado's per-arc delays as data.
# Usage: vivado -mode batch -source reference.tcl -tclargs <outdir> <part> <top> <xdc|-> <src>...
#   <src>: .v / .sv / .vhd files, or +define+NAME / +incdir+DIR.
#
# Writes in <outdir>:
#   summary.tsv              key numbers: WNS/TNS/WHS per clock, LUT/FF/BRAM/DSP, runtimes
#   timing_summary.rpt       report_timing_summary (setup + hold, uncertainty, pessimism)
#   timing_paths.rpt         50 worst setup paths, full clock expansion, every pin
#   timing_hold.rpt          20 worst hold paths
#   paths.tsv                the same 50 paths as data: slack, logic/net delay, skew,
#                            uncertainty, pessimism, levels
#   cell_arcs.tsv            every distinct (cell type, BEL, from pin, to pin) timing arc
#                            in the routed design with its slow/fast min/max delays
#   net_delays.tsv           every routed net load delay (slow max), with driver/load
#                            site and BEL pins, for interconnect-model fitting
#   design.sdf               write_sdf (per-cell IOPATH, per-net INTERCONNECT)
#   util.rpt / util_hier.rpt / power.rpt / drc.rpt / methodology.rpt / clock_util.rpt
#   design.bit, post_route.dcp
lassign $argv out part top xdc
set srcs [lrange $argv 4 end]
set out [file normalize $out]
file mkdir $out
set t0 [clock seconds]
proc secs {} { global t0; set t [expr {[clock seconds] - $t0}]; set t0 [clock seconds]; return $t }

create_project -in_memory -part $part
set defs {}
foreach s $srcs {
    switch -glob -- $s {
        +define+* { lappend defs [string range $s 8 end] }
        +incdir+* { set_property include_dirs [concat [get_property include_dirs [current_fileset]] [string range $s 8 end]] [current_fileset] }
        *.vhd - *.vhdl { read_vhdl $s }
        *.sv { read_verilog -sv $s }
        default { read_verilog $s }
    }
}
if {$xdc ne "-"} { foreach x [split $xdc ,] { read_xdc $x } }
set opts {}
foreach d $defs { lappend opts -verilog_define $d }
synth_design -top $top -part $part {*}$opts
set rt(synth) [secs]
opt_design;   set rt(opt) [secs]
place_design; set rt(place) [secs]
route_design; set rt(route) [secs]
write_checkpoint -force $out/post_route.dcp

report_timing_summary -max_paths 10 -report_unconstrained -file $out/timing_summary.rpt
report_timing -delay_type max -max_paths 50 -nworst 1 -path_type full_clock_expanded -input_pins -file $out/timing_paths.rpt
report_timing -delay_type min -max_paths 20 -nworst 1 -path_type full_clock_expanded -input_pins -file $out/timing_hold.rpt
report_utilization -file $out/util.rpt
report_utilization -hierarchical -file $out/util_hier.rpt
report_clock_utilization -file $out/clock_util.rpt
report_power -file $out/power.rpt
report_drc -file $out/drc.rpt
catch {report_methodology -file $out/methodology.rpt}
catch {report_design_analysis -logic_level_distribution -file $out/logic_levels.rpt}
write_sdf -force $out/design.sdf
set rt(reports) [secs]

source [file join [file dirname [info script]] timing_data.tcl]
dump_timing_data $out
set rt(data) [secs]

set_property SEVERITY Warning [get_drc_checks {NSTD-1 UCIO-1}]
write_bitstream -force $out/design.bit
set rt(bitstream) [secs]

set f [open $out/summary.tsv w]
puts $f "key\tvalue"
foreach c [get_clocks] {
    set p [lindex [get_timing_paths -quiet -max_paths 1 -group $c -delay_type max] 0]
    set h [lindex [get_timing_paths -quiet -max_paths 1 -group $c -delay_type min] 0]
    puts $f "clock.$c.period_ns\t[get_property PERIOD $c]"
    puts $f "clock.$c.wns_ns\t[get_property -quiet SLACK $p]"
    puts $f "clock.$c.whs_ns\t[get_property -quiet SLACK $h]"
}
set p [lindex [get_timing_paths -max_paths 1 -delay_type max] 0]
puts $f "wns_all_ns\t[get_property -quiet SLACK $p]"
foreach {k ref} {lut LUT ff FD carry CARRY ramb18 RAMB18 ramb36 RAMB36 dsp DSP} {
    puts $f "cells.$k\t[llength [get_cells -hier -quiet -filter "IS_PRIMITIVE && REF_NAME =~ ${ref}*"]]"
}
foreach k [lsort [array names rt]] { puts $f "runtime_s.$k\t$rt($k)" }
puts $f "vivado\t[lindex [split [version] \n] 0]"
puts $f "part\t$part"
close $f
puts "reference.tcl: done -> $out"
