# pick_dsp_site.tcl -- resolve one fixed DSP48E2 site for all job-2 runs.
#
# Usage:
#   vivado -mode batch -source pick_dsp_site.tcl -tclargs <fpga_part> <site_out>
#
# Example:
#   vivado -mode batch -source pick_dsp_site.tcl -tclargs xcvu3p-ffvc1517-2-e site.txt
#
# Opens the device in PinPlanning mode (no synthesis, no license
# checkout for implementation), enumerates every DSP48E2 site, and
# writes the lowest site in dictionary order (lowest X, then lowest Y)
# to <site_out>. Every job-2 run places its single DSP at this site, so
# placement is identical across baseline and variants and a .rbt diff
# can only come from the fuzzed attribute.
#
# xcvu3p has no HBM, so no DSP column is partially taken by HBM pins
# (the case the BitFiltrator one_dsp script guards against with
# bottom/top pairs). One fixed site is enough here; if the part changes
# to an HBM part, re-check that the picked site is a real DSP site with
# `get_sites <site>` before running the sweep.
#
# No em dashes anywhere.

if { [llength $::argv] != 2 } {
  puts "received mandatory args: $::argv"
  puts "expected mandatory args: <fpga_part> <site_out>"
  exit 1
}

set fpga_part [lindex $::argv 0]
set site_out [lindex $::argv 1]

create_project "pick_dsp_site" -in_memory -part ${fpga_part}
set_property DESIGN_MODE PinPlanning [current_fileset]
open_io_design -name io_1

set sites [get_sites -regexp "DSP48E2_X\\d+Y\\d+"]
if { [llength ${sites}] == 0 } {
  puts "ERROR: no DSP48E2 sites found on part ${fpga_part}"
  exit 1
}
set sites_sorted [lsort -dictionary ${sites}]
set site [lindex ${sites_sorted} 0]
puts "picked DSP site ${site} of [llength ${sites_sorted}] DSP48E2 sites on ${fpga_part}"

set fp [open ${site_out} w]
puts ${fp} ${site}
close ${fp}
