# Inventory of what this Vivado install offers: every part, every board part.
# Usage: vivado -mode batch -source inventory.tcl -tclargs <outdir>
# Writes parts.tsv, boards.tsv and version.txt. Needs no license.
set out [file normalize [lindex $argv 0]]
file mkdir $out

set f [open $out/version.txt w]
puts $f [version]
close $f

set props {FAMILY ARCHITECTURE DEVICE PACKAGE SPEED TEMPERATURE_GRADE \
    LUT_ELEMENTS FLIPFLOPS BLOCK_RAMS ULTRA_RAMS DSP IO_PIN_COUNT MMCM GB_TRANSCEIVERS}
set f [open $out/parts.tsv w]
puts $f [join [concat NAME $props] \t]
foreach p [get_parts] {
    set row [list $p]
    foreach k $props { lappend row [get_property -quiet $k $p] }
    puts $f [join $row \t]
}
close $f

set f [open $out/boards.tsv w]
puts $f "NAME\tDISPLAY_NAME\tPART_NAME\tVENDOR\tFILE_VERSION"
foreach b [get_board_parts -quiet] {
    set row [list $b]
    foreach k {DISPLAY_NAME PART_NAME VENDOR FILE_VERSION} { lappend row [get_property -quiet $k $b] }
    puts $f [join $row \t]
}
close $f
puts "inventory: [llength [get_parts]] parts, [llength [get_board_parts -quiet]] board parts -> $out"
