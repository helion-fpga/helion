// Helion-ST registered 4-digit BCD incrementer with carry chain (not AXI).
// Digits are packed as {d3,d2,d1,d0} each 4b (0..9). enable increments by 1
// with per-digit BCD adjust + carry; load via Helion-ST bytes. Real FF+LUT.
module h_bcd_inc (
    input  wire        clk,
    input  wire        resetn,
    // Helion-ST load path (two bytes → 16b BCD word)
    input  wire        st_valid,
    input  wire [7:0]  st_data,
    output wire        st_ready,
    input  wire        hi_byte,  // 0=digits[7:0], 1=digits[15:8]
    input  wire        enable,   // increment BCD by 1
    input  wire        clear,    // zero all digits
    output reg  [15:0] digits,   // {d3,d2,d1,d0}
    output reg         carry_out,// sticky overflow from MSD
    output wire        st_out_valid
);
    reg out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    // Per-digit BCD +1 with carry-in → LUT fabric
    wire [3:0] d0 = digits[3:0];
    wire [3:0] d1 = digits[7:4];
    wire [3:0] d2 = digits[11:8];
    wire [3:0] d3 = digits[15:12];

    wire       c0 = (d0 >= 4'd9);
    wire [3:0] n0 = c0 ? 4'd0 : (d0 + 4'd1);

    wire       c1 = c0 && (d1 >= 4'd9);
    wire [3:0] n1 = !c0 ? d1 : (c1 ? 4'd0 : (d1 + 4'd1));

    wire       c2 = c1 && (d2 >= 4'd9);
    wire [3:0] n2 = !c1 ? d2 : (c2 ? 4'd0 : (d2 + 4'd1));

    wire       c3 = c2 && (d3 >= 4'd9);
    wire [3:0] n3 = !c2 ? d3 : (c3 ? 4'd0 : (d3 + 4'd1));

    wire [15:0] next_digits = {n3, n2, n1, n0};

    always @(posedge clk) begin
        if (!resetn) begin
            digits    <= 16'h0;
            carry_out <= 1'b0;
            out_v     <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (clear) begin
                digits    <= 16'h0;
                carry_out <= 1'b0;
                out_v     <= 1'b1;
            end else if (st_valid) begin
                if (hi_byte)
                    digits[15:8] <= st_data;
                else
                    digits[7:0]  <= st_data;
            end else if (enable) begin
                digits    <= next_digits;
                carry_out <= c3;
                out_v     <= 1'b1;
            end
        end
    end
endmodule
