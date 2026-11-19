# run_uram_variant.tcl -- one Vivado run for one job-5 pass-A variant.
#
# Usage:
#   vivado -mode batch -source run_uram_variant.tcl -tclargs \
#     <fpga_part> <variant_verilog> <uram_site> <out_dir>
#
# Example:
#   vivado -mode batch -source run_uram_variant.tcl -tclargs \
#     xcvu3p-ffvc1517-2-e variants/v021/uram_variant.v URAM288_X0Y0 out/v021
#
# Non-project flow, following the standard flow template in
# vivado-fuzzing-instructions.md exactly: read_verilog, synth_design,
# opt_design, place_design, route_design, write_bitstream. The single
# URAM cell is placed at the fixed site from pick_uram_site.tcl with
# place_cell, so placement is identical for every variant.
#
# Fixes vs the shipped package's only DSP script
# (one_dsp_in_every_dsp_column.tcl), which is the same class of script
# this driver is modeled on: that script writes the bitstream WITHOUT
# -raw_bitfile and WITHOUT -logic_location_file, so it produces neither
# the .rbt the diff method needs nor the .ll that maps the cell to its
# frames. This driver requests both on every run, per the instructions
# ("ALWAYS request both on every run"). It sets neither CRC-disable nor
# compression, so the only difference between two runs is the fuzzed
# attribute.
#
# Outputs in <out_dir>: uram.bit, uram.rbt, uram.ll, post_route.dcp,
# run_info.txt (part, site, Vivado version). The driver exits nonzero
# if any of .bit/.rbt/.ll is missing, so run_job5.sh stops instead of
# silently recording a gap. It also exits nonzero if synthesis does not
# produce exactly one URAM288 cell: a variant that maps elsewhere (or
# is optimized away) is a failed run, not a zero diff.
#
# Batch-mode quirks (UG894, per the instructions): no `exec` is used
# anywhere in this script; all diffing and post-processing happens
# AFTER Vivado exits, in run_job5.sh. The design has no top-level
# ports and no XDC (all URAM inputs are tied inside the module), so
# expect warnings about unconstrained/no-port designs; warnings are
# not failures. Only a missing output file is a failure.
#
# No em dashes anywhere.

if { [llength $::argv] != 4 } {
  puts "received mandatory args: $::argv"
  puts "expected mandatory args: <fpga_part> <variant_verilog> <uram_site> <out_dir>"
  exit 1
}

set fpga_part [lindex $::argv 0]
set variant_verilog [lindex $::argv 1]
set uram_site [lindex $::argv 2]
set out_dir [lindex $::argv 3]

file mkdir ${out_dir}

read_verilog ${variant_verilog}
synth_design -top uram_single -part ${fpga_part}

set uram_cells [get_cells -hierarchical -filter {REF_NAME == URAM288}]
if { [llength ${uram_cells}] != 1 } {
  puts "ERROR: expected exactly one URAM288 cell, found [llength ${uram_cells}]"
  exit 1
}
set uram_cell [lindex ${uram_cells} 0]
if { ${uram_cell} ne "URAM288_inst" } {
  puts "ERROR: expected the URAM288 cell to be URAM288_inst, found ${uram_cell}"
  exit 1
}
place_cell ${uram_cell} ${uram_site}

opt_design
place_design
route_design
write_checkpoint -force ${out_dir}/post_route.dcp

write_bitstream -force -raw_bitfile -logic_location_file ${out_dir}/uram.bit

set fp [open ${out_dir}/run_info.txt w]
puts ${fp} "part: ${fpga_part}"
puts ${fp} "variant_verilog: ${variant_verilog}"
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
