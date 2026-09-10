// Helion-ST registered Hamming distance between two 32b words (not AXI).
// Loads A/B via Helion-ST bytes (st_valid + byte_sel + which); on enable
// registers q = popcount(A XOR B). Real FF+LUT fabric.
module h_hamming (
    input  wire        clk,
    input  wire        resetn,
    // Helion-ST load path (byte-wise into A or B)
    input  wire        st_valid,
    input  wire [7:0]  st_data,
    output wire        st_ready,
    input  wire [1:0]  byte_sel, // which byte of selected operand
    input  wire        which,    // 0=load A, 1=load B
    input  wire        enable,   // compute & register distance
    output reg  [5:0]  q,        // 0..32 Hamming distance
    output wire        st_out_valid
);
    reg [31:0] a_r;
    reg [31:0] b_r;
    reg        out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    wire [31:0] diff = a_r ^ b_r;
    // Combinational 32b popcount of XOR → LUT fabric
    wire [5:0] dist =
          diff[0]  + diff[1]  + diff[2]  + diff[3]
        + diff[4]  + diff[5]  + diff[6]  + diff[7]
        + diff[8]  + diff[9]  + diff[10] + diff[11]
        + diff[12] + diff[13] + diff[14] + diff[15]
        + diff[16] + diff[17] + diff[18] + diff[19]
        + diff[20] + diff[21] + diff[22] + diff[23]
        + diff[24] + diff[25] + diff[26] + diff[27]
        + diff[28] + diff[29] + diff[30] + diff[31];

    always @(posedge clk) begin
        if (!resetn) begin
            a_r   <= 32'h0;
            b_r   <= 32'h0;
            q     <= 6'h0;
            out_v <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (st_valid) begin
                if (!which) begin
                    case (byte_sel)
                        2'b00: a_r[7:0]   <= st_data;
                        2'b01: a_r[15:8]  <= st_data;
                        2'b10: a_r[23:16] <= st_data;
                        2'b11: a_r[31:24] <= st_data;
                    endcase
                end else begin
                    case (byte_sel)
                        2'b00: b_r[7:0]   <= st_data;
                        2'b01: b_r[15:8]  <= st_data;
                        2'b10: b_r[23:16] <= st_data;
                        2'b11: b_r[31:24] <= st_data;
                    endcase
                end
            end else if (enable) begin
                q     <= dist;
                out_v <= 1'b1;
            end
        end
    end
endmodule
