// ZedBoard OLED demo for the Helion 7-series backend.
// Top: zedboard_oled. Pins are in examples/zedboard_oled.xdc.
//
// Drives the onboard 128x32 SSD1306 OLED over a bit-banged SPI link.
// Power-on sequence follows the SSD1306 datasheet order: VDD on, wait,
// VBAT on, wait, RES pulse, then the init command list, then the display
// RAM is filled with an alternating stripe pattern and led[0] lights to
// signal done. SCLK runs at 500 kHz (100 MHz / 200). SPI mode 0: SCLK
// idles low, SDIN changes on the falling edge, MSB first.
//
// No simulator was installed on this box (no verilator, no iverilog),
// so no testbench is shipped with this file; see docs/zedboard/rung5-summary.json.
module zedboard_oled (
    input  logic       clk,
    output logic       oled_dc,
    output logic       oled_res,
    output logic       oled_sclk,
    output logic       oled_sdin,
    output logic       oled_vbat,
    output logic       oled_vdd,
    output logic       led
);
    // Init command list (command byte, data byte, has_data flag packed as
    // {is_data, byte}). The list ends with the 0xAF display-on command.
    // Sequence: display off, clock divide, multiplex 32, offset 0, start
    // line 0, charge pump on, horizontal addressing, segment remap, COM
    // scan direction, COM pins, contrast, precharge, VCOMH, resume RAM,
    // normal display, display on.
    localparam logic [15:0] INIT_ROM [0:17] = '{
        16'h00ae, 16'h00d5, 16'h0180, 16'h00a8, 16'h011f, 16'h00d3,
        16'h0100, 16'h0040, 16'h008d, 16'h0114, 16'h0020, 16'h0100,
        16'h00a1, 16'h00c8, 16'h00da, 16'h0102, 16'h0081, 16'h018f
    };
    localparam logic [15:0] INIT_TAIL [0:6] = '{
        16'h00d9, 16'h01f1, 16'h00db, 16'h0140, 16'h00a4, 16'h00a6,
        16'h00af
    };

    localparam int unsigned ROM_LEN   = 25;
    localparam int unsigned DATA_LEN  = 512; // 128 cols x 32 rows / 8
    localparam int unsigned SCLK_DIV  = 100; // half period: 100 MHz / 200
    localparam int unsigned WAIT_VDD  = 100_000; // 1 ms at 100 MHz
    localparam int unsigned WAIT_VBAT = 100_000; // 1 ms at 100 MHz
    localparam int unsigned WAIT_RES  = 10_000;  // 100 us at 100 MHz

    typedef enum logic [2:0] {
        ST_PWR_VDD,
        ST_WAIT_VDD,
        ST_PWR_VBAT,
        ST_WAIT_VBAT,
        ST_RESET,
        ST_SEND,
        ST_DONE
    } state_t;

    state_t        state;
    logic [17:0]   wait_cnt;
    logic [5:0]    rom_idx;
    logic [9:0]    data_idx;
    logic [7:0]    tx_byte;
    logic [3:0]    bit_cnt;
    logic [7:0]    div_cnt;
    logic          sclk_phase;
    logic          sending;
    logic [7:0]    data_byte;

    // Data pattern: alternating stripes, 0xAA on even pages, 0x55 on odd.
    assign data_byte = data_idx[5] ? 8'h55 : 8'haa;

    // Byte source: first the init ROM (with its tail), then display data.
    // The ROM is split in two arrays because SystemVerilog array literals
    // stay readable at this size; rom_idx spans both.
    function automatic logic [15:0] rom_entry(input logic [5:0] idx);
        if (idx < 6'd18)
            rom_entry = INIT_ROM[idx[4:0]];
        else
            rom_entry = INIT_TAIL[idx[4:0] - 5'd18];
    endfunction

    always_ff @(posedge clk) begin
        case (state)
            ST_PWR_VDD: begin
                oled_vdd  <= 1'b1;
                oled_vbat <= 1'b0;
                oled_res  <= 1'b0;
                oled_dc   <= 1'b0;
                oled_sclk <= 1'b0;
                oled_sdin <= 1'b0;
                wait_cnt  <= 18'd0;
                rom_idx   <= 6'd0;
                data_idx  <= 10'd0;
                sending   <= 1'b0;
                led       <= 1'b0;
                state     <= ST_WAIT_VDD;
            end

            ST_WAIT_VDD: begin
                if (wait_cnt == WAIT_VDD[17:0]) begin
                    wait_cnt <= 18'd0;
                    state    <= ST_PWR_VBAT;
                end else begin
                    wait_cnt <= wait_cnt + 18'd1;
                end
            end

            ST_PWR_VBAT: begin
                oled_vbat <= 1'b1;
                wait_cnt  <= 18'd0;
                state     <= ST_WAIT_VBAT;
            end

            ST_WAIT_VBAT: begin
                if (wait_cnt == WAIT_VBAT[17:0]) begin
                    wait_cnt <= 18'd0;
                    state    <= ST_RESET;
                end else begin
                    wait_cnt <= wait_cnt + 18'd1;
                end
            end

            ST_RESET: begin
                // RES low for 100 us, then high; display is ready after.
                oled_res <= 1'b1;
                if (wait_cnt == WAIT_RES[17:0]) begin
                    wait_cnt <= 18'd0;
                    state    <= ST_SEND;
                end else begin
                    wait_cnt <= wait_cnt + 18'd1;
                end
            end

            ST_SEND: begin
                if (!sending) begin
                    // Load the next byte: init commands first, then data.
                    if (rom_idx < ROM_LEN[5:0]) begin
                        tx_byte    <= rom_entry(rom_idx)[7:0];
                        oled_dc    <= rom_entry(rom_idx)[15:8] != 8'h00;
                        rom_idx    <= rom_idx + 6'd1;
                    end else if (data_idx < DATA_LEN[9:0]) begin
                        tx_byte    <= data_byte;
                        oled_dc    <= 1'b1;
                        data_idx   <= data_idx + 10'd1;
                    end else begin
                        state <= ST_DONE;
                    end
                    bit_cnt    <= 4'd8;
                    div_cnt    <= 8'd0;
                    sclk_phase <= 1'b0;
                    sending    <= 1'b1;
                end else if (div_cnt == SCLK_DIV[7:0]) begin
                    div_cnt <= 8'd0;
                    if (!sclk_phase) begin
                        // Falling edge setup: present the next bit, then
                        // raise SCLK so the SSD1306 samples it.
                        oled_sdin  <= tx_byte[7];
                        tx_byte    <= {tx_byte[6:0], 1'b0};
                        oled_sclk  <= 1'b1;
                        sclk_phase <= 1'b1;
                    end else begin
                        oled_sclk  <= 1'b0;
                        sclk_phase <= 1'b0;
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
