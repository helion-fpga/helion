# Which BITSTREAM.* properties a checkpoint exposes, on get_designs vs current_design.
set dcp [lindex $::env(VX_ARGS) 0]
open_checkpoint $dcp
foreach {label obj} [list get_designs [get_designs] current_design [current_design]] {
    set ps [lsearch -all -inline [list_property $obj] BITSTREAM.*]
    puts "PROBE $label count=[llength $ps] debug=[expr {[lsearch $ps BITSTREAM.GENERAL.DEBUGBITSTREAM] >= 0}] perframecrc=[expr {[lsearch $ps BITSTREAM.GENERAL.PERFRAMECRC] >= 0}]"
    puts "PROBE $label set=[catch {set_property BITSTREAM.GENERAL.DEBUGBITSTREAM YES $obj} e] $e"
}
puts "PROBE part=[get_property PART [get_designs]] all=[lsearch -all -inline [list_property [get_designs]] BITSTREAM.GENERAL.*]"
