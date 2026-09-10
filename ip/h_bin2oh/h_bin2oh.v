// Helion-ST registered binary→onehot (4b binary → 16b one-hot) (not AXI).
// Loads bin[3:0] via Helion-ST (st_valid); on enable registers
// oh = 16'b1 << bin. Real FF+LUT fabric (decoder mux tree).
module h_bin2oh (
    input  wire        clk,
    input  wire        resetn,
    // Helion-ST load path
    input  wire        st_valid,
    input  wire [7:0]  st_data,
    output wire        st_ready,
    input  wire        enable,   // compute & register one-hot
    output reg  [15:0] oh,
    output wire        st_out_valid
);
    reg [3:0] bin_r;
    reg       out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    // Explicit one-hot decoder → denser LUT fabric than << alone
    wire [3:0] b = bin_r;
    wire [15:0] decode =
          ({16{b == 4'd0}}  & 16'h0001)
        | ({16{b == 4'd1}}  & 16'h0002)
        | ({16{b == 4'd2}}  & 16'h0004)
        | ({16{b == 4'd3}}  & 16'h0008)
        | ({16{b == 4'd4}}  & 16'h0010)
        | ({16{b == 4'd5}}  & 16'h0020)
        | ({16{b == 4'd6}}  & 16'h0040)
        | ({16{b == 4'd7}}  & 16'h0080)
        | ({16{b == 4'd8}}  & 16'h0100)
        | ({16{b == 4'd9}}  & 16'h0200)
        | ({16{b == 4'd10}} & 16'h0400)
        | ({16{b == 4'd11}} & 16'h0800)
        | ({16{b == 4'd12}} & 16'h1000)
        | ({16{b == 4'd13}} & 16'h2000)
        | ({16{b == 4'd14}} & 16'h4000)
        | ({16{b == 4'd15}} & 16'h8000);

    always @(posedge clk) begin
        if (!resetn) begin
            bin_r <= 4'h0;
            oh    <= 16'h0;
            out_v <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (st_valid) begin
                bin_r <= st_data[3:0];
            end else if (enable) begin
                oh    <= decode;
                out_v <= 1'b1;
            end
        end
    end
endmodule
