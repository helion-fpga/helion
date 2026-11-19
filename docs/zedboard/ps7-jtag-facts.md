# PS7 JTAG facts for the ZedBoard rung 7 board script

Independent fact sheet, researched from the documents listed below.
It was NOT derived from any rung 7 script or other script in this repo.
Every fact carries a section or table cite. Where a fact is not in UG585,
that is stated explicitly.

Sources checked 2026-11-19:

- Xilinx UG585 (v1.13) April 2, 2021, Zynq-7000 SoC Technical Reference
  Manual, 1825 pages. Downloaded as ug585-Zynq-7000-TRM.pdf and read as
  extracted text. `pdfinfo` confirms Title "Zynq-7000 SoC Technical
  Reference Manual", 1825 pages, CreationDate Apr 2 2021.
- Digilent ZedBoard Hardware User Guide, Version 1.1, August 1, 2012
  (UG533). Used ONLY for board-level facts UG585 cannot state: the
  33.333 MHz PS oscillator and the JP7-JP11 jumper strapping.
- OpenOCD upstream master, commit 2b93ad7 ("flash/nor: report duration
  of 'flash erase_check'"), checked 2026-11-19, for the board/target
  configs and the cortex_a memory-access code.
- OpenOCD User's Guide, General Commands, section 15.3 Memory access
  commands (openocd.org doc/html/General-Commands.html), for mdw/mww
  semantics.

## 1. SLCR unlock and lock

SLCR base is 0xF800_0000. UG585 Chapter 4, section 4.1 Address Map,
Table 4-1 lists F800_0000 to F800_0BFF as SLCR. Section 4.3 SLCR
Registers points at Table 4-3 for the SLCR register map.

| Register | Absolute address | Key | Field |
| --- | --- | --- | --- |
| SLCR_UNLOCK | 0xF8000008 | 0xDF0D | UNLOCK_KEY, bits 15:0 |
| SLCR_LOCK | 0xF8000004 | 0x767B | LOCK_KEY, bits 15:0 |
| SLCR_LOCKSTA | 0xF800000C | (read only) | LOCK status, reset 0x00000001 (locked) |

Cite: UG585 Appendix B, Register Details, entries "Register (slcr)
SLCR_LOCK" (relative 0x00000004, absolute 0xF8000004, wo, reset
0x00000000; LOCK_KEY "Write the lock key, 0x767B, to write protect
the slcr registers") and "Register (slcr) SLCR_UNLOCK" (relative
0x00000008, absolute 0xF8000008, wo, reset 0x00000000; UNLOCK_KEY
"Write the unlock key, 0xDF0D, to enable writes to the slcr
registers"). Both registers cover "all slcr registers, 0xF800_0000
to 0xF800_0B74". A read of either register returns zero. SLCR_LOCKSTA
reset value 0x00000001 means the SLCR is locked after reset until
the unlock key is written.

Chapter 6, section 6.1.5 BootROM Execution, and Chapter 16, section
16.3.3, both show the usage pattern: write 0xDF0D to SLCR_UNLOCK,
do the SLCR writes, then write 0x767B to SLCR_LOCK.

## 2. FPGA0_CLK_CTRL (FCLK0)

| Item | Value |
| --- | --- |
| Absolute address | 0xF8000170 (relative 0x00000170 in SLCR) |
| Access | rw, 32 bits |
| Reset value | 0x00101800 |

Fields, UG585 Appendix B, "Register (slcr) FPGA0_CLK_CTRL":

| Field | Bits | Reset | Meaning |
| --- | --- | --- | --- |
| DIVISOR1 | 25:20 | 0x1 | Second cascade divider |
| DIVISOR0 | 13:8 | 0x18 (24) | First cascade divider |
| SRCSEL | 5:4 | 0x0 | Clock source select |

SRCSEL encoding (same entry): 0x (0 or 1) means IO PLL, 10 means
Arm PLL, 11 means DDR PLL. So at reset, FCLK0 is sourced from the
IO PLL with divisors 24 and 1.

Cite for the divisor arithmetic: UG585 Chapter 25, section 25.3
System-wide Clock Frequency Examples: "The PLL output frequency
is determined by the frequency of the input clock PS_CLK multiplied
by the PLL feedback divider value (M value). ... For each of the
clocks listed in the lower half of the table, the clock frequency
equals the sourced PLL frequency divided by the divisor value. For
some of the peripheral clocks, such as CAN, Ethernet, and the PL,
there are two cascaded dividers." Section 25.7 PL Clocks, Table
25-10/Figure 25-10, confirms FPGA0_CLK_CTRL carries SRCSEL,
DIVISOR0 (bits 13:8) and DIVISOR1 (bits 25:20) for PL FCLK 0.

Reset-state arithmetic (derived from the two cited register values,
not a separate UG585 statement): FCLK0 = IO_PLL / (24 x 1). With
the IO PLL value from section 3 below (866.658 MHz), FCLK0 at
reset is about 36.11 MHz.

## 3. IO PLL state after the BootROM in JTAG boot mode on a ZedBoard

Conclusion: LOCKED (enabled, not bypassed), at 866.658 MHz
(33.333 MHz x 26), provided the board jumpers are in the JTAG
configuration described below. The chain of reasoning is:

(a) The PLL bypass is a board strap, not a BootROM decision.
UG585 Chapter 6, section 6.2.5 Boot Mode Pin Settings: "Five pins,
BOOT_MODE[4:0], are used to select the boot mode, JTAG chain
config, and if the PLLs are bypassed." Section 6.2.3 Clocks and
PLLs: "If the PLLs are bypassed, the PS_CLK can be toggled as slow
as desired and up to its rated input frequency." Table 6-4 (Boot
Mode MIO Strapping Pins) legend: "Mode for all 3 PLLs: PLL
Enabled 0; PLL Bypassed 1. Hardware waits for PLL to lock, then
executes BootROM."

(b) The JTAG boot-mode row of Table 6-4 fixes BOOT_MODE[2:0] to
0, 0, 0 (MIO[5]=BOOT_MODE[0]=0, MIO[4]=BOOT_MODE[2]=0,
MIO[3]=BOOT_MODE[1]=0). BOOT_MODE[4] (MIO[6]) is a separate
strap choice, not fixed by the JTAG row. Section 6.2.5: "The
sampled values of these pins are written into the slcr.BOOT_MODE
[BOOT_MODE] and [PLL_BYPASS] bit fields."

(c) slcr.BOOT_MODE, Appendix B: PLL_BYPASS bit 4, ro: "0: PLLs
are enabled and their outputs are routed to the clock
generators. 1: PLLs are disabled and bypassed." The register
absolute address is 0xF800025C (relative 0x0000025C).

(d) On the ZedBoard, JP11 (Boot_Mode[4]/MIO[6]) selects this.
UG533 Table 22, Jumper Settings: "JP11 Boot_Mode[4]/MIO[6],
GND - PLL Used, uses PS PLLs. VCC bypasses internal PS PLLs."
Default is GND (PLL used). JTAG mode also needs JP8/JP9/JP10
moved off the default "110 - SD Card" to 000 per the Table 6-4
JTAG row, and JP7 at GND (cascaded JTAG chains). So a ZedBoard
jumpered for JTAG boot has BOOT_MODE[4] = 0, PLLs enabled.

(e) The BootROM does not reprogram the PLLs in JTAG mode.
Chapter 6, section 6.1.4 JTAG Slave Boot: "In JTAG boot mode,
the BootROM code does minimal system configuration and enables
a JTAG interface. Then, the system goes into an idle state
waiting for the DAP controller to restart CPU 0." Section
6.1.5 BootROM Execution: "It reads the boot mode pin
information and, if the boot mode is not JTAG, the BootROM
configures the controller for the selected boot device." In
JTAG mode there is no BootROM header and no register
initialization, so the PLL control registers keep their reset
values. Section 6.1.11 Development Environment: "In JTAG boot
mode, the BootROM disables access to the hard-coded ROM memory,
as usual, and then executes the WFE instruction in the CPU."
Section 6.1.10 Starting Code on CPU 1: "The BootROM puts CPU 1
into the Wait for Event mode."

(f) IO PLL reset frequency. IO_PLL_CTRL, Appendix B: absolute
address 0xF8000108 (relative 0x00000108), rw, reset value
0x0001A008. PLL_FDIV bits 18:12 reset to 0x1A (26).
PLL_BYPASS_QUAL bit 3 resets to 0x1, meaning bypass is
"controlled by the value of the sampled BOOT_MODE pin
strapping resistor PLL_BYPASS", and PLL_BYPASS_FORCE bit 4
resets to 0x0. With the strap at 0 (PLL used), the IO PLL is
enabled. Chapter 25, section 25.3: PLL output = PS_CLK x
feedback divider. ZedBoard PS_CLK is 33.333 MHz (UG533,
section 1 Introduction, "On-board Oscillators: 33.333 MHz
(PS)"). So IO PLL = 33.333 MHz x 26 = 866.658 MHz, locked.
Table 25-3 confirms the formula shape with its own example:
IO PLL feedback divider 30 at 33.33 MHz input gives 1000 MHz.

Caveats, stated plainly:

- UG585 does not state a ZedBoard jumper setting anywhere; the
  jumper half of this conclusion comes from UG533 Table 22, not
  UG585. If JP11 were moved to VCC, the PLLs would be bypassed
  and every PS clock would run at the raw 33.333 MHz PS_CLK
  instead. A script that assumes "PLL used" should read back
  slcr.BOOT_MODE (0xF800025C) bit 4 and fail loudly if it is 1.
- UG585 section 6.2.3 also notes: "The maximum frequency during
  BootROM execution is 500 MHz across all speed grade
  specifications." That note is about BootROM execution clocks
  generally; it does not change the IO PLL lock frequency derived
  above, and UG585 gives no mechanism by which the BootROM
  reprograms IO_PLL_CTRL in JTAG mode.
- The 866.658 MHz value is the reset FDIV (26) applied to the
  board PS_CLK. Any FSBL or user code that writes IO_PLL_CTRL
  changes it; the claim here covers only the state immediately
  after the BootROM in JTAG mode, before any FSBL runs.

## 4. FPGA_RST_CTRL

| Item | Value |
| --- | --- |
| Absolute address | 0xF8000240 (relative 0x00000240 in SLCR) |
| Access | rw, 32 bits |
| Reset value | 0x01F33F0F |

Fields, UG585 Appendix B, "Register (slcr) FPGA_RST_CTRL":

| Field | Bits | Reset | Meaning |
| --- | --- | --- | --- |
| FPGA3_OUT_RST | 3 | 1 | PL Reset 3 (FCLKRESETN3). 0 de-asserts, 1 asserts reset |
| FPGA2_OUT_RST | 2 | 1 | PL Reset 2 (FCLKRESETN2). Same polarity |
| FPGA1_OUT_RST | 1 | 1 | PL Reset 1 (FCLKRESETN1). Same polarity |
| FPGA0_OUT_RST | 0 | 1 | PL Reset 0 (FCLKRESETN0). Same polarity |

All four PL resets are ASSERTED at reset (reset value 1). The
register text notes the FCLKRESETN polarity can be inverted in
the PS7 wrapper, so the board script should treat "write 0 to
de-assert" as the register-level truth, not as a board-level
guarantee.

Reserved-bit trap, same entry: bits 24:20, 17:16, 13:8 reset to 1
but are marked "Reserved - always write with 0". A read-modify-write
that preserves those bits writes 1s back into them; the safe
de-assert write is 0x00000000, not a read-modify-write of the
reset value. Chapter 16, section 16.3.3 (Gigabit Ethernet, EMIO
example) shows exactly this usage: "Write a value of 0 to the
slcr.FPGA_RST_CTRL register", then "Write 0b1 to bits
slcr.FPGA_RST_CTRL[FPGA{0-3}_OUT_RST]", then "Write a value of 0
to the slcr.FPGA_RST_CTRL register" again.

## 5. LVL_SHFTR_EN

| Item | Value |
| --- | --- |
| Absolute address | 0xF8000900 (relative 0x00000900 in SLCR) |
| Access | rw, 32 bits |
| Reset value | 0x00000000 |

Fields, UG585 Appendix B, "Register (slcr) LVL_SHFTR_EN":
USER_LVL_SHFTR_EN, bits 3:0, reset 0x0:

- 0x0 = disable all level shifters
- 0xA = enable PS-to-PL level shifters
- 0xF = enable all level shifters
- All other values = reserved

Bit 4 is reserved ("Do not modify") and bits 31:5 are reserved.

The value that enables both directions is 0xF. Cite, UG585
Chapter 2, section 2.4 PS-PL Voltage Level Shifter Enables,
Example power-up sequence:

1. Power-up the PL; LVL_SHFTR_EN should be 0x0.
2. Enable the PS-to-PL level shifters: write 0x0A.
3. Program the PL.
4. Wait for DONE (devcfg.INT_STS [PCFG_DONE_INT] = 1).
5. Enable the PL-to-PS level shifters: write 0x0F.
6. Begin to use the PS-PL signals.

Power-down sequence (same section): stop using the signals,
write 0x0, power down the PL, leave the register at 0x0 while
the PL is powered down. "The PS does not allow the voltage
level shifters to be enabled until the PL global signals
indicate that it is safe to do so. The PL is fully programmed
when the PL DONE signal is High."

Ordering note for the board script: 0x0A before programming the
PL, 0x0F only after DONE. Writing 0x0F before the PL is
programmed is not the documented sequence. Chapter 16, section
16.3.3 independently corroborates the 0xA step: "Enable the
level shifters for PS user inputs to FPGA. Write a value of
4'b1010 to bit field slcr.LVL_SHFTR_EN[USER_LVL_SHFTR_EN]."

## 6. M_AXI_GP0 address window

| Item | Value |
| --- | --- |
| Window | 0x4000_0000 to 0x7FFF_FFFF (1 GB) |
| Direction | PS master to PL slave |

Cite: UG585 Chapter 4, section 4.1 Address Map, Table 4-1
(comprehensive system level address map): the row
"4000_0000 to 7FFF_FFFF, PL, General Purpose Port #0 to the
PL, M_AXI_GP0". The neighboring rows are 0x8000_0000 to
0xBFFF_FFFF for M_AXI_GP1 and 0x0010_0000 to 0x3FFF_FFFF for
DDR. Section 4.1 also states (near Table 4-1): "The M_AXI_GP
addresses are directly from the PS; they are not remapped on"
the PL side, so the address the PS writes is the address the
PL sees.

Consequence for the board script: a 32-bit access to
0x4000_0000 and up is a PS-to-PL AXI transaction through
M_AXI_GP0. It only reaches PL logic if the PL is programmed,
the PS-PL level shifters are enabled (section 5 above), and
the PL design actually decodes that address. UG585 does not
define what a given PL design decodes; that is design-specific.

## 7. OpenOCD: 32-bit reads and writes at 0x4000_0000 and up with CPU0 halted

What UG585 does NOT cover: UG585 says nothing about OpenOCD,
its board configs, or its mdw/mww commands. Everything in this
section is cited to OpenOCD sources instead, as instructed.

### 7.1 Which config files

The task names `board/digilent_zedboard.cfg`. That was the
historical path, created by OpenOCD commit 0b8db11 (2014-03-21,
"tcl: add Zynq-7000 target and Zedboard board configs"). In
current upstream master (commit 2b93ad7, checked 2026-11-19)
the files were renamed:

- Board config: `tcl/board/digilent/zedboard.cfg` (was
  `tcl/board/digilent_zedboard.cfg`). Content, verified from
  the master checkout:

```
# Digilent Zedboard Rev.C, Rev.D with Xilinx Zynq chip
source [find interface/ftdi/digilent_jtag_smt2.cfg]
reset_config srst_only srst_push_pull
source [find target/xlnx/zynq-7000.cfg]
```

- Target config: `tcl/target/xlnx/zynq-7000.cfg` (was
  `tcl/target/zynq_7000.cfg`). Content, verified from the
  master checkout (abridged to the JTAG/target lines):

```
jtag newtap zynq_pl bs -irlen 6 -ignore-version -ircapture 0x1 -irmask 0x03 \
    -expected-id 0x03727093 ... (all Zynq-7000 PL IDs; 0x03727093 is XC7Z020)
jtag newtap zynq cpu -irlen 4 -ircapture 0x1 -irmask 0xf -expected-id 0x4ba00477
dap create zynq.dap -tap zynq.cpu
target create zynq.cpu0 cortex_a -dap zynq.dap -coreid 0 -dbgbase 0x80090000
target create zynq.cpu1 cortex_a -dap zynq.dap -coreid 1 -dbgbase 0x80092000
target smp zynq.cpu0 zynq.cpu1
adapter speed 1000
zynq.cpu0 configure -event reset-assert-post "cortex_a dbginit"
zynq.cpu1 configure -event reset-assert-post "cortex_a dbginit"
```

- Interface config: `tcl/interface/ftdi/digilent_jtag_smt2.cfg`
  (FTDI driver, USB VID:PID 0x0403:0x6014, the on-board
  USB-JTAG). UG533 section 2.3.3 calls the on-board circuit
  "USB-JTAG Programming using Digilent SMT1-equivalent circuit";
  OpenOCD drives it with the SMT2 config.

Practical consequence: `openocd -f board/digilent_zedboard.cfg`
resolves only on OpenOCD installs that still ship the old
path. On current master the invocation is
`openocd -f board/digilent/zedboard.cfg`. A rung 7 script
should probe for the file or pin the OpenOCD version instead
of assuming one path.

### 7.2 Which AP, and how mdw/mww reach 0x4000_0000

Two separate mechanisms are involved. Do not conflate them.

(a) Debug register access uses the APB-AP. UG585 Chapter 27,
section 27.2 Functional Description: "The Arm DAP is composed
of one debug port (DP) and up to three access ports (APs).
Among the three APs, Zynq-7000 devices only implement APB-AP
as the bus master to access all debug components and AHB-AP
to access system memory space directly." The target config
creates one DAP (`dap create zynq.dap -tap zynq.cpu`) and two
`cortex_a` targets on it. It passes no `-ap-num`, so OpenOCD
discovers the debug AP by type: OpenOCD master
`src/target/cortex_a.c`, `cortex_a_examine_first`: "Search for
the APB-AP - it is needed for access to debug registers"
(`dap_find_get_ap(swjdp, AP_TYPE_APB_AP, ...)`). All debug
register reads/writes go through that APB-AP
(`mem_ap_read_atomic_u32(armv7a->debug_ap, ...)`).

(b) The 32-bit memory access itself does NOT go through the
AHB-AP in this config. It goes through the halted CPU. OpenOCD
master `src/target/cortex_a.c`: `cortex_a_read_memory` calls
`cortex_a_read_cpu_memory`, which injects load opcodes into the
halted core and moves the data out through the debug
communications channel (`cortex_a_read_cpu_memory_slow`:
"Issue a load of the appropriate size to R1", then move R1 to
DTRTX and read it back via the debug AP). The write path is
the mirror image (store opcode injection). The AHB-AP that
UG585 describes exists in the DAP and can access system memory
directly, but the stock zynq-7000.cfg creates no `mem_ap`
target on it, so `mdw`/`mww` never touch it.

So the precise answer to "which AP": for `mdw`/`mww` at
0x4000_0000, the JTAG traffic runs over the DAP's APB-AP
carrying debug-register/DCC transfers, and the actual AXI
transaction to 0x4000_0000 is executed by CPU0 itself while
halted. No AP number is hardcoded anywhere; OpenOCD finds the
APB-AP by ROM-table type scan. UG585 does not number the APs
in its text, so no AP number is claimed here.

Precondition: CPU0 must be halted, because the access is
executed by the core (opcode injection through the debug
state). In JTAG boot mode this is the natural state: the
BootROM leaves CPU0 in WFE (section 3(e) above) and OpenOCD
halts it on connect/reset.

### 7.3 Does mdw need phys?

OpenOCD User's Guide, General Commands, section 15.3 Memory
access commands, for `mdw [phys] addr [count]` and
`mww [phys] addr word [count]`: "When the current target has
an MMU which is present and active, addr is interpreted as a
virtual address. Otherwise, or if the optional phys flag is
specified, addr is interpreted as a physical address."

In the rung 7 state (just after the BootROM in JTAG mode, no
FSBL or OS has run), the MMU is not active: the BootROM does
minimal configuration and parks the CPUs in WFE (section
3(e)), and nothing has enabled the MMU. So plain `mdw
0x40000000` already treats the address as physical, and `phys`
is NOT needed. `mdw phys 0x40000000` is still correct and is
the safe form: it forces physical interpretation even if some
earlier software did enable the MMU. OpenOCD master
`src/target/cortex_a.c` confirms the mechanism:
`cortex_a_read_memory` (no phys) calls
`cortex_a_prep_memaccess(target, false)`, which checks
`cortex_a_mmu` and, if the MMU is enabled, keeps translation
on (`cortex_a_mmu_modify(target, true)`); the phys path calls
`cortex_a_prep_memaccess(target, true)`, which turns
translation off (`cortex_a_mmu_modify(target, false)`). Both
paths then read through the CPU.

One warning: if the MMU ever is active and the script uses
plain `mdw 0x40000000` without `phys`, OpenOCD will translate
0x40000000 through the page tables, which is not the PL window
unless the tables happen to map it 1:1. Prefer `mdw phys` /
`mww phys` in the board script so the behavior does not depend
on MMU state.

## 8. What UG585 does not say (unverified, do not assume)

- ZedBoard jumper positions and the 33.333 MHz PS oscillator:
  not in UG585. Cited to UG533 instead (sections 1 and 4,
  Table 22). The "PLL used" conclusion depends on JP11 staying
  at its GND default; verify on the actual board.
- OpenOCD behavior: not in UG585 at all. Cited to OpenOCD
  master sources and the OpenOCD User's Guide instead.
- What address a particular PL design decodes inside the
  0x4000_0000 to 0x7FFF_FFFF window: design-specific, not in
  UG585. A read of an undecoded M_AXI_GP0 address aborts on
  the AXI bus; the board script must only touch addresses its
  PL design implements.
- The exact AP numbers (AP0 vs AP1) for the AHB-AP and APB-AP:
  UG585 Chapter 27 names the two APs and their roles but does
  not number them in the text read. No AP number is claimed
  in this sheet; OpenOCD finds the APB-AP by type.
- "MMU is off after the BootROM" is an inference from the
  BootROM minimal-configuration text (section 3(e)) plus the
  fact that no software has run to enable it, not a direct
  UG585 sentence. The `phys` recommendation in section 7.3
  makes the script robust to that inference being wrong.

## Appendix: quick reference

| Item | Value |
| --- | --- |
| SLCR_UNLOCK | 0xF8000008, key 0xDF0D |
| SLCR_LOCK | 0xF8000004, key 0x767B |
| SLCR_LOCKSTA | 0xF800000C, reset 0x00000001 (locked) |
| FPGA0_CLK_CTRL | 0xF8000170, reset 0x00101800, SRCSEL IO PLL, DIV0 24, DIV1 1 |
| IO_PLL_CTRL | 0xF8000108, reset 0x0001A008, FDIV 26 |
| IO PLL after BootROM, JTAG mode, JP11 GND | Locked, 33.333 MHz x 26 = 866.658 MHz |
| FCLK0 at reset | 866.658 / 24 = about 36.11 MHz |
| FPGA_RST_CTRL | 0xF8000240, reset 0x01F33F0F, bits 3:0 assert PL resets, write 0 to de-assert |
| LVL_SHFTR_EN | 0xF8000900, reset 0x00000000, 0xA PS-to-PL, 0xF both directions |
| BOOT_MODE | 0xF800025C, bit 4 PLL_BYPASS, 0 enabled, 1 bypassed |
| M_AXI_GP0 | 0x4000_0000 to 0x7FFF_FFFF |
| OpenOCD mdw/mww at 0x4000_0000 | Through halted CPU0 via APB-AP debug/DCC transfers, not the AHB-AP; `phys` not required with MMU off, recommended anyway |
