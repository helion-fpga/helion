# CMT/RCLK inventory dump (Job 4 audit dumps, no bitstream needed).
#
#   vivado -mode batch -source dump_clk_inventory.tcl -notrace -tclargs <part> <dir_out>
#
# Writes cmt_tiles.txt (every CMT tile with its BUFGCE/MMCM/PLL
# sites), rclk_tiles.txt (every RCLK tile with its BUFCE sites), and
# cmt_pips.txt (PIP inventory of one CMT tile). These confirm the
# tile/site naming the golden assumes (CMT_L, BUFGCE_X0Y0) and the
# proposed BUFGCE FASM spelling in tests/fixtures/clk_blinky.fasm.
# If a tile coordinate below does not exist on the part, verify with
# get_tiles and use a real one; note the substitution in runs.txt.

proc main { part dir_out } {
  file mkdir $dir_out
  set fp [open [file join $dir_out cmt_tiles.txt] w]
  foreach tile [get_tiles -filter {TILE_TYPE =~ CMT*}] {
    puts $fp [get_property NAME $tile]
    foreach site [get_sites -of_objects $tile] {
      puts $fp "  [get_property NAME $site] [get_property SITE_TYPE $site]"
    }
  }
  close $fp
  set fp [open [file join $dir_out rclk_tiles.txt] w]
  foreach tile [get_tiles -filter {TILE_TYPE =~ RCLK*}] {
    puts $fp [get_property NAME $tile]
    foreach site [get_sites -of_objects $tile] {
      puts $fp "  [get_property NAME $site] [get_property SITE_TYPE $site]"
    }
  }
  close $fp
  # One CMT tile's PIPs. Pick a real CMT tile first if CMT_L_X0Y0
  # does not exist; record the choice in runs.txt.
  set tiles [get_tiles -filter {TILE_TYPE =~ CMT*}]
  if { [llength $tiles] > 0 } {
    set tile [lindex $tiles 0]
    set fp [open [file join $dir_out cmt_pips.txt] w]
    puts $fp "tile: [get_property NAME $tile]"
    foreach pip [get_pips -of_objects $tile] {
      puts $fp [get_property NAME $pip]
    }
    close $fp
  }
}

if { [llength $::argv] != 2 } {
  puts "received mandatory args: $::argv"
  puts "expected mandatory args: <part> <dir_out>"
} else {
  main [lindex $::argv 0] [lindex $::argv 1]
}
