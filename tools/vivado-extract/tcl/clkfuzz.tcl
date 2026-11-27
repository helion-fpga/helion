# Clock-PIP specimens: random legal sites for every clock buffer, the MMCM and
# each clock's flops; Vivado places and routes; record the PIPs and a bitstream.
# Usage: clkfuzz.tcl <outdir> <part> <top> <rtl.v> <xdc> <seed> <count>
# Writes spec_<n>.bit.gz (debug bitstream) and spec_<n>.pips.tsv (net, tile,
# tile type, pip) for every net driven by a clock buffer or the MMCM.
if {[llength $argv] == 0 && [info exists ::env(VX_ARGS)]} { set argv $::env(VX_ARGS) }
lassign $argv out part top rtl xdc seed count
set out [file normalize $out]
file mkdir $out
set log [open $out/clkfuzz.log a]
proc note {msg} { global log; puts $log $msg; flush $log; puts $msg }
expr {srand($seed)}
proc pick {l} { lindex $l [expr {int(rand() * [llength $l])}] }

if {![file exists $out/synth.dcp]} {
    create_project -in_memory -part $part
    read_verilog $rtl
    read_xdc $xdc
    synth_design -top $top -part $part -flatten_hierarchy rebuilt
    write_checkpoint -force $out/synth.dcp
    close_project
}
for {set n 0} {$n < $count} {incr n} {
    set id [format "%d_%03d" $seed $n]
    if {[file exists $out/spec_$id.bit.gz]} continue
    open_checkpoint $out/synth.dcp
    set_property SEVERITY Warning [get_drc_checks -quiet {NSTD-1 UCIO-1 RTSTAT-10 PDRC-153 LUTLP-1 BIVC-1 PLCK-12 PLHOLDVIO-2 REQP-1576}]
    set regions [get_clock_regions]
    set mmcm [get_cells mmcm_i]
    set msite [pick [get_sites -filter {SITE_TYPE == MMCME2_ADV}]]
    set_property LOC $msite $mmcm
    set mreg [get_clock_regions -of [get_sites $msite]]
    # BUFGs: distinct random sites.
    set bufg_sites [get_sites -filter {SITE_TYPE == BUFGCTRL}]
    set used {}
    foreach b [get_cells -hier -filter {REF_NAME == BUFG}] {
        set s [pick $bufg_sites]
        while {[lsearch $used $s] >= 0} { set s [pick $bufg_sites] }
        lappend used $s
        set_property LOC $s $b
    }
    # Each clock's flops go to one random slice (its own) of a random region;
    # buffers that only reach their own region (BUFHCE on the MMCM, BUFR) and
    # their flops take the MMCM's region.
    set taken {}
    set slices_used {}
    foreach buf [get_cells -hier -filter {REF_NAME =~ BUF*}] {
        set bt [get_property REF_NAME $buf]
        set reg [expr {[get_property NAME $buf] in {h_m r_m} ? $mreg : [pick $regions]}]
        if {$bt ne "BUFG"} {
            set cand [get_sites -quiet -of [get_clock_regions $reg] -filter "SITE_TYPE == $bt"]
            foreach t $taken { set cand [lsearch -all -inline -not -exact $cand $t] }
            if {[llength $cand]} { set s [pick $cand]; lappend taken $s; set_property LOC $s $buf }
        }
        set onet [get_nets -of [get_pins -of $buf -filter {DIRECTION == OUT}]]
        set ffs [get_cells -quiet -of [get_pins -quiet -leaf -of $onet -filter {DIRECTION == IN}] -filter {IS_SEQUENTIAL}]
        if {![llength $ffs]} continue
        set sl [pick [get_sites -of [get_clock_regions $reg] -filter {SITE_TYPE == SLICEL || SITE_TYPE == SLICEM}]]
        while {[lsearch $slices_used $sl] >= 0} { set sl [pick [get_sites -of [get_clock_regions $reg] -filter {SITE_TYPE == SLICEL || SITE_TYPE == SLICEM}]] }
        lappend slices_used $sl
        catch {set_property LOC $sl $ffs}
    }
    # Random LOCs break the clock placer's dedicated-route pairing (Place 30-150/30-574).
    set_property CLOCK_DEDICATED_ROUTE FALSE [get_nets -of [get_pins -of [get_cells -hier -filter {REF_NAME =~ BUF* || REF_NAME == MMCME2_ADV || REF_NAME =~ IBUF*}] -filter {DIRECTION == OUT}]]
    if {[catch {place_design; route_design} e]} { note "$id\tFAIL\t$msite\t[string map {\n " "} $e]"; close_design; continue }
    set unrouted [llength [get_nets -quiet -hier -filter {ROUTE_STATUS != ROUTED && ROUTE_STATUS != INTRASITE && ROUTE_STATUS != NOLOADS && TYPE != POWER && TYPE != GROUND}]]
    # 2017.3 on 7-series has no DEBUGBITSTREAM; bitframes.py then maps frames with BITFRAMES_PART.
    if {"BITSTREAM.GENERAL.DEBUGBITSTREAM" in [list_property [current_design]]} {
        set_property BITSTREAM.GENERAL.DEBUGBITSTREAM YES [current_design]
    }
    set_property BITSTREAM.GENERAL.COMPRESS FALSE [current_design]
    set pf [open $out/spec_$id.pips.tsv w]
    foreach drv [get_cells -hier -filter {REF_NAME =~ BUF* || REF_NAME == MMCME2_ADV || REF_NAME =~ IBUF*}] {
        foreach net [get_nets -quiet -of [get_pins -of $drv -filter {DIRECTION == OUT}]] {
            foreach p [get_pips -quiet -of $net] {
                set t [get_tiles -of $p]
                puts $pf "$net\t$t\t[get_property TYPE $t]\t$p"
            }
        }
    }
    close $pf
    if {[catch {write_bitstream -force $out/spec_$id.bit} e]} {
        note "$id\tBIT_FAIL\t$msite\t[string map {\n " "} $e]"
    } else {
        exec gzip -f $out/spec_$id.bit $out/spec_$id.pips.tsv
        note "$id\tOK\t$msite\tunrouted=$unrouted"
    }
    close_design
}
note "clkfuzz done seed $seed"
