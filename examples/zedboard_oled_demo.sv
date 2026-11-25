// ZedBoard OLED animation demo for the Helion 7-series backend.
// Top: zedboard_oled_demo. Pins: examples/zedboard_oled_demo.xdc.
//
// Same SSD1306 power-up, init list and 500 kHz SPI as zedboard_oled.sv, but
// it never stops sending: the display RAM is rewritten forever, about 120
// frames per second, and every frame is drawn by the fabric on the fly.
//   Top half (pages 0-1): "HELION ON ZYNQ * BUILT ON A MAC", 5x7 font at
//   double size, scrolling left.
//   Bottom half (pages 2-3): an 8x8 ball above a dashed ground line that
//   scrolls the other way.
//   LD0: frame heartbeat (toggles every 32 frames, about 2 Hz).
// The init list adds column range 0-127 and page range 0-3 (0x21, 0x22), so
// horizontal addressing wraps from the last byte back to the top-left
// instead of running into the four RAM pages a 32-row panel doesn't show.
module zedboard_oled_demo (
    input  logic       clk,
    output logic       oled_dc,
    output logic       oled_res,
    output logic       oled_sclk,
    output logic       oled_sdin,
    output logic       oled_vbat = 1'b1, // active low, off until sequenced
    output logic       oled_vdd = 1'b1,  // active low, off until sequenced
    output logic       led
);
    localparam logic [15:0] INIT_ROM [0:17] = '{
        16'h00ae, 16'h00d5, 16'h0080, 16'h00a8, 16'h001f, 16'h00d3,
        16'h0000, 16'h0040, 16'h008d, 16'h0014, 16'h0020, 16'h0000,
        16'h00a1, 16'h00c8, 16'h00da, 16'h0002, 16'h0081, 16'h008f
    };
    // Tail: precharge, VCOMH, resume RAM, normal, column range 0-127, page
    // range 0-3, display on. The last entry is a NOP (0xE3) that is never
    // sent; it keeps rom_entry in range when rom_idx reaches ROM_LEN.
    localparam logic [15:0] INIT_TAIL [0:13] = '{
        16'h00d9, 16'h00f1, 16'h00db, 16'h0040, 16'h00a4, 16'h00a6,
        16'h0021, 16'h0000, 16'h007f, 16'h0022, 16'h0000, 16'h0003,
        16'h00af, 16'h00e3
    };

    // 5x7 font in 8-column cells (5 glyph columns, 3 blank), bit 0 = top.
    // Glyph codes: 0=' ', 1='*', 2='A', 3='B', 4='C', 5='E', 6='H', 7='I',
    // 8='L', 9='M', 10='N', 11='O', 12='Q', 13='T', 14='U', 15='Y', 16='Z'.
    localparam logic [7:0] FONT [0:135] = '{
        8'h00, 8'h00, 8'h00, 8'h00, 8'h00, 8'h00, 8'h00, 8'h00,
        8'h14, 8'h08, 8'h3e, 8'h08, 8'h14, 8'h00, 8'h00, 8'h00,
        8'h7e, 8'h11, 8'h11, 8'h11, 8'h7e, 8'h00, 8'h00, 8'h00,
        8'h7f, 8'h49, 8'h49, 8'h49, 8'h36, 8'h00, 8'h00, 8'h00,
        8'h3e, 8'h41, 8'h41, 8'h41, 8'h22, 8'h00, 8'h00, 8'h00,
        8'h7f, 8'h49, 8'h49, 8'h49, 8'h41, 8'h00, 8'h00, 8'h00,
        8'h7f, 8'h08, 8'h08, 8'h08, 8'h7f, 8'h00, 8'h00, 8'h00,
        8'h00, 8'h41, 8'h7f, 8'h41, 8'h00, 8'h00, 8'h00, 8'h00,
        8'h7f, 8'h40, 8'h40, 8'h40, 8'h40, 8'h00, 8'h00, 8'h00,
        8'h7f, 8'h02, 8'h0c, 8'h02, 8'h7f, 8'h00, 8'h00, 8'h00,
        8'h7f, 8'h04, 8'h08, 8'h10, 8'h7f, 8'h00, 8'h00, 8'h00,
        8'h3e, 8'h41, 8'h41, 8'h41, 8'h3e, 8'h00, 8'h00, 8'h00,
        8'h3e, 8'h41, 8'h51, 8'h21, 8'h5e, 8'h00, 8'h00, 8'h00,
        8'h01, 8'h01, 8'h7f, 8'h01, 8'h01, 8'h00, 8'h00, 8'h00,
        8'h3f, 8'h40, 8'h40, 8'h40, 8'h3f, 8'h00, 8'h00, 8'h00,
        8'h07, 8'h08, 8'h70, 8'h08, 8'h07, 8'h00, 8'h00, 8'h00,
        8'h61, 8'h51, 8'h49, 8'h45, 8'h43, 8'h00, 8'h00, 8'h00
    };
    // "HELION ON ZYNQ * BUILT ON A MAC " (32 characters = 512 pixels at 2x,
    // so the scroll position wraps with a plain 9-bit counter).
    localparam logic [4:0] MSG [0:31] = '{
        5'd6, 5'd5, 5'd8, 5'd7, 5'd11, 5'd10, 5'd0, 5'd11, 5'd10, 5'd0, 5'd16, 5'd15, 5'd10, 5'd12, 5'd0, 5'd1,
        5'd0, 5'd3, 5'd14, 5'd7, 5'd8, 5'd13, 5'd0, 5'd11, 5'd10, 5'd0, 5'd2, 5'd0, 5'd9, 5'd2, 5'd4, 5'd0
    };
    // 8x8 ball, one byte per column, bit 0 = top row.
    localparam logic [7:0] BALL [0:7] = '{
        8'h3c, 8'h7e, 8'hff, 8'hff, 8'hff, 8'hff, 8'h7e, 8'h3c
    };

    localparam int unsigned ROM_LEN   = 31;
    localparam int unsigned SCLK_DIV  = 100;        // half period: 100 MHz / 200
    localparam int unsigned WAIT_VDD  = 100_000;    // 1 ms
    localparam int unsigned WAIT_VBAT = 10_000_000; // 100 ms
    localparam int unsigned WAIT_RES  = 10_000;     // 100 us

    typedef enum logic [2:0] {
        ST_PWR_VDD,
        ST_WAIT_VDD,
        ST_RESET,
        ST_PWR_VBAT,
        ST_WAIT_VBAT,
        ST_SEND
    } state_t;

    state_t        state;
    logic [23:0]   wait_cnt;
    logic [5:0]    rom_idx;
    logic [8:0]    data_idx;   // {page[1:0], column[6:0]}, wraps every frame
    logic [7:0]    tx_byte;
    logic [3:0]    bit_cnt;
    logic [7:0]    div_cnt;
    logic          sclk_phase;
    logic          sending;
    logic [15:0]   entry;
    logic [7:0]    load_byte;

    // Animation state, updated once per frame.
    logic [8:0]    scroll = 9'd0;  // text offset in pixels
    logic [7:0]    frame = 8'd0;
    logic [6:0]    bx = 7'd60;     // ball column, 0..120
    logic [2:0]    by = 3'd2;      // ball top row within the bottom half, 0..7
    logic          dx = 1'b1;      // 1 = moving right
    logic          dy = 1'b1;      // 1 = moving down
    logic          frame_tick;

    function automatic logic [15:0] rom_entry(input logic [5:0] idx);
        if (idx < 6'd18)
            rom_entry = INIT_ROM[idx[4:0]];
        else
            rom_entry = INIT_TAIL[4'(idx[4:0] - 5'd18)];
    endfunction

    // Double every bit: glyph row r becomes display rows 2r and 2r+1.
    function automatic logic [15:0] double_bits(input logic [7:0] g);
        for (int i = 0; i < 8; i++) begin
            double_bits[2*i]   = g[i];
            double_bits[2*i+1] = g[i];
        end
    endfunction

    // Pixel pipeline. data_idx changes only when a byte is loaded, about
    // 1,600 clocks apart, so two register stages here cost nothing and keep
    // the font and sprite lookups off the SPI state machine's timing paths.
    // Stage 1: font column, ball column, ground bit.
    logic [1:0]  page;
    logic [6:0]  col;
    logic [8:0]  tx;           // text pixel column (2x: one glyph column per 2 px)
    logic [6:0]  ball_dc;      // column inside the ball sprite
    logic [7:0]  glyph_q;
    logic [15:0] ball_q;
    logic        ground_q;
    logic [1:0]  page_q;
    logic [7:0]  data_byte;
    logic [6:0]  gx;           // ground pattern column

    assign page    = data_idx[8:7];
    assign col     = data_idx[6:0];
    assign tx      = {2'b00, col} + scroll;
    assign ball_dc = col - bx;
    assign gx      = col + {scroll[5:0], 1'b0}; // scrolls at twice the text speed

    always_ff @(posedge clk) begin
        glyph_q  <= FONT[{MSG[tx[8:4]], tx[3:1]}];
        ball_q   <= (ball_dc < 7'd8) ? (16'(BALL[ball_dc[2:0]]) << by) : 16'd0;
        ground_q <= ~gx[2];       // dashes: 4 on, 4 off
        page_q   <= page;
    end

    // Stage 2: pick the page's byte.
    logic [15:0] big;
    assign big = double_bits(glyph_q);
    always_ff @(posedge clk) begin
        case (page_q)
            2'd0: data_byte <= big[7:0];
            2'd1: data_byte <= big[15:8];
            2'd2: data_byte <= ball_q[7:0];
            default: data_byte <= ball_q[15:8] | {ground_q, 7'd0};
        endcase
    end

    assign entry = rom_entry(rom_idx);
    assign load_byte = (rom_idx < ROM_LEN[5:0]) ? entry[7:0] : data_byte;
    assign frame_tick = (state == ST_SEND) && !sending && (rom_idx == ROM_LEN[5:0]) && (data_idx == 9'd511);

    // Once per frame: advance the scroll and move the ball.
    always_ff @(posedge clk) begin
        if (frame_tick) begin
            frame <= frame + 8'd1;
            if (frame[0]) scroll <= scroll + 9'd1; // 60 px/s
            // Ball: one pixel per frame in x, every other frame in y. At a
            // wall it turns and steps back, so it touches the wall for one
            // frame and never wraps (bx is 7 bits, by only 3).
            if (dx) begin
                if (bx == 7'd120) begin dx <= 1'b0; bx <= bx - 7'd1; end
                else bx <= bx + 7'd1;
            end else begin
                if (bx == 7'd0) begin dx <= 1'b1; bx <= bx + 7'd1; end
                else bx <= bx - 7'd1;
            end
            if (frame[0]) begin
                if (dy) begin
                    if (by == 3'd7) begin dy <= 1'b0; by <= by - 3'd1; end
                    else by <= by + 3'd1;
                end else begin
                    if (by == 3'd0) begin dy <= 1'b1; by <= by + 3'd1; end
                    else by <= by - 3'd1;
                end
            end
        end
    end

    // Hold the FSM for 16 cycles after configuration: GWE release is not
    // synchronous to clk, so a state change on the first edge can tear.
    logic [15:0] por = 16'd0;
    always_ff @(posedge clk) por <= {por[14:0], 1'b1};

    always_ff @(posedge clk) begin
        if (!por[15]) state <= ST_PWR_VDD;
        else case (state)
            ST_PWR_VDD: begin
                oled_vdd  <= 1'b0;
                oled_vbat <= 1'b1;
                oled_res  <= 1'b0;
                oled_dc   <= 1'b0;
                oled_sclk <= 1'b0;
                oled_sdin <= 1'b0;
                wait_cnt  <= 24'd0;
                rom_idx   <= 6'd0;
                data_idx  <= 9'd0;
                sending   <= 1'b0;
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
                if (wait_cnt == WAIT_VBAT[23:0]) begin
                    wait_cnt <= 24'd0;
                    state    <= ST_SEND;
                end else begin
                    wait_cnt <= wait_cnt + 24'd1;
                end
            end

            ST_SEND: begin
                if (!sending) begin
                    // Init bytes with D/C low, then display bytes with D/C
                    // high, forever. The first bit goes out while SCLK is low.
                    oled_dc    <= (rom_idx == ROM_LEN[5:0]);
                    if (rom_idx < ROM_LEN[5:0])
                        rom_idx  <= rom_idx + 6'd1;
                    else
                        data_idx <= data_idx + 9'd1;
                    tx_byte    <= {load_byte[6:0], 1'b0};
                    oled_sdin  <= load_byte[7];
                    bit_cnt    <= 4'd8;
                    div_cnt    <= 8'd0;
                    sclk_phase <= 1'b0;
                    sending    <= 1'b1;
                end else if (div_cnt == SCLK_DIV[7:0]) begin
                    div_cnt <= 8'd0;
                    if (!sclk_phase) begin
                        oled_sclk  <= 1'b1;
                        sclk_phase <= 1'b1;
                    end else begin
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

            default: state <= ST_PWR_VDD;
        endcase
    end

    assign led = frame[5];
endmodule
