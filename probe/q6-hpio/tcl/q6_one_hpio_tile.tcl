# Q6 steps 2 and 3: one HPIO tile base FAR plus the offset ruling.
#
# Purpose: place one IOB in each of the (up to) two HPIO tiles of
# clock-region row 0, generate a bitstream with a logic-location (.ll)
# file, and read the FAR of each placed IOB from the .ll file. If the
# two tiles share a column major, the .ll FARs prove it; if they do
# not, the .ll FARs give both column majors. Comparing the observed
# bit positions against the prjuray-db segbits_hpio_right.db bits
# decides the offset question (Q4.3 item 4): whether the second tile's
# bits equal the first tile's bits shifted by the tile offset
# difference, or equal the segbits bits for the next IOB indices.
#
# Inputs: the XDC and Verilog files produced by q6_gen_io_xdc.py from
# the step 1 inventory. Do not hand-write them: the generator picks
# the real row-0 HPIO tiles and their real package pins.
#
# Run (on the cluster, from the probe directory):
#
#   vivado -mode batch -source tcl/q6_one_hpio_tile.tcl -notrace \
#     -tclargs xcku3p-ffva676-1-e q6-io-probe.xdc q6-io-probe.v q6-io-probe.bit
#
# Outputs: q6-io-probe.bit, q6-io-probe.ll, q6-io-probe.rbt (the .ll
# file is the ground truth; the .rbt is the ASCII bitstream for
# diffing), plus q6-io-probe.dcp. Bring back all four files.
#
# This flow is modeled on BitFiltrator's one_ff_in_every_clb_column.tcl
# (same project setup, same place/route/write_bitstream sequence, same
# COMPRESS FALSE and CRC DISABLE settings, same PPURQ-1 DRC disable for
# the CATTRIP port). The only difference is the design: IBUF/OBUF
# pairs on real package pins instead of a ring register, because IO
# placement is by PACKAGE_PIN, not by place_cell.
#
# License note: xcku3p is a WebPACK part (the BitFiltrator device file
# records license Webpack), so write_bitstream is expected to work
# under the free WebPACK license. If Vivado reports a license error
# instead, STOP, paste the exact error into the run log, and bring
# back nothing else from this step. Do not work around a license
# error, and do not run any license-port connect check.
#
# No em dashes anywhere.
#
# This script is original Helion crew work.

proc q6_main { fpga_part xdc_file verilog_file bitstream_out } {
  create_project "q6_one_hpio_tile" -in_memory -part ${fpga_part}

  read_verilog ${verilog_file}
  read_xdc ${xdc_file}
  synth_design -top q6_io_probe -no_srlextract -no_timing_driven

  place_design
  route_design

  set dcp_out [string map {.bit .dcp} ${bitstream_out}]
  write_checkpoint -force ${dcp_out}

  # Same CATTRIP/PPURQ-1 handling as one_ff_in_every_clb_column.tcl:
  # the probe bitstream is never programmed into a device.
  set_property IS_ENABLED 0 [get_drc_checks {PPURQ-1}]

  set_property BITSTREAM.GENERAL.COMPRESS FALSE [current_design]
  set_property BITSTREAM.GENERAL.CRC DISABLE [current_design]
  write_bitstream -force -raw_bitfile -logic_location_file ${bitstream_out}

  # Fail loud if any documented output is missing: the .rbt is the
  # ASCII bitstream for diffing (step 3) and the .ll is the ground
  # truth for the FAR reading (step 2). Same shape as the DSP
  # branch's run_dsp_variant.tcl.
  set rbt_out [string map {.bit .rbt} ${bitstream_out}]
  set ll_out [string map {.bit .ll} ${bitstream_out}]
  foreach out_file [list ${bitstream_out} ${rbt_out} ${ll_out}] {
    if { ![file exists ${out_file}] } {
      puts "ERROR: missing output ${out_file}"
      exit 1
    }
  }
  puts "Q6 step 2 bitstream written to ${bitstream_out}"
}

if { [llength $::argv] != 4 } {
  puts "received mandatory args: $::argv"
  puts "expected mandatory args: <fpga_part> <xdc_file> <verilog_file> <bitstream_out>"
  exit 1
} else {
  set fpga_part [lindex $::argv 0]
  set xdc_file [lindex $::argv 1]
  set verilog_file [lindex $::argv 2]
  set bitstream_out [lindex $::argv 3]
  q6_main ${fpga_part} ${xdc_file} ${verilog_file} ${bitstream_out}
}
