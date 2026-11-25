# Dump the device model Vivado's Device view draws, for one part, without a
# design (pin-planning mode: needs no synthesis/implementation license).
# Usage: vivado -mode batch -source device.tcl -tclargs <outdir> <part> [full] [speed]
#
# Writes (TSV, one header row each):
#   part.tsv            every part property
#   grid.tsv            every tile: name type col row x y clock_region sites(name:type,...)
#   clock_regions.tsv   every clock region and its properties
#   iobanks.tsv         every IO bank and its properties
#   package_pins.tsv    every package pin and its properties
#   speed_models.tsv    (with "speed") every speed model and its properties
#   tile_types/<T>.wires.tsv  one tile per type: wire name + properties
#   tile_types/<T>.pips.tsv   one tile per type: pip name + properties (incl. speed model)
#   tile_types/<T>.sites.tsv  site, site type, site pin -> tile wire
#   site_types/<S>.bels.tsv   BEL name, type, pins (name:dir,...)
#   site_types/<S>.pins.tsv   site pin, direction
#   site_types/<S>.sitepips.tsv  site PIPs (routing muxes inside a site)
# With "full": also nodes.tsv, node -> wires for the whole device (big).
# With "speed": also speed_models.tsv, written last: Vivado 2017.3 segfaults in
# get_speed_models on UltraScale+ parts in pin-planning mode, so it is opt-in.
set out [file normalize [lindex $argv 0]]
set part [lindex $argv 1]
set full [expr {"full" in [lrange $argv 2 end]}]
set speed [expr {"speed" in [lrange $argv 2 end]}]
file mkdir $out $out/tile_types $out/site_types

create_project -in_memory -part $part
set_property DESIGN_MODE PinPlanning [current_fileset]
open_io_design -name io_1
puts "device.tcl: opened $part"

proc clean {v} { string map {\t " " \n " "} $v }
# Write objs with every property of their class (first object decides the columns).
proc dump {path objs {skip {}}} {
    set f [open $path w]
    if {[llength $objs] == 0} { close $f; return 0 }
    set keys {}
    foreach k [list_property [lindex $objs 0]] { if {[lsearch $skip $k] < 0} { lappend keys $k } }
    puts $f [join $keys \t]
    foreach o $objs {
        set row {}
        foreach k $keys { lappend row [clean [get_property -quiet $k $o]] }
        puts $f [join $row \t]
    }
    close $f
    return [llength $objs]
}

dump $out/part.tsv [get_parts $part]
puts "clock regions: [dump $out/clock_regions.tsv [get_clock_regions -quiet]]"
puts "io banks:      [dump $out/iobanks.tsv [get_iobanks -quiet]]"
puts "package pins:  [dump $out/package_pins.tsv [get_package_pins -quiet]]"

# Grid: one row per tile, plus the first tile seen of each type.
set f [open $out/grid.tsv w]
puts $f "NAME\tTYPE\tCOLUMN\tROW\tGRID_POINT_X\tGRID_POINT_Y\tINT_TILE_X\tINT_TILE_Y\tCLOCK_REGION\tSITES"
set rep [dict create]
foreach t [get_tiles] {
    set ty [get_property TYPE $t]
    if {![dict exists $rep $ty]} { dict set rep $ty $t }
    set sites {}
    foreach s [get_sites -quiet -of_objects $t] { lappend sites "$s:[get_property SITE_TYPE $s]" }
    set cr [get_clock_regions -quiet -of_objects $t]
    puts $f [join [list $t $ty [get_property -quiet COLUMN $t] [get_property -quiet ROW $t] \
        [get_property -quiet GRID_POINT_X $t] [get_property -quiet GRID_POINT_Y $t] \
        [get_property -quiet INT_TILE_X $t] [get_property -quiet INT_TILE_Y $t] \
        $cr [join $sites ,]] \t]
}
close $f
puts "tiles: [llength [get_tiles]] in [dict size $rep] types"

# Per tile type: wires, pips, sites and their pin wiring.
set site_rep [dict create]
dict for {ty t} $rep {
    dump $out/tile_types/$ty.wires.tsv [get_wires -quiet -of_objects $t]
    set pips [get_pips -quiet -of_objects $t]
    set f [open $out/tile_types/$ty.pips.tsv w]
    puts $f "NAME\tUPHILL\tDOWNHILL\tIS_DIRECTIONAL\tIS_BUFFERED_2_0\tIS_BUFFERED_2_1\tIS_PSEUDO\tIS_TEST_PIP\tIS_EXCLUDED_PIP\tSPEED_INDEX\tSPEED_MODEL"
    foreach p $pips {
        set sm [expr {$speed ? [get_speed_models -quiet -of_objects $p] : ""}]
        puts $f [join [list [get_property NAME $p] \
            [get_wires -quiet -uphill -of_objects $p] [get_wires -quiet -downhill -of_objects $p] \
            [get_property -quiet IS_DIRECTIONAL $p] [get_property -quiet IS_BUFFERED_2_0 $p] \
            [get_property -quiet IS_BUFFERED_2_1 $p] [get_property -quiet IS_PSEUDO $p] \
            [get_property -quiet IS_TEST_PIP $p] [get_property -quiet IS_EXCLUDED_PIP $p] \
            [get_property -quiet SPEED_INDEX $p] [join $sm ,]] \t]
    }
    close $f
    set f [open $out/tile_types/$ty.sites.tsv w]
    puts $f "SITE\tSITE_TYPE\tPIN\tDIRECTION\tTILE_WIRE"
    foreach s [get_sites -quiet -of_objects $t] {
        set st [get_property SITE_TYPE $s]
        if {![dict exists $site_rep $st]} { dict set site_rep $st $s }
        foreach sp [get_site_pins -quiet -of_objects $s] {
            set w [get_wires -quiet -of_objects [get_nodes -quiet -of_objects $sp] -filter "TILE_NAME == $t"]
            puts $f [join [list $s $st [lindex [split $sp /] end] [get_property -quiet DIRECTION $sp] [join $w ,]] \t]
        }
    }
    close $f
}

# Per site type: BELs and their pins, site pins, site PIPs.
dict for {st s} $site_rep {
    set f [open $out/site_types/$st.bels.tsv w]
    puts $f "BEL\tTYPE\tPINS"
    foreach b [get_bels -quiet -of_objects $s] {
        set pins {}
        foreach bp [get_bel_pins -quiet -of_objects $b] { lappend pins "[lindex [split $bp /] end]:[get_property -quiet DIRECTION $bp]" }
        puts $f [join [list [lindex [split $b /] end] [get_property -quiet TYPE $b] [join $pins ,]] \t]
    }
    close $f
    set f [open $out/site_types/$st.pins.tsv w]
    puts $f "PIN\tDIRECTION"
    foreach sp [get_site_pins -quiet -of_objects $s] { puts $f "[lindex [split $sp /] end]\t[get_property -quiet DIRECTION $sp]" }
    close $f
    set f [open $out/site_types/$st.sitepips.tsv w]
    puts $f "SITE_PIP"
    foreach p [get_site_pips -quiet -of_objects $s] { puts $f [lindex [split $p /] end] }
    close $f
}
puts "site types: [dict size $site_rep]"

if {$full} {
    set f [open $out/nodes.tsv w]
    puts $f "NODE\tWIRES"
    foreach n [get_nodes] { puts $f "$n\t[join [get_wires -of_objects $n] ,]" }
    close $f
}
if {$speed} { puts "speed models:  [dump $out/speed_models.tsv [get_speed_models -quiet]]" }
puts "device.tcl: done $part -> $out"
