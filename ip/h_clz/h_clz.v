// Helion-ST registered count-leading-zeros — 32b data path (not AXI).
// Loads 32b word via four Helion-ST bytes (st_valid + byte_sel), then on
// enable registers clz = number of leading zeros in din[31:0]. Real FF+LUT fabric.
module h_clz (
    input  wire       clk,
    input  wire       resetn,
    // Helion-ST load path (byte-wise into din)
    input  wire       st_valid,
    input  wire [7:0] st_data,
    output wire       st_ready,
    input  wire [1:0] byte_sel, // which byte of din[31:0] to load
    input  wire       enable,   // compute & register CLZ
    output reg  [5:0] count,    // 0..32
    output wire       st_out_valid
);
    reg [31:0] din;
    reg        out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    // Combinational 32b count-leading-zeros (priority mux tree → LUTs)
    // din[31] is MSB; count==32 when din==0.
    wire [5:0] clz =
          din[31] ? 6'd0  :
          din[30] ? 6'd1  :
          din[29] ? 6'd2  :
          din[28] ? 6'd3  :
          din[27] ? 6'd4  :
          din[26] ? 6'd5  :
          din[25] ? 6'd6  :
          din[24] ? 6'd7  :
          din[23] ? 6'd8  :
          din[22] ? 6'd9  :
          din[21] ? 6'd10 :
          din[20] ? 6'd11 :
          din[19] ? 6'd12 :
          din[18] ? 6'd13 :
          din[17] ? 6'd14 :
          din[16] ? 6'd15 :
          din[15] ? 6'd16 :
          din[14] ? 6'd17 :
          din[13] ? 6'd18 :
          din[12] ? 6'd19 :
          din[11] ? 6'd20 :
          din[10] ? 6'd21 :
          din[9]  ? 6'd22 :
          din[8]  ? 6'd23 :
          din[7]  ? 6'd24 :
          din[6]  ? 6'd25 :
          din[5]  ? 6'd26 :
          din[4]  ? 6'd27 :
          din[3]  ? 6'd28 :
          din[2]  ? 6'd29 :
          din[1]  ? 6'd30 :
          din[0]  ? 6'd31 :
                    6'd32;

    always @(posedge clk) begin
        if (!resetn) begin
            din   <= 32'h0;
            count <= 6'h0;
            out_v <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (st_valid) begin
                case (byte_sel)
                    2'b00: din[7:0]   <= st_data;
                    2'b01: din[15:8]  <= st_data;
                    2'b10: din[23:16] <= st_data;
                    2'b11: din[31:24] <= st_data;
                endcase
            end else if (enable) begin
                count <= clz;
                out_v <= 1'b1;
            end
        end
    end
endmodule
