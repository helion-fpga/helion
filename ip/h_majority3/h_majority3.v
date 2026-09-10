// Helion-ST 3-input majority voter registered, bus width 8 (not AXI).
// Loads lanes a/b/c via Helion-ST + sel; on enable registers
// maj[i] = (a&b)|(b&c)|(a&c) per bit. Real FF+LUT fabric.
module h_majority3 (
    input  wire       clk,
    input  wire       resetn,
    // Helion-ST load path
    input  wire       st_valid,
    input  wire [7:0] st_data,
    output wire       st_ready,
    input  wire [1:0] sel,      // 0=a 1=b 2=c
    input  wire       enable,   // compute & register majority
    output reg  [7:0] maj,
    output wire       st_out_valid
);
    reg [7:0] a_r;
    reg [7:0] b_r;
    reg [7:0] c_r;
    reg       out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    wire [7:0] maj_w = (a_r & b_r) | (b_r & c_r) | (a_r & c_r);

    always @(posedge clk) begin
        if (!resetn) begin
            a_r   <= 8'h00;
            b_r   <= 8'h00;
            c_r   <= 8'h00;
            maj   <= 8'h00;
            out_v <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (st_valid) begin
                case (sel)
                    2'd0: a_r <= st_data;
                    2'd1: b_r <= st_data;
                    default: c_r <= st_data;
                endcase
            end else if (enable) begin
                maj   <= maj_w;
                out_v <= 1'b1;
            end
        end
    end
endmodule
