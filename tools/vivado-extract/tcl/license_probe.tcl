# For each part, run a 4-bit counter through synth -> route -> write_bitstream
# and record which stage (if any) the license stops.
# Usage: vivado -mode batch -source license_probe.tcl -tclargs <outdir> <part>...
set out [file normalize [lindex $argv 0]]
set parts [lrange $argv 1 end]
file mkdir $out
set rtl $out/probe_top.v
set f [open $rtl w]
puts $f "module probe_top(input clk, output reg \[3:0\] q); always @(posedge clk) q <= q + 1'b1; endmodule"
close $f

set log [open $out/license.tsv w]
puts $log "part\tstage_reached\tseconds\terror"
foreach part $parts {
    set t0 [clock seconds]
    set reached none
    set err ""
    close_project -quiet
    foreach {stage cmd} {
        synth {create_project -in_memory -part $part; read_verilog $rtl
               synth_design -top probe_top -part $part}
        place {opt_design; place_design}
        route {route_design}
        bit   {set_property SEVERITY Warning [get_drc_checks {NSTD-1 UCIO-1}]
               write_bitstream -force $out/$part.bit}
    } {
        if {[catch $cmd msg]} { set err [string map {\n " " \t " "} $msg]; break }
        set reached $stage
    }
    puts $log "$part\t$reached\t[expr {[clock seconds] - $t0}]\t$err"
    flush $log
}
close $log
