// ZedBoard OLED demo for the Helion 7-series backend.
// Top: zedboard_oled. Pins are in examples/zedboard_oled.xdc.
//
// Drives the onboard 128x32 SSD1306 OLED over a bit-banged SPI link.
// Power-on sequence follows the SSD1306 datasheet order: VDD on, RES
// pulse (low then high), VBAT on, wait 100 ms, then the init command
// list, then the display RAM is filled with an alternating stripe pattern
// and led[0] lights to signal done. SCLK runs at 500 kHz (100 MHz / 200).
// SPI mode 0: SCLK idles low, SDIN changes on the falling edge of SCLK,
// MSB first, so each bit is stable for a full half period before the
// rising edge that clocks it into the panel.
//
// OLED-VDD and OLED-VBAT are active low: the FPGA pin drives low to switch
// the rail on, high to switch it off. Sources: Digilent DSDB Reference
// Manual Table 11 (DOC 502-014, same UG-2832HSWEG04 display) lists VBAT#
// "Power enable for internal power supply" Active-low and VDD# "Power
// enable for digital power" Active-low; the Digilent Orbit BoosterPack
// manual (same display) states the VDD/VBAT pins "are driven low to turn
// on the power supplies"; the TU Darmstadt oled_ctrl.v ZedBoard controller
// drives both pins high at reset (off) and low to power the panel on.
// ZedBoard schematic Rev C.1 sheet 6 shows the DISP1 connector
// (UG-2832HSWEG04) with VBAT on pin 5 and VDD on pin 7 wired to FPGA
// pins U11/U12.
//
// A self-checking testbench is in examples/zedboard_oled_tb.sv. No
// simulator is installed on this box, so it is not run here; it is run
// on the Mac side. See docs/zedboard/rung5-summary.json.
module zedboard_oled (
    input  logic       clk,
    output logic       oled_dc,
    output logic       oled_res,
    output logic       oled_sclk,
    output logic       oled_sdin,
    // Active low: the initial 1 keeps both rails off from configuration
    // until the first clock edge drives the power sequence.
    output logic       oled_vbat = 1'b1,
    output logic       oled_vdd = 1'b1,
    output logic       led
);
    // Init command list, one byte per entry. D/C is low for every entry
    // (commands and their arguments alike) and high only for display RAM
    // bytes, so the ROM carries no data flag. The list ends with the 0xAF
    // display-on command. Sequence: display off, clock divide, multiplex
    // 32, offset 0, start line 0, charge pump on, horizontal addressing,
    // segment remap, COM scan direction, COM pins, contrast, precharge,
    // VCOMH, resume RAM, normal display, display on.
    localparam logic [15:0] INIT_ROM [0:17] = '{
        16'h00ae, 16'h00d5, 16'h0080, 16'h00a8, 16'h001f, 16'h00d3,
        16'h0000, 16'h0040, 16'h008d, 16'h0014, 16'h0020, 16'h0000,
        16'h00a1, 16'h00c8, 16'h00da, 16'h0002, 16'h0081, 16'h008f
    };
    localparam logic [15:0] INIT_TAIL [0:6] = '{
        16'h00d9, 16'h00f1, 16'h00db, 16'h0040, 16'h00a4, 16'h00a6,
        16'h00af
    };

    localparam int unsigned ROM_LEN   = 25;
    localparam int unsigned DATA_LEN  = 512; // 128 cols x 32 rows / 8
    localparam int unsigned SCLK_DIV  = 100; // half period: 100 MHz / 200
    localparam int unsigned WAIT_VDD  = 100_000;    // 1 ms at 100 MHz
    localparam int unsigned WAIT_VBAT = 10_000_000; // 100 ms at 100 MHz
    localparam int unsigned WAIT_RES  = 10_000;     // 100 us at 100 MHz

    typedef enum logic [2:0] {
        ST_PWR_VDD,
        ST_WAIT_VDD,
        ST_RESET,
        ST_PWR_VBAT,
        ST_WAIT_VBAT,
        ST_SEND,
        ST_DONE
    } state_t;

    state_t        state;
    logic [23:0]   wait_cnt;
    logic [5:0]    rom_idx;
    logic [9:0]    data_idx;
    logic [7:0]    tx_byte;
    logic [3:0]    bit_cnt;
    logic [7:0]    div_cnt;
    logic          sclk_phase;
    logic          sending;
    logic [15:0]   entry;
    logic [7:0]    load_byte;
    logic [7:0]    data_byte;

    // Data pattern: alternating stripes, 0xAA on even pages, 0x55 on odd.
    assign data_byte = data_idx[5] ? 8'h55 : 8'haa;

    // Byte source: first the init ROM (with its tail), then display data.
    // The ROM is split in two arrays because SystemVerilog array literals
    // stay readable at this size; rom_idx spans both. entry is a plain
    // signal because a part-select of a function call result does not parse.
    function automatic logic [15:0] rom_entry(input logic [5:0] idx);
        if (idx < 6'd18)
            rom_entry = INIT_ROM[idx[4:0]];
        else
            // 3' cast: the tail index is 0..6, made explicit so Verilator
            // does not warn WIDTHTRUNC on the 5-bit to 3-bit truncation.
            rom_entry = INIT_TAIL[3'(idx[4:0] - 5'd18)];
    endfunction

    assign entry = rom_entry(rom_idx);
    assign load_byte = (rom_idx < ROM_LEN[5:0]) ? entry[7:0] : data_byte;

    always_ff @(posedge clk) begin
        case (state)
            ST_PWR_VDD: begin
                // VDD and VBAT are active low: low switches the rail on.
                oled_vdd  <= 1'b0;
                oled_vbat <= 1'b1;
                oled_res  <= 1'b0;
                oled_dc   <= 1'b0;
                oled_sclk <= 1'b0;
                oled_sdin <= 1'b0;
                wait_cnt  <= 24'd0;
                rom_idx   <= 6'd0;
                data_idx  <= 10'd0;
                sending   <= 1'b0;
                led       <= 1'b0;
                state     <= ST_WAIT_VDD;
            end

            ST_WAIT_VDD: begin
                if (wait_cnt == WAIT_VDD[23:0]) begin
                    wait_cnt <= 24'd0;
                    state    <= ST_RESET;
                end else begin
                    wait_cnt <= wait_cnt + 24'd1;
                end
            end

            ST_RESET: begin
                // RES stays low for 100 us, then goes high. The reset
                // pulse happens before VBAT is switched on.
                if (wait_cnt == WAIT_RES[23:0]) begin
                    oled_res <= 1'b1;
                    wait_cnt <= 24'd0;
                    state    <= ST_PWR_VBAT;
                end else begin
                    wait_cnt <= wait_cnt + 24'd1;
                end
            end

            ST_PWR_VBAT: begin
                oled_vbat <= 1'b0;
                wait_cnt  <= 24'd0;
                state     <= ST_WAIT_VBAT;
            end

            ST_WAIT_VBAT: begin
                // Wait 100 ms after VBAT before the first init byte; the
                // display-on command (0xAF) goes out at the end of the
                // init list, well after this wait.
                if (wait_cnt == WAIT_VBAT[23:0]) begin
                    wait_cnt <= 24'd0;
                    state    <= ST_SEND;
                end else begin
                    wait_cnt <= wait_cnt + 24'd1;
                end
            end

            ST_SEND: begin
                if (!sending) begin
                    // Load the next byte: init commands first, then data.
                    // D/C stays low for every init byte, commands and
                    // their arguments alike, and goes high only for
                    // display data. The first bit goes on SDIN now, while
                    // SCLK is low.
                    if (rom_idx < ROM_LEN[5:0]) begin
                        oled_dc    <= 1'b0;
                        rom_idx    <= rom_idx + 6'd1;
                        tx_byte    <= {load_byte[6:0], 1'b0};
                        oled_sdin  <= load_byte[7];
                        bit_cnt    <= 4'd8;
                        div_cnt    <= 8'd0;
                        sclk_phase <= 1'b0;
                        sending    <= 1'b1;
                    end else if (data_idx < DATA_LEN[9:0]) begin
                        oled_dc    <= 1'b1;
                        data_idx   <= data_idx + 10'd1;
                        tx_byte    <= {load_byte[6:0], 1'b0};
                        oled_sdin  <= load_byte[7];
                        bit_cnt    <= 4'd8;
                        div_cnt    <= 8'd0;
                        sclk_phase <= 1'b0;
                        sending    <= 1'b1;
                    end else begin
                        state <= ST_DONE;
                    end
                end else if (div_cnt == SCLK_DIV[7:0]) begin
                    div_cnt <= 8'd0;
                    if (!sclk_phase) begin
                        // Raise SCLK. SDIN already holds the next bit, put
                        // there at load time or at the last falling edge,
                        // so it has been stable for a full half period.
                        oled_sclk  <= 1'b1;
                        sclk_phase <= 1'b1;
                    end else begin
                        // Lower SCLK and present the next bit on SDIN. The
                        // panel samples on the rising edge, so changing
                        // SDIN with the falling edge gives a full half
                        // period of setup time.
                        oled_sclk  <= 1'b0;
                        sclk_phase <= 1'b0;
                        oled_sdin  <= tx_byte[7];
                        tx_byte    <= {tx_byte[6:0], 1'b0};
                        if (bit_cnt == 4'd1) begin
                            bit_cnt <= 4'd8;
                            sending <= 1'b0;
                        end else begin
                            bit_cnt <= bit_cnt - 4'd1;
                        end
                    end
                end else begin
                    div_cnt <= div_cnt + 8'd1;
                end
            end

            ST_DONE: begin
                led <= 1'b1;
            end

            default: begin
                state <= ST_PWR_VDD;
            end
        endcase
    end
endmodule
