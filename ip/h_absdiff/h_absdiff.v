// Helion-ST registered absolute difference |a-b| on 32b (not AXI).
// Loads A/B via Helion-ST bytes (st_valid + byte_sel + which); on enable
// registers q = (a>=b) ? (a-b) : (b-a) as unsigned. Real FF+LUT fabric.
module h_absdiff (
    input  wire        clk,
    input  wire        resetn,
    // Helion-ST load path (byte-wise into A or B)
    input  wire        st_valid,
    input  wire [7:0]  st_data,
    output wire        st_ready,
    input  wire [1:0]  byte_sel, // which byte of selected operand
    input  wire        which,    // 0=load A, 1=load B
    input  wire        enable,   // compute & register |a-b|
    output reg  [31:0] q,
    output wire        st_out_valid
);
    reg [31:0] a_r;
    reg [31:0] b_r;
    reg        out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    // Unsigned abs-diff → LUT fabric (compare + subtract both ways)
    wire a_ge_b = (a_r >= b_r);
    wire [31:0] amb = a_r - b_r;
    wire [31:0] bma = b_r - a_r;
    wire [31:0] res = a_ge_b ? amb : bma;

    always @(posedge clk) begin
        if (!resetn) begin
            a_r   <= 32'h0;
            b_r   <= 32'h0;
            q     <= 32'h0;
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
                q     <= res;
                out_v <= 1'b1;
            end
        end
    end
endmodule
