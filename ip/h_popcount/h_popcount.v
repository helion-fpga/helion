// Helion-ST registered population-count — 32b data path (not AXI).
// Loads 32b word via four Helion-ST bytes (st_valid + byte_sel), then on
// enable registers popcount = sum of set bits. Real FF+LUT fabric.
module h_popcount (
    input  wire       clk,
    input  wire       resetn,
    // Helion-ST load path (byte-wise into din)
    input  wire       st_valid,
    input  wire [7:0] st_data,
    output wire       st_ready,
    input  wire [1:0] byte_sel, // which byte of din[31:0] to load
    input  wire       enable,   // compute & register popcount
    output reg  [5:0] count,    // 0..32
    output wire       st_out_valid
);
    reg [31:0] din;
    reg        out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    // Combinational 32b popcount (tree of adders → LUTs)
    wire [5:0] pc =
          din[0]  + din[1]  + din[2]  + din[3]
        + din[4]  + din[5]  + din[6]  + din[7]
        + din[8]  + din[9]  + din[10] + din[11]
        + din[12] + din[13] + din[14] + din[15]
        + din[16] + din[17] + din[18] + din[19]
        + din[20] + din[21] + din[22] + din[23]
        + din[24] + din[25] + din[26] + din[27]
        + din[28] + din[29] + din[30] + din[31];

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
                count <= pc;
                out_v <= 1'b1;
            end
        end
    end
endmodule
