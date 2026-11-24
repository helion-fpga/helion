// ZedBoard HDMI out (ADV7511) colour-bar demo for the Helion 7-series backend.
// Top: zedboard_hdmi. Pins are in examples/zedboard_hdmi.xdc.
//
// Two independent parts, both clocked by the 100 MHz GCLK:
//  1. A write-only I2C master that programs the ADV7511 once after
//     configuration (50 kHz SCL, 41 single-register writes), then lights
//     led[0].
//  2. A 640x480 at 60 Hz video source that runs from power-on and drives
//     the 16-bit YCbCr 4:2:2 bus (HD-D[15:0]) with HD-DE, HD-HSYNC,
//     HD-VSYNC and a 25 MHz HD-CLK (100 MHz / 4). The ADV7511 ignores it
//     until it is programmed. led[1] toggles every 32 frames (about 1 Hz)
//     as a heartbeat that proves the fabric runs.
//
// Bus format, the ADV7511 setup and the register values follow Analog
// Devices' own ZedBoard reference design, the board this file targets:
//  - analogdevicesinc/linux arch/arm/boot/dts/xilinx/zynq-zed-adv7511.dtsi:
//    ADV7511 at 7-bit I2C address 0x39 (write byte 0x72); input depth 8,
//    colorspace yuv422, clock 1x, style 1, justification right, clock
//    delay 0.
//  - Linux drivers/gpu/drm/bridge/adv7511/adv7511_drv.c (v6.6, written and
//    maintained with Analog Devices) turns those settings into registers:
//    adv7511_set_link_config: 0x15[3:0] = input ID 1 (YCbCr 4:2:2, separate
//    syncs, 1x clock), 0x16 = color depth 3 << 4 | style reg 2 << 2 (the
//    driver notes the style field does not match the datasheet numbering:
//    style 1 is field value 2), 0x48 = justification 1 << 3, 0xBA =
//    ((0 + 1200) / 400) << 5 = 0x60 (no clock delay).
//    adv7511_fixed_registers: 0x98=03 0x9A=E0 0x9C=30 0x9D=61 0xA2=A4
//    0xA3=A4 0xE0=D0 0xF9=00 0x55=02, written after power-up because the
//    chip resets them while powered down.
//    __adv7511_power_on: 0x41 bit 6 (power down) cleared (default 0x50 ->
//    0x10) and 0xD6[7:6] = 11 (HPD source "none": the chip ignores the HPD
//    pin, so it powers up and stays up whether or not the monitor's HPD
//    is high yet).
//    adv7511_set_config_csc for an RGB/DVI sink: CSC enabled with the
//    adv7511_csc_ycbcr_to_rgb table and scaling factor 4 (0x18[6:5] = 2),
//    written with 0x1A bit 5 (CSC update mode) set during the update;
//    0x16 bit 7 (4:2:2 out) and bit 0 (YCbCr out) left 0; 0xAF bit 1 = 0
//    selects DVI (0xAF default 0x14 from the driver's register defaults,
//    so 0x14 is written).
//  - analogdevicesinc/hdl library/common/ad_ss_444to422.v (the 16-bit
//    HDMI path of the same design): HD-D[15:8] carries chroma, HD-D[7:0]
//    carries luma, chroma alternates Cb, Cr starting with Cb on the first
//    active pixel.
// The ADV7511 Programming Guide itself could not be downloaded here (the
// analog.com host refused the connection), so every register meaning above
// rests on the Linux driver and the ADI ZedBoard device tree, not on the
// guide: see docs/zedboard/rung5-av.md (TODO-UNVERIFIED against the guide).
//
// Colour bars: eight 80-pixel bars, white, yellow, cyan, green, magenta,
// red, blue, black, as ITU-R BT.709 limited-range YCbCr of 100 % RGB
// (Kr = 0.2126, Kb = 0.0722: Y = 16 + 46.559 R + 156.629 G + 15.812 B,
// Cb = 128 - 25.664 R - 86.336 G + 112 B, Cr = 128 + 112 R - 101.730 G
// - 10.270 B). BT.709, not BT.601, because the CSC table above (Linux
// adv7511_csc_ycbcr_to_rgb) has BT.709 coefficient ratios: Cr->R / Y =
// 0x734 / 0x4AD = 1.541 (BT.709 1.540, BT.601 1.371). Decoding these bars
// with that table (coefficients x 4 / 4096, offsets x 4 / 16 at 8 bits)
// gives each bar's RGB within 1 LSB of 0 or 255; BT.601 bars would come out
// e.g. green (0, 217, 0), red (255, 24, 0). Bars are 80 pixels wide, an even
// number, so a Cb/Cr pair never straddles a bar.
//
// Video timing: 640x480 at 60 Hz (VESA DMT), 25.000 MHz pixel clock (0.7 %
// below 25.175 MHz, the same rate examples/zedboard_vga.sv uses):
//   Horizontal: visible 640, front porch 16, sync 96, back porch 48, total
//               800 pixels; HSYNC active low.
//   Vertical:   visible 480, front porch 10, sync 2, back porch 33, total
//               525 lines; VSYNC active low.
// Every video output is a flop that changes in pixel phase 0; HD-CLK goes
// low in phase 0 and high in phase 2, so data, DE and syncs are stable
// 20 ns before and 20 ns after each HD-CLK rising edge.
//
// I2C: SCL and SDA are driven push-pull, because Helion's v2 front end
// does not accept an inout port on the top module and has no tristate
// output primitive (HELION_GAPS). The master never reads the bus: it drives
// SDA low itself for each ACK slot, starting 50 ns after SCL falls on the
// 8th bit, the level an acknowledging slave drives. The FPGA can still
// drive a 1 against the slave for up to those 50 ns, if the slave pulls SDA
// low that soon after SCL falls. A slave that NACKs only releases the line,
// so a NACK cannot be seen and there is no ACK-error LED.
// shortcut: no ACK check and no clock-stretch support, upgrade to an
// open-drain IOBUF once Helion supports inout pins.
module zedboard_hdmi (
    input  logic        clk,
    output logic        hd_clk,
    output logic [15:0] hd_d,
    output logic        hd_de,
    output logic        hd_hsync,
    output logic        hd_vsync,
    // Idle-high I2C lines from configuration on.
    output logic        hd_scl = 1'b1,
    output logic        hd_sda = 1'b1,
    output logic [1:0]  led
);
    // ------------------------------------------------------------------
    // I2C writer. ROM entry = {unused, last byte of a transaction, byte}.
    // Each transaction is a 3-byte write: 0x72 (address 0x39, write), the
    // register, the value.
    // ------------------------------------------------------------------
    localparam int unsigned Q      = 500;  // quarter SCL period: 50 kHz
    localparam int unsigned N_ENT  = 123;  // 41 writes x 3 bytes
    localparam int unsigned T_INIT = 10_000; // 100 us after configuration

    typedef enum logic [2:0] {
        I_GAP,
        I_START,
        I_LOAD,
        I_BIT,
        I_STOP,
        I_DONE
    } istate_t;

    istate_t     ist;
    logic [7:0]  idx;
    logic [9:0]  ent;
    logic [8:0]  qcnt;
    logic [1:0]  q;
    logic [3:0]  bitn;
    logic [8:0]  sh;
    logic [13:0] wcnt = T_INIT[13:0];
    logic        qtick;
    logic        init_done;
    logic        beat;

    assign led = {beat, init_done};

    always_comb begin
        case (idx)
            8'd0: ent = 10'h072;
            8'd1: ent = 10'h0d6;
            8'd2: ent = 10'h1c0;
            8'd3: ent = 10'h072;
            8'd4: ent = 10'h041;
            8'd5: ent = 10'h110;
            8'd6: ent = 10'h072;
            8'd7: ent = 10'h098;
            8'd8: ent = 10'h103;
            8'd9: ent = 10'h072;
            8'd10: ent = 10'h09a;
            8'd11: ent = 10'h1e0;
            8'd12: ent = 10'h072;
            8'd13: ent = 10'h09c;
            8'd14: ent = 10'h130;
            8'd15: ent = 10'h072;
            8'd16: ent = 10'h09d;
            8'd17: ent = 10'h161;
            8'd18: ent = 10'h072;
            8'd19: ent = 10'h0a2;
            8'd20: ent = 10'h1a4;
            8'd21: ent = 10'h072;
            8'd22: ent = 10'h0a3;
            8'd23: ent = 10'h1a4;
            8'd24: ent = 10'h072;
            8'd25: ent = 10'h0e0;
            8'd26: ent = 10'h1d0;
            8'd27: ent = 10'h072;
            8'd28: ent = 10'h0f9;
            8'd29: ent = 10'h100;
            8'd30: ent = 10'h072;
            8'd31: ent = 10'h055;
            8'd32: ent = 10'h102;
            8'd33: ent = 10'h072;
            8'd34: ent = 10'h015;
            8'd35: ent = 10'h101;
            8'd36: ent = 10'h072;
            8'd37: ent = 10'h016;
            8'd38: ent = 10'h138;
            8'd39: ent = 10'h072;
            8'd40: ent = 10'h048;
            8'd41: ent = 10'h108;
            8'd42: ent = 10'h072;
            8'd43: ent = 10'h0ba;
            8'd44: ent = 10'h160;
            8'd45: ent = 10'h072;
            8'd46: ent = 10'h01a;
            8'd47: ent = 10'h124;
            8'd48: ent = 10'h072;
            8'd49: ent = 10'h018;
            8'd50: ent = 10'h1c7;
            8'd51: ent = 10'h072;
            8'd52: ent = 10'h019;
            8'd53: ent = 10'h134;
            8'd54: ent = 10'h072;
            8'd55: ent = 10'h01b;
            8'd56: ent = 10'h1ad;
            8'd57: ent = 10'h072;
            8'd58: ent = 10'h01c;
            8'd59: ent = 10'h100;
            8'd60: ent = 10'h072;
            8'd61: ent = 10'h01d;
            8'd62: ent = 10'h100;
            8'd63: ent = 10'h072;
            8'd64: ent = 10'h01e;
            8'd65: ent = 10'h11c;
            8'd66: ent = 10'h072;
            8'd67: ent = 10'h01f;
            8'd68: ent = 10'h11b;
            8'd69: ent = 10'h072;
            8'd70: ent = 10'h020;
            8'd71: ent = 10'h11d;
            8'd72: ent = 10'h072;
            8'd73: ent = 10'h021;
            8'd74: ent = 10'h1dc;
            8'd75: ent = 10'h072;
            8'd76: ent = 10'h022;
            8'd77: ent = 10'h104;
            8'd78: ent = 10'h072;
            8'd79: ent = 10'h023;
            8'd80: ent = 10'h1ad;
            8'd81: ent = 10'h072;
            8'd82: ent = 10'h024;
            8'd83: ent = 10'h11f;
            8'd84: ent = 10'h072;
            8'd85: ent = 10'h025;
            8'd86: ent = 10'h124;
            8'd87: ent = 10'h072;
            8'd88: ent = 10'h026;
            8'd89: ent = 10'h101;
            8'd90: ent = 10'h072;
            8'd91: ent = 10'h027;
            8'd92: ent = 10'h135;
            8'd93: ent = 10'h072;
            8'd94: ent = 10'h028;
            8'd95: ent = 10'h100;
            8'd96: ent = 10'h072;
            8'd97: ent = 10'h029;
            8'd98: ent = 10'h100;
            8'd99: ent = 10'h072;
            8'd100: ent = 10'h02a;
            8'd101: ent = 10'h104;
            8'd102: ent = 10'h072;
            8'd103: ent = 10'h02b;
            8'd104: ent = 10'h1ad;
            8'd105: ent = 10'h072;
            8'd106: ent = 10'h02c;
            8'd107: ent = 10'h108;
            8'd108: ent = 10'h072;
            8'd109: ent = 10'h02d;
            8'd110: ent = 10'h17c;
            8'd111: ent = 10'h072;
            8'd112: ent = 10'h02e;
            8'd113: ent = 10'h11b;
            8'd114: ent = 10'h072;
            8'd115: ent = 10'h02f;
            8'd116: ent = 10'h177;
            8'd117: ent = 10'h072;
            8'd118: ent = 10'h01a;
            8'd119: ent = 10'h104;
            8'd120: ent = 10'h072;
            8'd121: ent = 10'h0af;
            8'd122: ent = 10'h114;
            default: ent = 10'h000;
        endcase
    end

    assign qtick = (qcnt == Q[8:0] - 9'd1);

    always_ff @(posedge clk) begin
        if (ist == I_GAP || ist == I_LOAD || ist == I_DONE || qtick)
            qcnt <= 9'd0;
        else
            qcnt <= qcnt + 9'd1;

        case (ist)
            I_GAP: begin
                // Bus idle (both lines high) for the initial delay.
                q <= 2'd0;
                if (wcnt == 14'd0)
                    ist <= (idx == N_ENT[7:0]) ? I_DONE : I_START;
                else
                    wcnt <= wcnt - 14'd1;
            end

            I_START: if (qtick) begin
                // q0: both high, q1: SDA falls (START), q2: SCL falls.
                q <= q + 2'd1;
                if (q == 2'd0) hd_sda <= 1'b0;
                if (q == 2'd1) hd_scl <= 1'b0;
                if (q == 2'd2) ist <= I_LOAD;
            end

            I_LOAD: begin
                // Eight data bits then the ACK slot, driven low.
                sh   <= {ent[7:0], 1'b0};
                bitn <= 4'd0;
                q    <= 2'd0;
                ist  <= I_BIT;
            end

            I_BIT: if (!qtick) begin
                // In q0 SCL is low. NXP UM10204 Rev. 7.0, Table 11: SDA must
                // be valid within 3.45 us of the SCL fall (tVD;DAT, tVD;ACK
                // max) and may not move before SCL is below 0.3 VDD (note
                // [2]). A slave may start its ACK 0 ns after the 8th fall
                // (tHD;DAT min), so the ACK level goes out 50 ns after it.
                // A data bit goes out 2.5 us after the fall: under 3.45 us,
                // late enough for the slave to release the ACK it may hold
                // after the previous byte, and 7.5 us before SCL rises.
                if (q == 2'd0 && bitn == 4'd8 && qcnt == 9'd4)
                    hd_sda <= 1'b0;
                if (q == 2'd0 && bitn != 4'd8 && qcnt == 9'd249) begin
                    hd_sda <= sh[8];
                    sh <= {sh[7:0], 1'b0};
                end
            end else begin
                // q0: SCL low (SDA set above); q1, q2: SCL high (the slave
                // samples); q3: SCL low again.
                q <= q + 2'd1;
                case (q)
                    2'd0: begin end
                    2'd1: hd_scl <= 1'b1;
                    2'd2: hd_scl <= 1'b1;
                    default: begin
                        hd_scl <= 1'b0;
                        if (bitn == 4'd8) begin
                            if (ent[8]) begin
                                ist <= I_STOP;
                            end else begin
                                idx <= idx + 8'd1;
                                ist <= I_LOAD;
                            end
                        end else begin
                            bitn <= bitn + 4'd1;
                        end
                    end
                endcase
            end

            I_STOP: if (qtick) begin
                // q0: SDA low, q1: SCL high, q2: SDA rises (STOP), q3: idle.
                q <= q + 2'd1;
                case (q)
                    2'd0: hd_sda <= 1'b0;
                    2'd1: hd_scl <= 1'b1;
                    2'd2: hd_sda <= 1'b1;
                    default: begin
                        idx  <= idx + 8'd1;
                        wcnt <= 14'd0;
                        ist  <= I_GAP;
                    end
                endcase
            end

            I_DONE: init_done <= 1'b1;

            default: ist <= I_GAP;
        endcase
    end

    // ------------------------------------------------------------------
    // 640x480 video, YCbCr 4:2:2 on a 16-bit bus.
    // ------------------------------------------------------------------
    localparam int unsigned H_VISIBLE = 640;
    localparam int unsigned H_FRONT   = 16;
    localparam int unsigned H_SYNC    = 96;
    localparam int unsigned H_TOTAL   = 800;
    localparam int unsigned V_VISIBLE = 480;
    localparam int unsigned V_FRONT   = 10;
    localparam int unsigned V_SYNC    = 2;
    localparam int unsigned V_TOTAL   = 525;

    logic [1:0] ph;      // pixel phase, 4 system clocks per pixel
    logic [9:0] h_cnt;
    logic [9:0] v_cnt;
    logic [4:0] frames;
    logic [2:0] bar;
    logic       visible;
    logic [7:0] y, cb, cr;

    assign bar     = 3'(h_cnt / 10'd80);
    assign visible = (h_cnt < H_VISIBLE[9:0]) && (v_cnt < V_VISIBLE[9:0]);

    always_comb begin
        case (bar)
            3'd0:    begin y = 8'd235; cb = 8'd128; cr = 8'd128; end // white
            3'd1:    begin y = 8'd219; cb = 8'd16;  cr = 8'd138; end // yellow
            3'd2:    begin y = 8'd188; cb = 8'd154; cr = 8'd16;  end // cyan
            3'd3:    begin y = 8'd173; cb = 8'd42;  cr = 8'd26;  end // green
            3'd4:    begin y = 8'd78;  cb = 8'd214; cr = 8'd230; end // magenta
            3'd5:    begin y = 8'd63;  cb = 8'd102; cr = 8'd240; end // red
            3'd6:    begin y = 8'd32;  cb = 8'd240; cr = 8'd118; end // blue
            default: begin y = 8'd16;  cb = 8'd128; cr = 8'd128; end // black
        endcase
    end

    always_ff @(posedge clk) begin
        ph <= ph + 2'd1;
        if (ph == 2'd2)
            hd_clk <= 1'b1;
        if (ph == 2'd0) begin
            hd_clk   <= 1'b0;
            // Outputs for the pixel at (h_cnt, v_cnt).
            hd_de    <= visible;
            hd_hsync <= !((h_cnt >= H_VISIBLE[9:0] + H_FRONT[9:0]) &&
                          (h_cnt <  H_VISIBLE[9:0] + H_FRONT[9:0] + H_SYNC[9:0]));
            hd_vsync <= !((v_cnt >= V_VISIBLE[9:0] + V_FRONT[9:0]) &&
                          (v_cnt <  V_VISIBLE[9:0] + V_FRONT[9:0] + V_SYNC[9:0]));
            // Even pixel carries Cb, odd pixel Cr; blanking is black.
            if (visible)
                hd_d <= {h_cnt[0] ? cr : cb, y};
            else
                hd_d <= 16'h8010;
            // Advance to the next pixel.
            if (h_cnt == H_TOTAL[9:0] - 10'd1) begin
                h_cnt <= 10'd0;
                if (v_cnt == V_TOTAL[9:0] - 10'd1) begin
                    v_cnt  <= 10'd0;
                    frames <= frames + 5'd1;
                    if (frames == 5'd31)
                        beat <= ~beat;
                end else begin
                    v_cnt <= v_cnt + 10'd1;
                end
            end else begin
                h_cnt <= h_cnt + 10'd1;
            end
        end
    end
endmodule
