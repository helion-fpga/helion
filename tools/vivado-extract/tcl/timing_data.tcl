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
        # Full BEL (SLICEL.A6LUT): the site type is part of the key.
        set bel [get_property -quiet BEL $c]
        # Slice output pin a LUT leaves through (A vs AMUX); none = stays inside the slice.
        set opin ""
        if {[string match LUT* $ref]} {
            set site [get_sites -quiet -of_objects $c]
            foreach sp [get_site_pins -quiet -of_objects [get_nets -quiet -of_objects [get_pins -quiet $c/O]]] {
                if {[string match "$site/*" $sp] && [get_property -quiet DIRECTION $sp] eq "OUT"} {
                    set opin [lindex [split $sp /] end]
                }
            }
        }
        foreach a [get_timing_arcs -quiet -of_objects $c] {
            # LUT delay depends on the physical pin (A1..A6), not the logical one: record both.
            set from [lindex [split [get_property FROM_PIN $a] /] end]
            set bp [get_bel_pins -quiet -of_objects [get_pins -quiet [get_property FROM_PIN $a]]]
            if {[string match LUT* $ref] && $bp ne ""} { append from "([lindex [split [lindex $bp 0] /] end])" }
            set to [lindex [split [get_property TO_PIN $a] /] end]
            if {$opin ne ""} { append to "($opin)" }
            set row [list $ref $bel $from $to [get_property -quiet TYPE $a] [get_property -quiet SENSE $a]]
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

    # One row per LUT arc per cell (not aggregated), with what the output drives,
    # to explain why some LUT arcs cost ~124 ps and others ~300 ps.
    set f [open $out/lut_arcs.tsv w]
    puts $f [join {CELL BEL PIN PHYS_PIN SLOW_MAX_PS OUT_SITE_PIN FANOUT LOAD_REFS IN_DRIVER_SITE_PIN} \t]
    foreach c [get_cells -hier -filter {IS_PRIMITIVE && REF_NAME =~ LUT* && STATUS != UNPLACED}] {
        set site [get_sites -quiet -of_objects $c]
        set n [get_nets -quiet -of_objects [get_pins -quiet $c/O]]
        set opin ""
        foreach sp [get_site_pins -quiet -of_objects $n] {
            if {[string match "$site/*" $sp] && [get_property -quiet DIRECTION $sp] eq "OUT"} {
                set opin [lindex [split $sp /] end]
            }
        }
        set loads [get_pins -quiet -leaf -of_objects $n -filter {DIRECTION == IN}]
        set refs [lsort -unique [get_property -quiet REF_NAME [get_cells -quiet -of_objects $loads]]]
        foreach a [get_timing_arcs -quiet -of_objects $c] {
            set fp [get_property FROM_PIN $a]
            # Slice output pin the input's net leaves its driver through (AQ vs AMUX ...).
            set drv ""
            set in [get_nets -quiet -of_objects [get_pins -quiet $fp]]
            set dc [get_cells -quiet -of_objects [get_pins -quiet -leaf -of_objects $in -filter {DIRECTION == OUT}]]
            set ds [get_sites -quiet -of_objects $dc]
            foreach sp [get_site_pins -quiet -of_objects $in] {
                if {$ds ne "" && [string match "[lindex $ds 0]/*" $sp] && [get_property -quiet DIRECTION $sp] eq "OUT"} {
                    set drv [lindex [split $sp /] end]
                }
            }
            set bp [lindex [split [lindex [get_bel_pins -quiet -of_objects [get_pins -quiet $fp]] 0] /] end]
            set d 0
            foreach k {DELAY_SLOW_MAX_RISE DELAY_SLOW_MAX_FALL} {
                set v [get_property -quiet $k $a]
                if {$v ne "" && $v > $d} { set d $v }
            }
            puts $f [join [list $c [get_property -quiet BEL $c] [lindex [split $fp /] end] $bp \
                [expr {round($d * 1000)}] $opin [llength $loads] [join $refs ,] $drv] \t]
        }
    }
    close $f
}
proc clean {v} { string map {\t " " \n " "} $v }
