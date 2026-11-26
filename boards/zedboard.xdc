# Avnet/Digilent ZedBoard (xc7z020clg484-1): pins for Helion's 7-series flow.
# Source: Digilent ZedBoard master XDC (digilent-xdc, Zedboard-Master.xdc); every pin checked against the Project X-Ray
# package pin table for xc7z020clg484-1 (crates/helion-xilinx7/tests/boards.rs).
# The comment after each line is the board's own label for that pin (silkscreen or schematic net).
# Switches and buttons are on the Vadj banks (34, 35): LVCMOS18 matches the factory J18 setting (1.8 V).

# Clock
set_property -dict { PACKAGE_PIN Y9 IOSTANDARD LVCMOS33 } [get_ports { clk }] # GCLK
create_clock -period 10.000 -name sys_clk [get_ports { clk }]

# Slide switches
set_property -dict { PACKAGE_PIN F22 IOSTANDARD LVCMOS18 } [get_ports { sw[0] }] # SW0
set_property -dict { PACKAGE_PIN G22 IOSTANDARD LVCMOS18 } [get_ports { sw[1] }] # SW1
set_property -dict { PACKAGE_PIN H22 IOSTANDARD LVCMOS18 } [get_ports { sw[2] }] # SW2
set_property -dict { PACKAGE_PIN F21 IOSTANDARD LVCMOS18 } [get_ports { sw[3] }] # SW3
set_property -dict { PACKAGE_PIN H19 IOSTANDARD LVCMOS18 } [get_ports { sw[4] }] # SW4
set_property -dict { PACKAGE_PIN H18 IOSTANDARD LVCMOS18 } [get_ports { sw[5] }] # SW5
set_property -dict { PACKAGE_PIN H17 IOSTANDARD LVCMOS18 } [get_ports { sw[6] }] # SW6
set_property -dict { PACKAGE_PIN M15 IOSTANDARD LVCMOS18 } [get_ports { sw[7] }] # SW7

# Push buttons (btnc, btnd, btnl, btnr, btnu)
set_property -dict { PACKAGE_PIN P16 IOSTANDARD LVCMOS18 } [get_ports { btn[0] }] # BTNC
set_property -dict { PACKAGE_PIN R16 IOSTANDARD LVCMOS18 } [get_ports { btn[1] }] # BTND
set_property -dict { PACKAGE_PIN N15 IOSTANDARD LVCMOS18 } [get_ports { btn[2] }] # BTNL
set_property -dict { PACKAGE_PIN R18 IOSTANDARD LVCMOS18 } [get_ports { btn[3] }] # BTNR
set_property -dict { PACKAGE_PIN T18 IOSTANDARD LVCMOS18 } [get_ports { btn[4] }] # BTNU

# LEDs
set_property -dict { PACKAGE_PIN T22 IOSTANDARD LVCMOS33 } [get_ports { led[0] }] # LD0
set_property -dict { PACKAGE_PIN T21 IOSTANDARD LVCMOS33 } [get_ports { led[1] }] # LD1
set_property -dict { PACKAGE_PIN U22 IOSTANDARD LVCMOS33 } [get_ports { led[2] }] # LD2
set_property -dict { PACKAGE_PIN U21 IOSTANDARD LVCMOS33 } [get_ports { led[3] }] # LD3
set_property -dict { PACKAGE_PIN V22 IOSTANDARD LVCMOS33 } [get_ports { led[4] }] # LD4
set_property -dict { PACKAGE_PIN W22 IOSTANDARD LVCMOS33 } [get_ports { led[5] }] # LD5
set_property -dict { PACKAGE_PIN U19 IOSTANDARD LVCMOS33 } [get_ports { led[6] }] # LD6
set_property -dict { PACKAGE_PIN U14 IOSTANDARD LVCMOS33 } [get_ports { led[7] }] # LD7

# Pmod JA
set_property -dict { PACKAGE_PIN Y11 IOSTANDARD LVCMOS33 } [get_ports { ja[0] }] # JA1
set_property -dict { PACKAGE_PIN AA11 IOSTANDARD LVCMOS33 } [get_ports { ja[1] }] # JA2
set_property -dict { PACKAGE_PIN Y10 IOSTANDARD LVCMOS33 } [get_ports { ja[2] }] # JA3
set_property -dict { PACKAGE_PIN AA9 IOSTANDARD LVCMOS33 } [get_ports { ja[3] }] # JA4
set_property -dict { PACKAGE_PIN AB11 IOSTANDARD LVCMOS33 } [get_ports { ja[4] }] # JA7
set_property -dict { PACKAGE_PIN AB10 IOSTANDARD LVCMOS33 } [get_ports { ja[5] }] # JA8
set_property -dict { PACKAGE_PIN AB9 IOSTANDARD LVCMOS33 } [get_ports { ja[6] }] # JA9
set_property -dict { PACKAGE_PIN AA8 IOSTANDARD LVCMOS33 } [get_ports { ja[7] }] # JA10

# Pmod JB
set_property -dict { PACKAGE_PIN W12 IOSTANDARD LVCMOS33 } [get_ports { jb[0] }] # JB1
set_property -dict { PACKAGE_PIN W11 IOSTANDARD LVCMOS33 } [get_ports { jb[1] }] # JB2
set_property -dict { PACKAGE_PIN V10 IOSTANDARD LVCMOS33 } [get_ports { jb[2] }] # JB3
set_property -dict { PACKAGE_PIN W8 IOSTANDARD LVCMOS33 } [get_ports { jb[3] }] # JB4
set_property -dict { PACKAGE_PIN V12 IOSTANDARD LVCMOS33 } [get_ports { jb[4] }] # JB7
set_property -dict { PACKAGE_PIN W10 IOSTANDARD LVCMOS33 } [get_ports { jb[5] }] # JB8
set_property -dict { PACKAGE_PIN V9 IOSTANDARD LVCMOS33 } [get_ports { jb[6] }] # JB9
set_property -dict { PACKAGE_PIN V8 IOSTANDARD LVCMOS33 } [get_ports { jb[7] }] # JB10

# Pmod JC
set_property -dict { PACKAGE_PIN AB7 IOSTANDARD LVCMOS33 } [get_ports { jc[0] }] # JC1
set_property -dict { PACKAGE_PIN AB6 IOSTANDARD LVCMOS33 } [get_ports { jc[1] }] # JC2
set_property -dict { PACKAGE_PIN Y4 IOSTANDARD LVCMOS33 } [get_ports { jc[2] }] # JC3
set_property -dict { PACKAGE_PIN AA4 IOSTANDARD LVCMOS33 } [get_ports { jc[3] }] # JC4
set_property -dict { PACKAGE_PIN R6 IOSTANDARD LVCMOS33 } [get_ports { jc[4] }] # JC7
set_property -dict { PACKAGE_PIN T6 IOSTANDARD LVCMOS33 } [get_ports { jc[5] }] # JC8
set_property -dict { PACKAGE_PIN T4 IOSTANDARD LVCMOS33 } [get_ports { jc[6] }] # JC9
set_property -dict { PACKAGE_PIN U4 IOSTANDARD LVCMOS33 } [get_ports { jc[7] }] # JC10

# Pmod JD
set_property -dict { PACKAGE_PIN V7 IOSTANDARD LVCMOS33 } [get_ports { jd[0] }] # JD1
set_property -dict { PACKAGE_PIN W7 IOSTANDARD LVCMOS33 } [get_ports { jd[1] }] # JD2
set_property -dict { PACKAGE_PIN V5 IOSTANDARD LVCMOS33 } [get_ports { jd[2] }] # JD3
set_property -dict { PACKAGE_PIN V4 IOSTANDARD LVCMOS33 } [get_ports { jd[3] }] # JD4
set_property -dict { PACKAGE_PIN W6 IOSTANDARD LVCMOS33 } [get_ports { jd[4] }] # JD7
set_property -dict { PACKAGE_PIN W5 IOSTANDARD LVCMOS33 } [get_ports { jd[5] }] # JD8
set_property -dict { PACKAGE_PIN U6 IOSTANDARD LVCMOS33 } [get_ports { jd[6] }] # JD9
set_property -dict { PACKAGE_PIN U5 IOSTANDARD LVCMOS33 } [get_ports { jd[7] }] # JD10

# OLED display (Bank 13); pins as in examples/zedboard_oled_demo.xdc
set_property -dict { PACKAGE_PIN U10 IOSTANDARD LVCMOS33 } [get_ports { oled_dc }] # OLED-DC
set_property -dict { PACKAGE_PIN U9 IOSTANDARD LVCMOS33 } [get_ports { oled_res }] # OLED-RES
set_property -dict { PACKAGE_PIN AB12 IOSTANDARD LVCMOS33 } [get_ports { oled_sclk }] # OLED-SCLK
set_property -dict { PACKAGE_PIN AA12 IOSTANDARD LVCMOS33 } [get_ports { oled_sdin }] # OLED-SDIN
set_property -dict { PACKAGE_PIN U11 IOSTANDARD LVCMOS33 } [get_ports { oled_vbat }] # OLED-VBAT
set_property -dict { PACKAGE_PIN U12 IOSTANDARD LVCMOS33 } [get_ports { oled_vdd }] # OLED-VDD

# HDMI output (Bank 33); pins as in examples/zedboard_hdmi.xdc
set_property -dict { PACKAGE_PIN W18 IOSTANDARD LVCMOS33 } [get_ports { hd_clk }] # HD-CLK
set_property -dict { PACKAGE_PIN Y13 IOSTANDARD LVCMOS33 } [get_ports { hd_d[0] }] # HD-D0
set_property -dict { PACKAGE_PIN AA13 IOSTANDARD LVCMOS33 } [get_ports { hd_d[1] }] # HD-D1
set_property -dict { PACKAGE_PIN AA14 IOSTANDARD LVCMOS33 } [get_ports { hd_d[2] }] # HD-D2
set_property -dict { PACKAGE_PIN Y14 IOSTANDARD LVCMOS33 } [get_ports { hd_d[3] }] # HD-D3
set_property -dict { PACKAGE_PIN AB15 IOSTANDARD LVCMOS33 } [get_ports { hd_d[4] }] # HD-D4
set_property -dict { PACKAGE_PIN AB16 IOSTANDARD LVCMOS33 } [get_ports { hd_d[5] }] # HD-D5
set_property -dict { PACKAGE_PIN AA16 IOSTANDARD LVCMOS33 } [get_ports { hd_d[6] }] # HD-D6
set_property -dict { PACKAGE_PIN AB17 IOSTANDARD LVCMOS33 } [get_ports { hd_d[7] }] # HD-D7
set_property -dict { PACKAGE_PIN AA17 IOSTANDARD LVCMOS33 } [get_ports { hd_d[8] }] # HD-D8
set_property -dict { PACKAGE_PIN Y15 IOSTANDARD LVCMOS33 } [get_ports { hd_d[9] }] # HD-D9
set_property -dict { PACKAGE_PIN W13 IOSTANDARD LVCMOS33 } [get_ports { hd_d[10] }] # HD-D10
set_property -dict { PACKAGE_PIN W15 IOSTANDARD LVCMOS33 } [get_ports { hd_d[11] }] # HD-D11
set_property -dict { PACKAGE_PIN V15 IOSTANDARD LVCMOS33 } [get_ports { hd_d[12] }] # HD-D12
set_property -dict { PACKAGE_PIN U17 IOSTANDARD LVCMOS33 } [get_ports { hd_d[13] }] # HD-D13
set_property -dict { PACKAGE_PIN V14 IOSTANDARD LVCMOS33 } [get_ports { hd_d[14] }] # HD-D14
set_property -dict { PACKAGE_PIN V13 IOSTANDARD LVCMOS33 } [get_ports { hd_d[15] }] # HD-D15
set_property -dict { PACKAGE_PIN U16 IOSTANDARD LVCMOS33 } [get_ports { hd_de }] # HD-DE
set_property -dict { PACKAGE_PIN V17 IOSTANDARD LVCMOS33 } [get_ports { hd_hsync }] # HD-HSYNC
set_property -dict { PACKAGE_PIN W17 IOSTANDARD LVCMOS33 } [get_ports { hd_vsync }] # HD-VSYNC
set_property -dict { PACKAGE_PIN AA18 IOSTANDARD LVCMOS33 } [get_ports { hd_scl }] # HD-SCL
set_property -dict { PACKAGE_PIN Y16 IOSTANDARD LVCMOS33 } [get_ports { hd_sda }] # HD-SDA

# Audio codec (Bank 13); pins as in examples/zedboard_audio.xdc
set_property -dict { PACKAGE_PIN AB1 IOSTANDARD LVCMOS33 } [get_ports { ac_adr0 }] # AC-ADR0
set_property -dict { PACKAGE_PIN Y5 IOSTANDARD LVCMOS33 } [get_ports { ac_adr1 }] # AC-ADR1
set_property -dict { PACKAGE_PIN Y8 IOSTANDARD LVCMOS33 } [get_ports { ac_sdata }] # AC-GPIO0
set_property -dict { PACKAGE_PIN AA6 IOSTANDARD LVCMOS33 } [get_ports { ac_bclk }] # AC-GPIO2
set_property -dict { PACKAGE_PIN Y6 IOSTANDARD LVCMOS33 } [get_ports { ac_lrclk }] # AC-GPIO3
set_property -dict { PACKAGE_PIN AB2 IOSTANDARD LVCMOS33 } [get_ports { ac_mclk }] # AC-MCLK
set_property -dict { PACKAGE_PIN AB4 IOSTANDARD LVCMOS33 } [get_ports { ac_scl }] # AC-SCK
set_property -dict { PACKAGE_PIN AB5 IOSTANDARD LVCMOS33 } [get_ports { ac_sda }] # AC-SDA

# VGA connector (Bank 33); pins as in examples/zedboard_vga.xdc
set_property -dict { PACKAGE_PIN V20 IOSTANDARD LVCMOS33 } [get_ports { vga_r[0] }] # VGA-R1
set_property -dict { PACKAGE_PIN U20 IOSTANDARD LVCMOS33 } [get_ports { vga_r[1] }] # VGA-R2
set_property -dict { PACKAGE_PIN V19 IOSTANDARD LVCMOS33 } [get_ports { vga_r[2] }] # VGA-R3
set_property -dict { PACKAGE_PIN V18 IOSTANDARD LVCMOS33 } [get_ports { vga_r[3] }] # VGA-R4
set_property -dict { PACKAGE_PIN AB22 IOSTANDARD LVCMOS33 } [get_ports { vga_g[0] }] # VGA-G1
set_property -dict { PACKAGE_PIN AA22 IOSTANDARD LVCMOS33 } [get_ports { vga_g[1] }] # VGA-G2
set_property -dict { PACKAGE_PIN AB21 IOSTANDARD LVCMOS33 } [get_ports { vga_g[2] }] # VGA-G3
set_property -dict { PACKAGE_PIN AA21 IOSTANDARD LVCMOS33 } [get_ports { vga_g[3] }] # VGA-G4
set_property -dict { PACKAGE_PIN Y21 IOSTANDARD LVCMOS33 } [get_ports { vga_b[0] }] # VGA-B1
set_property -dict { PACKAGE_PIN Y20 IOSTANDARD LVCMOS33 } [get_ports { vga_b[1] }] # VGA-B2
set_property -dict { PACKAGE_PIN AB20 IOSTANDARD LVCMOS33 } [get_ports { vga_b[2] }] # VGA-B3
set_property -dict { PACKAGE_PIN AB19 IOSTANDARD LVCMOS33 } [get_ports { vga_b[3] }] # VGA-B4
set_property -dict { PACKAGE_PIN AA19 IOSTANDARD LVCMOS33 } [get_ports { vga_hs }] # VGA-HS
set_property -dict { PACKAGE_PIN Y19 IOSTANDARD LVCMOS33 } [get_ports { vga_vs }] # VGA-VS
