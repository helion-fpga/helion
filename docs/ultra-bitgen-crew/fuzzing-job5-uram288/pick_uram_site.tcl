# pick_uram_site.tcl -- resolve one fixed URAM288 site for all job-5 runs.
#
# Usage:
#   vivado -mode batch -source pick_uram_site.tcl -tclargs <fpga_part> <site_out>
#
# Example:
#   vivado -mode batch -source pick_uram_site.tcl -tclargs xcvu3p-ffvc1517-2-e site.txt
#
# Opens the device in PinPlanning mode (no synthesis, no license
# checkout for implementation), enumerates every URAM288 site, and
# writes the lowest site in dictionary order (lowest X, then lowest Y)
# to <site_out>. Every job-5 run places its single URAM at this site, so
# placement is identical across baseline and variants and a .rbt diff
# can only come from the fuzzed attribute (pass A) or the fuzzed
# content (pass B).
#
# xcvu3p has one URAM column per SLR region grouping; the picked site
# is the lowest site on the part, which is in the lowest URAM column.
# If the part changes, re-check that the picked site is a real URAM288
# site with `get_sites <site>` before running the sweep.
#
# No em dashes anywhere.

if { [llength $::argv] != 2 } {
  puts "received mandatory args: $::argv"
  puts "expected mandatory args: <fpga_part> <site_out>"
  exit 1
}

set fpga_part [lindex $::argv 0]
set site_out [lindex $::argv 1]

create_project "pick_uram_site" -in_memory -part ${fpga_part}
set_property DESIGN_MODE PinPlanning [current_fileset]
open_io_design -name io_1

set sites [get_sites -regexp "URAM288_X\\d+Y\\d+"]
if { [llength ${sites}] == 0 } {
  puts "ERROR: no URAM288 sites found on part ${fpga_part}"
  exit 1
}
set sites_sorted [lsort -dictionary ${sites}]
set site [lindex ${sites_sorted} 0]
puts "picked URAM site ${site} of [llength ${sites_sorted}] URAM288 sites on ${fpga_part}"

set fp [open ${site_out} w]
puts ${fp} ${site}
close ${fp}
