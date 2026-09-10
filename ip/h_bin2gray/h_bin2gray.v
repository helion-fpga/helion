// Helion-ST registered binary→gray converter (8b) (not AXI).
// Loads binary via Helion-ST (st_valid); on enable registers
// gray = binary ^ (binary >> 1). Real FF+LUT fabric.
// Distinct from h_gray_cnt (counter) and inverse of h_gray2bin.
module h_bin2gray (
    input  wire       clk,
    input  wire       resetn,
    // Helion-ST load path
    input  wire       st_valid,
    input  wire [7:0] st_data,
    output wire       st_ready,
    input  wire       enable,   // compute & register gray
    output reg  [7:0] gray,
    output wire       st_out_valid
);
    reg [7:0] bin_r;
    reg       out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    wire [7:0] g = bin_r ^ (bin_r >> 1);

    always @(posedge clk) begin
        if (!resetn) begin
            bin_r <= 8'h00;
            gray  <= 8'h00;
            out_v <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (st_valid) begin
                bin_r <= st_data;
            end else if (enable) begin
                gray  <= g;
                out_v <= 1'b1;
            end
        end
    end
endmodule
