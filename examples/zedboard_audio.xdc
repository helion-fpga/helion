# ----------------------------------------------------------------------------
# ZedBoard (xc7z020clg484-1) pin constraints for examples/zedboard_audio.sv.
#
# Primary source: official Digilent master XDC
#   https://github.com/Digilent/digilent-xdc/blob/master/Zedboard-Master.xdc
#   repo commit 00a3404901f35aa9567b01ecb3f2c233b6efe9f4, section
#   "Audio Codec - Bank 13". Every PACKAGE_PIN below is copied from that
#   file. The master XDC names the FPGA-side signals SDATA_O (AC-GPIO0),
#   SDATA_I (AC-GPIO1), BCLK_O (AC-GPIO2), LRCLK_O (AC-GPIO3) and MCLK_O
#   (AC-MCLK). Cross-check: Analog Devices' ZedBoard constraints
#   (analogdevicesinc/hdl projects/common/zed/zed_system_constr.xdc) use
#   the same pins for i2s_mclk AB2, i2s_bclk AA6, i2s_lrclk Y6,
#   i2s_sdata_out Y8, i2s_sdata_in AA7 and the codec I2C pair
#   iic_mux_scl[0]/iic_mux_sda[0] = AB4/AB5, all LVCMOS33, with the PL as
#   the I2S clock source.
#
# Signal mapping for examples/zedboard_audio.sv:
#   clk      <- GCLK, 100 MHz PL oscillator (Bank 13, fixed 3.3V)
#   ac_mclk  <- AC-MCLK              ac_bclk  <- AC-GPIO2 (BCLK)
#   ac_lrclk <- AC-GPIO3 (LRCLK)     ac_sdata <- AC-GPIO0 (codec DAC data)
#   ac_adr0  <- AC-ADR0              ac_adr1  <- AC-ADR1
#   ac_scl   <- AC-SCK               ac_sda   <- AC-SDA
#   led[0]   <- LD0 (init sequence sent), led[1] <- LD1 (heartbeat)
# AC-GPIO1 (codec ADC data, AA7) is not used.
# ----------------------------------------------------------------------------

# Clock: 100 MHz GCLK.
set_property PACKAGE_PIN Y9 [get_ports {clk}]
set_property IOSTANDARD LVCMOS33 [get_ports {clk}]
create_clock -period 10.000 -name sys_clk [get_ports {clk}]

# Audio Codec - Bank 13.
set_property PACKAGE_PIN AB1 [get_ports {ac_adr0}];   # "AC-ADR0"
set_property PACKAGE_PIN Y5  [get_ports {ac_adr1}];   # "AC-ADR1"
set_property PACKAGE_PIN Y8  [get_ports {ac_sdata}];  # "AC-GPIO0"
set_property PACKAGE_PIN AA6 [get_ports {ac_bclk}];   # "AC-GPIO2"
set_property PACKAGE_PIN Y6  [get_ports {ac_lrclk}];  # "AC-GPIO3"
set_property PACKAGE_PIN AB2 [get_ports {ac_mclk}];   # "AC-MCLK"
set_property PACKAGE_PIN AB4 [get_ports {ac_scl}];    # "AC-SCK"
set_property PACKAGE_PIN AB5 [get_ports {ac_sda}];    # "AC-SDA"

# Status LEDs - Bank 33.
set_property PACKAGE_PIN T22 [get_ports {led[0]}];    # "LD0"
set_property PACKAGE_PIN T21 [get_ports {led[1]}];    # "LD1"

# Bank 13 and Bank 33 are fixed to 3.3V on ZedBoard.
set_property IOSTANDARD LVCMOS33 [get_ports -of_objects [get_iobanks 13]];
set_property IOSTANDARD LVCMOS33 [get_ports -of_objects [get_iobanks 33]];
