// Helion-ST binary↔gray counter — enable/load (not AXI).
// Holds binary count; gray = binary ^ (binary>>1). Load via Helion-ST byte
// (low nibble) when load=1; else enable advances. Real FF+LUT fabric.
module h_gray_cnt (
    input  wire       clk,
    input  wire       resetn,
    // Helion-ST load path
    input  wire       st_valid,
    input  wire [7:0] st_data,
    output wire       st_ready,
    input  wire       load,     // 1=load binary from st_data[3:0]
    input  wire       enable,   // advance binary counter
    output reg  [3:0] binary,
    output wire [3:0] gray,
    output wire       st_out_valid
);
    reg out_v;

    assign st_ready     = 1'b1;
    assign gray         = binary ^ (binary >> 1);
    assign st_out_valid = out_v;

    always @(posedge clk) begin
        if (!resetn) begin
            binary <= 4'h0;
            out_v  <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (st_valid && load) begin
                binary <= st_data[3:0];
                out_v  <= 1'b1;
            end else if (enable) begin
                binary <= binary + 4'h1;
                out_v  <= 1'b1;
            end
        end
    end
endmodule
