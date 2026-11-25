# Timing data as TSV from the open, routed design (sourced by reference.tcl, redump.tcl).
#   dump_timing_data <outdir>  -> paths.tsv, cell_arcs.tsv, net_delays.tsv
proc dump_timing_data {out} {
    set f [open $out/paths.tsv w]
    set pk {SLACK REQUIREMENT DATAPATH_DELAY DATAPATH_LOGIC_DELAY DATAPATH_NET_DELAY SKEW UNCERTAINTY \
            CLOCK_PESSIMISM LOGIC_LEVELS STARTPOINT_PIN ENDPOINT_PIN STARTPOINT_CLOCK ENDPOINT_CLOCK}
    puts $f [join $pk \t]
    foreach p [get_timing_paths -delay_type max -max_paths 50 -nworst 1] {
        set row {}
        foreach k $pk { lappend row [clean [get_property -quiet $k $p]] }
        puts $f [join $row \t]
    }
    close $f

    # Cell arcs: one row per distinct (REF_NAME, BEL type, FROM, TO, TYPE, delays).
    set f [open $out/cell_arcs.tsv w]
    set ak {DELAY_SLOW_MAX_RISE DELAY_SLOW_MAX_FALL DELAY_SLOW_MIN_RISE DELAY_SLOW_MIN_FALL \
            DELAY_FAST_MAX_RISE DELAY_FAST_MAX_FALL DELAY_FAST_MIN_RISE DELAY_FAST_MIN_FALL}
    puts $f [join [concat REF_NAME BEL FROM TO TYPE SENSE $ak COUNT] \t]
    array set seen {}
    foreach c [get_cells -hier -filter {IS_PRIMITIVE && STATUS != UNPLACED}] {
        set ref [get_property REF_NAME $c]
        set bel [lindex [split [get_property -quiet BEL $c] .] end]
        foreach a [get_timing_arcs -quiet -of_objects $c] {
            # LUT delay depends on the physical pin (A1..A6), not the logical one: record both.
            set from [lindex [split [get_property FROM_PIN $a] /] end]
            set bp [get_bel_pins -quiet -of_objects [get_pins -quiet [get_property FROM_PIN $a]]]
            if {[string match LUT* $ref] && $bp ne ""} { append from "([lindex [split [lindex $bp 0] /] end])" }
            set row [list $ref $bel $from \
                [lindex [split [get_property TO_PIN $a] /] end] [get_property -quiet TYPE $a] [get_property -quiet SENSE $a]]
            foreach k $ak { lappend row [get_property -quiet $k $a] }
            set key [join $row \t]
            if {[info exists seen($key)]} { incr seen($key) } else { set seen($key) 1 }
        }
    }
    foreach key [lsort [array names seen]] { puts $f "$key\t$seen($key)" }
    close $f

    set f [open $out/net_delays.tsv w]
    puts $f "NET\tDRIVER_PIN\tDRIVER_SITE\tLOAD_PIN\tLOAD_SITE\tSLOW_MAX\tFAST_MIN\tPIPS"
    foreach n [get_nets -hier -filter {ROUTE_STATUS == ROUTED && TYPE == SIGNAL}] {
        set drv [get_pins -quiet -leaf -of_objects $n -filter {DIRECTION == OUT}]
        set dsite [get_sites -quiet -of_objects [get_cells -quiet -of_objects $drv]]
        set npips [llength [get_pips -quiet -of_objects $n]]
        foreach d [get_net_delays -quiet -of_objects $n -to [get_pins -quiet -leaf -of_objects $n -filter {DIRECTION == IN}]] {
            set to [get_property -quiet TO_PIN $d]
            puts $f [join [list $n $drv $dsite $to [get_sites -quiet -of_objects [get_cells -quiet -of_objects [get_pins -quiet $to]]] \
                [get_property -quiet SLOW_MAX $d] [get_property -quiet FAST_MIN $d] $npips] \t]
        }
    }
    close $f
}
proc clean {v} { string map {\t " " \n " "} $v }
