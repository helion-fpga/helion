# PS7 to PL interface timing: what is public

Research date: 2026-11-19. Question: is any PS7 PL-interface timing public
(DS187, UG585, prjxray timings) for M_AXI_GP0 setup and clock-to-out at the
PS7 pins?

## Answer

No. No public document gives a setup time, hold time, or clock-to-out for
M_AXI_GP0, or for any other PS7 AXI port, at the PS7 pins. There is no public
number to use as a PS7-side timing budget for the PS7 to PL boundary, and
none is invented in this file.

The only public AXI number at the PS-PL boundary is a maximum clock
frequency, not a per-signal timing value:

- DS187 v1.20, Table 19, symbol FAXI_MAX, "Maximum AXI interface
  performance": max 250 MHz, no minimum, no per-speed-grade split.

That is a ceiling on the AXI interface clock. It is not a setup time, not a
hold time, and not a clock-to-out, and it says nothing about how much of a
250 MHz period the PS7 consumes or delivers at its pins.

## What DS187 says

Source: Xilinx DS187 (v1.20) June 15, 2017, "Zynq-7000 All Programmable SoC
(Z-7007S, Z-7012S, Z-7014S, Z-7010, Z-7015, and Z-7020): DC and AC Switching
Characteristics", read as the full PDF text (ickey.cn mirror of the Xilinx
document, same version and date).

- Table 19, "PS-PL Interface Performance", PDF page 15, is the only PS-PL
  table in the document. Its rows are maximum frequencies only:
  FEMIOGEMCLK 125 MHz, FEMIOSDCLK 25 MHz, FEMIOSPICLK 25 MHz,
  FEMIOJTAGCLK 20 MHz, FEMIOTRACECLK 125 MHz, FFTMCLK 125 MHz,
  FEMIODMACLK 100 MHz, and FAXI_MAX 250 MHz. No row gives a setup time,
  hold time, or clock-to-out for any PS-PL signal.
- The string "M_AXI_GP0" does not occur anywhere in DS187 (verified by
  in-document text search of the full PDF). The string "FAXI" occurs only
  twice: in Table 19 itself, and in the revision history, which says FAXI_MAX
  was moved and added to Table 19.
- The other PS tables cover different interfaces, not the PS-PL AXI boundary:
  Table 17 CPU clock domains, Table 18 PS DDR clock domains, Table 20
  PS_CLK input requirements (30 to 60 MHz), Table 21 PS PLL, Tables 22-23
  PS reset and power supply timing, Table 24 PCAP (FPCAPCK max 100 MHz).
- DS187 does publish setup and hold style numbers, but only for the DDR
  interfaces (Tables 25-29, for example TCACK command/address output setup
  and TCKCA hold) and for PL fabric and SelectIO in the PL switching
  characteristics section. None of those numbers applies to the PS7 AXI
  pins.
- FAXI_MAX 250 MHz has no speed-grade split in Table 19, so it reads as a
  single limit across grades, including the ZedBoard part XC7Z020CLG484-1
  (speed grade -1). It is still only a clock-frequency ceiling.

Independent confirmation: an AMD support forum thread on Zynq HP/GP slave
bandwidth quotes the same table as "DS187 Table 19 ... the maximum AXI
interface clock is 250 MHz at the PL-PS boundary", and then discusses that
150 MHz figures elsewhere are examples, not limits from that table.

## What UG585 says

Source: Xilinx UG585, "Zynq-7000 AP SoC Technical Reference Manual",
v1.15, docs.amd.com page "AXI Signals", read 2026-11-19.

- UG585 lists the PS-PL AXI signals and their directions only. For
  M_AXI_GP{0,1}: MAXIGP{0,1}ACLK is an input (I) to the PS, MAXIGP{0,1}
  ARESETN is an output (O), address and control signals such as
  MAXIGP{0,1}ARADDR[31:0] and MAXIGP{0,1}AWVALID are outputs (O), and
  return signals such as MAXIGP{0,1}RDATA[31:0], MAXIGP{0,1}ARREADY, and
  MAXIGP{0,1}BVALID are inputs (I). No timing value of any kind appears on
  that page.
- UG585 also states that the PL level shifters must be enabled by the
  LVL_SHFTR_EN register before PL logic communication can occur. That is a
  functional requirement, not a timing number.
- The 150 MHz figure often quoted for the PL side is an example operating
  frequency, not a datasheet timing spec. It appears in UG585-derived
  teaching material (ICTP workshop slides citing UG585, "PL Freq. is
  150 MHz", with a 650 MHz CPU to 150 MHz PL ratio of 4.33) and in AMD forum
  discussion of UG585 Figure 5.1, where 150 MHz is described as an example
  interconnect clock. It is not a setup time or a clock-to-out, and it must
  not be used as one.
- UG585 is a functional and register reference. It does not contain an AC
  switching characteristics section for the PS-PL boundary. DS187 is the
  document that would carry such numbers, and it does not carry them for
  this interface.

## What prjxray / openXC7 says

Source: openXC7/prjxray-db on GitHub, default branch master, commit
0c3504f26f5cf77b6a7605c7bc83ebe1ac67b26f (the same revision the base branch
DB.md cites), read 2026-11-19.

- The repository describes itself as "Project X-Ray Database: XC7 Series"
  and its README states: "This repo contains the bitstream documentation
  database for Xilinx Series 7 devices." Its top-level directories are the
  per-family bitstream databases: artix7, kintex7, spartan7, virtex7, zynq7.
  There is no timing directory and no timing data file at the top level.
- prjxray-db documents which bitstream bits configure which fabric
  features (tilegrid, segbits, pips). That is what nextpnr-xilinx builds its
  chip database from and what fasm2frames / xc7frames2bit consume. It does
  not document PS7 internal delays, and it contains no setup, hold, or
  clock-to-out values for the PS7 AXI pins.
- The open flow's timing model is an uncalibrated estimate, not a measured
  PS7 spec. A 2026 peer-reviewed paper using the open flow states that
  nextpnr-xilinx has a "delay model [that] is derived from the Project X-Ray
  database and has not been calibrated against the vendor tool; the timing
  values are therefore estimates, and hold timing is not analyzed by this
  tool" (MDPI, "Streaming Fixed-Point Datapath for Retinex-Inspired
  Low-Light Image Enhancement", 2026). Those estimates cover fabric paths,
  not the PS7 boundary.
- Timing analysis is the known open gap in the reversed flow. A November
  2026 public note on the openXC7 ecosystem observes that the XC7 bitstreams
  have been fully reversed but "the only major gap is that the timing
  analysis hasn't been fully reversed" (Mastodon, @pervognsen,
  2026-11-12).

## What this means for Helion

- Treat all PS7-side timing at the PL boundary as unknown and closed. Do
  not invent a setup or clock-to-out number for M_AXI_GP0, and do not derive
  one from FAXI_MAX 250 MHz or from the 150 MHz example frequency. Neither
  number bounds per-signal PS7 delay.
- The only public hard limit usable in a design rule is the FAXI_MAX clock
  ceiling of 250 MHz from DS187 Table 19. A ZedBoard M_AXI_GP0 clock must
  stay at or below that, and in practice at the configured FCLK / AXI clock
  the design actually uses (commonly 100 MHz or 150 MHz in example designs).
- Sign-off timing for the PS7 boundary can only come from the vendor flow:
  Vivado timing analysis of an implemented design containing the
  processing_system7 block. For fabric delays, Vivado exposes delay ranges
  through its speed models (AMD forum guidance points at the get_speed_models
  Tcl command for LUT and route delays). That route needs Vivado and is
  outside this research task; no Vivado-derived PS7 number is claimed here.
- If a future measurement or Vivado run produces a PS7 boundary number, it
  should be recorded here with the exact command, Vivado version, part
  (XC7Z020CLG484-1), speed grade (-1), and design it came from. Until then,
  this file's statement stands: none is public.

## Sources

1. Xilinx DS187 (v1.20) June 15, 2017, "Zynq-7000 All Programmable SoC DC
   and AC Switching Characteristics", Table 19 "PS-PL Interface Performance"
   (PDF page 15), FAXI_MAX 250 MHz. Full PDF text read 2026-11-19.
2. Xilinx UG585 v1.15, "Zynq-7000 AP SoC Technical Reference Manual",
   docs.amd.com page "AXI Signals" (signal directions for M_AXI_GP{0,1},
   S_AXI_GP{0,1}, S_AXI_HP{0:3}, S_AXI_ACP; LVL_SHFTR_EN note). No timing
   values on that page. Read 2026-11-19.
3. openXC7/prjxray-db, GitHub, commit
   0c3504f26f5cf77b6a7605c7bc83ebe1ac67b26f, README ("bitstream documentation
   database for Xilinx Series 7 devices") and top-level directory listing.
   Read 2026-11-19.
4. AMD support forum, "What constrains the bandwidth on the Zynq HP/GP
   slave interfaces?", quoting DS187 Table 19 FAXI_MAX 250 MHz and
   discussing 150 MHz as UG585 example values.
5. ICTP workshop slides, "AXI data transfer caracterization in Zynq
   devices" (2018), citing UG585, PL frequency 150 MHz example and
   650 MHz / 150 MHz = 4.33 ratio.
6. MDPI 2026, "Streaming Fixed-Point Datapath for Retinex-Inspired
   Low-Light Image Enhancement", on the nextpnr-xilinx delay model being
   derived from the Project X-Ray database, uncalibrated, estimates only,
   hold timing not analyzed.
7. Mastodon, @pervognsen, 2026-11-12, on timing analysis as the remaining
   gap in the reversed XC7 flow.
