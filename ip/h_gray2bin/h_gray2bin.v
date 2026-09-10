// Helion-ST registered gray→binary converter (8b) (not AXI).
// Loads gray via Helion-ST (st_valid); on enable registers binary
// via classic prefix-XOR unfold. Real FF+LUT fabric.
module h_gray2bin (
    input  wire       clk,
    input  wire       resetn,
    // Helion-ST load path
    input  wire       st_valid,
    input  wire [7:0] st_data,
    output wire       st_ready,
    input  wire       enable,   // compute & register binary
    output reg  [7:0] binary,
    output wire       st_out_valid
);
    reg [7:0] gray_r;
    reg       out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    // Explicit unfold → denser LUT fabric than loop alone
    wire [7:0] g = gray_r;
    wire [7:0] b;
    assign b[7] = g[7];
    assign b[6] = b[7] ^ g[6];
    assign b[5] = b[6] ^ g[5];
    assign b[4] = b[5] ^ g[4];
    assign b[3] = b[4] ^ g[3];
    assign b[2] = b[3] ^ g[2];
    assign b[1] = b[2] ^ g[1];
    assign b[0] = b[1] ^ g[0];

    always @(posedge clk) begin
        if (!resetn) begin
            gray_r <= 8'h00;
            binary <= 8'h00;
            out_v  <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (st_valid) begin
                gray_r <= st_data;
            end else if (enable) begin
                binary <= b;
                out_v  <= 1'b1;
            end
        end
    end
endmodule
