# run_dsp_variant.tcl -- one Vivado run for one job-2 variant.
#
# Usage:
#   vivado -mode batch -source run_dsp_variant.tcl -tclargs \
#     <fpga_part> <variant_verilog> <dsp_site> <out_dir>
#
# Example:
#   vivado -mode batch -source run_dsp_variant.tcl -tclargs \
#     xcvu3p-ffvc1517-2-e variants/v042/dsp_variant.v DSP48E2_X0Y0 out/v042
#
# Non-project flow, following the standard flow template in
# vivado-fuzzing-instructions.md exactly: read_verilog, synth_design,
# opt_design, place_design, route_design, write_bitstream. The single
# DSP cell is placed at the fixed site from pick_dsp_site.tcl with
# place_cell, so placement is identical for every variant.
#
# Fixes vs the shipped package's only DSP script
# (one_dsp_in_every_dsp_column.tcl):
# - That script writes the bitstream WITHOUT -raw_bitfile and WITHOUT
#   -logic_location_file, so it produces neither the .rbt the diff
#   method needs nor the .ll that maps the DSP to its frames. This
#   driver requests both on every run, per the instructions ("ALWAYS
#   request both on every run").
# - That script disables CRC (BITSTREAM.GENERAL.CRC DISABLE) and
#   compression, diverging from the documented standard flow template,
#   which sets neither. This driver sets neither, so the only difference
#   between two runs is the fuzzed attribute.
# - That script's CATTRIP/PPURQ-1 DRC workaround is for HBM parts
#   (U50 etc.). xcvu3p has no HBM and no CATTRIP port; the workaround
#   is not copied here.
#
# Outputs in <out_dir>: dsp.bit, dsp.rbt, dsp.ll, post_route.dcp,
# run_info.txt (part, site, Vivado version). The driver exits nonzero
# if any of .bit/.rbt/.ll is missing, so run_job2.sh stops instead of
# silently recording a gap.
#
# Batch-mode quirks (UG894, per the instructions): no `exec` is used
# anywhere in this script; all diffing and post-processing happens
# AFTER Vivado exits, in run_job2.sh. The design has no top-level
# ports and no XDC (all DSP inputs are tied inside the module), so
# expect warnings about unconstrained/no-port designs; warnings are
# not failures. Only a missing output file is a failure.
#
# No em dashes anywhere.

if { [llength $::argv] != 4 } {
  puts "received mandatory args: $::argv"
  puts "expected mandatory args: <fpga_part> <variant_verilog> <dsp_site> <out_dir>"
  exit 1
}

set fpga_part [lindex $::argv 0]
set variant_verilog [lindex $::argv 1]
set dsp_site [lindex $::argv 2]
set out_dir [lindex $::argv 3]

file mkdir ${out_dir}

read_verilog ${variant_verilog}
synth_design -top dsp_single -part ${fpga_part}

set dsp_cell [get_cells DSP48E2_inst]
if { [llength ${dsp_cell}] != 1 } {
  puts "ERROR: expected exactly one DSP48E2_inst cell, found [llength ${dsp_cell}]"
  exit 1
}
place_cell ${dsp_cell} ${dsp_site}

opt_design
place_design
route_design
write_checkpoint -force ${out_dir}/post_route.dcp

write_bitstream -force -raw_bitfile -logic_location_file ${out_dir}/dsp.bit

set fp [open ${out_dir}/run_info.txt w]
puts ${fp} "part: ${fpga_part}"
puts ${fp} "variant_verilog: ${variant_verilog}"
puts ${fp} "dsp_site: ${dsp_site}"
puts ${fp} "vivado_version: [version]"
puts ${fp} "placed_cell: ${dsp_cell}"
close ${fp}

foreach suffix {bit rbt ll} {
  set path ${out_dir}/dsp.${suffix}
  if { ![file exists ${path}] } {
    puts "ERROR: missing output ${path}"
    exit 1
  }
}
puts "run complete: ${out_dir}/dsp.bit, dsp.rbt, dsp.ll"
