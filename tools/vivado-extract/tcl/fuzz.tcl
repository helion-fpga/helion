# One-feature-at-a-time bitstream fuzzing on a single routed design.
# Usage: vivado -mode batch -source fuzz.tcl -tclargs <outdir> <part> <top> <rtl.v> <xdc|-> <sweep.tsv>
#
# Implements <rtl.v> once (base.dcp), writes a debug bitstream base.bit, then for
# every sweep row sets ONE property, writes <id>.bit, and restores the property.
# sweep.tsv columns: id  kind(cell|port)  pattern  property  value
# Every .bit is gzipped; diff them locally with py/bitframes.py diff base.bit.gz <id>.bit.gz.
# Also writes base.ll (logic location file: FF/LUTRAM/BRAM bit -> FAR, per site),
# which anchors each site to its frame address.
lassign $argv out part top rtl xdc sweep
set out [file normalize $out]
file mkdir $out
set log [open $out/fuzz.log a]
proc note {msg} { global log; puts $log $msg; flush $log; puts $msg }

if {[file exists $out/base.dcp]} {
    open_checkpoint $out/base.dcp
} else {
    create_project -in_memory -part $part
    read_verilog $rtl
    if {$xdc ne "-"} { read_xdc $xdc }
    synth_design -top $top -part $part -flatten_hierarchy rebuilt
    opt_design
    place_design
    route_design
    write_checkpoint -force $out/base.dcp
    report_utilization -file $out/base_util.rpt
}
# Pins in fuzz designs are left unconstrained on purpose.
set_property SEVERITY Warning [get_drc_checks {NSTD-1 UCIO-1 RTSTAT-10 PDRC-153 LUTLP-1 BIVC-1}]
set_property BITSTREAM.GENERAL.DEBUGBITSTREAM YES [current_design]
set_property BITSTREAM.GENERAL.COMPRESS FALSE [current_design]

proc bit {name} {
    global out
    write_bitstream -force -logic_location_file $out/$name.bit
    exec gzip -f $out/$name.bit $out/$name.ll
}
if {![file exists $out/base.bit.gz]} { bit base }

set f [open $sweep]
while {[gets $f line] >= 0} {
    if {$line eq "" || [string index $line 0] eq "#"} continue
    lassign [split $line \t] id kind pat prop val
    if {[file exists $out/$id.bit.gz]} continue
    set objs [expr {$kind eq "port" ? [get_ports -quiet $pat] : [get_cells -quiet -hier $pat]}]
    if {[llength $objs] == 0} { note "$id\tSKIP\tno $kind matches $pat"; continue }
    # PROP may list several properties (AREG,ACASCREG) with matching VALUEs (0,0)
    # for attributes Vivado only accepts changed together.
    set props [split $prop ,]
    set vals [split $val ,]
    set old {}
    foreach p $props { lappend old [get_property -quiet $p [lindex $objs 0]] }
    set err ""
    foreach p $props v $vals {
        if {[catch {set_property $p $v $objs} e]} { set err $e; break }
    }
    if {$err ne ""} {
        note "$id\tSET_FAIL\t$prop=$val\t[string map {\n " "} $err]"
    } elseif {[catch {bit $id} err]} {
        note "$id\tBIT_FAIL\t$prop=$val\t[string map {\n " "} $err]"
    } else {
        note "$id\tOK\t$prop\t[join $old ,]\t$val"
    }
    foreach p $props o $old { catch {set_property $p $o $objs} }
}
close $f
note "fuzz done"
