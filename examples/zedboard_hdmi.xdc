# ----------------------------------------------------------------------------
# ZedBoard (xc7z020clg484-1) pin constraints for examples/zedboard_hdmi.sv.
#
# Primary source: official Digilent master XDC
#   https://github.com/Digilent/digilent-xdc/blob/master/Zedboard-Master.xdc
#   repo commit 00a3404901f35aa9567b01ecb3f2c233b6efe9f4, section
#   "HDMI Output - Bank 33". Every PACKAGE_PIN below is copied from that
#   file (the file spells the HD-D15 port "HS_D15"; the pin name is
#   "HD-D15"). Cross-check: Analog Devices' ZedBoard constraints
#   (analogdevicesinc/hdl projects/common/zed/zed_system_constr.xdc) give
#   the same pins for hdmi_out_clk, hdmi_vsync, hdmi_hsync, hdmi_data_e,
#   hdmi_data[15:0] (in the same bit order) and for the HDMI I2C pair
#   iic_mux_scl[1]/iic_mux_sda[1] = AA18/Y16, all LVCMOS33.
#
# Signal mapping for examples/zedboard_hdmi.sv:
#   clk       <- GCLK, 100 MHz PL oscillator (Bank 13, fixed 3.3V)
#   hd_clk    <- HD-CLK            hd_de     <- HD-DE
#   hd_hsync  <- HD-HSYNC          hd_vsync  <- HD-VSYNC
#   hd_d[n]   <- HD-Dn, n = 0..15
#   hd_scl    <- HD-SCL            hd_sda    <- HD-SDA
#   led[0]    <- LD0 (init sequence sent), led[1] <- LD1 (frame heartbeat)
# Bank 33 is fixed to 3.3V on ZedBoard (master XDC note at the end of the
# file), so every HDMI pin and both LEDs are LVCMOS33.
# ----------------------------------------------------------------------------

# Clock: 100 MHz GCLK.
set_property PACKAGE_PIN Y9 [get_ports {clk}]
set_property IOSTANDARD LVCMOS33 [get_ports {clk}]
create_clock -period 10.000 -name sys_clk [get_ports {clk}]

# HDMI Output - Bank 33.
set_property PACKAGE_PIN W18  [get_ports {hd_clk}];    # "HD-CLK"
set_property PACKAGE_PIN Y13  [get_ports {hd_d[0]}];   # "HD-D0"
set_property PACKAGE_PIN AA13 [get_ports {hd_d[1]}];   # "HD-D1"
set_property PACKAGE_PIN AA14 [get_ports {hd_d[2]}];   # "HD-D2"
set_property PACKAGE_PIN Y14  [get_ports {hd_d[3]}];   # "HD-D3"
set_property PACKAGE_PIN AB15 [get_ports {hd_d[4]}];   # "HD-D4"
set_property PACKAGE_PIN AB16 [get_ports {hd_d[5]}];   # "HD-D5"
set_property PACKAGE_PIN AA16 [get_ports {hd_d[6]}];   # "HD-D6"
set_property PACKAGE_PIN AB17 [get_ports {hd_d[7]}];   # "HD-D7"
set_property PACKAGE_PIN AA17 [get_ports {hd_d[8]}];   # "HD-D8"
set_property PACKAGE_PIN Y15  [get_ports {hd_d[9]}];   # "HD-D9"
set_property PACKAGE_PIN W13  [get_ports {hd_d[10]}];  # "HD-D10"
set_property PACKAGE_PIN W15  [get_ports {hd_d[11]}];  # "HD-D11"
set_property PACKAGE_PIN V15  [get_ports {hd_d[12]}];  # "HD-D12"
set_property PACKAGE_PIN U17  [get_ports {hd_d[13]}];  # "HD-D13"
set_property PACKAGE_PIN V14  [get_ports {hd_d[14]}];  # "HD-D14"
set_property PACKAGE_PIN V13  [get_ports {hd_d[15]}];  # "HD-D15"
set_property PACKAGE_PIN U16  [get_ports {hd_de}];     # "HD-DE"
set_property PACKAGE_PIN V17  [get_ports {hd_hsync}];  # "HD-HSYNC"
set_property PACKAGE_PIN W17  [get_ports {hd_vsync}];  # "HD-VSYNC"
set_property PACKAGE_PIN AA18 [get_ports {hd_scl}];    # "HD-SCL"
set_property PACKAGE_PIN Y16  [get_ports {hd_sda}];    # "HD-SDA"

# Status LEDs - Bank 33.
set_property PACKAGE_PIN T22 [get_ports {led[0]}];     # "LD0"
set_property PACKAGE_PIN T21 [get_ports {led[1]}];     # "LD1"

# Bank 13 and Bank 33 are fixed to 3.3V on ZedBoard.
set_property IOSTANDARD LVCMOS33 [get_ports -of_objects [get_iobanks 13]];
set_property IOSTANDARD LVCMOS33 [get_ports -of_objects [get_iobanks 33]];
