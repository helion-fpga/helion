# run_uram_content_probe.tcl -- one Vivado run for one job-5 pass-B probe.
#
# Usage:
#   vivado -mode batch -source run_uram_content_probe.tcl -tclargs \
#     <fpga_part> <probe_dir> <uram_site> <out_dir>
#
# Example:
#   vivado -mode batch -source run_uram_content_probe.tcl -tclargs \
#     xcvu3p-ffvc1517-2-e probes/p001 URAM288_X0Y0 out/p001
#
# Non-project flow, following the standard flow template in
# vivado-fuzzing-instructions.md: read_verilog, read_mem, synth_design,
# opt_design, place_design, route_design, write_bitstream. The probe
# directory holds uram_probe.v (an xpm_memory_spram with
# MEMORY_PRIMITIVE "ultra" and MEMORY_INIT_FILE "probe.mem") and
# probe.mem (4096 lines of 18 hex digits, line 0 is word 0).
#
# `read_mem` reads the .mem file in non-project mode (the same
# command used in non-project flows that load BRAM init files). If a
# Vivado version does not provide it, this script fails at that line
# and the driver stops: the failure is reported with the log, never
# worked around by dropping the init file.
#
# The single XPM cell is placed at the fixed site from
# pick_uram_site.tcl with place_cell, so placement is identical for
# every probe and a .rbt diff can only come from the fuzzed content.
# The driver exits nonzero if synthesis does not produce exactly one
# URAM288 cell, and if any of .bit/.rbt/.ll is missing.
#
# Outputs in <out_dir>: uram.bit, uram.rbt, uram.ll, post_route.dcp,
# run_info.txt (part, probe dir, site, Vivado version).
#
# No em dashes anywhere.

if { [llength $::argv] != 4 } {
  puts "received mandatory args: $::argv"
  puts "expected mandatory args: <fpga_part> <probe_dir> <uram_site> <out_dir>"
  exit 1
}

set fpga_part [lindex $::argv 0]
set probe_dir [lindex $::argv 1]
set uram_site [lindex $::argv 2]
set out_dir [lindex $::argv 3]

file mkdir ${out_dir}

read_verilog ${probe_dir}/uram_probe.v
read_mem ${probe_dir}/probe.mem
synth_design -top uram_content_probe -part ${fpga_part}

set uram_cells [get_cells -hierarchical -filter {REF_NAME == URAM288}]
if { [llength ${uram_cells}] != 1 } {
  puts "ERROR: expected exactly one URAM288 cell, found [llength ${uram_cells}]"
  exit 1
}
set uram_cell [lindex ${uram_cells} 0]
place_cell ${uram_cell} ${uram_site}

opt_design
place_design
route_design
write_checkpoint -force ${out_dir}/post_route.dcp

write_bitstream -force -raw_bitfile -logic_location_file ${out_dir}/uram.bit

set fp [open ${out_dir}/run_info.txt w]
puts ${fp} "part: ${fpga_part}"
puts ${fp} "probe_dir: ${probe_dir}"
puts ${fp} "uram_site: ${uram_site}"
puts ${fp} "vivado_version: [version]"
puts ${fp} "placed_cell: ${uram_cell}"
close ${fp}

foreach suffix {bit rbt ll} {
  set path ${out_dir}/uram.${suffix}
  if { ![file exists ${path}] } {
    puts "ERROR: missing output ${path}"
    exit 1
  }
}
puts "run complete: ${out_dir}/uram.bit, uram.rbt, uram.ll"
