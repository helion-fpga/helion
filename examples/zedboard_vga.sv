// ZedBoard VGA 640x480 test pattern for the Helion 7-series backend.
// Top: zedboard_vga. Pins are in examples/zedboard_vga.xdc.
//
// Standard 640x480 at 60 Hz timing, driven from the 100 MHz GCLK with a
// divide-by-4 pixel clock enable (25 MHz, 0.7 percent below the 25.175 MHz
// standard rate; every common VGA monitor locks to it).
//   Horizontal: visible 640, front porch 16, sync 96, back porch 48,
//               total 800 pixels. Sync active low.
//   Vertical:   visible 480, front porch 10, sync 2, back porch 33,
//               total 525 lines. Sync active low.
// Test pattern: eight vertical color bars across the visible area, with
// a one-pixel white border frame. led[0] toggles once per frame.
//
// No simulator was installed on this box (no verilator, no iverilog),
// so no testbench is shipped with this file; see docs/zedboard/rung5-summary.json.
module zedboard_vga (
    input  logic       clk,
    output logic [3:0] vga_r,
    output logic [3:0] vga_g,
    output logic [3:0] vga_b,
    output logic       vga_hs,
    output logic       vga_vs,
    output logic       led
);
    // Horizontal timing (pixels).
    localparam int unsigned H_VISIBLE = 640;
    localparam int unsigned H_FRONT   = 16;
    localparam int unsigned H_SYNC    = 96;
    localparam int unsigned H_BACK    = 48;
    localparam int unsigned H_TOTAL   = 800;

    // Vertical timing (lines).
    localparam int unsigned V_VISIBLE = 480;
    localparam int unsigned V_FRONT   = 10;
    localparam int unsigned V_SYNC    = 2;
    localparam int unsigned V_BACK    = 33;
    localparam int unsigned V_TOTAL   = 525;

    logic [9:0] h_cnt;
    logic [9:0] v_cnt;
    logic [1:0] pix_div;
    logic       pix_en;
    logic       frame_tick;

    // Pixel clock enable: one pulse every 4 system clocks (25 MHz).
    always_ff @(posedge clk) begin
        pix_div <= pix_div + 2'd1;
        pix_en  <= (pix_div == 2'd3);
    end

    // Counters advance on the pixel clock enable.
    always_ff @(posedge clk) begin
        if (pix_en) begin
            if (h_cnt == H_TOTAL[9:0] - 10'd1) begin
                h_cnt <= 10'd0;
                if (v_cnt == V_TOTAL[9:0] - 10'd1) begin
                    v_cnt      <= 10'd0;
                    frame_tick <= 1'b1;
                end else begin
                    v_cnt      <= v_cnt + 10'd1;
                    frame_tick <= 1'b0;
                end
            end else begin
                h_cnt      <= h_cnt + 10'd1;
                frame_tick <= 1'b0;
            end
        end else begin
            frame_tick <= 1'b0;
        end
    end

    // Sync pulses are active low during the sync intervals.
    assign vga_hs = !((h_cnt >= H_VISIBLE[9:0] + H_FRONT[9:0]) &&
                      (h_cnt < H_VISIBLE[9:0] + H_FRONT[9:0] + H_SYNC[9:0]));
    assign vga_vs = !((v_cnt >= V_VISIBLE[9:0] + V_FRONT[9:0]) &&
                      (v_cnt < V_VISIBLE[9:0] + V_FRONT[9:0] + V_SYNC[9:0]));

    // Frame heartbeat: led toggles once per frame (about 60 Hz).
    always_ff @(posedge clk) begin
        if (frame_tick)
            led <= ~led;
    end

    // Color bars: bar index is x / 80 (640 / 8 = 80 pixels per bar).
    // Border frame overrides the bars.
    logic [2:0] bar;
    logic       visible;
    logic       border;
    logic [3:0] bar_r, bar_g, bar_b;

    assign bar     = h_cnt[9:0] / 10'd80;
    assign visible = (h_cnt < H_VISIBLE[9:0]) && (v_cnt < V_VISIBLE[9:0]);
    assign border  = visible && ((h_cnt == 10'd0) || (h_cnt == H_VISIBLE[9:0] - 10'd1) ||
                                 (v_cnt == 10'd0) || (v_cnt == V_VISIBLE[9:0] - 10'd1));

    always_comb begin
        case (bar)
            3'd0:    begin bar_r = 4'hf; bar_g = 4'hf; bar_b = 4'hf; end // white
            3'd1:    begin bar_r = 4'hf; bar_g = 4'hf; bar_b = 4'h0; end // yellow
            3'd2:    begin bar_r = 4'h0; bar_g = 4'hf; bar_b = 4'hf; end // cyan
            3'd3:    begin bar_r = 4'h0; bar_g = 4'hf; bar_b = 4'h0; end // green
            3'd4:    begin bar_r = 4'hf; bar_g = 4'h0; bar_b = 4'hf; end // magenta
            3'd5:    begin bar_r = 4'hf; bar_g = 4'h0; bar_b = 4'h0; end // red
            3'd6:    begin bar_r = 4'h0; bar_g = 4'h0; bar_b = 4'hf; end // blue
            default: begin bar_r = 4'h0; bar_g = 4'h0; bar_b = 4'h0; end // black
        endcase
    end

    always_comb begin
        if (!visible) begin
            vga_r = 4'h0;
            vga_g = 4'h0;
            vga_b = 4'h0;
        end else if (border) begin
            vga_r = 4'hf;
            vga_g = 4'hf;
            vga_b = 4'hf;
        end else begin
            vga_r = bar_r;
            vga_g = bar_g;
            vga_b = bar_b;
        end
    end
endmodule
