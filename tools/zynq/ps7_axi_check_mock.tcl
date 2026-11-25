# Dry run of ps7_axi_check.tcl with no hardware: openocd's init, targets, halt,
# read_memory, write_memory, sleep and ms are replaced by a model of the SLCR
# registers the script touches, devcfg INT_STS and the register block of
# examples/zedboard_ps7_axi.sv. It checks the script's logic and its order of
# writes, not the board.
#
#   tclsh tools/zynq/ps7_axi_check_mock.tcl
#   openocd -c "adapter driver dummy" -f target/zynq_7000.cfg \
#     -f tools/zynq/ps7_axi_check.tcl -f tools/zynq/ps7_axi_check_mock.tcl -c shutdown
#
# The model: SLCR writes are ignored while locked; the IO PLL locks only with
# the loop settings for its M and with RESET and PWRDWN low, and FDIV may only
# change while it is bypassed and in reset; FCLK0 = (bypassed ? PS_CLK : PS_CLK
# x M) / DIVISOR0 / DIVISOR1 drives COUNTER; a GP0 access before DONE, the
# level shifters and the reset release is an error (on the board it hangs).

if {[info commands ps7_axi_check] eq ""} {
    source [file join [file dirname [info script]] ps7_axi_check.tcl]
}
if {[info commands echo] eq ""} {
    proc echo {msg} { puts $msg }
}

set PS_CLK 33333333.0
# UG585 "PLLs" Table 1 rows used here (M -> IO_PLL_CFG [21:4]), see ps7_axi_check.tcl.
array set pll_cfg {26 0x001772C0 30 0x001452C0}

proc mock_power_on {strap} {
    global m
    array unset m
    array set m [list t 0 locked 1 cnt 0.0 errors {} \
        0xF8000108 0x0001A008 0xF800010C 0x3F 0xF8000118 0x00177EA0 \
        0xF8000170 0x00101800 0xF8000174 0 0xF8000240 0x01F33F0F \
        0xF800025C [expr {$strap << 4}] 0xF8000900 0 0xF800700C 0x4 \
        scratch 0 leds 0 pll_lock 1 strb_bug 0]
}

proc mock_error {msg} {
    global m
    lappend m(errors) $msg
    error $msg
}

proc mock_bypassed {} {
    global m
    set c $m(0xF8000108)
    if {$c & 8} { return [expr {($m(0xF800025C) >> 4) & 1}] }
    expr {($c >> 4) & 1}
}

proc mock_fclk {} {
    global m PS_CLK
    set c $m(0xF8000170)
    set src [expr {[mock_bypassed] ? $PS_CLK : ($m(pll_lock) ? $PS_CLK * (($m(0xF8000108) >> 12) & 0x7F) : 0.0)}]
    expr {$src / (($c >> 8) & 0x3F) / (($c >> 20) & 0x3F)}
}

proc mock_tick {ms} {
    global m
    set m(cnt) [expr {$m(cnt) + [mock_fclk] * $ms / 1000.0}]
    incr m(t) $ms
}

proc mock_pll_update {} {
    global m pll_cfg
    set c $m(0xF8000108)
    set fdiv [expr {($c >> 12) & 0x7F}]
    set m(pll_lock) [expr {!($c & 3) && [info exists pll_cfg($fdiv)] && ($m(0xF8000118) & 0x003FFFF0) == $pll_cfg($fdiv)}]
    set m(0xF800010C) [expr {($m(pll_lock) ? 4 : 0) | ($m(pll_lock) || [mock_bypassed] ? 0x20 : 0) | 0x1B}]
}

proc mock_gp0_ready {} {
    global m
    if {!($m(0xF800700C) & 4) || ($m(0xF8000900) & 0xF) != 0xF || ($m(0xF8000240) & 1)} {
        mock_error "GP0 access with the PL not ready (DONE, level shifters, FCLKRESETN0): would hang"
    }
}

proc read_memory {addr width count args} {
    global m
    mock_tick 1
    set a [format 0x%08X $addr]
    if {$width != 32 || $count != 1} { mock_error "read $a: width $width count $count" }
    if {$a eq "0x40000008"} { mock_gp0_ready; return [list [format 0x%08x [expr {wide($m(cnt)) & 0xFFFFFFFF}]]] }
    if {($addr & 0xFFFFFFF0) == 0x40000000} {
        mock_gp0_ready
        switch -- $a {
            0x40000000 { return [list 0x484c0701] }
            0x40000004 { return [list [format 0x%08x $m(scratch)]] }
            0x4000000C { return [list [format 0x%08x $m(leds)]] }
        }
        mock_error "read $a: SLVERR"
    }
    if {$a eq "0xF800000C"} { return [list $m(locked)] }
    if {![info exists m($a)]} { mock_error "read $a: not modelled" }
    return [list [format 0x%08x $m($a)]]
}

proc write_memory {addr width data args} {
    global m
    mock_tick 1
    set a [format 0x%08X $addr]
    set v [expr {[lindex $data 0]}]
    if {($addr & 0xFFFFFFF0) == 0x40000000} {
        mock_gp0_ready
        set lane [expr {$addr & 3}]
        set strb [expr {((1 << ($width / 8)) - 1) << $lane}]
        if {$lane % ($width / 8)} { mock_error "write $a: unaligned $width-bit" }
        set wdata [expr {$v << (8 * $lane)}]
        switch -- [expr {$addr & ~3}] {
            1073741828 {
                for {set k 0} {$k < 4} {incr k} {
                    if {($strb >> $k) & 1 || $m(strb_bug)} {
                        set mask [expr {0xFF << (8 * $k)}]
                        set m(scratch) [expr {($m(scratch) & ~$mask) | ($wdata & $mask)}]
                    }
                }
            }
            1073741836 { if {$strb & 1} { set m(leds) [expr {$wdata & 0x7F}] } }
            default { mock_error "write $a: SLVERR" }
        }
        return
    }
    if {$width != 32} { mock_error "write $a: width $width" }
    switch -- $a {
        0xF8000008 { if {$v == 0xDF0D} { set m(locked) 0 }; return }
        0xF8000004 { if {$v == 0x767B} { set m(locked) 1 }; return }
    }
    if {![info exists m($a)]} { mock_error "write $a: not modelled" }
    if {$m(locked)} { return }
    if {$a eq "0xF8000108" && (($v ^ $m($a)) & 0x7F000) && !([mock_bypassed] && ($m($a) & 1))} {
        mock_error "IO_PLL_CTRL: FDIV changed while the PLL is not bypassed and in reset"
    }
    if {$a eq "0xF8000900" && (($v ^ $m($a)) & 0x10)} { mock_error "LVL_SHFTR_EN: reserved bit 4 modified" }
    if {$a eq "0xF8000240" && ($v & 0x01F33F00)} { mock_error "FPGA_RST_CTRL: a 'Write 0' bit written 1" }
    if {$a eq "0xF8000240" && ($v & 1)} { set m(scratch) 0; set m(leds) 0 }
    set m($a) $v
    mock_pll_update
}

proc init {} {}
proc targets {args} {}
proc halt {} {}
proc sleep {ms args} { mock_tick $ms }
proc ms {} { global m; return $m(t) }

# name, PLL strap, setup, expected failed checks (-1: any), expected FCLK0 MHz.
set cases {
    {strap_pll_enabled 0 {} 0 100}
    {strap_pll_bypassed 1 {} 0 100}
    {second_run 0 {set m(scratch) 0x5A5A5A5A; set m(leds) 0x11; set m(0xF8000240) 0} 0 100}
    {scratch_ignores_wstrb 0 {set m(strb_bug) 1} 3 100}
    {ps_clk_50mhz 0 {set PS_CLK 50000000.0} 1 150}
    {pl_not_configured 0 {set m(0xF800700C) 0} 1 100}
}
set bad 0
foreach c $cases {
    lassign $c name strap setup want_fail want_mhz
    set PS_CLK 33333333.0
    mock_power_on $strap
    eval $setup
    echo "--- $name"
    set fails [ps7_axi_check]
    set mhz [expr {[mock_fclk] / 1e6}]
    set ok [expr {$fails == $want_fail && abs($mhz - $want_mhz) < 0.01 && $m(errors) eq {} && $m(locked) == 1}]
    if {$ok && $want_fail == 0} {
        set ok [expr {$m(leds) == 0x55 && $m(scratch) == 0x12347700}]
    }
    echo "=== $name: [expr {$ok ? {as expected} : {UNEXPECTED}}] ($fails failed checks, FCLK0 [format %.3f $mhz] MHz, model errors: [llength $m(errors)] $m(errors))"
    if {!$ok} { incr bad }
}
echo "ps7_axi_check_mock: [expr {[llength $cases] - $bad}]/[llength $cases] as expected"
if {$bad} {
    if {[info commands shutdown] ne ""} { shutdown error } else { exit 1 }
}
