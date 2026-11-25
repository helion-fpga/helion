# pick_uram_columns.tcl -- one URAM288 site per URAM column, for job-5 pass C.
#
# Usage:
#   vivado -mode batch -source pick_uram_columns.tcl -tclargs <fpga_part> <sites_out>
#
# Example:
#   vivado -mode batch -source pick_uram_columns.tcl -tclargs xcvu3p-ffvc1517-2-e out/column_sites.txt
#
# Opens the device in PinPlanning mode (no synthesis, no license
# checkout for implementation), enumerates every URAM288 site, groups
# sites by their X coordinate (one URAM column per X), and writes the
# lowest site in dictionary order for each X, one per line, to
# <sites_out>. gen_uram_column_design.py turns that list into a design
# with one URAM288 per column; run_uram_column.tcl implements it. The
# resulting .ll file maps every column's cell to its frames, which is
# the device-file input for URAM columns (how many columns xcvu3p has
# is not stated in any public source; this run measures it).
#
# No em dashes anywhere.

if { [llength $::argv] != 2 } {
  puts "received mandatory args: $::argv"
  puts "expected mandatory args: <fpga_part> <sites_out>"
  exit 1
}

set fpga_part [lindex $::argv 0]
set sites_out [lindex $::argv 1]

create_project "pick_uram_columns" -in_memory -part ${fpga_part}
set_property DESIGN_MODE PinPlanning [current_fileset]
open_io_design -name io_1

set sites [get_sites -regexp "URAM288_X\\d+Y\\d+"]
if { [llength ${sites}] == 0 } {
  puts "ERROR: no URAM288 sites found on part ${fpga_part}"
  exit 1
}
set sites_sorted [lsort -dictionary ${sites}]
# Group by X: the site name is URAM288_X<n>Y<m>. The lowest site per
# X is the first site seen for that X in dictionary order.
array set seen {}
set picked {}
foreach site ${sites_sorted} {
  regexp {URAM288_X(\d+)Y\d+} ${site} _ x
  if { ![info exists seen(${x})] } {
    set seen(${x}) 1
    lappend picked ${site}
  }
}
puts "picked [llength ${picked}] column sites of [llength ${sites_sorted}] URAM288 sites on ${fpga_part}"

set fp [open ${sites_out} w]
foreach site ${picked} {
  puts ${fp} ${site}
}
close ${fp}
