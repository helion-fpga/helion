// Helion-ST pulse stretcher/extender with programmable width (not AXI).
// Loads width via Helion-ST (st_valid + st_data as low byte; width_hi for
// upper). On rising edge of din, stretches dout high for `width` cycles
// (sticky busy until countdown done). Real FF+LUT fabric.
module h_pulse_ext (
    input  wire        clk,
    input  wire        resetn,
    // Helion-ST load path for width (byte-wise)
    input  wire        st_valid,
    input  wire [7:0]  st_data,
    output wire        st_ready,
    input  wire [1:0]  byte_sel, // which byte of 16b width
    input  wire        din,      // input pulse (rising edge triggers)
    input  wire        clear,    // force abort stretch
    output reg         dout,     // stretched output
    output reg         busy,     // high while stretching
    output wire        st_out_valid
);
    reg [15:0] width_r;
    reg [15:0] cnt_r;
    reg        prev_din;
    reg        have_prev;
    reg        out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    wire rise = have_prev && !prev_din && din;
    wire [15:0] cnt_m1 = cnt_r - 16'h1;

    always @(posedge clk) begin
        if (!resetn) begin
            width_r   <= 16'h1;
            cnt_r     <= 16'h0;
            dout      <= 1'b0;
            busy      <= 1'b0;
            prev_din  <= 1'b0;
            have_prev <= 1'b0;
            out_v     <= 1'b0;
        end else begin
            out_v <= 1'b0;

            if (st_valid) begin
                case (byte_sel)
                    2'b00: width_r[7:0]  <= st_data;
                    2'b01: width_r[15:8] <= st_data;
                    default: ;
                endcase
                out_v <= 1'b1;
            end

            if (clear) begin
                dout  <= 1'b0;
                busy  <= 1'b0;
                cnt_r <= 16'h0;
            end else if (busy) begin
                // Countdown stretch — LUT fabric for decrement + zero detect
                if (cnt_r == 16'h0 || cnt_r == 16'h1) begin
                    dout  <= 1'b0;
                    busy  <= 1'b0;
                    cnt_r <= 16'h0;
                end else begin
                    cnt_r <= cnt_m1;
                    dout  <= 1'b1;
                end
            end else if (rise) begin
                // Start stretch: width==0 → single-cycle pulse
                if (width_r == 16'h0) begin
                    dout  <= 1'b1;
                    busy  <= 1'b0;
                    cnt_r <= 16'h0;
                    out_v <= 1'b1;
                end else begin
                    dout  <= 1'b1;
                    busy  <= 1'b1;
                    cnt_r <= width_r;
                    out_v <= 1'b1;
                end
            end else begin
                dout <= 1'b0;
            end

            prev_din  <= din;
            have_prev <= 1'b1;
        end
    end
endmodule
