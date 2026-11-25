# Where every placed primitive of a checkpoint sits: one row per cell.
#   vivado -mode batch -source placement.tcl -tclargs <design.dcp> <outdir>
# Writes placement.tsv: CELL REF_NAME SITE SITE_TYPE BEL TILE TILE_TYPE COLUMN ROW
# (tile = the site's tile, so a fuzz diff's frame column can be tied to a tile).
set dcp [lindex $argv 0]
set out [lindex $argv 1]
file mkdir $out
open_checkpoint $dcp
set f [open $out/placement.tsv w]
puts $f [join {CELL REF_NAME SITE SITE_TYPE BEL TILE TILE_TYPE COLUMN ROW} "\t"]
foreach c [get_cells -hier -filter {IS_PRIMITIVE && STATUS != UNPLACED}] {
    set s [get_sites -quiet -of_objects $c]
    if {$s eq ""} { continue }
    set t [get_tiles -quiet -of_objects $s]
    puts $f [join [list $c [get_property REF_NAME $c] $s [get_property SITE_TYPE $s] \
        [get_property -quiet BEL $c] $t [get_property TYPE $t] \
        [get_property COLUMN $t] [get_property ROW $t]] "\t"]
}
close $f
