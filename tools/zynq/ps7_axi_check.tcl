# Rung 7 board check: the ARM writes and reads the PL registers of
# examples/zedboard_ps7_axi.sv over M_AXI_GP0, driven by openocd over JTAG.
#
# *** UNTESTED ON HARDWARE. ***  Written without a board. It has only been run
# against ps7_axi_check_mock.tcl (a model of the SLCR, devcfg and the register
# block) under tclsh and under openocd with the dummy adapter. See README.md.
#
#   ZedBoard in JTAG boot mode, then:
#   openFPGALoader -b zedboard out.bit
#   openocd -f board/digilent_zedboard.cfg -f tools/zynq/ps7_axi_check.tcl -c "ps7_axi_check; shutdown"
#
# Sources. "UG585" is the Zynq-7000 SoC Technical Reference Manual, v1.15
# (docs.amd.com), Appendix B register pages "Register (slcr) <NAME>" (address,
# reset value) and "<NAME> Details" (fields), unless another page is named.
# The IO PLL loop settings for M = 30 are from UG585 "PLLs", Table 1 (Table
# 25-6 in the PDF); its body is not in the HTML edition, so the values were
# taken from two transcriptions that agree: the zynq7000-hal crate (pll.rs,
# "TRM Table 25-6") and Xilinx's generated U-Boot ps7_init_gpl.c for the
# ZedBoard (board/xilinx/zynq/zed_hw_platform: 0XF8000118 <- 0x001452C0).
#
# Clocking assumption. PS_CLK on the ZedBoard is 33.333 MHz. The JTAG BootROM
# writes no PLL, clock, level-shifter or PL-reset register (UG585 "Registers
# Modified by the BootROM - Examples", Table 1), so after it the IO PLL is at
# its reset M = 26 (866.7 MHz, from which no integer divisor gives 100 MHz) or
# bypassed (PS_CLK itself), as the PLL strap MIO[6] = BOOT_MODE[4] says (UG585
# "PS PLL Initialization", "Boot Mode Pin Settings" Table 1). This script
# makes the result strap-independent: it takes IO PLL bypass away from the
# strap (PLL_BYPASS_QUAL = 0), sets M = 30 (1000 MHz), and divides by 10 x 1
# for FCLK0 = 100 MHz; then it measures FCLK0 with the PL's COUNTER.

proc ps7_hex {v} { format 0x%08X $v }

proc ps7_rd {addr} { expr {[lindex [read_memory $addr 32 1] 0]} }

proc ps7_wr {addr val {width 32}} { write_memory $addr $width [list $val] }

# Read-modify-write: only the bits in mask change.
proc ps7_rmw {addr mask val} { ps7_wr $addr [expr {([ps7_rd $addr] & ~$mask) | ($val & $mask)}] }

proc ps7_result {ok msg} {
    global ps7_n ps7_bad
    incr ps7_n
    if {$ok} { echo "PASS $msg" } else { incr ps7_bad; echo "FAIL $msg" }
}

# GP0 read compared with the expected value; a bus error is a FAIL.
proc ps7_expect {name addr want} {
    if {[catch {ps7_rd $addr} got]} {
        ps7_result 0 "$name: read [ps7_hex $addr]: $got"
    } else {
        ps7_result [expr {$got == $want}] "$name: [ps7_hex $addr] = [ps7_hex $got], expected [ps7_hex $want]"
    }
}

proc ps7_put {name addr val width} {
    if {[catch {ps7_wr $addr $val $width} err]} { ps7_result 0 "$name: write [ps7_hex $addr]: $err"; return 0 }
    return 1
}

# Returns the number of failed checks.
proc ps7_axi_check {} {
    global ps7_n ps7_bad
    set ps7_n 0
    set ps7_bad 0
    init
    targets zynq.cpu0
    # cortex_a reads and writes memory through the halted CPU. UG585 "APU and
    # OCM State after BootROM": "MMU, Icache, Dcache, L2 cache are all
    # disabled", so each access reaches GP0 once, with its own size.
    halt

    # SLCR_UNLOCK 0xF8000008, UNLOCK_KEY [15:0]: "Write the unlock key, 0xDF0D,
    # to enable writes to the slcr registers" (0xF800_0000 to 0xF800_0B74).
    ps7_wr 0xF8000008 0xDF0D
    # SLCR_LOCKSTA 0xF800000C, LOCK_STATUS [0]: 0 = writable, 1 = locked.
    if {[ps7_rd 0xF800000C] & 1} {
        ps7_result 0 "SLCR still locked after the unlock key"
        return $ps7_bad
    }

    # BOOT_MODE 0xF800025C: PLL_BYPASS [4] (0 = PLLs enabled, 1 = bypassed),
    # BOOT_MODE [3:0] = the sampled boot straps.
    set boot [ps7_rd 0xF800025C]
    echo "BOOT_MODE [ps7_hex $boot]: straps [expr {$boot & 0xF}], PLL strap [expr {($boot >> 4) & 1}] (1 = PLLs bypassed at power-on)"
    echo "before: IO_PLL_CTRL [ps7_hex [ps7_rd 0xF8000108]] IO_PLL_CFG [ps7_hex [ps7_rd 0xF8000118]] PLL_STATUS [ps7_hex [ps7_rd 0xF800010C]] FPGA0_CLK_CTRL [ps7_hex [ps7_rd 0xF8000170]]"

    # IO PLL to M = 30. IO_PLL_CTRL 0xF8000108: PLL_FDIV [18:12] (the PLL
    # must be bypassed and put in reset before it changes); PLL_BYPASS_FORCE
    # [4], PLL_BYPASS_QUAL [3] (0: bypass is FORCE's, 1: the strap's),
    # PLL_PWRDWN [1], PLL_RESET [0].
    ps7_rmw 0xF8000108 0x18 0x10            ;# QUAL = 0, FORCE = 1: bypassed
    ps7_rmw 0xF8000108 0x01 0x01            ;# RESET = 1
    # IO_PLL_CFG 0xF8000118: LOCK_CNT [21:12] = 325, PLL_CP [11:8] = 2,
    # PLL_RES [7:4] = 12, the M = 29..30 row ("the reset default values are
    # not supported", UG585 "PLLs").
    ps7_rmw 0xF8000118 0x003FFFF0 [expr {(325 << 12) | (2 << 8) | (12 << 4)}]
    ps7_rmw 0xF8000108 0x0007F002 [expr {30 << 12}]   ;# FDIV = 30, PWRDWN = 0
    ps7_rmw 0xF8000108 0x01 0x00            ;# RESET = 0
    # PLL_STATUS 0xF800010C: IO_PLL_LOCK [2] (1 = locked), IO_PLL_STABLE [5]
    # (1 = locked or bypassed). Lock takes tLOCK_PSPLL (data sheet).
    set st 0
    for {set k 0} {$k < 100 && !($st & 4)} {incr k} {
        sleep 1
        set st [ps7_rd 0xF800010C]
    }
    if {!($st & 4)} {
        ps7_result 0 "IO PLL did not lock (PLL_STATUS [ps7_hex $st]); it stays bypassed"
        return $ps7_bad
    }
    ps7_rmw 0xF8000108 0x10 0x00            ;# FORCE = 0 (with QUAL = 0): PLL drives its clocks
    set st [ps7_rd 0xF800010C]
    ps7_result [expr {($st & 0x24) == 0x24}] "IO PLL locked at M = 30, not bypassed (PLL_STATUS [ps7_hex $st], IO_PLL_CTRL [ps7_hex [ps7_rd 0xF8000108]])"

    # FPGA0_CLK_CTRL 0xF8000170: DIVISOR1 [25:20] = 1, DIVISOR0 [13:8] = 10,
    # SRCSEL [5:4] = 00 (0x: IO PLL). 33.333 MHz x 30 / 10 / 1 = 100 MHz.
    ps7_rmw 0xF8000170 0x03F03F30 [expr {(1 << 20) | (10 << 8)}]
    echo "FPGA0_CLK_CTRL [ps7_hex [ps7_rd 0xF8000170]]: FCLK0 = 33.333 MHz x 30 / 10 / 1 = 100 MHz if PS_CLK is 33.333 MHz"
    # FPGA0_THR_CTRL 0xF8000174, reset 0: bit 2 "Must be set to 1'b1 to use
    # this feature" (clock throttling); nothing here sets it.
    set thr [ps7_rd 0xF8000174]
    ps7_result [expr {($thr & 4) == 0}] "FCLK0 not throttled (FPGA0_THR_CTRL [ps7_hex $thr])"

    # devcfg INT_STS 0xF800700C (UG585 "Register (devcfg) INT_STS"),
    # PCFG_DONE_INT, 1: "Bitstream programming is complete and PL is in user
    # mode". Bit 2 per Xilinx embeddedsw xdevcfg_hw.h, XDCFG_IXR_PCFG_DONE_MASK
    # 0x4. GP0 to an unconfigured PL would hang the bus: stop here instead.
    set ints [ps7_rd 0xF800700C]
    ps7_result [expr {($ints & 4) != 0}] "PL configured (devcfg INT_STS [ps7_hex $ints], PCFG_DONE_INT = bit 2)"
    if {!($ints & 4)} {
        ps7_wr 0xF8000004 0x767B
        return $ps7_bad
    }

    # LVL_SHFTR_EN 0xF8000900, USER_LVL_SHFTR_EN [3:0]: 0xF enables all level
    # shifters (0xA would be PS-to-PL only; GP0 needs both ways). Bit 4:
    # "Reserved. Do not modify."
    ps7_rmw 0xF8000900 0xF 0xF
    # FPGA_RST_CTRL 0xF8000240: FPGAn_OUT_RST [3:0], 1 = FCLKRESETNn low, 0 =
    # high; bits 24:20, 17:16, 13:8 reset to 1 and say "Write 0". Pulse the
    # PL resets so a second run starts from the RTL's reset values too.
    ps7_wr 0xF8000240 0x0000000F
    sleep 1
    ps7_wr 0xF8000240 0x00000000

    # SLCR_LOCK 0xF8000004, LOCK_KEY [15:0]: "Write the lock key, 0x767B, to
    # write protect the slcr registers."
    ps7_wr 0xF8000004 0x767B
    ps7_result [expr {[ps7_rd 0xF800000C] & 1}] "SLCR locked again"
    sleep 1

    # The register block (examples/zedboard_ps7_axi.sv), GP0 window 0x4000_0000.
    ps7_expect "MAGIC" 0x40000000 0x484C0701
    ps7_expect "SCRATCH after reset" 0x40000004 0
    ps7_expect "LEDS after reset" 0x4000000C 0
    if {[ps7_put "SCRATCH word" 0x40000004 0xDEADBEEF 32]} { ps7_expect "SCRATCH word" 0x40000004 0xDEADBEEF }
    if {[ps7_put "SCRATCH byte 1" 0x40000005 0x77 8]} { ps7_expect "SCRATCH byte 1 (WSTRB 0010)" 0x40000004 0xDEAD77EF }
    if {[ps7_put "SCRATCH half 1" 0x40000006 0x1234 16]} { ps7_expect "SCRATCH halfword 1 (WSTRB 1100)" 0x40000004 0x123477EF }
    if {[ps7_put "SCRATCH byte 0" 0x40000004 0x00 8]} { ps7_expect "SCRATCH byte 0 (WSTRB 0001)" 0x40000004 0x12347700 }
    if {[ps7_put "LEDS word" 0x4000000C 0xFFFFFFD5 32]} { ps7_expect "LEDS keeps bits 6:0" 0x4000000C 0x55 }
    if {[ps7_put "LEDS byte 1" 0x4000000D 0xFF 8]} { ps7_expect "LEDS ignores a byte 1 write (WSTRB 0010)" 0x4000000C 0x55 }
    if {[ps7_put "LEDS byte 0" 0x4000000C 0x2A 8]} { ps7_expect "LEDS byte 0" 0x4000000C 0x2A }
    ps7_put "LEDS pattern" 0x4000000C 0x55 32

    # COUNTER (0x4000_0008) counts FCLK0 cycles: measure FCLK0 over a second.
    if {[catch {
        set t0 [ms]
        set c0 [ps7_rd 0x40000008]
        sleep 1000
        set c1 [ps7_rd 0x40000008]
        set t1 [ms]
    } err]} {
        ps7_result 0 "COUNTER: $err"
    } else {
        set mhz [expr {(($c1 - $c0) & 0xFFFFFFFF) / 1000.0 / ($t1 - $t0)}]
        ps7_result [expr {$mhz > 98.0 && $mhz < 102.0}] "FCLK0 from COUNTER: [format %.2f $mhz] MHz over [expr {$t1 - $t0}] ms (expected 100, +-2% for JTAG latency)"
    }

    echo "ps7_axi_check: [expr {$ps7_n - $ps7_bad}]/$ps7_n PASS (LD0..LD6 = 0x55: LD0, LD2, LD4, LD6 on; LD7 blinks at 0.75 Hz)"
    return $ps7_bad
}
