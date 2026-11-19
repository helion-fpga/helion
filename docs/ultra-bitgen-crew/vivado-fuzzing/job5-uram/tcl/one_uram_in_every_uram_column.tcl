# Helion Job 5, phase 1: one URAM288 in every URAM column (UltraScale+).
#
# Derived from the BitFiltrator one_dsp_in_every_dsp_column.tcl pattern
# (MIT, EPFL VLSC; author Sahand Kashani), adapted for URAM288. The
# structure is the same: PinPlanning project to enumerate sites, then
# RTL synthesis of a replicated primitive (uram.v), explicit placement
# of one instance per column, place, route, bitstream.
#
# To call this script, use the following command:
#
#   vivado -mode batch -source one_uram_in_every_uram_column.tcl -notrace -tclargs <fpga_part> <bitstream_out> <uram_out>
#
# Example:
#
#   vivado -mode batch -source one_uram_in_every_uram_column.tcl -notrace -tclargs xcvu3p-ffvc1517-2-e output.bit urams.txt
#
# The outputs of this script are 5 files:
#   - <bitstream_out>
#   - raw bitstream (same prefix as <bitstream_out> with ".rbt" extension)
#   - logic location file (same prefix as <bitstream_out> with ".ll" extension)
#   - dcp file (same prefix as <bitstream_out> with ".dcp" extension)
#   - <uram_out> file: a text file listing the URAMs instantiated in the design in order (incrementing across clock regions
#                     horizontally, then vertically)
#
# Copy this file and uram.v into the vivado-fuzz-pkg/tcl/ directory
# next to helpers.tcl before running; helpers.tcl is reused verbatim
# from the package and is not duplicated here.
#
# Why bottom-most only (unlike the DSP script, which also takes the
# top-most DSP per column): the DSP script needs both because HBM
# pins take the space of SOME DSPs in the bottom-most DSP column, so
# the bottom-most site alone cannot tell which (SLR, major_row) a DSP
# is in. No such carve-out is documented for URAM columns, so the
# bottom-most URAM288 site per column (the BRAM job's rule) is enough
# to map every URAM column's frames. If the .ll output shows a column
# whose bottom-most URAM maps unexpectedly, re-run with the top-most
# site added and report it in runs.txt.
#
# Why -raw_bitfile and -logic_location_file: the fuzzing instructions
# say ALWAYS request both on every run. The shipped one_dsp script
# requests neither and the shipped one_bram script requests only the
# .ll file; both are package defects, recorded in the Job 5 README.
# This script requests both.

package require struct::list

source [file join [file dirname [info script]] helpers.tcl]

set script_path [file dirname [file normalize [info script]]]

# Enumerates all candidate URAMs that should be used in the design IN ORDER. These URAMs are the bottom-most
# URAMs (URAM288_X<x>Y<y>, where Y is the smallest in its clock region) in every URAM column.
proc get_candidate_urams {} {
  puts "Enumerating URAMs"

  # Empty list we will populate.
  set saved_uram_locs {}

  set uram_site_pattern "URAM288_X(\\\\d+)Y(\\\\d+)"

  # Get the min/max SLR indices.
  lassign [get_slr_index_boundaries] min_slr_idx max_slr_idx

  # Iterate over SLRs in order.
  for {set slr_idx ${min_slr_idx}} {${slr_idx} <= ${max_slr_idx}} {incr slr_idx} {
    set slr [get_slrs -filter "SLR_INDEX == ${slr_idx}"]
    puts "Processing SLR ${slr}"

    # We want to iterate over the clock regions of the SLR in order. One might think an easy way of
    # doing this is to just call `lsort -dictionary [get_clock_regions -of_objects ${slr}]`, but
    # this is false. The command above would iterate over the clock regions in column order
    # {X0Y0, X0Y1, X0Y2, ..., X1Y0, ...}, whereas we want to iterate over the clock regions in row order
    # {X0Y0, X1Y0, X2Y0, ..., X0Y1, ...}.
    # We therefore resort to manual indexing to select the clock region of interest.

    # Get the min/max col/row index for the clock regions ("cr") in the current SLR.
    # Clock regions are the large tiles of the form "X(\\d+)Y(\\d+)".
    lassign [get_slr_clock_region_boundaries ${slr}] min_cr_col_idx max_cr_col_idx min_cr_row_idx max_cr_row_idx

    # Iterate over the clock regions of the SLR in order.
    for {set cr_row_idx ${min_cr_row_idx}} {${cr_row_idx} <= ${max_cr_row_idx}} {incr cr_row_idx} {
      for {set cr_col_idx ${min_cr_col_idx}} {${cr_col_idx} <= ${max_cr_col_idx}} {incr cr_col_idx} {
        set clock_region [get_clock_regions -of_objects ${slr} X${cr_col_idx}Y${cr_row_idx}]
        puts "Processing clock region ${clock_region}"

        set cr_tiles [get_tiles -quiet -of_objects ${clock_region}]

        # Some clock regions could have nothing inside them. This often happens in Zynq devices as
        # some clock regions are taken up by hard processors. These processors are not visible in the
        # floorplan and are not returned as tiles when querying the contents of the clock region.
        if { [llength ${cr_tiles}] > 0 } {
          # Get the min/max col index for the tiles in the clock region.
          lassign [get_clock_region_tile_col_boundaries ${clock_region}] min_tile_col_idx max_tile_col_idx

          # Iterate over the columns in the clock region in order.
          for {set tile_col_idx ${min_tile_col_idx}} {${tile_col_idx} <= ${max_tile_col_idx}} {incr tile_col_idx} {
            # Tiles at the given column.
            # Note that we use "-quiet" as we know some columns contain no tiles (and hence no sites).
            # Vivado emits a warning for every such column it encounters and this gives
            # the impression something is wrong. However, we handle this case explicitly
            # in what follows, so we use "quiet" here to remove the warning.
            set tiles [get_tiles -quiet -of_objects ${clock_region} -filter "COLUMN == ${tile_col_idx}"]

            # Keep sites that correspond to URAMs. We again use -quiet to avoid warnings
            # since we explicitly handle the case where there are no sites below.
            set sites [get_sites -quiet -of_objects ${tiles} -regexp ${uram_site_pattern}]

            if { [llength ${sites}] > 0 } {
              # Select the site with the smallest Y coordinate. We can sort the sites and select
              # the first one using `lsort -dictionary {list}` as all sites have names like "URAM288_X(\\d+)Y(\\d+)"
              # and the X coordinate is identical between all entries (they are in the same column). The
              # entries will therefore be sorted by their Y-value.
              set sites_sorted [lsort -dictionary ${sites}]
              set min_site [lindex ${sites_sorted} 0]
              puts "Processing (SLR, major_row, uram_min) sites (${slr}, ${cr_row_idx}, ${min_site})"

              # Keep track of the URAM. Note that we use a row major that is RELATIVE to the SLR.
              lappend saved_uram_locs [list ${slr} ${cr_row_idx} ${min_site}]
            }; # non-empty sites
          }; # clock region tile column idx
        }; # clock region empty?
      }; # clock region col idx
    }; # clock region row idx
  }; # SLR

  return ${saved_uram_locs}
}

proc main { fpga_part bitstream_out_name uram_out_name } {
  # We create an in-memory project to avoid having something written to disk at the current working directory.
  create_project "one_uram_in_every_uram_column" -in_memory -part ${fpga_part}
  # We create an "I/O Planning Project" so we don't need to run implementation to be able to open the device for inspection.
  set_property DESIGN_MODE PinPlanning [current_fileset]
  # Many commands below only work on an open design.
  open_io_design -name io_1

  # We first need to know how many URAMs to instantiate. This is why we used an "I/O Planning Project" as it
  # allows us to open the device for inspection. The normal "RTL" mode does not let us do this without synthesizing a
  # design first to have a design we can "open"

  # Each entry is a (slr, clock region row index, bottom col URAM).
  # We will only instantiate the bottom URAM (later down when doing `place_cell`).
  set uram_locs [get_candidate_urams]
  set num_urams [llength ${uram_locs}]

  # Write list of URAMs to file.
  set file_out [open ${uram_out_name} w]
  puts ${file_out} [format_json [tcl_to_json ${uram_locs}]]
  close ${file_out}

  puts "Instantiating ${num_urams} URAMs (one per URAM column)"

  set_property DESIGN_MODE RTL [current_fileset]

  # Synthesize a replicated URAM design described in a verilog file. Note that `synth_design` will
  # automatically cause the project to transition to a standard "RTL" kernel again.
  # We also don't care about timing at all in this design, so we disable timing-driven synthesis entirely.
  read_verilog $::script_path/uram.v
  synth_design -top uram -no_timing_driven -generic G_SIZE=${num_urams}

  # Note that this design does not require an XDC file as nothing is connected to FPGA pins.

  # Place all URAMs at the locations we computed before synthesis.
  foreach idx [struct::list iota ${num_urams}] uram_loc_info ${uram_locs} {
    # (SLR, major_row, uram_min)
    set uram_loc [lindex ${uram_loc_info} 2]
    # The name we query here is the name used in the verilog file.
    # The brackets are escaped (\[...\]) on purpose: unescaped [${idx}]
    # inside double quotes is Tcl command substitution, and Vivado Tcl
    # rejects it (tclsh: invalid command name "3"). The shipped DSP/BRAM
    # scripts carry the same latent bug unescaped; see Job 5 README
    # defect 5. Do not remove the backslashes.
    set uram_cell [get_cells "URAM288_gen\[${idx}\].URAM288_inst"]
    # Note that uram_loc is a SITE, not a BEL. However, the site we have chosen can accommodate
    # our cell in only 1 way (one URAM288 BEL per site), so we let Vivado choose this
    # configuration automatically (unlike for SLICEs where a register could be placed in
    # multiple places and we need to specify which one we want explicitly).
    place_cell ${uram_cell} ${uram_loc}
  }

  # # Optimizing the design is not really needed as it is somewhat performed by `synth_design`.
  # opt_design
  place_design
  route_design

  set checkpoint_out_name [replace_file_extension ${bitstream_out_name} "dcp"]
  write_checkpoint -force ${checkpoint_out_name}

  # # Debug
  # start_gui

  # Boards with HBM (like the U50) have a catastrophic trip (CATTRIP) port which
  # is supposed to be explicitly driven. If not driven, a DRC check fails and we
  # cannot generate a bitstream. This CATTRIP port is responsible for shutting
  # down the FPGA's power rails if the HBM reaches a critical temperature to
  # avoid damaging the card. Even an empty bitstream for a HBM-enabled device
  # needs to have the CATTRIP port driven,
  # However, we do not want to use an XDC file in this TCL script to make it
  # work with all possible parts without requiring users to somehow find a XDC
  # file for each one. We therefore disable the DRC. This is safe as we are not
  # programming any device with this bitstream anyways.
  set_property IS_ENABLED 0 [get_drc_checks {PPURQ-1}]

  # Generate the bitstream. COMPRESS off and CRC off match the package
  # pattern (deterministic frames, easy diffing). -raw_bitfile and
  # -logic_location_file are both requested: the .rbt is the diff input
  # and the .ll is the ground truth for which frame/bit each placed
  # URAM maps to.
  set_property BITSTREAM.GENERAL.COMPRESS FALSE     [current_design]
  set_property BITSTREAM.GENERAL.CRC DISABLE        [current_design]
  write_bitstream -force -raw_bitfile -logic_location_file ${bitstream_out_name}
}

# We test the length of $::argv instead of just checking $::argc as $::argc
# contains the count of args BEFORE options parsing. Options parsing removes
# optional args and the length of $::argv is the only way to get an accurate
# count of the mandatory args that are left behind.
if { [llength $::argv] != 3 } {
  puts "received mandatory args: $::argv"
  puts "expected mandatory args: <fpga_part> <bitstream_out> <uram_out>"
} else {
  set fpga_part [lindex $::argv 0]
  set bitstream_out [lindex $::argv 1]
  set uram_out [lindex $::argv 2]
  main ${fpga_part} ${bitstream_out} ${uram_out}
}
